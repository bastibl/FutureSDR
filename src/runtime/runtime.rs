use async_lock::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use axum::Router;
use futures::channel::oneshot;
use futures::prelude::*;
use std::fmt;
use std::sync::Arc;

use crate::runtime;
use crate::runtime::BlockDescription;
use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::ControlPort;
use crate::runtime::Error;
use crate::runtime::Flowgraph;
use crate::runtime::FlowgraphDescription;
use crate::runtime::FlowgraphHandle;
use crate::runtime::FlowgraphId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::FlowgraphTask;
use crate::runtime::Pmt;
use crate::runtime::PortId;
use crate::runtime::RunningFlowgraph;
use crate::runtime::channel::mpsc::Receiver;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::channel::mpsc::channel;
use crate::runtime::config;
use crate::runtime::scheduler::Scheduler;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::scheduler::SmolScheduler;
use crate::runtime::scheduler::Task;
#[cfg(target_arch = "wasm32")]
use crate::runtime::scheduler::WasmScheduler;

#[cfg(not(target_arch = "wasm32"))]
/// Default scheduler used by [`Runtime`] and [`RuntimeHandle`] on native targets.
pub type DefaultScheduler = SmolScheduler;

#[cfg(target_arch = "wasm32")]
/// Default scheduler used by [`Runtime`] and [`RuntimeHandle`] on WASM targets.
pub type DefaultScheduler = WasmScheduler;

/// Executor and control-plane owner for [`Flowgraph`]s and async tasks.
///
/// A [`Runtime`] owns a scheduler, starts flowgraphs, and provides a control
/// port on native targets. It is generic over the scheduler implementation, but
/// most applications can use [`Runtime::new`] with the default scheduler.
pub struct Runtime<S = DefaultScheduler> {
    scheduler: S,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
    #[cfg(not(target_arch = "wasm32"))]
    _control_port: ControlPort<S>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Runtime<DefaultScheduler> {
    /// Construct a new [`Runtime`] using [`DefaultScheduler::default()`].
    pub fn new() -> Self {
        Self::with_custom_routes(Router::new())
    }

    /// Block the current thread until a future completes.
    pub fn block_on<T>(future: impl Future<Output = T>) -> T {
        async_io::block_on(future)
    }

    /// Construct a runtime with additional routes for the integrated webserver.
    pub fn with_custom_routes(routes: Router) -> Self {
        Self::with_config(DefaultScheduler::default(), routes)
    }
}

impl<S> Drop for Runtime<S> {
    fn drop(&mut self) {
        debug!("Runtime dropped");
    }
}

#[cfg(target_arch = "wasm32")]
impl Runtime<DefaultScheduler> {
    /// Construct a runtime using the WASM scheduler.
    pub fn new() -> Self {
        Self::with_scheduler(DefaultScheduler::default())
    }
}

impl Default for Runtime<DefaultScheduler> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Scheduler> Runtime<S> {
    /// Spawn an async task on the runtime scheduler.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.scheduler.spawn(future)
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await initialization.
    ///
    /// Returns once the flowgraph is initialized and running. The returned
    /// [`RunningFlowgraph`] can be used to send messages, stop the graph, or
    /// wait for completion.
    pub async fn start_async(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let running = start_flowgraph(self.scheduler.clone(), fg).await?;
        self.flowgraphs
            .try_lock()
            .ok_or(Error::LockError)?
            .push(running.handle());
        Ok(running)
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await its termination.
    pub async fn run_async(&self, fg: Flowgraph) -> Result<Flowgraph, Error> {
        self.start_async(fg).await?.wait_async().await
    }

    /// Get the [`Scheduler`] that is associated with the [`Runtime`].
    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }

    /// Create a clonable [`RuntimeHandle`] for starting and querying flowgraphs.
    pub fn handle(&self) -> RuntimeHandle<S> {
        RuntimeHandle {
            scheduler: self.scheduler.clone(),
            flowgraphs: self.flowgraphs.clone(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Scheduler> Runtime<S> {
    /// Spawn an async task and detach its handle.
    pub fn spawn_background<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) {
        self.scheduler.spawn(future).detach();
    }

    /// Start a [`Flowgraph`] on the [`Runtime`].
    ///
    /// Blocks until the flowgraph is initialized and running.
    pub fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        async_io::block_on(self.start_async(fg))
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and block until it terminates.
    pub fn run(&self, fg: Flowgraph) -> Result<Flowgraph, Error> {
        let running = async_io::block_on(self.start_async(fg))?;
        running.wait()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Scheduler + Sync> Runtime<S> {
    /// Construct a [`Runtime`] with a custom [`Scheduler`].
    pub fn with_scheduler(scheduler: S) -> Self {
        Self::with_config(scheduler, Router::new())
    }

    /// Construct a runtime with a custom scheduler and webserver routes.
    pub fn with_config(scheduler: S, routes: Router) -> Self {
        runtime::init();

        let flowgraphs = Arc::new(Mutex::new(Vec::new()));
        let handle = RuntimeHandle {
            scheduler: scheduler.clone(),
            flowgraphs: flowgraphs.clone(),
        };

        Runtime {
            scheduler,
            flowgraphs,
            _control_port: ControlPort::new(handle, routes),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl<S: Scheduler> Runtime<S> {
    /// Construct a [`Runtime`] with a custom [`Scheduler`].
    pub fn with_scheduler(scheduler: S) -> Self {
        runtime::init();

        let flowgraphs = Arc::new(Mutex::new(Vec::new()));
        Runtime {
            scheduler,
            flowgraphs,
        }
    }
}

/// Clonable runtime control handle used by web handlers and external control code.
///
/// A `RuntimeHandle` can start new flowgraphs on the same scheduler as the
/// owning [`Runtime`] and look up flowgraphs that have been registered with the
/// control plane.
pub struct RuntimeHandle<S = DefaultScheduler> {
    scheduler: S,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
}

impl<S: Clone> Clone for RuntimeHandle<S> {
    fn clone(&self) -> Self {
        Self {
            scheduler: self.scheduler.clone(),
            flowgraphs: self.flowgraphs.clone(),
        }
    }
}

impl<S> fmt::Debug for RuntimeHandle<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHandle")
            .field("flowgraphs", &self.flowgraphs)
            .finish()
    }
}

impl<S> PartialEq for RuntimeHandle<S> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.flowgraphs, &other.flowgraphs)
    }
}

impl<S: Scheduler> RuntimeHandle<S> {
    /// Start a [`Flowgraph`] on the runtime.
    pub async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let running = start_flowgraph(self.scheduler.clone(), fg).await?;
        self.add_flowgraph(running.handle()).await;
        Ok(running)
    }

    /// Add a [`FlowgraphHandle`] to make it available to web handlers.
    async fn add_flowgraph(&self, handle: FlowgraphHandle) -> FlowgraphId {
        let mut v = self.flowgraphs.lock().await;
        let l = v.len();
        v.push(handle);
        FlowgraphId(l)
    }

    /// Get the control handle for a running flowgraph by id.
    pub async fn get_flowgraph(&self, id: FlowgraphId) -> Option<FlowgraphHandle> {
        self.flowgraphs.lock().await.get(id.0).cloned()
    }

    /// Get the ids of flowgraphs known to this runtime handle.
    pub async fn get_flowgraphs(&self) -> Vec<FlowgraphId> {
        self.flowgraphs
            .lock()
            .await
            .iter()
            .enumerate()
            .map(|x| FlowgraphId(x.0))
            .collect()
    }
}

async fn start_flowgraph<S: Scheduler>(
    scheduler: S,
    fg: Flowgraph,
) -> Result<RunningFlowgraph, Error> {
    let queue_size = config::config().queue_size;
    let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

    let (tx, rx) = oneshot::channel::<Result<(), Error>>();
    let task = spawn_run_flowgraph(scheduler, fg, fg_inbox.clone(), fg_inbox_rx, tx);

    rx.await
        .map_err(|_| Error::RuntimeError("run_flowgraph panicked".to_string()))??;

    let handle = FlowgraphHandle::new(fg_inbox);
    Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_run_flowgraph<S: Scheduler>(
    scheduler: S,
    fg: Flowgraph,
    fg_inbox: Sender<FlowgraphMessage>,
    fg_inbox_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Task<Result<Flowgraph, Error>> {
    let scheduler_clone = scheduler.clone();
    scheduler.spawn(run_flowgraph(
        fg,
        scheduler_clone,
        fg_inbox,
        fg_inbox_rx,
        initialized,
    ))
}

#[cfg(target_arch = "wasm32")]
fn spawn_run_flowgraph<S: Scheduler>(
    _scheduler: S,
    fg: Flowgraph,
    fg_inbox: Sender<FlowgraphMessage>,
    fg_inbox_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Task<Result<Flowgraph, Error>> {
    Task::spawn_local(run_flowgraph(fg, fg_inbox, fg_inbox_rx, initialized))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn run_flowgraph<S: Scheduler>(
    mut fg: Flowgraph,
    scheduler: S,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Result<Flowgraph, Error> {
    debug!("in run_flowgraph");

    let (mut inboxes, ids) = fg.inboxes()?;
    let blocks = fg.take_blocks()?;
    let stream_edges_desc = fg.stream_edge_endpoints();
    let message_edges_desc = fg.message_edges.clone();
    let block_tasks = scheduler.run_domain(blocks, &main_channel);
    let local_tasks = fg.run_local_domains(main_channel.clone())?;

    let run_result = run_flowgraph_loop(
        &mut inboxes,
        &ids,
        stream_edges_desc,
        message_edges_desc,
        main_channel,
        main_rx,
        initialized,
    )
    .await;

    if run_result.is_err() {
        for inbox in inboxes.iter_mut().flatten() {
            if inbox.send(BlockMessage::Terminate).await.is_err() {
                debug!("runtime tried to terminate block during shutdown cleanup");
            }
        }
    }

    let mut finished_blocks = Vec::with_capacity(block_tasks.len());
    for task in block_tasks {
        finished_blocks.push(task.await);
    }
    fg.restore_blocks(finished_blocks)?;

    fg.join_local_domains(local_tasks).await?;

    run_result?;
    Ok(fg)
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn run_flowgraph(
    mut fg: Flowgraph,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Result<Flowgraph, Error> {
    debug!("in run_flowgraph");

    let (mut inboxes, ids) = fg.inboxes()?;
    let stream_edges_desc = fg.stream_edge_endpoints();
    let message_edges_desc = fg.message_edges.clone();
    let block_tasks = fg.spawn_wasm_blocks(main_channel.clone())?;

    let run_result = run_flowgraph_loop(
        &mut inboxes,
        &ids,
        stream_edges_desc,
        message_edges_desc,
        main_channel,
        main_rx,
        initialized,
    )
    .await;

    if run_result.is_err() {
        for inbox in inboxes.iter_mut().flatten() {
            if inbox.send(BlockMessage::Terminate).await.is_err() {
                debug!("runtime tried to terminate block during shutdown cleanup");
            }
        }
    }

    let mut finished_blocks = Vec::with_capacity(block_tasks.len());
    for task in block_tasks {
        finished_blocks.push(task.await);
    }
    fg.restore_blocks(finished_blocks)?;

    run_result?;
    Ok(fg)
}

async fn run_flowgraph_loop(
    inboxes: &mut [Option<crate::runtime::dev::BlockInbox>],
    ids: &[BlockId],
    stream_edges_desc: Vec<(BlockId, PortId, BlockId, PortId)>,
    message_edges_desc: Vec<(BlockId, PortId, BlockId, PortId)>,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Result<(), Error> {
    debug!("init blocks");
    // init blocks
    let mut active_blocks = 0u32;
    for inbox in inboxes.iter_mut().flatten() {
        inbox.send(BlockMessage::Initialize).await?;
        active_blocks += 1;
    }

    debug!("wait for blocks init");
    // wait until all blocks are initialized
    let mut i = active_blocks;
    let mut queue = Vec::new();
    let mut block_error = false;
    loop {
        if i == 0 {
            break;
        }

        let m = main_rx.recv().await.ok_or_else(|| {
            Error::RuntimeError("no reply from blocks during init phase".to_string())
        })?;

        match m {
            FlowgraphMessage::Initialized => i -= 1,
            FlowgraphMessage::BlockError { .. } => {
                i -= 1;
                active_blocks -= 1;
                block_error = true;
            }
            x => {
                debug!(
                    "queueing unhandled message received during initialization {:?}",
                    &x
                );
                queue.push(x);
            }
        }
    }

    debug!("running blocks");
    for inbox in inboxes.iter_mut().flatten() {
        inbox.notify();
        if inbox.is_closed() {
            debug!("runtime wanted to start block that already terminated");
        }
    }

    for m in queue.into_iter() {
        main_channel.try_send(m)?;
    }

    initialized
        .send(Ok(()))
        .map_err(|_| Error::RuntimeError("main thread panic during flowgraph init".to_string()))?;

    if block_error {
        main_channel.try_send(FlowgraphMessage::Terminate)?;
    }

    let mut terminated = false;

    // main loop
    loop {
        if active_blocks == 0 {
            break;
        }

        let m = main_rx.recv().await.ok_or_else(|| {
            Error::RuntimeError("all senders to flowgraph inbox dropped".to_string())
        })?;

        match m {
            FlowgraphMessage::BlockCall {
                block_id,
                port_id,
                data,
                tx,
            } => {
                if let Some(Some(inbox)) = inboxes.get_mut(block_id.0) {
                    if inbox
                        .send(BlockMessage::Call { port_id, data })
                        .await
                        .is_ok()
                    {
                        let _ = tx.send(Ok(()));
                    } else {
                        let _ = tx.send(Err(Error::BlockTerminated));
                    }
                } else {
                    let _ = tx.send(Err(Error::InvalidBlock(block_id)));
                }
            }
            FlowgraphMessage::BlockCallback {
                block_id,
                port_id,
                data,
                tx,
            } => {
                let (block_tx, block_rx) = oneshot::channel::<Result<Pmt, Error>>();
                if let Some(Some(inbox)) = inboxes.get_mut(block_id.0) {
                    if inbox
                        .send(BlockMessage::Callback {
                            port_id,
                            data,
                            tx: block_tx,
                        })
                        .await
                        .is_ok()
                    {
                        match block_rx.await? {
                            Ok(p) => tx.send(Ok(p)).ok(),
                            Err(e) => tx.send(Err(Error::HandlerError(e.to_string()))).ok(),
                        };
                    } else {
                        let _ = tx.send(Err(Error::BlockTerminated));
                    }
                } else {
                    let _ = tx.send(Err(Error::InvalidBlock(block_id)));
                }
            }
            FlowgraphMessage::BlockDone { .. } => {
                active_blocks -= 1;
            }
            FlowgraphMessage::BlockError { .. } => {
                block_error = true;
                active_blocks -= 1;
                let _ = main_channel.send(FlowgraphMessage::Terminate).await;
            }
            FlowgraphMessage::BlockDescription { block_id, tx } => {
                if let Some(Some(b)) = inboxes.get_mut(block_id.0) {
                    let (b_tx, rx) = oneshot::channel::<BlockDescription>();
                    if b.send(BlockMessage::BlockDescription { tx: b_tx })
                        .await
                        .is_ok()
                    {
                        if let Ok(b) = rx.await {
                            let _ = tx.send(Ok(b));
                        } else {
                            let _ = tx.send(Err(Error::RuntimeError(format!(
                                "Block {block_id:?} terminated or crashed"
                            ))));
                        }
                    } else {
                        let _ = tx.send(Err(Error::BlockTerminated));
                    }
                } else {
                    let _ = tx.send(Err(Error::InvalidBlock(block_id)));
                }
            }
            FlowgraphMessage::FlowgraphDescription { tx } => {
                let mut blocks = Vec::new();
                for id in ids.iter() {
                    let (b_tx, rx) = oneshot::channel::<BlockDescription>();
                    if let Some(Some(inbox)) = inboxes.get_mut(id.0)
                        && inbox
                            .send(BlockMessage::BlockDescription { tx: b_tx })
                            .await
                            .is_ok()
                    {
                        blocks.push(rx.await?);
                    }
                }

                if tx
                    .send(FlowgraphDescription {
                        blocks,
                        stream_edges: stream_edges_desc.clone(),
                        message_edges: message_edges_desc.clone(),
                    })
                    .is_err()
                {
                    error!("Failed to send flowgraph description. Receiver may have disconnected.");
                }
            }
            FlowgraphMessage::Terminate => {
                if !terminated {
                    for inbox in inboxes.iter_mut().flatten() {
                        if inbox.send(BlockMessage::Terminate).await.is_err() {
                            debug!("runtime tried to terminate block that was already terminated");
                        }
                    }
                    terminated = true;
                }
            }
            _ => warn!("main loop received unhandled message"),
        }
    }

    if block_error {
        return Err(Error::RuntimeError("A block raised an error".to_string()));
    }

    Ok(())
}

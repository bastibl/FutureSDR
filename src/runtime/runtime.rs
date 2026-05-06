use async_lock::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use axum::Router;
use futures::channel::oneshot;
use futures::prelude::*;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::runtime;
use crate::runtime::BlockDescription;
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
use crate::runtime::RunningFlowgraph;
use crate::runtime::RuntimeId;
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

static NEXT_RUNTIME_ID: AtomicUsize = AtomicUsize::new(0);

trait SpawnBound: Scheduler + Sync + 'static {}
impl<T: Scheduler + Sync + 'static> SpawnBound for T {}

type DynSpawn = dyn Spawn + Send + Sync + 'static;

/// Executor and control-plane owner for [`Flowgraph`]s and async tasks.
///
/// A [`Runtime`] owns a scheduler, starts flowgraphs, and provides a control
/// port on native targets. It is generic over the scheduler implementation, but
/// most applications can use [`Runtime::new`] with the default scheduler.
#[cfg(not(target_arch = "wasm32"))]
pub struct Runtime<S = SmolScheduler> {
    id: RuntimeId,
    scheduler: S,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
    _control_port: ControlPort,
}

#[cfg(target_arch = "wasm32")]
/// Executor and control-plane owner for [`Flowgraph`]s and async tasks on WASM.
pub struct Runtime<S = WasmScheduler> {
    id: RuntimeId,
    scheduler: S,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Runtime<SmolScheduler> {
    /// Construct a new [`Runtime`] using [`SmolScheduler::default()`].
    pub fn new() -> Self {
        Self::with_custom_routes(Router::new())
    }

    /// Block the current thread until a future completes.
    pub fn block_on<T>(future: impl Future<Output = T>) -> T {
        async_io::block_on(future)
    }

    /// Construct a runtime with additional routes for the integrated webserver.
    pub fn with_custom_routes(routes: Router) -> Self {
        runtime::init();

        let scheduler = SmolScheduler::default();
        let flowgraphs = Arc::new(Mutex::new(Vec::new()));
        let id = RuntimeId(NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed));
        let handle = RuntimeHandle {
            runtime_id: id,
            flowgraphs: flowgraphs.clone(),
            scheduler: Arc::new(RuntimeSpawner {
                runtime_id: id,
                scheduler: scheduler.clone(),
            }),
        };

        Runtime {
            id,
            scheduler,
            flowgraphs,
            _control_port: ControlPort::new(handle, routes),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for Runtime<SmolScheduler> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S> Drop for Runtime<S> {
    fn drop(&mut self) {
        debug!("Runtime dropped");
    }
}

#[cfg(target_arch = "wasm32")]
impl<S> Drop for Runtime<S> {
    fn drop(&mut self) {
        debug!("Runtime dropped");
    }
}

#[cfg(target_arch = "wasm32")]
impl Runtime<WasmScheduler> {
    /// Construct a runtime using the WASM scheduler.
    pub fn new() -> Self {
        runtime::init();

        let flowgraphs = Arc::new(Mutex::new(Vec::new()));
        let id = RuntimeId(NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed));
        Runtime {
            id,
            scheduler: WasmScheduler::new(),
            flowgraphs,
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for Runtime<WasmScheduler> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<S: Scheduler> Runtime<S> {
    /// Create a flowgraph owned by this runtime.
    pub fn flowgraph(&self) -> Flowgraph {
        Flowgraph::new_with_runtime(Some(self.id))
    }

    fn validate_flowgraph(&self, fg: &Flowgraph) -> Result<(), Error> {
        if let Some(runtime_id) = fg.runtime_id
            && runtime_id != self.id
        {
            return Err(Error::RuntimeError(
                "flowgraph belongs to a different runtime".to_string(),
            ));
        }
        Ok(())
    }

    /// Spawn an async task on the runtime scheduler.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.scheduler.spawn(future)
    }

    /// Spawn an async task and detach its handle.
    pub fn spawn_background<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) {
        self.scheduler.spawn(future).detach();
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await initialization.
    ///
    /// Returns once the flowgraph is initialized and running. The returned
    /// [`RunningFlowgraph`] can be used to send messages, stop the graph, or
    /// wait for completion.
    pub async fn start_async(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        self.validate_flowgraph(&fg)?;
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = self.scheduler.spawn(run_flowgraph(
            fg,
            self.scheduler.clone(),
            fg_inbox.clone(),
            fg_inbox_rx,
            tx,
        ));

        rx.await
            .map_err(|_| Error::RuntimeError("run_flowgraph panicked".to_string()))??;

        let handle = FlowgraphHandle::new(fg_inbox);
        self.flowgraphs
            .try_lock()
            .ok_or(Error::LockError)?
            .push(handle.clone());

        Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
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

    /// Start a [`Flowgraph`] on the [`Runtime`] and await its termination.
    pub async fn run_async(&self, fg: Flowgraph) -> Result<Flowgraph, Error> {
        self.start_async(fg).await?.wait_async().await
    }

    /// Get the [`Scheduler`] that is associated with the [`Runtime`].
    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }
}

#[cfg(target_arch = "wasm32")]
impl<S: Scheduler> Runtime<S> {
    /// Create a flowgraph owned by this runtime.
    pub fn flowgraph(&self) -> Flowgraph {
        Flowgraph::new_with_runtime(Some(self.id))
    }

    fn validate_flowgraph(&self, fg: &Flowgraph) -> Result<(), Error> {
        if let Some(runtime_id) = fg.runtime_id
            && runtime_id != self.id
        {
            return Err(Error::RuntimeError(
                "flowgraph belongs to a different runtime".to_string(),
            ));
        }
        Ok(())
    }

    /// Spawn an async task on the runtime scheduler.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.scheduler.spawn(future)
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await initialization.
    pub async fn start_async(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        self.validate_flowgraph(&fg)?;
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = Task::spawn_local(run_flowgraph(fg, fg_inbox.clone(), fg_inbox_rx, tx));

        rx.await
            .map_err(|_| Error::RuntimeError("run_flowgraph panicked".to_string()))??;

        let handle = FlowgraphHandle::new(fg_inbox);
        self.flowgraphs
            .try_lock()
            .ok_or(Error::LockError)?
            .push(handle.clone());

        Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await its termination.
    pub async fn run_async(&self, fg: Flowgraph) -> Result<Flowgraph, Error> {
        self.start_async(fg).await?.wait_async().await
    }

    /// Get the [`Scheduler`] that is associated with the [`Runtime`].
    pub fn scheduler(&self) -> &S {
        &self.scheduler
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
        let id = RuntimeId(NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed));
        let handle = RuntimeHandle {
            runtime_id: id,
            flowgraphs: flowgraphs.clone(),
            scheduler: Arc::new(RuntimeSpawner {
                runtime_id: id,
                scheduler: scheduler.clone(),
            }),
        };

        Runtime {
            id,
            scheduler,
            flowgraphs,
            _control_port: ControlPort::new(handle, routes),
        }
    }

    /// Create a clonable [`RuntimeHandle`] for starting and querying flowgraphs.
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            runtime_id: self.id,
            flowgraphs: self.flowgraphs.clone(),
            scheduler: Arc::new(RuntimeSpawner {
                runtime_id: self.id,
                scheduler: self.scheduler.clone(),
            }),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl<S: Scheduler + Sync> Runtime<S> {
    /// Construct a [`Runtime`] with a custom [`Scheduler`].
    pub fn with_scheduler(scheduler: S) -> Self {
        runtime::init();

        let flowgraphs = Arc::new(Mutex::new(Vec::new()));
        let id = RuntimeId(NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed));
        Runtime {
            id,
            scheduler,
            flowgraphs,
        }
    }

    /// Create a clonable [`RuntimeHandle`] for starting and querying flowgraphs.
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            runtime_id: self.id,
            flowgraphs: self.flowgraphs.clone(),
            scheduler: Arc::new(self.scheduler.clone()),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
trait Spawn {
    async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error>;
}

#[cfg(not(target_arch = "wasm32"))]
struct RuntimeSpawner<S> {
    runtime_id: RuntimeId,
    scheduler: S,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<S> Spawn for RuntimeSpawner<S>
where
    S: SpawnBound,
{
    async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        if let Some(runtime_id) = fg.runtime_id
            && runtime_id != self.runtime_id
        {
            return Err(Error::RuntimeError(
                "flowgraph belongs to a different runtime".to_string(),
            ));
        }
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = self.scheduler.spawn(run_flowgraph(
            fg,
            self.scheduler.clone(),
            fg_inbox.clone(),
            fg_inbox_rx,
            tx,
        ));

        rx.await.or(Err(Error::RuntimeError(
            "run_flowgraph crashed".to_string(),
        )))??;

        let handle = FlowgraphHandle::new(fg_inbox);
        Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
    }
}

#[cfg(target_arch = "wasm32")]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<S: SpawnBound> Spawn for S {
    async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = Task::spawn_local(run_flowgraph(fg, fg_inbox.clone(), fg_inbox_rx, tx));

        rx.await.or(Err(Error::RuntimeError(
            "run_flowgraph crashed".to_string(),
        )))??;

        let handle = FlowgraphHandle::new(fg_inbox);
        Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
    }
}

/// Clonable runtime control handle used by web handlers and external control code.
///
/// A `RuntimeHandle` can start new flowgraphs on the same scheduler as the
/// owning [`Runtime`] and look up flowgraphs that have been registered with the
/// control plane.
#[derive(Clone)]
pub struct RuntimeHandle {
    runtime_id: RuntimeId,
    scheduler: Arc<DynSpawn>,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
}

impl fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHandle")
            .field("runtime_id", &self.runtime_id)
            .field("flowgraphs", &self.flowgraphs)
            .finish()
    }
}

impl PartialEq for RuntimeHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scheduler, &other.scheduler)
    }
}

impl RuntimeHandle {
    /// Start a [`Flowgraph`] on the runtime.
    pub async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let running = self.scheduler.start(fg).await?;
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

    let run_result: Result<(), Error> = async {
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

        initialized.send(Ok(())).map_err(|_| {
            Error::RuntimeError("main thread panic during flowgraph init".to_string())
        })?;

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
                        error!(
                            "Failed to send flowgraph description. Receiver may have disconnected."
                        );
                    }
                }
                FlowgraphMessage::Terminate => {
                    if !terminated {
                        for inbox in inboxes.iter_mut().flatten() {
                            if inbox.send(BlockMessage::Terminate).await.is_err() {
                                debug!(
                                    "runtime tried to terminate block that was already terminated"
                                );
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

    let run_result: Result<(), Error> = async {
        debug!("init blocks");
        let mut active_blocks = 0u32;
        for inbox in inboxes.iter_mut().flatten() {
            inbox.send(BlockMessage::Initialize).await?;
            active_blocks += 1;
        }

        debug!("wait for blocks init");
        let mut i = active_blocks;
        let mut queue = Vec::new();
        let mut block_error = false;
        while i > 0 {
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
                x => queue.push(x),
            }
        }

        for inbox in inboxes.iter_mut().flatten() {
            inbox.notify();
        }

        for m in queue.into_iter() {
            main_channel.try_send(m)?;
        }

        initialized.send(Ok(())).map_err(|_| {
            Error::RuntimeError("main thread panic during flowgraph init".to_string())
        })?;

        if block_error {
            main_channel.try_send(FlowgraphMessage::Terminate)?;
        }

        let mut terminated = false;
        while active_blocks > 0 {
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
                FlowgraphMessage::BlockDone { .. } => active_blocks -= 1,
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

                    let _ = tx.send(FlowgraphDescription {
                        blocks,
                        stream_edges: stream_edges_desc.clone(),
                        message_edges: message_edges_desc.clone(),
                    });
                }
                FlowgraphMessage::Terminate => {
                    if !terminated {
                        for inbox in inboxes.iter_mut().flatten() {
                            let _ = inbox.send(BlockMessage::Terminate).await;
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
    .await;

    if run_result.is_err() {
        for inbox in inboxes.iter_mut().flatten() {
            let _ = inbox.send(BlockMessage::Terminate).await;
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

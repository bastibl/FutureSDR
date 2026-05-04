#[cfg(not(target_arch = "wasm32"))]
use async_executor::LocalExecutor;
use async_lock::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use axum::Router;
use futures::channel::oneshot;
use futures::prelude::*;
use std::fmt;
use std::sync::Arc;

use crate::runtime;
use crate::runtime::BlockDescription;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
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
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::BoxBlock;
use crate::runtime::channel::mpsc::Receiver;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::channel::mpsc::channel;
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_block::StoredLocalBlock;
use crate::runtime::scheduler::Scheduler;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::scheduler::SmolScheduler;
use crate::runtime::scheduler::Task;
#[cfg(target_arch = "wasm32")]
use crate::runtime::scheduler::WasmScheduler;

#[cfg(not(target_arch = "wasm32"))]
trait SpawnBound: Scheduler + Sync + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Scheduler + Sync + 'static> SpawnBound for T {}

#[cfg(target_arch = "wasm32")]
trait SpawnBound: Scheduler + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: Scheduler + 'static> SpawnBound for T {}

#[cfg(not(target_arch = "wasm32"))]
type DynSpawn = dyn Spawn + Send + Sync + 'static;
#[cfg(target_arch = "wasm32")]
type DynSpawn = dyn Spawn + 'static;

/// Executor and control-plane owner for [`Flowgraph`]s and async tasks.
///
/// A [`Runtime`] owns a scheduler, starts flowgraphs, and provides a control
/// port on native targets. It is generic over the scheduler implementation, but
/// most applications can use [`Runtime::new`] with the default scheduler.
pub struct Runtime<S> {
    scheduler: S,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
    _control_port: ControlPort,
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
        let handle = RuntimeHandle {
            flowgraphs: flowgraphs.clone(),
            scheduler: Arc::new(scheduler.clone()),
        };

        Runtime {
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
        Runtime {
            scheduler: WasmScheduler,
            flowgraphs,
            _control_port: ControlPort::new(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for Runtime<WasmScheduler> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Scheduler> Runtime<S> {
    /// Spawn an async task on the runtime scheduler.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.scheduler.spawn(future)
    }

    /// Spawn an async task on the runtime scheduler.
    #[cfg(target_arch = "wasm32")]
    pub fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        self.scheduler.spawn(future)
    }

    /// Spawn an async task and detach its handle.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_background<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) {
        self.scheduler.spawn(future).detach();
    }

    /// Spawn a blocking task
    ///
    /// This is usually moved in a separate thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_blocking<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        self.scheduler.spawn_blocking(future)
    }

    /// Spawn a blocking task
    ///
    /// This is usually moved in a separate thread.
    #[cfg(target_arch = "wasm32")]
    pub fn spawn_blocking<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        self.scheduler.spawn_blocking(future)
    }

    /// Spawn a blocking task in the background
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_blocking_background<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) {
        self.scheduler.spawn_blocking(future).detach();
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await initialization.
    ///
    /// Returns once the flowgraph is initialized and running. The returned
    /// [`RunningFlowgraph`] can be used to send messages, stop the graph, or
    /// wait for completion.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn start_async(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        if fg.has_local_domains() {
            return Err(Error::RuntimeError(
                "flowgraphs with local domains currently require Runtime::run".to_string(),
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

        rx.await
            .map_err(|_| Error::RuntimeError("run_flowgraph panicked".to_string()))??;

        let handle = FlowgraphHandle::new(fg_inbox);
        self.flowgraphs
            .try_lock()
            .ok_or(Error::LockError)?
            .push(handle.clone());

        Ok(RunningFlowgraph::new(handle, FlowgraphTask::new(task)))
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and await initialization.
    ///
    /// On WASM, ordinary flowgraph blocks run through the local block path
    /// because the browser executor is single-threaded.
    #[cfg(target_arch = "wasm32")]
    pub async fn start_async(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = self.scheduler.spawn(run_flowgraph_local(
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        async_io::block_on(self.start_async(fg))
    }

    /// Start a [`Flowgraph`] on the [`Runtime`] and block until it terminates.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run(&self, fg: Flowgraph) -> Result<Flowgraph, Error> {
        if fg.has_local_domains() {
            return run_flowgraph_local_blocking(fg);
        }
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
            flowgraphs: flowgraphs.clone(),
            scheduler: Arc::new(scheduler.clone()),
        };

        Runtime {
            scheduler,
            flowgraphs,
            _control_port: ControlPort::new(handle, routes),
        }
    }

    /// Create a clonable [`RuntimeHandle`] for starting and querying flowgraphs.
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            flowgraphs: self.flowgraphs.clone(),
            scheduler: Arc::new(self.scheduler.clone()),
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
            _control_port: ControlPort::new(),
        }
    }

    /// Create a clonable [`RuntimeHandle`] for starting and querying flowgraphs.
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
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
#[async_trait::async_trait]
impl<S: SpawnBound> Spawn for S {
    async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = self.spawn(run_flowgraph(
            fg,
            self.clone(),
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
#[async_trait::async_trait(?Send)]
impl<S: SpawnBound> Spawn for S {
    async fn start(&self, fg: Flowgraph) -> Result<RunningFlowgraph, Error> {
        let queue_size = config::config().queue_size;
        let (fg_inbox, fg_inbox_rx) = channel::<FlowgraphMessage>(queue_size);

        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let task = self.spawn(run_flowgraph_local(
            fg,
            self.clone(),
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

/// Clonable runtime control handle used by web handlers and external control code.
///
/// A `RuntimeHandle` can start new flowgraphs on the same scheduler as the
/// owning [`Runtime`] and look up flowgraphs that have been registered with the
/// control plane.
#[derive(Clone)]
pub struct RuntimeHandle {
    scheduler: Arc<DynSpawn>,
    flowgraphs: Arc<Mutex<Vec<FlowgraphHandle>>>,
}

impl fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeHandle")
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
enum LocalFinishedBlock {
    Normal(BlockId, BoxBlock),
    Local(BlockId, StoredLocalBlock),
}

#[cfg(not(target_arch = "wasm32"))]
fn run_flowgraph_local_blocking(fg: Flowgraph) -> Result<Flowgraph, Error> {
    async_io::block_on(async move {
        let queue_size = config::config().queue_size;
        let (main_channel, main_rx) = channel::<FlowgraphMessage>(queue_size);
        run_flowgraph_local(fg, main_channel, main_rx).await
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_flowgraph_local(
    mut fg: Flowgraph,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
) -> Result<Flowgraph, Error> {
    debug!("in run_flowgraph_local");

    let normal_blocks = fg.take_blocks()?;
    let local_blocks = fg.take_local_blocks()?;

    let mut inboxes: Vec<BlockInbox> = normal_blocks
        .iter()
        .map(|b| b.inbox())
        .chain(local_blocks.iter().map(|b| b.as_ref().inbox()))
        .collect();

    let ex = LocalExecutor::new();
    let mut block_tasks = Vec::with_capacity(normal_blocks.len() + local_blocks.len());

    for block in normal_blocks {
        let main_channel = main_channel.clone();
        let task = ex.spawn(async move {
            let mut block = block;
            let id = block.id();
            block.run(main_channel).await;
            LocalFinishedBlock::Normal(id, block)
        });
        block_tasks.push(task);
    }

    for block in local_blocks {
        let main_channel = main_channel.clone();
        let task = ex.spawn(async move {
            let mut block = block;
            let id = block.as_ref().id();
            block.as_mut().run(main_channel).await;
            LocalFinishedBlock::Local(id, block)
        });
        block_tasks.push(task);
    }

    let run = async {
        let mut active_blocks = 0u32;
        for inbox in inboxes.iter_mut() {
            inbox.send(BlockMessage::Initialize).await?;
            active_blocks += 1;
        }

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

        for inbox in inboxes.iter_mut() {
            inbox.notify();
        }

        for m in queue.into_iter() {
            main_channel.try_send(m)?;
        }

        if block_error {
            main_channel.try_send(FlowgraphMessage::Terminate)?;
        }

        let mut terminated = false;
        while active_blocks > 0 {
            let m = main_rx.recv().await.ok_or_else(|| {
                Error::RuntimeError("all senders to flowgraph inbox dropped".to_string())
            })?;

            match m {
                FlowgraphMessage::BlockDone { .. } => {
                    active_blocks -= 1;
                }
                FlowgraphMessage::BlockError { .. } => {
                    block_error = true;
                    active_blocks -= 1;
                    let _ = main_channel.send(FlowgraphMessage::Terminate).await;
                }
                FlowgraphMessage::Terminate => {
                    if !terminated {
                        for inbox in inboxes.iter_mut() {
                            if inbox.send(BlockMessage::Terminate).await.is_err() {
                                debug!(
                                    "runtime tried to terminate block that was already terminated"
                                );
                            }
                        }
                        terminated = true;
                    }
                }
                _ => warn!("local main loop received unhandled message"),
            }
        }

        if block_error {
            return Err(Error::RuntimeError("A block raised an error".to_string()));
        }

        Ok(())
    };

    let run_result = ex.run(run).await;

    if run_result.is_err() {
        for inbox in inboxes.iter_mut() {
            if inbox.send(BlockMessage::Terminate).await.is_err() {
                debug!("runtime tried to terminate block during shutdown cleanup");
            }
        }
    }

    let mut normal_finished = Vec::new();
    let mut local_finished = Vec::new();
    for task in block_tasks {
        match task.await {
            LocalFinishedBlock::Normal(id, block) => normal_finished.push((id, block)),
            LocalFinishedBlock::Local(id, block) => local_finished.push((id, block)),
        }
    }

    fg.restore_blocks(normal_finished)?;
    fg.restore_local_blocks(local_finished)?;

    run_result?;
    Ok(fg)
}

#[cfg(target_arch = "wasm32")]
async fn run_flowgraph_local<S: Scheduler>(
    mut fg: Flowgraph,
    scheduler: S,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Result<Flowgraph, Error> {
    debug!("in run_flowgraph_local");

    let blocks = fg.take_blocks()?;
    let mut inboxes: Vec<BlockInbox> = blocks.iter().map(|b| b.as_ref().inbox()).collect();
    let ids: Vec<_> = blocks.iter().map(|b| b.as_ref().id()).collect();
    let mut block_tasks = Vec::with_capacity(blocks.len());

    for block in blocks {
        let main_channel = main_channel.clone();
        let task = if block.as_ref().is_blocking() {
            scheduler.spawn_blocking(async move {
                let mut block = block;
                let id = block.as_ref().id();
                block.as_mut().run(main_channel).await;
                (id, block)
            })
        } else {
            scheduler.spawn(async move {
                let mut block = block;
                let id = block.as_ref().id();
                block.as_mut().run(main_channel).await;
                (id, block)
            })
        };
        block_tasks.push(task);
    }

    let run_result: Result<(), Error> = async {
        debug!("init blocks");
        let mut active_blocks = 0u32;
        for inbox in inboxes.iter_mut() {
            inbox.send(BlockMessage::Initialize).await?;
            active_blocks += 1;
        }

        debug!("wait for blocks init");
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
        for inbox in inboxes.iter_mut() {
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
                    if let Some(inbox) = inboxes.get_mut(block_id.0) {
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
                    if let Some(inbox) = inboxes.get_mut(block_id.0) {
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
                    if let Some(ref mut b) = inboxes.get_mut(block_id.0) {
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
                        if let Some(inbox) = inboxes.get_mut(id.0)
                            && inbox
                                .send(BlockMessage::BlockDescription { tx: b_tx })
                                .await
                                .is_ok()
                        {
                            blocks.push(rx.await?);
                        }
                    }

                    let stream_edges = fg.stream_edges.clone();
                    let message_edges = fg.message_edges.clone();

                    if tx
                        .send(FlowgraphDescription {
                            blocks,
                            stream_edges,
                            message_edges,
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
                        for inbox in inboxes.iter_mut() {
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
        for inbox in inboxes.iter_mut() {
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn run_flowgraph<S: Scheduler>(
    mut fg: Flowgraph,
    scheduler: S,
    main_channel: Sender<FlowgraphMessage>,
    main_rx: Receiver<FlowgraphMessage>,
    initialized: oneshot::Sender<Result<(), Error>>,
) -> Result<Flowgraph, Error> {
    debug!("in run_flowgraph");

    let blocks = fg.take_blocks()?;
    let mut inboxes: Vec<BlockInbox> = blocks.iter().map(|b| b.inbox()).collect();
    let ids: Vec<_> = blocks.iter().map(|b| b.id()).collect();
    let block_tasks = scheduler.run_flowgraph(blocks, &main_channel);

    let run_result: Result<(), Error> = async {
        debug!("init blocks");
        // init blocks
        let mut active_blocks = 0u32;
        for inbox in inboxes.iter_mut() {
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
        for inbox in inboxes.iter_mut() {
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
                    if let Some(inbox) = inboxes.get_mut(block_id.0) {
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
                    if let Some(inbox) = inboxes.get_mut(block_id.0) {
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
                    if let Some(ref mut b) = inboxes.get_mut(block_id.0) {
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
                        if let Some(inbox) = inboxes.get_mut(id.0)
                            && inbox
                                .send(BlockMessage::BlockDescription { tx: b_tx })
                                .await
                                .is_ok()
                        {
                            blocks.push(rx.await?);
                        }
                    }

                    let stream_edges = fg.stream_edges.clone();
                    let message_edges = fg.message_edges.clone();

                    if tx
                        .send(FlowgraphDescription {
                            blocks,
                            stream_edges,
                            message_edges,
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
                        for inbox in inboxes.iter_mut() {
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
        for inbox in inboxes.iter_mut() {
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

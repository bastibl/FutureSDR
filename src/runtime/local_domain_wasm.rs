use futures::Future;
use futures::channel::oneshot;
use futures::future::Either;
use slab::Slab;
use std::cell::UnsafeCell;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;
use web_sys::Worker;
use web_sys::WorkerOptions;
use web_sys::WorkerType;

use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::PortId;
use crate::runtime::block::BlockObject;
use crate::runtime::block::LocalBlock;
use crate::runtime::block_inbox::BlockInboxReader;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::dev::BlockInbox;

pub(crate) type LocalBlockBuilder =
    Box<dyn FnOnce(BlockInbox, BlockInboxReader) -> Box<dyn LocalBlock> + Send + 'static>;

type LocalDomainAsyncExec = Box<
    dyn for<'a> FnOnce(&'a mut LocalDomainState) -> Pin<Box<dyn Future<Output = ()> + 'a>>
        + Send
        + 'static,
>;

type TopologyEdge = (BlockId, PortId, BlockId, PortId);

pub(crate) struct LocalDomainState {
    blocks: Vec<Option<Box<dyn LocalBlock>>>,
    stream_edges: Vec<TopologyEdge>,
    message_edges: Vec<TopologyEdge>,
}

impl LocalDomainState {
    pub(crate) fn new() -> Self {
        Self {
            blocks: Vec::new(),
            stream_edges: Vec::new(),
            message_edges: Vec::new(),
        }
    }

    pub(crate) fn add_stream_edge(&mut self, edge: TopologyEdge) {
        self.stream_edges.push(edge);
    }

    pub(crate) fn add_message_edge(&mut self, edge: TopologyEdge) {
        self.message_edges.push(edge);
    }

    pub(crate) fn topology(&self) -> (Vec<TopologyEdge>, Vec<TopologyEdge>) {
        (self.stream_edges.clone(), self.message_edges.clone())
    }

    pub(crate) fn insert_block(
        &mut self,
        local_id: usize,
        block: Box<dyn LocalBlock>,
    ) -> Result<(), Error> {
        if self.blocks.len() <= local_id {
            self.blocks.resize_with(local_id + 1, || None);
        }
        if self.blocks[local_id].is_some() {
            return Err(Error::RuntimeError(format!(
                "local block slot {local_id} was inserted more than once"
            )));
        }
        self.blocks[local_id] = Some(block);
        Ok(())
    }

    pub(crate) fn block(
        &self,
        local_id: usize,
        block_id: BlockId,
    ) -> Result<&dyn BlockObject, Error> {
        self.blocks
            .get(local_id)
            .and_then(Option::as_ref)
            .map(|block| block.as_ref() as &dyn BlockObject)
            .ok_or(Error::InvalidBlock(block_id))
    }

    pub(crate) fn block_mut(
        &mut self,
        local_id: usize,
        block_id: BlockId,
    ) -> Result<&mut dyn BlockObject, Error> {
        self.blocks
            .get_mut(local_id)
            .and_then(Option::as_mut)
            .map(|block| block.as_mut() as &mut dyn BlockObject)
            .ok_or(Error::InvalidBlock(block_id))
    }

    pub(crate) fn two_blocks_mut(
        &mut self,
        src: (usize, BlockId),
        dst: (usize, BlockId),
    ) -> Result<(&mut dyn BlockObject, &mut dyn BlockObject), Error> {
        let (src_local, src_id) = src;
        let (dst_local, dst_id) = dst;
        if src_local == dst_local {
            return Err(Error::LockError);
        }
        let invalid_block = if src_local >= self.blocks.len() {
            src_id
        } else {
            dst_id
        };
        let [src_slot, dst_slot] = self
            .blocks
            .get_disjoint_mut([src_local, dst_local])
            .map_err(|err| match err {
                std::slice::GetDisjointMutError::IndexOutOfBounds => {
                    Error::InvalidBlock(invalid_block)
                }
                std::slice::GetDisjointMutError::OverlappingIndices => Error::LockError,
            })?;
        let src_block = src_slot.as_mut().ok_or(Error::LockError)?.as_mut();
        let dst_block = dst_slot.as_mut().ok_or(Error::LockError)?.as_mut();
        Ok((src_block, dst_block))
    }
}

struct SpinReply<T> {
    state: Arc<SpinReplyState<T>>,
}

struct SpinReplyReceiver<T> {
    state: Arc<SpinReplyState<T>>,
}

struct SpinReplyState<T> {
    ready: AtomicI32,
    value: UnsafeCell<Option<T>>,
}

// `value` is written exactly once before `ready` is set with Release ordering.
// Receivers read it only after observing `ready` with Acquire ordering.
unsafe impl<T: Send> Send for SpinReplyState<T> {}
unsafe impl<T: Send> Sync for SpinReplyState<T> {}

fn spin_reply<T>() -> (SpinReply<T>, SpinReplyReceiver<T>) {
    let state = Arc::new(SpinReplyState {
        ready: AtomicI32::new(0),
        value: UnsafeCell::new(None),
    });
    (
        SpinReply {
            state: state.clone(),
        },
        SpinReplyReceiver { state },
    )
}

impl<T> SpinReply<T> {
    fn send(self, value: T) {
        unsafe {
            *self.state.value.get() = Some(value);
        }
        self.state.ready.store(1, Ordering::Release);
        let ready = atomic_i32_view(&self.state.ready);
        let _ = js_sys::Atomics::notify(&ready, 0);
    }
}

impl<T> SpinReplyReceiver<T> {
    fn recv(self) -> Result<T, Error> {
        if web_sys::window().is_some() {
            return Err(Error::RuntimeError(
                "synchronous WASM local-domain operation on the browser thread".to_string(),
            ));
        }

        while self.state.ready.load(Ordering::Acquire) == 0 {
            let ready = atomic_i32_view(&self.state.ready);
            let result =
                js_sys::Atomics::wait_with_timeout(&ready, 0, 0, 10_000.0).map_err(|e| {
                    Error::RuntimeError(format!("waiting for local domain failed: {e:?}"))
                })?;
            if result == "timed-out" {
                return Err(Error::RuntimeError(
                    "timed out waiting for WASM local domain".to_string(),
                ));
            }
        }

        unsafe {
            Ok((*self.state.value.get())
                .take()
                .expect("reply value missing"))
        }
    }
}

fn atomic_i32_view(value: &AtomicI32) -> js_sys::Int32Array {
    // The returned view is used immediately for one Atomics operation and is
    // not kept across allocations or awaits.
    unsafe { js_sys::Int32Array::view_mut_raw(value as *const AtomicI32 as *mut i32, 1) }
}

enum LocalDomainMessage {
    Build {
        local_id: usize,
        builder: LocalBlockBuilder,
        inbox: BlockInbox,
        inbox_rx: BlockInboxReader,
    },
    Exec(Box<dyn FnOnce(&mut LocalDomainState) + Send + 'static>),
    ExecAsync(LocalDomainAsyncExec),
    Run {
        main_channel: Sender<FlowgraphMessage>,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Terminate,
}

pub(crate) struct LocalDomainRuntime {
    controller: LocalDomainController,
    blocks: usize,
    running: bool,
}

impl LocalDomainRuntime {
    pub(crate) fn new() -> Self {
        Self::try_new().expect("failed to create local domain")
    }

    pub(crate) fn try_new() -> Result<Self, Error> {
        Ok(Self {
            controller: LocalDomainController::try_new()?,
            blocks: 0,
            running: false,
        })
    }

    pub(crate) fn reserve_block(&mut self) -> usize {
        let local_id = self.blocks;
        self.blocks += 1;
        local_id
    }

    pub(crate) fn block_count(&self) -> usize {
        self.blocks
    }

    pub(crate) fn reserve_blocks(&mut self, n: usize) {
        self.blocks += n;
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn handle(&self) -> LocalDomainHandle {
        self.controller.handle()
    }

    pub(crate) fn build(
        &self,
        local_id: usize,
        builder: LocalBlockBuilder,
    ) -> Result<BlockInbox, Error> {
        self.controller.build(local_id, builder)
    }

    pub(crate) fn exec<R>(
        &self,
        f: impl FnOnce(&mut LocalDomainState) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        self.controller.exec(f)
    }

    pub(crate) async fn exec_async<R>(
        &self,
        f: impl for<'a> FnOnce(
            &'a mut LocalDomainState,
        ) -> Pin<Box<dyn Future<Output = Result<R, Error>> + 'a>>
        + Send
        + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        self.controller.exec_async(f).await
    }

    pub(crate) fn topology(
        &self,
    ) -> impl Future<Output = Result<(Vec<TopologyEdge>, Vec<TopologyEdge>), Error>> + Send + 'static
    {
        let handle = self.handle();
        async move {
            handle
                .exec_async(|state| Box::pin(async move { Ok(state.topology()) }))
                .await
        }
    }

    pub(crate) fn run_if_needed(
        &mut self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<Option<oneshot::Receiver<Result<(), Error>>>, Error> {
        if self.blocks == 0 {
            return Ok(None);
        }
        let task = self.controller.run(main_channel)?;
        self.running = true;
        Ok(Some(task))
    }

    pub(crate) fn mark_stopped(&mut self) {
        self.running = false;
    }
}

pub(crate) struct LocalDomainController {
    tx: kanal::Sender<LocalDomainMessage>,
    terminate: Arc<AtomicBool>,
    worker: Option<WasmWorker>,
    domain_id: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct LocalDomainHandle {
    tx: kanal::Sender<LocalDomainMessage>,
}

impl LocalDomainHandle {
    pub(crate) fn exec<R>(
        &self,
        f: impl FnOnce(&mut LocalDomainState) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        let (reply, rx) = spin_reply();
        self.tx
            .send(LocalDomainMessage::Exec(Box::new(move |state| {
                reply.send(f(state));
            })))
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.recv()?
    }

    pub(crate) async fn exec_async<R>(
        &self,
        f: impl for<'a> FnOnce(
            &'a mut LocalDomainState,
        ) -> Pin<Box<dyn Future<Output = Result<R, Error>> + 'a>>
        + Send
        + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LocalDomainMessage::ExecAsync(Box::new(move |state| {
                Box::pin(async move {
                    let _ = reply.send(f(state).await);
                })
            })))
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }
}

impl LocalDomainController {
    pub(crate) fn try_new() -> Result<Self, Error> {
        let (tx, rx) = kanal::unbounded();
        let terminate = Arc::new(AtomicBool::new(false));
        let init = WasmLocalDomainInit {
            rx,
            terminate: terminate.clone(),
        };
        let domain_id = WASM_LOCAL_DOMAINS.lock().unwrap().insert(init);
        let worker_script = default_worker_script();
        let worker = spawn_local_domain_worker(&worker_script, domain_id).map_err(|e| {
            let _ = WASM_LOCAL_DOMAINS.lock().unwrap().try_remove(domain_id);
            Error::RuntimeError(format!(
                "failed to spawn WASM local-domain worker from {worker_script:?}: {e:?}. \
                 Serve a worker script that dispatches FutureSDR scheduler/local-domain init \
                 messages, or configure it with \
                 futuresdr::runtime::scheduler::wasm::set_worker_script(path)."
            ))
        })?;

        Ok(Self {
            tx,
            terminate,
            worker: Some(worker),
            domain_id: Some(domain_id),
        })
    }

    pub(crate) fn handle(&self) -> LocalDomainHandle {
        LocalDomainHandle {
            tx: self.tx.clone(),
        }
    }

    pub(crate) fn build(
        &self,
        local_id: usize,
        builder: LocalBlockBuilder,
    ) -> Result<BlockInbox, Error> {
        // Return the sender side immediately so flowgraph construction on the
        // browser thread does not busy-wait for the local-domain worker. FIFO
        // ordering on `tx` still guarantees the Build message is handled before
        // later Exec/Run messages from the same controller.
        let (inbox, inbox_rx) =
            crate::runtime::block_inbox::channel(crate::runtime::config::config().queue_size);
        self.tx
            .send(LocalDomainMessage::Build {
                local_id,
                builder,
                inbox: inbox.clone(),
                inbox_rx,
            })
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        Ok(inbox)
    }

    pub(crate) fn exec<R>(
        &self,
        f: impl FnOnce(&mut LocalDomainState) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        self.handle().exec(f)
    }

    pub(crate) async fn exec_async<R>(
        &self,
        f: impl for<'a> FnOnce(
            &'a mut LocalDomainState,
        ) -> Pin<Box<dyn Future<Output = Result<R, Error>> + 'a>>
        + Send
        + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        self.handle().exec_async(f).await
    }

    pub(crate) fn run(
        &self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<oneshot::Receiver<Result<(), Error>>, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LocalDomainMessage::Run {
                main_channel,
                reply,
            })
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        Ok(rx)
    }
}

impl Drop for LocalDomainController {
    fn drop(&mut self) {
        self.terminate.store(true, Ordering::Release);
        let _ = self.tx.send(LocalDomainMessage::Terminate);
        if let Some(id) = self.domain_id.take() {
            let _ = WASM_LOCAL_DOMAINS.lock().unwrap().try_remove(id);
        }
        if let Some(worker) = self.worker.take() {
            worker.terminate();
        }
    }
}

static WASM_LOCAL_DOMAINS: once_cell::sync::Lazy<Mutex<Slab<WasmLocalDomainInit>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Slab::new()));

struct WasmLocalDomainInit {
    rx: kanal::Receiver<LocalDomainMessage>,
    terminate: Arc<AtomicBool>,
}

struct WasmWorker(Worker);

unsafe impl Send for WasmWorker {}
unsafe impl Sync for WasmWorker {}

impl WasmWorker {
    fn terminate(self) {
        self.0.terminate();
    }
}

fn default_worker_script() -> String {
    crate::runtime::scheduler::wasm::worker_script()
}

/// WASM local-domain worker entry point.
///
/// Application worker scripts should call this after initializing the generated
/// wasm-bindgen module with the `module` and `memory` values sent by the local
/// domain runtime.
#[wasm_bindgen]
pub fn futuresdr_wasm_local_domain_worker_entry(domain_id: usize) {
    crate::runtime::init();
    let init = WASM_LOCAL_DOMAINS.lock().unwrap().try_remove(domain_id);
    if let Some(init) = init {
        wasm_bindgen_futures::spawn_local(async move {
            run_domain_worker(init).await;
        });
    } else {
        error!(
            "WASM local-domain worker got invalid domain id {}",
            domain_id
        );
    }
}

fn spawn_local_domain_worker(worker_script: &str, domain_id: usize) -> Result<WasmWorker, JsValue> {
    crate::runtime::scheduler::wasm::reset_wasm_thread_metadata();

    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let worker = Worker::new_with_options(worker_script, &options)?;
    let init = js_sys::Object::new();

    js_sys::Reflect::set(
        &init,
        &JsValue::from_str("type"),
        &JsValue::from_str("futuresdr-wasm-local-domain-init"),
    )?;
    js_sys::Reflect::set(&init, &JsValue::from_str("module"), &wasm_bindgen::module())?;
    js_sys::Reflect::set(&init, &JsValue::from_str("memory"), &wasm_bindgen::memory())?;
    js_sys::Reflect::set(
        &init,
        &JsValue::from_str("domain_id"),
        &JsValue::from_f64(domain_id as f64),
    )?;
    if let Err(e) = worker.post_message(&init) {
        worker.terminate();
        return Err(e);
    }

    Ok(WasmWorker(worker))
}

async fn run_domain_worker(init: WasmLocalDomainInit) {
    let WasmLocalDomainInit { rx, terminate } = init;
    let mut state = LocalDomainState::new();

    loop {
        let message = match rx.try_recv() {
            Ok(Some(message)) => message,
            Ok(None) => {
                gloo_timers::future::TimeoutFuture::new(1).await;
                continue;
            }
            Err(_) => break,
        };

        match message {
            LocalDomainMessage::Build {
                local_id,
                builder,
                inbox,
                inbox_rx,
            } => {
                let block = builder(inbox, inbox_rx);
                if let Err(e) = state.insert_block(local_id, block) {
                    error!("failed to insert local block: {e}");
                }
            }
            LocalDomainMessage::Exec(f) => f(&mut state),
            LocalDomainMessage::ExecAsync(f) => f(&mut state).await,
            LocalDomainMessage::Run {
                main_channel,
                reply,
            } => {
                let result = run_local_domain(&mut state, main_channel, terminate.clone()).await;
                let _ = reply.send(result);
                if terminate.load(Ordering::Acquire) {
                    break;
                }
            }
            LocalDomainMessage::Terminate => break,
        }
    }
}

async fn run_local_domain(
    state: &mut LocalDomainState,
    main_channel: Sender<FlowgraphMessage>,
    terminate: Arc<AtomicBool>,
) -> Result<(), Error> {
    let mut tasks = Vec::new();
    let mut inboxes = Vec::new();

    for (local_id, slot) in state.blocks.iter_mut().enumerate() {
        if let Some(block) = slot.take() {
            inboxes.push(block.as_ref().inbox());
            let main_channel = main_channel.clone();
            let (tx, rx) = oneshot::channel();
            wasm_bindgen_futures::spawn_local(async move {
                let mut block = block;
                block.as_mut().run(main_channel).await;
                let _ = tx.send((local_id, block));
            });
            tasks.push(rx);
        }
    }

    let run_tasks = async move {
        let mut finished = Vec::with_capacity(tasks.len());
        for task in tasks {
            finished.push(
                task.await
                    .map_err(|_| Error::RuntimeError("local block task canceled".to_string()))?,
            );
        }
        Ok::<_, Error>(finished)
    };
    let terminate_task = async {
        while !terminate.load(Ordering::Acquire) {
            gloo_timers::future::TimeoutFuture::new(1).await;
        }
    };
    futures::pin_mut!(run_tasks);
    futures::pin_mut!(terminate_task);

    let finished = match futures::future::select(run_tasks, terminate_task).await {
        Either::Left((finished, _)) => finished?,
        Either::Right((_, run_tasks)) => {
            for inbox in inboxes {
                if inbox.send(BlockMessage::Terminate).await.is_err() {
                    debug!("local domain tried to terminate block that was already terminated");
                }
            }
            run_tasks.await?
        }
    };

    finished
        .into_iter()
        .try_for_each(|(local_id, block)| state.insert_block(local_id, block))
}

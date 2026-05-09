#[cfg(not(target_arch = "wasm32"))]
use async_executor::LocalExecutor;
use futures::channel::oneshot;
use futures::future::Either;
#[cfg(target_arch = "wasm32")]
use slab::Slab;
#[cfg(target_arch = "wasm32")]
use std::cell::UnsafeCell;
#[cfg(target_arch = "wasm32")]
use std::hint::spin_loop;
#[cfg(target_arch = "wasm32")]
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::AtomicBool;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc as sync_mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::Worker;
#[cfg(target_arch = "wasm32")]
use web_sys::WorkerOptions;
#[cfg(target_arch = "wasm32")]
use web_sys::WorkerType;

use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::block::BlockObject;
use crate::runtime::block::LocalBlock;
use crate::runtime::channel::mpsc::Sender;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;

pub(crate) type LocalBlockBuilder = Box<dyn FnOnce() -> Box<dyn LocalBlock> + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type LocalExecutorFactory = Box<dyn FnOnce() -> LocalExecutor<'static> + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
type SyncReply<T> = oneshot::Sender<Result<T, Error>>;
#[cfg(target_arch = "wasm32")]
type SyncReply<T> = SpinReply<Result<T, Error>>;

pub(crate) struct LocalDomainState {
    blocks: Vec<Option<Box<dyn LocalBlock>>>,
}

impl LocalDomainState {
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

#[cfg(target_arch = "wasm32")]
struct SpinReply<T> {
    state: Arc<SpinReplyState<T>>,
}

#[cfg(target_arch = "wasm32")]
struct SpinReplyReceiver<T> {
    state: Arc<SpinReplyState<T>>,
}

#[cfg(target_arch = "wasm32")]
struct SpinReplyState<T> {
    ready: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

#[cfg(target_arch = "wasm32")]
unsafe impl<T: Send> Send for SpinReplyState<T> {}
#[cfg(target_arch = "wasm32")]
unsafe impl<T: Send> Sync for SpinReplyState<T> {}

#[cfg(target_arch = "wasm32")]
fn spin_reply<T>() -> (SpinReply<T>, SpinReplyReceiver<T>) {
    let state = Arc::new(SpinReplyState {
        ready: AtomicBool::new(false),
        value: UnsafeCell::new(None),
    });
    (
        SpinReply {
            state: state.clone(),
        },
        SpinReplyReceiver { state },
    )
}

#[cfg(target_arch = "wasm32")]
impl<T> SpinReply<T> {
    fn send(self, value: T) {
        unsafe {
            *self.state.value.get() = Some(value);
        }
        self.state.ready.store(true, Ordering::Release);
    }
}

#[cfg(target_arch = "wasm32")]
impl<T> SpinReplyReceiver<T> {
    fn recv(self) -> T {
        while !self.state.ready.load(Ordering::Acquire) {
            spin_loop();
        }
        unsafe {
            (*self.state.value.get())
                .take()
                .expect("reply value missing")
        }
    }
}

enum LocalDomainMessage {
    Build {
        local_id: usize,
        builder: LocalBlockBuilder,
        reply: SyncReply<BlockInbox>,
    },
    Exec(Box<dyn FnOnce(&mut LocalDomainState) + Send + 'static>),
    Run {
        main_channel: Sender<FlowgraphMessage>,
        #[cfg(not(target_arch = "wasm32"))]
        executor_factory: LocalExecutorFactory,
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
        Self {
            controller: LocalDomainController::new(),
            blocks: 0,
            running: false,
        }
    }

    pub(crate) fn reserve_block(&mut self) -> usize {
        let local_id = self.blocks;
        self.blocks += 1;
        local_id
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

    pub(crate) fn run_if_needed(
        &mut self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<Option<oneshot::Receiver<Result<(), Error>>>, Error> {
        if self.blocks == 0 {
            return Ok(None);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let task = self
            .controller
            .run(main_channel, default_local_executor_factory())?;
        #[cfg(target_arch = "wasm32")]
        let task = self.controller.run(main_channel)?;
        self.running = true;
        Ok(Some(task))
    }

    pub(crate) fn mark_stopped(&mut self) {
        self.running = false;
    }
}

pub(crate) struct LocalDomainController {
    #[cfg(not(target_arch = "wasm32"))]
    tx: sync_mpsc::Sender<LocalDomainMessage>,
    #[cfg(target_arch = "wasm32")]
    tx: kanal::Sender<LocalDomainMessage>,
    #[cfg(not(target_arch = "wasm32"))]
    terminate_tx: Option<oneshot::Sender<()>>,
    #[cfg(target_arch = "wasm32")]
    terminate: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    worker: Option<WasmWorker>,
    #[cfg(target_arch = "wasm32")]
    domain_id: Option<usize>,
    #[cfg(not(target_arch = "wasm32"))]
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct LocalDomainHandle {
    #[cfg(not(target_arch = "wasm32"))]
    tx: sync_mpsc::Sender<LocalDomainMessage>,
    #[cfg(target_arch = "wasm32")]
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(LocalDomainMessage::Exec(Box::new(move |state| {
                    let _ = reply.send(f(state));
                })))
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
            async_io::block_on(rx)
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (reply, rx) = spin_reply();
            self.tx
                .send(LocalDomainMessage::Exec(Box::new(move |state| {
                    reply.send(f(state));
                })))
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
            rx.recv()
        }
    }
}

impl LocalDomainController {
    pub(crate) fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = sync_mpsc::channel();
            let (terminate_tx, terminate_rx) = oneshot::channel();
            let join = thread::Builder::new()
                .stack_size(config::config().stack_size)
                .name("futuresdr-local".to_string())
                .spawn(move || run_domain_thread(rx, terminate_rx))
                .expect("failed to spawn local domain thread");

            Self {
                tx,
                terminate_tx: Some(terminate_tx),
                join: Some(join),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (tx, rx) = kanal::unbounded();
            let terminate = Arc::new(AtomicBool::new(false));
            let init = WasmLocalDomainInit {
                rx,
                terminate: terminate.clone(),
            };
            let domain_id = WASM_LOCAL_DOMAINS.lock().unwrap().insert(init);
            let worker = match spawn_local_domain_worker(default_worker_script(), domain_id) {
                Ok(worker) => worker,
                Err(e) => {
                    let _ = WASM_LOCAL_DOMAINS.lock().unwrap().try_remove(domain_id);
                    panic!("failed to spawn WASM local-domain worker: {e:?}");
                }
            };

            Self {
                tx,
                terminate,
                worker: Some(worker),
                domain_id: Some(domain_id),
            }
        }
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(LocalDomainMessage::Build {
                    local_id,
                    builder,
                    reply,
                })
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
            async_io::block_on(rx)
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (reply, rx) = spin_reply();
            self.tx
                .send(LocalDomainMessage::Build {
                    local_id,
                    builder,
                    reply,
                })
                .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
            rx.recv()
        }
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn run(
        &self,
        main_channel: Sender<FlowgraphMessage>,
        executor_factory: LocalExecutorFactory,
    ) -> Result<oneshot::Receiver<Result<(), Error>>, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LocalDomainMessage::Run {
                main_channel,
                executor_factory,
                reply,
            })
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        Ok(rx)
    }

    #[cfg(target_arch = "wasm32")]
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(terminate_tx) = self.terminate_tx.take() {
                let _ = terminate_tx.send(());
            }
            let _ = self.tx.send(LocalDomainMessage::Terminate);
            if let Some(join) = self.join.take()
                && join.join().is_err()
            {
                debug!("local domain thread panicked during shutdown");
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
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
}

#[cfg(target_arch = "wasm32")]
static WASM_LOCAL_DOMAINS: once_cell::sync::Lazy<Mutex<Slab<WasmLocalDomainInit>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Slab::new()));

#[cfg(target_arch = "wasm32")]
struct WasmLocalDomainInit {
    rx: kanal::Receiver<LocalDomainMessage>,
    terminate: Arc<AtomicBool>,
}

#[cfg(target_arch = "wasm32")]
struct WasmWorker(Worker);

#[cfg(target_arch = "wasm32")]
unsafe impl Send for WasmWorker {}
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for WasmWorker {}

#[cfg(target_arch = "wasm32")]
impl WasmWorker {
    fn terminate(self) {
        self.0.terminate();
    }
}

#[cfg(target_arch = "wasm32")]
fn default_worker_script() -> &'static str {
    crate::runtime::scheduler::wasm::DEFAULT_WORKER_SCRIPT
}

/// WASM local-domain worker entry point.
///
/// Application worker scripts should call this after initializing the generated
/// wasm-bindgen module with the `module` and `memory` values sent by the local
/// domain runtime.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn futuresdr_wasm_local_domain_worker_entry(domain_id: usize) {
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
async fn run_domain_worker(init: WasmLocalDomainInit) {
    let WasmLocalDomainInit { rx, terminate } = init;
    let mut state = LocalDomainState { blocks: Vec::new() };

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
                reply,
            } => {
                let block = builder();
                let inbox = block.as_ref().inbox();
                let result = insert_at(&mut state.blocks, local_id, block).map(|_| inbox);
                reply.send(result);
            }
            LocalDomainMessage::Exec(f) => f(&mut state),
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

#[cfg(not(target_arch = "wasm32"))]
fn run_domain_thread(
    rx: sync_mpsc::Receiver<LocalDomainMessage>,
    mut terminate_rx: oneshot::Receiver<()>,
) {
    let mut state = LocalDomainState { blocks: Vec::new() };

    while let Ok(message) = rx.recv() {
        match message {
            LocalDomainMessage::Build {
                local_id,
                builder,
                reply,
            } => {
                let block = builder();
                let inbox = block.as_ref().inbox();
                let result = insert_at(&mut state.blocks, local_id, block).map(|_| inbox);
                let _ = reply.send(result);
            }
            LocalDomainMessage::Exec(f) => f(&mut state),
            LocalDomainMessage::Run {
                main_channel,
                executor_factory,
                reply,
            } => {
                let executor = executor_factory();
                let result = async_io::block_on(run_local_domain(
                    &mut state,
                    executor,
                    main_channel,
                    &mut terminate_rx,
                ));
                let _ = reply.send(result);
            }
            LocalDomainMessage::Terminate => break,
        }
    }
}

fn insert_at(
    blocks: &mut Vec<Option<Box<dyn LocalBlock>>>,
    local_id: usize,
    block: Box<dyn LocalBlock>,
) -> Result<(), Error> {
    if blocks.len() <= local_id {
        blocks.resize_with(local_id + 1, || None);
    }
    if blocks[local_id].is_some() {
        return Err(Error::RuntimeError(format!(
            "local block slot {local_id} was inserted more than once"
        )));
    }
    blocks[local_id] = Some(block);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
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
        .try_for_each(|(local_id, block)| insert_at(&mut state.blocks, local_id, block))
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_local_domain(
    state: &mut LocalDomainState,
    ex: LocalExecutor<'static>,
    main_channel: Sender<FlowgraphMessage>,
    terminate_rx: &mut oneshot::Receiver<()>,
) -> Result<(), Error> {
    let mut tasks = Vec::new();
    let mut inboxes = Vec::new();

    for (local_id, slot) in state.blocks.iter_mut().enumerate() {
        if let Some(block) = slot.take() {
            inboxes.push(block.as_ref().inbox());
            let main_channel = main_channel.clone();
            let task = ex.spawn(async move {
                let mut block = block;
                block.as_mut().run(main_channel).await;
                (local_id, block)
            });
            tasks.push(task);
        }
    }

    let finished = ex
        .run(async move {
            let run_tasks = async move {
                let mut finished = Vec::with_capacity(tasks.len());
                for task in tasks {
                    finished.push(task.await);
                }
                finished
            };
            futures::pin_mut!(run_tasks);

            match futures::future::select(run_tasks, terminate_rx).await {
                Either::Left((finished, _)) => finished,
                Either::Right((_, run_tasks)) => {
                    for inbox in inboxes {
                        if inbox.send(BlockMessage::Terminate).await.is_err() {
                            debug!(
                                "local domain tried to terminate block that was already terminated"
                            );
                        }
                    }
                    run_tasks.await
                }
            }
        })
        .await;

    finished
        .into_iter()
        .try_for_each(|(local_id, block)| insert_at(&mut state.blocks, local_id, block))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn default_local_executor_factory() -> LocalExecutorFactory {
    Box::new(LocalExecutor::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use crate::runtime::BlockId;
    use crate::runtime::BlockPortCtx;
    use crate::runtime::PortId;
    use crate::runtime::block::BlockObject;
    use crate::runtime::block_inbox;
    use crate::runtime::block_inbox::BlockInboxReader;
    use crate::runtime::buffer::BufferReader;

    struct WaitForTerminate {
        id: BlockId,
        inbox: BlockInbox,
        inbox_rx: BlockInboxReader,
    }

    impl WaitForTerminate {
        fn new(id: BlockId) -> Self {
            let (inbox, inbox_rx) = block_inbox::channel(4);
            Self {
                id,
                inbox,
                inbox_rx,
            }
        }
    }

    impl BlockObject for WaitForTerminate {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn inbox(&self) -> BlockInbox {
            self.inbox.clone()
        }

        fn id(&self) -> BlockId {
            self.id
        }

        fn stream_input(&mut self, id: &PortId) -> Result<&mut dyn BufferReader, Error> {
            Err(Error::InvalidStreamPort(
                BlockPortCtx::Id(self.id),
                id.clone(),
            ))
        }

        fn connect_stream_output(
            &mut self,
            id: &PortId,
            _reader: &mut dyn BufferReader,
        ) -> Result<(), Error> {
            Err(Error::InvalidStreamPort(
                BlockPortCtx::Id(self.id),
                id.clone(),
            ))
        }

        fn message_inputs(&self) -> &'static [&'static str] {
            &[]
        }

        fn connect(
            &mut self,
            _src_port: &PortId,
            _sender: BlockInbox,
            _dst_port: &PortId,
        ) -> Result<(), Error> {
            Ok(())
        }

        fn type_name(&self) -> &str {
            "WaitForTerminate"
        }

        fn is_blocking(&self) -> bool {
            false
        }
    }

    #[async_trait::async_trait(?Send)]
    impl LocalBlock for WaitForTerminate {
        async fn run(&mut self, main_inbox: Sender<FlowgraphMessage>) {
            while let Some(message) = self.inbox_rx.recv().await {
                if matches!(message, BlockMessage::Terminate) {
                    break;
                }
            }

            let _ = main_inbox
                .send(FlowgraphMessage::BlockDone { block_id: self.id })
                .await;
        }
    }

    #[test]
    fn controller_drop_terminates_running_local_blocks() -> Result<(), Error> {
        let controller = LocalDomainController::new();
        controller.build(0, Box::new(|| Box::new(WaitForTerminate::new(BlockId(0)))))?;
        let (main_tx, _main_rx) = crate::runtime::channel::mpsc::channel(4);
        let run = controller.run(main_tx, default_local_executor_factory())?;

        drop(controller);

        async_io::block_on(run)
            .map_err(|_| Error::RuntimeError("local domain task canceled".to_string()))?
    }
}

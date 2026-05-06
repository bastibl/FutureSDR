#[cfg(not(target_arch = "wasm32"))]
use async_executor::LocalExecutor;
#[cfg(not(target_arch = "wasm32"))]
use futures::channel::oneshot;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::Error;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::FlowgraphMessage;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::channel::mpsc::Sender;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::config;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::dev::BlockInbox;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_block::StoredLocalBlock;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type LocalBlockBuilder = Box<dyn FnOnce() -> StoredLocalBlock + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type LocalExecutorFactory = Box<dyn FnOnce() -> LocalExecutor<'static> + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
type SyncReply<T> = mpsc::Sender<Result<T, Error>>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct LocalDomainState {
    pub(crate) blocks: Vec<Option<StoredLocalBlock>>,
}

#[cfg(not(target_arch = "wasm32"))]
enum LocalDomainMessage {
    Build {
        local_id: usize,
        builder: LocalBlockBuilder,
        reply: SyncReply<BlockInbox>,
    },
    InsertBlock {
        local_id: usize,
        block: StoredLocalBlock,
        reply: SyncReply<BlockInbox>,
    },
    Exec(Box<dyn FnOnce(&mut LocalDomainState) + Send + 'static>),
    Run {
        main_channel: Sender<FlowgraphMessage>,
        executor_factory: LocalExecutorFactory,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Terminate,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct LocalDomainController {
    tx: mpsc::Sender<LocalDomainMessage>,
    join: Option<thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalDomainController {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .stack_size(config::config().stack_size)
            .name("futuresdr-local".to_string())
            .spawn(move || run_domain_thread(rx))
            .expect("failed to spawn local domain thread");

        Self {
            tx,
            join: Some(join),
        }
    }

    pub(crate) fn build(
        &self,
        local_id: usize,
        builder: LocalBlockBuilder,
    ) -> Result<BlockInbox, Error> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(LocalDomainMessage::Build {
                local_id,
                builder,
                reply,
            })
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.recv()
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }

    pub(crate) fn insert_block(
        &self,
        local_id: usize,
        block: StoredLocalBlock,
    ) -> Result<BlockInbox, Error> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(LocalDomainMessage::InsertBlock {
                local_id,
                block,
                reply,
            })
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.recv()
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }

    pub(crate) fn exec<R>(
        &self,
        f: impl FnOnce(&mut LocalDomainState) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
    {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(LocalDomainMessage::Exec(Box::new(move |state| {
                let _ = reply.send(f(state));
            })))
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.recv()
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }

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
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for LocalDomainController {
    fn drop(&mut self) {
        let _ = self.tx.send(LocalDomainMessage::Terminate);
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            debug!("local domain thread panicked during shutdown");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_domain_thread(rx: mpsc::Receiver<LocalDomainMessage>) {
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
            LocalDomainMessage::InsertBlock {
                local_id,
                block,
                reply,
            } => {
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
                let result =
                    async_io::block_on(run_local_domain(&mut state, executor, main_channel));
                let _ = reply.send(result);
            }
            LocalDomainMessage::Terminate => break,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_at(
    blocks: &mut Vec<Option<StoredLocalBlock>>,
    local_id: usize,
    block: StoredLocalBlock,
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

#[cfg(not(target_arch = "wasm32"))]
async fn run_local_domain(
    state: &mut LocalDomainState,
    ex: LocalExecutor<'static>,
    main_channel: Sender<FlowgraphMessage>,
) -> Result<(), Error> {
    let mut tasks = Vec::new();

    for (local_id, slot) in state.blocks.iter_mut().enumerate() {
        if let Some(block) = slot.take() {
            let main_channel = main_channel.clone();
            let task = ex.spawn(async move {
                let mut block = block;
                block.as_mut().run(main_channel).await;
                (local_id, block)
            });
            tasks.push(task);
        }
    }

    ex.run(async move {
        let mut finished = Vec::with_capacity(tasks.len());
        for task in tasks {
            finished.push(task.await);
        }
        finished
    })
    .await
    .into_iter()
    .try_for_each(|(local_id, block)| insert_at(&mut state.blocks, local_id, block))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn default_local_executor_factory() -> LocalExecutorFactory {
    Box::new(LocalExecutor::new)
}

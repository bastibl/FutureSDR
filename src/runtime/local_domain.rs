use async_executor::LocalExecutor;
use futures::Future;
use futures::channel::oneshot;
use futures::future::Either;
use std::pin::Pin;
use std::sync::mpsc as sync_mpsc;
use std::thread;

use crate::runtime::BlockId;
use crate::runtime::BlockMessage;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::block::BlockObject;
use crate::runtime::block::LocalBlock;
use crate::runtime::block_inbox::BlockInboxReader;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;

pub(crate) type LocalBlockBuilder =
    Box<dyn FnOnce(BlockInbox, BlockInboxReader) -> Box<dyn LocalBlock> + Send + 'static>;

type LocalDomainAsyncExec = Box<
    dyn for<'a> FnOnce(&'a mut LocalDomainState) -> Pin<Box<dyn Future<Output = ()> + 'a>>
        + Send
        + 'static,
>;

pub(crate) type LocalExecutorFactory = Box<dyn FnOnce() -> LocalExecutor<'static> + Send + 'static>;

pub(crate) struct LocalDomainState {
    blocks: Vec<Option<Box<dyn LocalBlock>>>,
    stream_edges: Vec<(
        BlockId,
        crate::runtime::PortId,
        BlockId,
        crate::runtime::PortId,
    )>,
    message_edges: Vec<(
        BlockId,
        crate::runtime::PortId,
        BlockId,
        crate::runtime::PortId,
    )>,
}

impl LocalDomainState {
    pub(crate) fn new() -> Self {
        Self {
            blocks: Vec::new(),
            stream_edges: Vec::new(),
            message_edges: Vec::new(),
        }
    }

    pub(crate) fn add_stream_edge(
        &mut self,
        edge: (
            BlockId,
            crate::runtime::PortId,
            BlockId,
            crate::runtime::PortId,
        ),
    ) {
        self.stream_edges.push(edge);
    }

    pub(crate) fn add_message_edge(
        &mut self,
        edge: (
            BlockId,
            crate::runtime::PortId,
            BlockId,
            crate::runtime::PortId,
        ),
    ) {
        self.message_edges.push(edge);
    }

    pub(crate) fn topology(
        &self,
    ) -> (
        Vec<(
            BlockId,
            crate::runtime::PortId,
            BlockId,
            crate::runtime::PortId,
        )>,
        Vec<(
            BlockId,
            crate::runtime::PortId,
            BlockId,
            crate::runtime::PortId,
        )>,
    ) {
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
    ) -> Result<
        (
            Vec<(
                BlockId,
                crate::runtime::PortId,
                BlockId,
                crate::runtime::PortId,
            )>,
            Vec<(
                BlockId,
                crate::runtime::PortId,
                BlockId,
                crate::runtime::PortId,
            )>,
        ),
        Error,
    > {
        self.exec(|state| Ok(state.topology()))
    }

    pub(crate) fn run_if_needed(
        &mut self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<Option<oneshot::Receiver<Result<(), Error>>>, Error> {
        if self.blocks == 0 {
            return Ok(None);
        }
        let task = self
            .controller
            .run(main_channel, default_local_executor_factory())?;
        self.running = true;
        Ok(Some(task))
    }

    pub(crate) fn mark_stopped(&mut self) {
        self.running = false;
    }
}

pub(crate) struct LocalDomainController {
    tx: sync_mpsc::Sender<LocalDomainMessage>,
    terminate_tx: Option<oneshot::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct LocalDomainHandle {
    tx: sync_mpsc::Sender<LocalDomainMessage>,
}

impl LocalDomainHandle {
    pub(crate) fn exec<R>(
        &self,
        f: impl FnOnce(&mut LocalDomainState) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, Error>
    where
        R: Send + 'static,
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
    pub(crate) fn new() -> Self {
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
        // Return the sender side immediately so flowgraph construction can wire
        // message ports without a synchronous round-trip to the local domain.
        // FIFO ordering on `tx` still guarantees the Build message is handled
        // before later Exec/Run messages from the same controller.
        let (inbox, inbox_rx) = crate::runtime::block_inbox::channel(config::config().queue_size);
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

impl Drop for LocalDomainController {
    fn drop(&mut self) {
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
}

fn run_domain_thread(
    rx: sync_mpsc::Receiver<LocalDomainMessage>,
    mut terminate_rx: oneshot::Receiver<()>,
) {
    let mut state = LocalDomainState::new();

    while let Ok(message) = rx.recv() {
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
            LocalDomainMessage::ExecAsync(f) => async_io::block_on(f(&mut state)),
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
        .try_for_each(|(local_id, block)| state.insert_block(local_id, block))
}

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
    use crate::runtime::block_inbox::BlockInboxReader;
    use crate::runtime::buffer::BufferReader;

    struct WaitForTerminate {
        id: BlockId,
        inbox: BlockInbox,
        inbox_rx: BlockInboxReader,
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
        controller.build(
            0,
            Box::new(|inbox, inbox_rx| {
                Box::new(WaitForTerminate {
                    id: BlockId(0),
                    inbox,
                    inbox_rx,
                })
            }),
        )?;
        let (main_tx, _main_rx) = crate::runtime::channel::mpsc::channel(4);
        let run = controller.run(main_tx, default_local_executor_factory())?;

        drop(controller);

        async_io::block_on(run)
            .map_err(|_| Error::RuntimeError("local domain task canceled".to_string()))?
    }
}

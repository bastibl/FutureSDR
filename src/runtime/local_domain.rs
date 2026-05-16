use async_executor::LocalExecutor;
use futures::Future;
use futures::future::Either;
use std::pin::Pin;
use std::thread;

use crate::runtime::BlockMessage;
use crate::runtime::Edge;
use crate::runtime::Error;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::channel::oneshot;
use crate::runtime::config;
use crate::runtime::dev::BlockInbox;
use crate::runtime::local_domain_common::LocalBlockBuilder;
use crate::runtime::local_domain_common::LocalDomainMessage;
use crate::runtime::local_domain_common::LocalDomainState;

pub(crate) struct LocalDomainRuntime {
    controller: LocalDomainController,
    blocks: usize,
    running: bool,
}

impl LocalDomainRuntime {
    pub(crate) fn new() -> Result<Self, Error> {
        Ok(Self {
            controller: LocalDomainController::new()?,
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

    pub(crate) async fn build(
        &self,
        local_id: usize,
        builder: LocalBlockBuilder,
    ) -> Result<BlockInbox, Error> {
        self.controller.build(local_id, builder).await
    }

    pub(crate) async fn exec<R>(
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
        self.controller.exec(f).await
    }

    pub(crate) async fn run_if_needed(
        &mut self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<Option<oneshot::Receiver<Result<(), Error>>>, Error> {
        if self.blocks == 0 {
            return Ok(None);
        }
        let task = self.controller.run(main_channel).await?;
        self.running = true;
        Ok(Some(task))
    }

    pub(crate) fn mark_stopped(&mut self) {
        self.running = false;
    }
}

pub(crate) struct LocalDomainController {
    tx: Sender<LocalDomainMessage>,
    terminate_tx: Option<oneshot::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct LocalDomainHandle {
    tx: Sender<LocalDomainMessage>,
}

impl LocalDomainHandle {
    pub(crate) async fn topology_async(&self) -> Result<(Vec<Edge>, Vec<Edge>), Error> {
        self.exec(|state| Box::pin(async move { Ok(state.topology()) }))
            .await
    }

    pub(crate) async fn exec<R>(
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
            .send(LocalDomainMessage::Exec(Box::new(move |state| {
                Box::pin(async move {
                    let _ = reply.send(f(state).await);
                })
            })))
            .await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }
}

impl LocalDomainController {
    pub(crate) fn new() -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel(config::config().queue_size);
        let (terminate_tx, terminate_rx) = oneshot::channel();
        let join = thread::Builder::new()
            .stack_size(config::config().stack_size)
            .name("futuresdr-local".to_string())
            .spawn(move || crate::runtime::block_on(run_domain_thread(rx, terminate_rx)))
            .map_err(|e| {
                Error::RuntimeError(format!("failed to spawn local domain thread: {e}"))
            })?;

        Ok(Self {
            tx,
            terminate_tx: Some(terminate_tx),
            join: Some(join),
        })
    }

    pub(crate) fn handle(&self) -> LocalDomainHandle {
        LocalDomainHandle {
            tx: self.tx.clone(),
        }
    }

    pub(crate) async fn build(
        &self,
        local_id: usize,
        builder: LocalBlockBuilder,
    ) -> Result<BlockInbox, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LocalDomainMessage::Build {
                local_id,
                builder,
                reply,
            })
            .await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        rx.await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?
    }

    pub(crate) async fn exec<R>(
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
        self.handle().exec(f).await
    }

    pub(crate) async fn run(
        &self,
        main_channel: Sender<FlowgraphMessage>,
    ) -> Result<oneshot::Receiver<Result<(), Error>>, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LocalDomainMessage::Run {
                main_channel,
                reply,
            })
            .await
            .map_err(|_| Error::RuntimeError("local domain terminated".to_string()))?;
        Ok(rx)
    }
}

impl Drop for LocalDomainController {
    fn drop(&mut self) {
        if let Some(terminate_tx) = self.terminate_tx.take() {
            let _ = terminate_tx.send(());
        }
        let _ = self.tx.try_send(LocalDomainMessage::Terminate);
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            debug!("local domain thread panicked during shutdown");
        }
    }
}

async fn run_domain_thread(
    rx: mpsc::Receiver<LocalDomainMessage>,
    mut terminate_rx: oneshot::Receiver<()>,
) {
    let mut state = LocalDomainState::new();

    while let Some(message) = rx.recv().await {
        match message {
            LocalDomainMessage::Build {
                local_id,
                builder,
                reply,
            } => {
                let block = builder();
                let inbox = block.inbox();
                let result = state.insert_block(local_id, block).map(|_| inbox);
                if let Err(e) = &result {
                    error!("failed to insert local block: {e}");
                }
                let _ = reply.send(result);
            }
            LocalDomainMessage::Exec(f) => f(&mut state).await,
            LocalDomainMessage::Run {
                main_channel,
                reply,
            } => {
                let result = run_local_domain(
                    &mut state,
                    LocalExecutor::new(),
                    main_channel,
                    &mut terminate_rx,
                )
                .await;
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

    for (local_id, slot) in state.block_slots_mut() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use crate::runtime::BlockId;
    use crate::runtime::BlockPortCtx;
    use crate::runtime::PortId;
    use crate::runtime::block::BlockObject;
    use crate::runtime::block::LocalBlock;
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
        let controller = LocalDomainController::new()?;
        crate::runtime::block_on(controller.build(
            0,
            Box::new(|| {
                let (inbox, inbox_rx) = crate::runtime::block_inbox::channel(4);
                Box::new(WaitForTerminate {
                    id: BlockId(0),
                    inbox,
                    inbox_rx,
                })
            }),
        ))?;
        let (main_tx, _main_rx) = crate::runtime::channel::mpsc::channel(4);
        let run = crate::runtime::block_on(controller.run(main_tx))?;

        drop(controller);

        crate::runtime::block_on(run)
            .map_err(|_| Error::RuntimeError("local domain task canceled".to_string()))?
    }
}

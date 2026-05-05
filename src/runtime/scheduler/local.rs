use async_executor::LocalExecutor;
use futures::channel::oneshot;
use std::thread;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::config;
use crate::runtime::local_block::StoredLocalBlock;
use crate::runtime::scheduler::LocalScheduler;
use crate::runtime::scheduler::scheduler::LocalTask;

/// Native scheduler for one local domain.
///
/// Each domain gets its own OS thread and a single-thread local executor.
#[derive(Clone, Debug, Default)]
pub struct ThreadLocalScheduler;

impl LocalScheduler for ThreadLocalScheduler {
    fn run_domain(
        &self,
        blocks: Vec<StoredLocalBlock>,
        main_channel: Sender<FlowgraphMessage>,
    ) -> LocalTask<Vec<(BlockId, StoredLocalBlock)>> {
        let (tx, rx) = oneshot::channel();

        thread::Builder::new()
            .stack_size(config::config().stack_size)
            .name("futuresdr-local".to_string())
            .spawn(move || {
                let finished = async_io::block_on(run_local_domain(blocks, main_channel));
                if tx.send(finished).is_err() {
                    debug!("local domain could not deliver final blocks");
                }
            })
            .expect("failed to spawn local domain thread");

        LocalTask::new(rx)
    }
}

async fn run_local_domain(
    blocks: Vec<StoredLocalBlock>,
    main_channel: Sender<FlowgraphMessage>,
) -> Vec<(BlockId, StoredLocalBlock)> {
    let ex = LocalExecutor::new();
    let mut tasks = Vec::with_capacity(blocks.len());

    for block in blocks {
        let main_channel = main_channel.clone();
        let task = ex.spawn(async move {
            let mut block = block;
            let id = block.as_ref().id();
            block.as_mut().run(main_channel).await;
            (id, block)
        });
        tasks.push(task);
    }

    ex.run(async move {
        let mut finished = Vec::with_capacity(tasks.len());
        for task in tasks {
            finished.push(task.await);
        }
        finished
    })
    .await
}

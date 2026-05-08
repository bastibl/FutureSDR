use futures::future::Future;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::block::Block;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::scheduler::Task;

/// Scheduler trait for normal send-capable runtime work.
///
/// A scheduler decides how normal block tasks and detached async tasks are run.
/// Schedulers receive a full scheduling domain of send-capable blocks;
/// local-domain blocks are handled separately by the runtime.
pub trait Scheduler: Clone + Send + 'static {
    /// Run one normal scheduling domain.
    ///
    /// Implementations spawn each block and return task handles that resolve to
    /// the block id and final block object. The runtime uses those handles to
    /// restore block state into the finished flowgraph.
    fn run_domain(
        &self,
        blocks: Vec<Box<dyn Block>>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, Box<dyn Block>)>>;

    /// Spawn an independent async task on this scheduler.
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static)
    -> Task<T>;
}

use futures::future::Future;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::flowgraph::NormalStoredBlock;
use crate::runtime::scheduler::Task;

/// Scheduler trait
///
/// This has to be implemented for every scheduler.
pub trait Scheduler: Clone + Send + 'static {
    /// Run one normal scheduling domain.
    fn run_domain(
        &self,
        blocks: Vec<NormalStoredBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>>;

    /// Spawn a task
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static)
    -> Task<T>;
}

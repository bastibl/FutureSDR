use futures::future::Future;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::Block;
use crate::runtime::channel::mpsc::Sender;
#[cfg(target_arch = "wasm32")]
use crate::runtime::dev::BlockObject;
use crate::runtime::scheduler::Task;

/// Scheduler trait
///
/// This has to be implemented for every scheduler.
pub trait Scheduler: Clone + Send + 'static {
    /// Run one normal scheduling domain.
    #[cfg(not(target_arch = "wasm32"))]
    fn run_domain(
        &self,
        blocks: Vec<Box<dyn Block>>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, Box<dyn Block>)>>;

    /// Run one normal scheduling domain.
    #[cfg(target_arch = "wasm32")]
    fn run_domain(
        &self,
        blocks: Vec<Box<dyn BlockObject>>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, Box<dyn BlockObject>)>>;

    /// Spawn a task
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static)
    -> Task<T>;
}

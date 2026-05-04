use futures::future::Future;

#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::BlockId;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::FlowgraphMessage;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::block::BoxBlock;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::scheduler::Task;

/// Scheduler trait
///
/// This has to be implemented for every scheduler.
#[cfg(not(target_arch = "wasm32"))]
pub trait Scheduler: Clone + Send + 'static {
    /// Run a whole [`Flowgraph`](crate::runtime::Flowgraph) on the
    /// [`Runtime`](crate::runtime::Runtime)
    fn run_flowgraph(
        &self,
        blocks: Vec<BoxBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, BoxBlock)>>;

    /// Spawn a task
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static)
    -> Task<T>;

    /// Spawn a blocking task in a separate thread
    fn spawn_blocking<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T>;
}

#[cfg(target_arch = "wasm32")]
/// Single-thread WASM scheduler trait.
pub trait Scheduler: Clone + 'static {
    /// Spawn a task
    fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;

    /// Spawn a blocking task in a separate thread
    fn spawn_blocking<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;
}

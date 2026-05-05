use futures::future::Future;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::flowgraph::NormalStoredBlock;
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
        blocks: Vec<NormalStoredBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>>;

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
    /// Run a whole [`Flowgraph`](crate::runtime::Flowgraph) on the
    /// [`Runtime`](crate::runtime::Runtime)
    fn run_flowgraph(
        &self,
        blocks: Vec<NormalStoredBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>>;

    /// Spawn a task
    fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;

    /// Spawn a blocking task in a separate thread
    fn spawn_blocking<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;
}

use async_lock::Mutex;
use crossfire::MAsyncTx;
use futures::future::Future;
use std::sync::Arc;

use crate::runtime::Block;
use crate::runtime::FlowgraphMessage;
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
        blocks: Vec<Arc<Mutex<dyn Block>>>,
        main_channel: &MAsyncTx<FlowgraphMessage>,
    );

    /// Spawn a task
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static)
    -> Task<T>;

    /// Spawn a blocking task in a separate thread
    fn spawn_blocking<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T>;
}

/// Scheduler trait
///
/// This has to be implemented for every scheduler.
#[cfg(target_arch = "wasm32")]
pub trait Scheduler: Clone + Send + 'static {
    /// Run a whole [`Flowgraph`](crate::runtime::Flowgraph) on the
    /// [`Runtime`](crate::runtime::Runtime)
    fn run_flowgraph(
        &self,
        blocks: Vec<Arc<Mutex<dyn Block>>>,
        main_channel: &MAsyncTx<FlowgraphMessage>,
    );

    /// Spawn a task
    fn spawn<T: Send + 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;

    /// Spawn a blocking task in a separate thread
    fn spawn_blocking<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + 'static,
    ) -> Task<T>;
}

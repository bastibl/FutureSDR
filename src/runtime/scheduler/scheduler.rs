#[cfg(not(target_arch = "wasm32"))]
use futures::channel::oneshot;
use futures::future::Future;
#[cfg(not(target_arch = "wasm32"))]
use futures::task::Context;
#[cfg(not(target_arch = "wasm32"))]
use futures::task::Poll;
#[cfg(not(target_arch = "wasm32"))]
use std::pin::Pin;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::flowgraph::NormalStoredBlock;
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::local_block::StoredLocalBlock;
use crate::runtime::scheduler::Task;

/// Scheduler trait
///
/// This has to be implemented for every scheduler.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
/// Single-thread WASM scheduler trait.
pub trait Scheduler: Clone + 'static {
    /// Run one normal scheduling domain.
    fn run_domain(
        &self,
        blocks: Vec<NormalStoredBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>>;

    /// Spawn a task
    fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T>;
}

/// Scheduler for single-thread local domains.
#[cfg(not(target_arch = "wasm32"))]
pub trait LocalScheduler: Clone + Send + 'static {
    /// Run one local scheduling domain.
    fn run_domain(
        &self,
        blocks: Vec<StoredLocalBlock>,
        main_channel: Sender<FlowgraphMessage>,
    ) -> LocalTask<Vec<(BlockId, StoredLocalBlock)>>;
}

/// Join handle for a local-domain thread.
#[cfg(not(target_arch = "wasm32"))]
pub struct LocalTask<T>(oneshot::Receiver<T>);

#[cfg(not(target_arch = "wasm32"))]
impl<T> LocalTask<T> {
    /// Create a local task from an oneshot receiver.
    pub(crate) fn new(rx: oneshot::Receiver<T>) -> Self {
        Self(rx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<T> Future for LocalTask<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let rx = &mut self.get_mut().0;
        match Pin::new(rx).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(v)) => Poll::Ready(v),
            Poll::Ready(Err(_)) => panic!("local domain task canceled"),
        }
    }
}

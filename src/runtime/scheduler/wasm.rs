//! WASM Scheduler
use futures::channel::oneshot;
use futures::future::Future;
use futures::task::Context;
use futures::task::Poll;
use std::pin::Pin;

use crate::runtime::BlockId;
use crate::runtime::FlowgraphMessage;
use crate::runtime::channel::mpsc::Sender;
use crate::runtime::flowgraph::NormalStoredBlock;
use crate::runtime::scheduler::Scheduler;

/// WASM Scheduler
///
/// This is at the moment a dummy implementation, as there is no mutli-threaded executor available
/// on wasm.
#[derive(Clone, Debug)]
pub struct WasmScheduler;

impl WasmScheduler {
    /// Create WASM Scheduler
    pub fn new() -> WasmScheduler {
        WasmScheduler
    }
}

impl Scheduler for WasmScheduler {
    fn run_domain(
        &self,
        blocks: Vec<NormalStoredBlock>,
        _main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>> {
        drop(blocks);
        panic!("wasm flowgraph blocks are run through Flowgraph's local compatibility shim")
    }

    fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        Task::spawn_local(future)
    }
}

/// WASM Async Task
pub struct Task<T>(oneshot::Receiver<T>);

impl<T: 'static> Task<T> {
    pub(crate) fn spawn_local(future: impl Future<Output = T> + 'static) -> Self {
        let (tx, rx) = oneshot::channel::<T>();
        wasm_bindgen_futures::spawn_local(async move {
            let t = future.await;
            if tx.send(t).is_err() {
                debug!("task cannot deliver final result");
            }
        });

        Task(rx)
    }
}

impl<T> Task<T> {
    /// Detach from Task (dummy function for WASM)
    pub fn detach(self) {}
}

impl<T> std::future::Future for Task<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let rx = &mut self.get_mut().0;

        match Pin::new(rx).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(v)) => Poll::Ready(v),
            Poll::Ready(Err(_)) => {
                panic!("Task canceled")
            }
        }
    }
}

impl Default for WasmScheduler {
    fn default() -> Self {
        Self::new()
    }
}

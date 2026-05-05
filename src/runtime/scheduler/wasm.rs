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
#[derive(Clone, Debug)]
pub struct WasmScheduler;

/// Placeholder normal scheduler type for WASM runtimes.
pub type DummyScheduler = WasmScheduler;

/// Local scheduler type for WASM runtimes.
pub type WasmLocalScheduler = WasmScheduler;

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
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>> {
        let mut tasks = Vec::with_capacity(blocks.len());
        for block in blocks {
            let main_channel = main_channel.clone();
            let task = self.spawn(async move {
                let mut block = block;
                let id = block.as_ref().id();
                block.as_mut().run(main_channel).await;
                (id, block)
            });
            tasks.push(task);
        }
        tasks
    }

    fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
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

/// WASM Async Task
pub struct Task<T>(oneshot::Receiver<T>);

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

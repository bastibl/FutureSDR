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

impl WasmScheduler {
    /// Create WASM Scheduler
    pub fn new() -> WasmScheduler {
        WasmScheduler
    }
}

impl Scheduler for WasmScheduler {
    fn run_flowgraph(
        &self,
        blocks: Vec<NormalStoredBlock>,
        main_channel: &Sender<FlowgraphMessage>,
    ) -> Vec<Task<(BlockId, NormalStoredBlock)>> {
        let mut tasks = Vec::with_capacity(blocks.len());
        for block in blocks {
            let main_channel = main_channel.clone();
            let task = if block.as_ref().is_blocking() {
                self.spawn_blocking(async move {
                    let mut block = block;
                    let id = block.as_ref().id();
                    block.as_mut().run(main_channel).await;
                    (id, block)
                })
            } else {
                self.spawn(async move {
                    let mut block = block;
                    let id = block.as_ref().id();
                    block.as_mut().run(main_channel).await;
                    (id, block)
                })
            };
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

    fn spawn_blocking<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        info!("no spawn blocking for wasm, using spawn");
        self.spawn(future)
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

//! WASM Scheduler
use futures::channel::oneshot;
use futures::future::Future;
use futures::task::Context;
use futures::task::Poll;
use std::pin::Pin;

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

use std::future::Future;
use std::time::Duration;

/// Cross-target timer used by FutureSDR async code.
pub struct Timer;

impl Timer {
    /// Complete after `duration` has elapsed.
    pub fn after(duration: Duration) -> impl Future<Output = ()> + Send {
        #[cfg(not(target_arch = "wasm32"))]
        {
            async move {
                async_io::Timer::after(duration).await;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                wasm_bindgen_futures::spawn_local(async move {
                    gloo_timers::future::sleep(duration).await;
                    let _ = tx.send(());
                });
                let _ = rx.await;
            }
        }
    }
}

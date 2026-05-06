use std::future::Future;
use std::time::Duration;

/// Cross-target timer used by FutureSDR async code.
pub struct Timer;

impl Timer {
    /// Complete after `duration` has elapsed.
    pub fn after(duration: Duration) -> impl Future<Output = ()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            async move {
                async_io::Timer::after(duration).await;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            async move {
                gloo_timers::future::sleep(duration).await;
            }
        }
    }
}

use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

/// Cross-target timer used by FutureSDR async code.
pub struct Timer {
    #[cfg(not(target_arch = "wasm32"))]
    inner: async_io::Timer,
    #[cfg(target_arch = "wasm32")]
    inner: gloo_timers::future::TimeoutFuture,
}

impl Timer {
    /// Complete after `duration` has elapsed.
    pub fn after(duration: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                inner: async_io::Timer::after(duration),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                inner: gloo_timers::future::sleep(duration),
            }
        }
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Pin::new(&mut self.inner).poll(cx).map(|_| ())
        }
        #[cfg(target_arch = "wasm32")]
        {
            Pin::new(&mut self.inner).poll(cx)
        }
    }
}

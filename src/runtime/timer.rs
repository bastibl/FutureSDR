use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

/// Cross-target timer used by FutureSDR async code.
pub struct Timer {
    #[cfg(not(target_arch = "wasm32"))]
    inner: Pin<Box<dyn Future<Output = ()> + Send>>,
    #[cfg(target_arch = "wasm32")]
    inner: Pin<Box<dyn Future<Output = ()>>>,
}

impl Timer {
    /// Complete after `duration` has elapsed.
    pub fn after(duration: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                inner: Box::pin(async move {
                    async_io::Timer::after(duration).await;
                }),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                inner: Box::pin(async move {
                    gloo_timers::future::sleep(duration).await;
                }),
            }
        }
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.as_mut().poll(cx)
    }
}

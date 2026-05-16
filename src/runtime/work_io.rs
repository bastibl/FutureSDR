use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// Work-loop control flags returned from [`Kernel::work`](crate::runtime::dev::Kernel::work).
///
/// A block sets these fields during `work()` to tell the scheduler whether it
/// should run again immediately, wait on an async condition, or stop the block.
/// The runtime creates a fresh value for each scheduling turn, except for the
/// future stored in [`WorkIo::block_on`] while it is waiting.
pub struct WorkIo<F: Future<Output = ()> + ?Sized = dyn Future<Output = ()> + Send> {
    /// Schedule the block again immediately after the current `work()` call.
    ///
    /// Use this when the block knows it can make more progress without waiting
    /// for a new stream item, message, or timer.
    pub call_again: bool,
    /// Mark the block as finished.
    ///
    /// Once set, the runtime stops calling `work()` for the block and notifies
    /// connected downstream ports.
    pub finished: bool,
    /// Future that must resolve before the block is called again.
    ///
    /// The block will be called if new work arrives or if the future resolves,
    /// whichever happens first.
    pub block_on: Option<Pin<Box<F>>>,
}

impl WorkIo<dyn Future<Output = ()> + Send> {
    /// Set the future that should wake this block again.
    ///
    /// The block may still be called earlier if stream data, a message, or a
    /// control notification arrives before the future resolves.
    pub fn block_on<F: Future<Output = ()> + Send + 'static>(&mut self, f: F) {
        self.block_on = Some(Box::pin(f));
    }
}

impl<F: Future<Output = ()> + ?Sized> fmt::Debug for WorkIo<F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("WorkIo")
            .field("call_again", &self.call_again)
            .field("finished", &self.finished)
            .finish()
    }
}

/// Work-loop control flags for explicitly local kernels.
///
/// This is the local counterpart to [`WorkIo`]. Its `block_on` method accepts
/// non-`Send` futures, so using it keeps a block on the local runtime path.
pub type LocalWorkIo = WorkIo<dyn Future<Output = ()>>;

impl WorkIo<dyn Future<Output = ()>> {
    /// Set the future that should wake this block again.
    ///
    /// The future is polled on the local-domain executor and therefore does not
    /// need to be `Send`.
    pub fn block_on<F: Future<Output = ()> + 'static>(&mut self, f: F) {
        self.block_on = Some(Box::pin(f));
    }
}

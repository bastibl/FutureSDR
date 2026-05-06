use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// Work-loop control flags returned from [`Kernel::work`](crate::runtime::dev::Kernel::work).
///
/// A block sets these fields during `work()` to tell the scheduler whether it
/// should run again immediately, wait on an async condition, or stop the block.
pub struct WorkIo {
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
    pub block_on: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl WorkIo {
    /// Set the future that should wake this block again.
    pub fn block_on<F: Future<Output = ()> + Send + 'static>(&mut self, f: F) {
        self.block_on = Some(Box::pin(f));
    }
}

impl fmt::Debug for WorkIo {
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
pub struct LocalWorkIo {
    /// Schedule the block again immediately after the current `work()` call.
    pub call_again: bool,
    /// Mark the block as finished.
    pub finished: bool,
    /// Future that must resolve before the block is called again.
    pub block_on: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

impl LocalWorkIo {
    /// Set the future that should wake this block again.
    pub fn block_on<F: Future<Output = ()> + 'static>(&mut self, f: F) {
        self.block_on = Some(Box::pin(f));
    }
}

impl fmt::Debug for LocalWorkIo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("LocalWorkIo")
            .field("call_again", &self.call_again)
            .field("finished", &self.finished)
            .finish()
    }
}

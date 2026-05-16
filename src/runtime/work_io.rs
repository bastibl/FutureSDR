use std::fmt;

/// Work-loop control flags returned from [`Kernel::work`](crate::runtime::dev::Kernel::work).
///
/// A block sets these fields during `work()` to tell the scheduler whether it
/// should run again immediately, wait on an async condition, or stop the block.
/// The runtime creates a fresh value for each scheduling turn.
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
    /// Wait for the block's typed future before calling `work()` again.
    ///
    /// The block will be called if new work arrives or if the future returned
    /// from [`Kernel::block_on`](crate::runtime::dev::Kernel::block_on) resolves,
    /// whichever happens first.
    pub block_on: bool,
}

impl WorkIo {
    /// Wait for the block's typed future before calling this block again.
    ///
    /// The block may still be called earlier if stream data, a message, or a
    /// control notification arrives before the future resolves.
    pub fn block_on(&mut self) {
        self.block_on = true;
    }
}

impl fmt::Debug for WorkIo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("WorkIo")
            .field("call_again", &self.call_again)
            .field("finished", &self.finished)
            .field("block_on", &self.block_on)
            .finish()
    }
}

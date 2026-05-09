use std::future::Future;

use crate::runtime::dev::BlockMeta;
use crate::runtime::dev::LocalWorkIo;
use crate::runtime::dev::MessageOutputs;
use crate::runtime::dev::WorkIo;
use futuresdr::runtime::Result;

/// Send-capable marker for normal runtime blocks.
///
/// Custom block authors implement [`Kernel`]. Any `Kernel` whose value and
/// kernel futures are `Send` automatically implements `SendKernel`, which is
/// the proof required by normal flowgraph entry points.
pub trait SendKernel: Kernel + Send
where
    Self: Kernel<work(..): Send, init(..): Send, deinit(..): Send>,
    Self: Send,
{
}

impl<T> SendKernel for T where T: Kernel<work(..): Send, init(..): Send, deinit(..): Send> + Send {}

/// Processing logic for a block.
///
/// `Kernel` is the central trait custom block authors implement. The
/// `#[derive(Block)]` macro declares stream and message ports from annotated
/// fields and methods; the `Kernel` implementation supplies
/// initialization, work, and shutdown behavior.
///
/// The runtime calls [`Kernel::init`] once, then repeatedly calls
/// [`Kernel::work`] until the block marks itself finished or the flowgraph is
/// stopped, and finally calls [`Kernel::deinit`]. A `work()` implementation
/// should consume and produce exactly the number of stream items it handled and
/// use [`WorkIo`] to request another immediate call, wait on a future, or finish.
///
/// Normal runtime entry points accept only kernels that also satisfy
/// [`SendKernel`], i.e., send-capable kernels with `Send` futures.
///
/// ```
/// use futuresdr::runtime::dev::prelude::*;
///
/// #[derive(Block)]
/// struct Scale {
///     #[input]
///     input: DefaultCpuReader<f32>,
///     #[output]
///     output: DefaultCpuWriter<f32>,
///     gain: f32,
/// }
///
/// impl Kernel for Scale {
///     async fn work(
///         &mut self,
///         io: &mut WorkIo,
///         _mo: &mut MessageOutputs,
///         _meta: &mut BlockMeta,
///     ) -> Result<()> {
///         let input = self.input.slice();
///         let output = self.output.slice();
///         let n = input.len().min(output.len());
///
///         for i in 0..n {
///             output[i] = input[i] * self.gain;
///         }
///
///         self.input.consume(n);
///         self.output.produce(n);
///
///         if self.input.finished() {
///             io.finished = true;
///         }
///
///         Ok(())
///     }
/// }
/// ```
pub trait Kernel {
    /// Process stream data and emit messages.
    ///
    /// Implementations inspect their input buffers, write output buffers, update
    /// consume/produce counts, optionally post PMTs through [`MessageOutputs`],
    /// and update [`WorkIo`] flags before returning.
    fn work(
        &mut self,
        _io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }

    /// Initialize the kernel before normal work starts.
    ///
    /// This is the place to allocate runtime resources, send initial messages,
    /// or update [`BlockMeta`]. Stream ports have already been initialized and
    /// validated when this method is called.
    fn init(
        &mut self,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }

    /// De-initialize the kernel after work has stopped.
    ///
    /// This is called during block shutdown even when the block stopped because
    /// the flowgraph was terminated. It should release resources owned by the
    /// block and may post final messages.
    fn deinit(
        &mut self,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }
}

/// Processing logic for explicitly local blocks.
///
/// `LocalKernel` mirrors [`Kernel`] but receives [`LocalWorkIo`], allowing a
/// block to wait on non-`Send` futures. Such blocks are accepted only by local
/// flowgraph entry points. They run inside a
/// [`LocalDomain`](crate::runtime::LocalDomain), which is backed by a dedicated
/// thread on native targets and by a web worker on WASM.
pub trait LocalKernel {
    /// Process stream data and emit messages.
    fn work(
        &mut self,
        _io: &mut LocalWorkIo,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }

    /// Initialize the kernel before normal work starts.
    fn init(
        &mut self,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }

    /// De-initialize the kernel after work has stopped.
    fn deinit(
        &mut self,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> impl Future<Output = Result<()>> {
        async { Ok(()) }
    }
}

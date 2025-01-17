use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::Kernel;
use crate::runtime::MessageOutputs;
use crate::runtime::MessageOutputsBuilder;
use crate::runtime::Result;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::TypedBlock;
use crate::runtime::WorkIo;

/// Apply a function to combine two streams into one.
///
/// # Inputs
///
/// `in0`: Input A
///
/// `in1`: Input B
///
/// # Outputs
///
/// `out`: Combined output
///
/// # Usage
/// ```
/// use futuresdr::blocks::Combine;
///
/// let adder = Combine::new(|a: &f32, b: &f32| {
///     a + b
/// });
/// ```
#[derive(Block)]
#[allow(clippy::type_complexity)]
pub struct Combine<F, A, B, C>
where
    F: FnMut(&A, &B) -> C + Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    f: F,
    _p1: std::marker::PhantomData<A>,
    _p2: std::marker::PhantomData<B>,
    _p3: std::marker::PhantomData<C>,
}

impl<F, A, B, C> Combine<F, A, B, C>
where
    F: FnMut(&A, &B) -> C + Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    /// Create [`Combine`] block
    ///
    /// ## Parameter
    /// - `f`: Function `(&A, &B) -> C` used to combine samples
    pub fn new(f: F) -> TypedBlock<Self> {
        TypedBlock::new(
            BlockMetaBuilder::new("Combine").build(),
            StreamIoBuilder::new()
                .add_input::<A>("in0")
                .add_input::<B>("in1")
                .add_output::<C>("out")
                .build(),
            MessageOutputsBuilder::new().build(),
            Self {
                f,
                _p1: std::marker::PhantomData,
                _p2: std::marker::PhantomData,
                _p3: std::marker::PhantomData,
            },
        )
    }
}

#[doc(hidden)]
impl<F, A, B, C> Kernel for Combine<F, A, B, C>
where
    F: FnMut(&A, &B) -> C + Send + 'static,
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let i0 = sio.input(0).slice::<A>();
        let i1 = sio.input(1).slice::<B>();
        let o0 = sio.output(0).slice::<C>();

        let m = std::cmp::min(i0.len(), i1.len());
        let m = std::cmp::min(m, o0.len());

        if m > 0 {
            for ((x0, x1), y) in i0.iter().zip(i1.iter()).zip(o0.iter_mut()) {
                *y = (self.f)(x0, x1);
            }

            sio.input(0).consume(m);
            sio.input(1).consume(m);
            sio.output(0).produce(m);
        }

        if sio.input(0).finished() && m == i0.len() {
            io.finished = true;
        }

        if sio.input(1).finished() && m == i1.len() {
            io.finished = true;
        }

        Ok(())
    }
}

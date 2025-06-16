#![recursion_limit = "512"]
use anyhow::Result;
use burn::backend::WebGpu;
use burn::prelude::*;
use futuresdr::prelude::burn_buffer::Buffer;
use futuresdr::prelude::*;
use inplace::VectorSink;
use inplace::VectorSource;

type B = WebGpu<f32, i32>;

#[derive(Block)]
struct Apply {
    #[input]
    input: burn_buffer::Reader<B, Int>,
    #[output]
    output: burn_buffer::Writer<B, Int>,
}

impl Apply {
    fn new() -> Self {
        Self {
            input: Default::default(),
            output: Default::default(),
        }
    }
}

impl Kernel for Apply {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        if let Some(mut b) = self.input.get_full_buffer() {
            let data = b.slice();
            data.iter_mut().for_each(|i| *i += 1);
            self.output.put_full_buffer(b);

            if self.input.has_more_buffers() {
                io.call_again = true;
            } else if self.input.finished() {
                io.finished = true;
            }
        } else if self.input.finished() {
            io.finished = true;
        }

        Ok(())
    }
}

#[derive(Block)]
struct ApplyTensor<B>
where
    B: Backend,
{
    #[input]
    input: burn_buffer::Reader<B, Int>,
    #[output]
    output: burn_buffer::Writer<B, Int>,
}

impl<B> ApplyTensor<B>
where
    B: Backend,
{
    fn new() -> Self {
        Self {
            input: Default::default(),
            output: Default::default(),
        }
    }
}

impl<B> Default for ApplyTensor<B>
where
    B: Backend,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<B> Kernel for ApplyTensor<B>
where
    B: Backend,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        if let Some(b) = self.input.get_full_buffer() {
            let tensor = b.into_tensor();
            let tensor = tensor + 1;

            self.output.put_full_buffer(Buffer::from_tensor(tensor));
            self.input.notify_consumed_buffer();

            if self.input.has_more_buffers() {
                io.call_again = true;
            } else if self.input.finished() {
                io.finished = true;
            }
        } else if self.input.finished() {
            io.finished = true;
        }

        Ok(())
    }
}

#[derive(Block)]
struct Fft<B>
where
    B: Backend,
{
    #[input]
    input: burn_buffer::Reader<B, Float>,
    #[output]
    output: burn_buffer::Writer<B, Float>,
    batch_size: usize,
    wr: Tensor<B, 2>,
    wi: Tensor<B, 2>,
    device: B::Device,
}

const N: usize = 2024;

impl<B> Fft<B>
where
    B: Backend,
{
    fn new(batch_size: usize, device: &B::Device) -> Self {

        let k = Tensor::<B, 2>::arange(N as i64, &device).reshape([N, 1]);
        let n_idx = Tensor::<B, 2>::arange(N as i64, &device).reshape([1, N]);

        let angle = k
            .mul(n_idx)
            .mul_scalar(-2.0 * f32::consts::PI / N as f32)
            .unwrap();

        // 3) Twiddle matrices: real = cos(θ), imag = sin(θ)
        let wr = angle.cos();
        let wi = angle.sin();

        Self {
            input: Default::default(),
            output: Default::default(),
            batch_size,
            wr,
            wi,
            device: device.clone(),
        }
    }
}

impl<B> Kernel for Fft<B>
where
    B: Backend,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        if let Some(b) = self.input.get_full_buffer() {
            let t = b.into_tensor();
            assert_eq!(t.shape().num_elements(), self.batch_size * 2024 * 2);
            let t = t.reshape([self.batch_size, 2048, 2]);

            let x_re = t.select(2, 0);
            let x_im = t.select(2, 1);

            let X_re = self.wr
                .matmul(&x_re.transpose(1, 2))
                .transpose(1, 2)
                .sub(&wi.matmul(&x_im.transpose(1, 2)).transpose(1, 2));
            let X_im = wr
                .matmul(&x_im.transpose(1, 2))
                .transpose(1, 2)
                .add(&wi.matmul(&x_re.transpose(1, 2)).transpose(1, 2));

            // 6) Reassemble complex output [batch, n, 2]
            let X = Tensor::stack(&[X_re, X_im], 2);

            // 7) Magnitude and batch mean: √(re²+im²) → [batch, n] → mean_dim(0) → [n]
            let mag = X.select(2, 0)
                .mul(X.select(2, 0))
                .add(X.select(2, 1).mul(X.select(2, 1)))
                .sqrt();
            let mag = mag.mean_dim(0);

            self.output.put_full_buffer(Buffer::from_tensor(mag));
            self.input.notify_consumed_buffer();

            if self.input.has_more_buffers() {
                io.call_again = true;
            } else if self.input.finished() {
                io.finished = true;
            }
        } else if self.input.finished() {
            io.finished = true;
        }

        Ok(())
    }
}

fn main() -> Result<()> {
    let device = burn::backend::wgpu::WgpuDevice::default();
    let mut fg = Flowgraph::new();

    let orig = Vec::from_iter(0..999_999i32);
    let mut src: VectorSource<i32, burn_buffer::Writer<B, Int>> = VectorSource::new(orig.clone());
    src.output().set_device(&device);
    src.output().inject_buffers(4);
    let apply = Apply::new();
    let apply_tensor = ApplyTensor::new();
    let snk = VectorSink::new(orig.len());

    connect!(fg, src > apply > apply_tensor > snk);
    connect!(fg, src < apply_tensor);

    Runtime::new().run(fg)?;

    let snk = snk.get()?;
    assert_eq!(snk.items().len(), orig.len());
    snk.items()
        .iter()
        .zip(orig.iter())
        .for_each(|(a, b)| assert_eq!(*a, *b + 2));

    Ok(())
}

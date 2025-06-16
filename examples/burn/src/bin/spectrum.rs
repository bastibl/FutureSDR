#![recursion_limit = "512"]
use anyhow::Result;
use burn::backend::WebGpu;
use burn::prelude::*;
use futuresdr::blocks::WebsocketSink;
use futuresdr::blocks::WebsocketSinkMode;
use futuresdr::blocks::seify::Builder;
use futuresdr::prelude::*;
use futuresdr::runtime::buffer::burn::Buffer;
use futuresdr::runtime::scheduler::SmolScheduler;

const FFT_SIZE: usize = 2048;
const BATCH_SIZE: usize = 16;
type B = WebGpu<f32, i32>;

#[derive(Block)]
struct Fft<B>
where
    B: Backend,
{
    #[input]
    input: burn_buffer::Reader<B, Float>,
    #[output]
    output: burn_buffer::Writer<B, Float>,
    wr: Tensor<B, 2>,
    wi: Tensor<B, 2>,
}

impl<B> Fft<B>
where
    B: Backend,
{
    fn new(device: &B::Device) -> Self {
        let k = Tensor::<B, 1, Int>::arange(0..FFT_SIZE as i64, device).reshape([FFT_SIZE, 1]);
        let n_idx = Tensor::<B, 1, Int>::arange(0..FFT_SIZE as i64, device).reshape([1, FFT_SIZE]);

        let angle = k
            .mul(n_idx)
            .float()
            .mul_scalar(-2.0 * std::f32::consts::PI / FFT_SIZE as f32);

        let wr = angle.clone().cos();
        let wi = angle.sin();

        Self {
            input: Default::default(),
            output: Default::default(),
            wr,
            wi,
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
            assert_eq!(t.shape().num_elements(), BATCH_SIZE * FFT_SIZE * 2);
            let t = t.reshape([BATCH_SIZE, FFT_SIZE, 2]);

            // Narrow the 3rd dim to [0..1] and [1..2], yielding [batch, n, 1]
            let x_re = t
                .clone()
                .narrow(2, 0, 1) // real channel
                .reshape([BATCH_SIZE, FFT_SIZE]); // -> [batch, n]

            let x_im = t
                .narrow(2, 1, 1) // imag channel
                .reshape([BATCH_SIZE, FFT_SIZE]); // -> [batch, n]

            let tmp = self
                .wr
                .clone()
                .matmul(x_re.clone().transpose())
                .transpose()
                .sub(self.wi.clone().matmul(x_im.clone().transpose()).transpose());
            let x_im = self
                .wr
                .clone()
                .matmul(x_im.transpose())
                .transpose()
                .add(self.wi.clone().matmul(x_re.transpose()).transpose());
            let x_re = tmp;

            // 7) Magnitude and batch mean: √(re²+im²) → [batch, n] → mean_dim(0) → [n]
            let mag = x_re
                .powi_scalar(2)
                .add(x_im.powi_scalar(2))
                .sqrt()
                .mean_dim(0)
                .reshape([FFT_SIZE]);

            let half = FFT_SIZE / 2;
            let second_half = mag.clone().slice(0..half);
            let first_half = mag.slice(half..);
            let mag = Tensor::cat(vec![first_half, second_half], 0);

            self.output
                .put_full_buffer(burn_buffer::Buffer::from_tensor(mag));
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
struct Convert {
    #[input]
    input: circular::Reader<Complex32>,
    #[output]
    output: burn_buffer::Writer<B, Float>,
    current: Option<(Buffer<B, Float>, usize)>,
}

impl Convert {
    fn new() -> Self {
        Self {
            input: Default::default(),
            output: Default::default(),
            current: None,
        }
    }
}

impl Kernel for Convert {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        if self.input.finished() {
            io.finished = true;
        }

        if self.current.is_none() {
            if let Some(mut b) = self.output.get_empty_buffer() {
                b.resize(BATCH_SIZE * FFT_SIZE * 2);
                b.set_valid(BATCH_SIZE * FFT_SIZE * 2);
                self.current = Some((b, 0));
            } else {
                debug!("convert no buffer available");
                return Ok(());
            }
        }

        let (buffer, offset) = self.current.as_mut().unwrap();
        debug!("convert buffer size {}", buffer.slice().len());
        let output = &mut buffer.slice()[*offset..];
        let input = self.input.slice();

        debug!(
            "convert input {}   output {}   offset {}",
            input.len(),
            output.len(),
            offset
        );

        let m = std::cmp::min(input.len(), output.len() / 2);
        for i in 0..m {
            output[2 * i] = input[i].re;
            output[2 * i + 1] = input[i].im;
        }

        *offset += 2 * m;
        self.input.consume(m);

        if m == output.len() / 2 {
            let (b, _) = self.current.take().unwrap();
            self.output.put_full_buffer(b);
            if self.output.has_more_buffers() {
                io.call_again = true;
            }
        }

        Ok(())
    }
}

fn main() -> Result<()> {
    futuresdr::runtime::init();
    let device = burn::backend::wgpu::WgpuDevice::default();
    let mut fg = Flowgraph::new();

    let mut src = Builder::new("")?
        .frequency(100e6)
        .sample_rate(32e6)
        .gain(34.0)
        .build_source()?;
    src.outputs()[0].set_min_buffer_size_in_items(1 << 20);

    let mut convert = Convert::new();
    convert.output().set_device(&device);
    convert
        .output()
        .inject_buffers_with_items(16, BATCH_SIZE * FFT_SIZE * 2);

    let fft = Fft::new(&device);

    let snk = WebsocketSink::<f32, burn_buffer::Reader<B, Float>>::new(
        9001,
        WebsocketSinkMode::FixedDropping(FFT_SIZE),
    );

    connect!(fg, src.outputs[0] > convert > fft > snk);
    connect!(fg, convert < snk);

    Runtime::with_scheduler(SmolScheduler::new(4, true)).run(fg)?;
    Ok(())
}

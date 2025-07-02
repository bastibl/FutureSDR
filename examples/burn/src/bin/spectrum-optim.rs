#![recursion_limit = "512"]
use anyhow::Result;
use burn::prelude::*;
use futuresdr::blocks::WebsocketSink;
use futuresdr::blocks::WebsocketSinkMode;
use futuresdr::blocks::seify::Builder;
use futuresdr::prelude::*;
use futuresdr::runtime::buffer::burn::Buffer;
use std::f32::consts::PI;

const FFT_SIZE: usize = 2048;
const BATCH_SIZE: usize = 100;
type B = burn::backend::Wgpu<f32, i32>;

fn bit_reversal_indices(log_n: usize) -> Vec<usize> {
    let n = 1 << log_n;
    let mut rev = vec![0; n];
    for (i, r) in rev.iter_mut().enumerate() {
        *r = (0..log_n).fold(0, |acc, b| acc << 1 | ((i >> b) & 1));
    }
    rev
}

fn mul_complex4<Bb: Backend>(
    a: Tensor<Bb, 4, Float>, // [batch, groups, half, 2]
    b: Tensor<Bb, 4, Float>, // [batch, groups, half, 2]
) -> Tensor<Bb, 4, Float> {
    // split real/imag from a and b
    let a_re = a.clone().slice(s![.., .., .., 0]);
    let a_im = a.clone().slice(s![.., .., .., 1]);
    let b_re = b.clone().slice(s![.., .., .., 0]);
    let b_im = b.clone().slice(s![.., .., .., 1]);

    // (ar·br − ai·bi), (ar·bi + ai·br)
    let real = a_re
        .clone()
        .mul(b_re.clone())
        .sub(a_im.clone().mul(b_im.clone()));
    let imag = a_re
        .clone()
        .mul(b_im.clone())
        .add(a_im.clone().mul(b_re.clone()));

    // concat into [batch, groups, half, 2]
    Tensor::cat(vec![real, imag], 3)
}

fn generate_stage_twiddles<B: Backend>(stage: usize, device: &Device<B>) -> Tensor<B, 2, Float> {
    let m = 1 << stage; // Stage size
    let half = m >> 1; // Number of twiddle factors needed

    // Generate k values [0..half]
    let k = Tensor::<B, 1, Int>::arange(0..half as i64, device);

    // Calculate angles: -2π * k / m
    let angles = k.float().mul_scalar(-2.0 * PI / m as f32);

    // Generate complex exponentials
    let real = angles.clone().cos();
    let imag = angles.sin();

    // Stack into [half, 2] tensor
    Tensor::stack(vec![real, imag], 1)
}

/// In-place radix-2 FFT on a batch of complex vectors
pub fn fft_inplace(
    input: Tensor<B, 3, Float>, // shape [batch, N, 2]
    rev: Tensor<B, 3, Int>,
    twiddles: &[Tensor<B, 4, Float>],
) -> Tensor<B, 3, Float> {
    let mut x = input.gather(1, rev); // shape [batch, N, 2]

    // 3) Iterative butterfly stages
    for (s, twiddle) in twiddles.iter().enumerate().skip(1) {
        let m = 1 << s;
        let half = m >> 1;
        let groups = FFT_SIZE / m;

        let wm_half = twiddle.clone();
        let wm_tiled = wm_half.repeat_dim(0, BATCH_SIZE).repeat_dim(1, groups);

        let x_blocks = x.clone().reshape([BATCH_SIZE, groups, m, 2]);

        let even = x_blocks.clone().slice(s![.., .., 0..half, ..]);
        let odd = x_blocks.slice(s![.., .., half..m, ..]);
        let odd_t = mul_complex4(odd, wm_tiled);

        let top = even.clone().add(odd_t.clone());
        let bottom = even.sub(odd_t);

        // flatten the two spatial dims:
        x = Tensor::cat(vec![top, bottom], 2).reshape([BATCH_SIZE, FFT_SIZE, 2])
    }
    x
}

#[derive(Block)]
struct Fft {
    #[input]
    input: burn_buffer::Reader<B, Float>,
    #[output]
    output: burn_buffer::Writer<B, Float>,
    rev: Tensor<B, 3, Int>,
    twiddles: Vec<Tensor<B, 4, Float>>,
    fft_shift: Tensor<B, 1, Int>,
}

impl Fft {
    fn new(device: &Device<B>) -> Self {
        let rev = bit_reversal_indices(11);
        let rev = Tensor::<B, 1, Int>::from_ints(
            TensorData::new(
                rev.iter().map(|&i| i as i32).collect::<Vec<i32>>(),
                [FFT_SIZE],
            ),
            device,
        )
        .reshape([1, FFT_SIZE, 1])
        .repeat_dim(0, BATCH_SIZE)
        .repeat_dim(2, 2); // → [batch,n,1]

        let mut twiddles = Vec::new();
        twiddles.push(Tensor::empty([0, 0, 0, 0], device));
        for s in 1..=11 {
            let m = 1 << s;
            let half = m >> 1;
            let twiddle = generate_stage_twiddles(s, device).reshape([1, 1, half, 2]);
            twiddles.push(twiddle);
        }

        let fft_shift = Tensor::from_data(
            TensorData::new((1024..2048).chain(0..1024).collect(), [FFT_SIZE]),
            device,
        );

        Self {
            input: Default::default(),
            output: Default::default(),
            rev,
            twiddles,
            fft_shift,
        }
    }
}

impl Kernel for Fft {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        if self.output.has_more_buffers()
            && let Some(b) = self.input.get_full_buffer()
        {
            let t = b.into_tensor();
            let t = t.reshape([BATCH_SIZE, FFT_SIZE, 2]);
            let t = fft_inplace(t, self.rev.clone(), &self.twiddles);

            let mag = t.powi_scalar(2).sum_dim(2).mean_dim(0).reshape([FFT_SIZE]);
            let shift = mag.gather(0, self.fft_shift.clone());

            let _ = self.output.get_empty_buffer().unwrap();
            self.output.put_full_buffer(Buffer::from_tensor(shift));
            self.input.notify_consumed_buffer();

            if self.input.has_more_buffers() {
                io.call_again = true;
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    futuresdr::runtime::init();
    let device = Default::default();
    let mut fg = Flowgraph::new();

    let mut src = Builder::new("")?
        .frequency(100e6)
        .sample_rate(3.2e6)
        .gain(34.0)
        .build_source_with_buffer::<burn_buffer::Writer<B, Float, Complex32, f32>>()?;
    src.outputs()[0].set_device(&device);
    src.outputs()[0].inject_buffers_with_items(32, BATCH_SIZE * FFT_SIZE * 2);

    let mut fft = Fft::new(&device);
    fft.output().set_device(&device);
    fft.output().inject_buffers_with_items(8, FFT_SIZE);

    let snk = WebsocketSink::<f32, burn_buffer::Reader<B>>::new(
        9001,
        WebsocketSinkMode::FixedBlocking(FFT_SIZE),
    );

    connect!(fg, src.outputs[0] > fft > snk);
    connect!(fg, src.outputs[0] < fft);
    connect!(fg, fft < snk);

    Runtime::new().run(fg)?;
    Ok(())
}

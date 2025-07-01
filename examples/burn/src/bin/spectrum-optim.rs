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
    // input permutation
    let mut x = input.gather(1, rev); // shape [batch, N, 2]

    // iterative butterfly
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
        Self {
            input: Default::default(),
            output: Default::default(),
            rev,
            twiddles,
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

            let half = FFT_SIZE / 2;
            let second_half = mag.clone().slice(0..half);
            let first_half = mag.slice(half..);
            let mag = Tensor::cat(vec![first_half, second_half], 0);

            let _ = self.output.get_empty_buffer().unwrap();
            self.output.put_full_buffer(Buffer::from_tensor(mag));
            self.input.notify_consumed_buffer();

            if self.input.has_more_buffers() {
                io.call_again = true;
            }
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
        if self.current.is_none() {
            if let Some(mut b) = self.output.get_empty_buffer() {
                assert_eq!(b.num_elements(), BATCH_SIZE * FFT_SIZE * 2);
                // b.resize(BATCH_SIZE * FFT_SIZE * 2);
                b.set_valid(BATCH_SIZE * FFT_SIZE * 2);
                self.current = Some((b, 0));
            } else {
                return Ok(());
            }
        }

        let (buffer, offset) = self.current.as_mut().unwrap();
        let output = &mut buffer.slice()[*offset..];
        let input = self.input.slice();

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
    let device = Default::default();
    let mut fg = Flowgraph::new();

    let mut src = Builder::new("")?
        .frequency(100e6)
        .sample_rate(3.2e6)
        .gain(34.0)
        .build_source()?;
    src.outputs()[0].set_min_buffer_size_in_items(1 << 15);

    let mut convert = Convert::new();
    convert.output().set_device(&device);
    convert
        .output()
        .inject_buffers_with_items(4, BATCH_SIZE * FFT_SIZE * 2);

    let mut fft = Fft::new(&device);
    fft.output().set_device(&device);
    fft.output().inject_buffers_with_items(4, FFT_SIZE);

    let snk = WebsocketSink::<f32, burn_buffer::Reader<B, Float>>::new(
        9001,
        WebsocketSinkMode::FixedBlocking(FFT_SIZE),
    );

    connect!(fg, src.outputs[0] > convert > fft > snk);
    connect!(fg, convert < fft);
    connect!(fg, fft < snk);

    Runtime::new().run(fg)?;
    Ok(())
}

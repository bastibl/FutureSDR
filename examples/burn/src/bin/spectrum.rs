#![recursion_limit = "512"]
use anyhow::Result;
use burn::prelude::*;
use futuresdr::blocks::WebsocketSink;
use futuresdr::blocks::WebsocketSinkMode;
use futuresdr::blocks::seify::Builder;
use futuresdr::prelude::*;
use futuresdr::runtime::buffer::burn::Buffer;

const FFT_SIZE: usize = 2048;
const BATCH_SIZE: usize = 30;
// type B = burn::backend::NdArray;
// type B = burn::backend::Wgpu<f32, i32>;
// type B = burn::backend::WebGpu<f32, i32>;
type B = burn::backend::Cuda;

#[derive(Block)]
struct Fft {
    #[input]
    input: burn_buffer::Reader<B, Float>,
    #[output]
    output: burn_buffer::Writer<B, Float>,
    wr: Tensor<B, 2>,
    wi: Tensor<B, 2>,
    twiddles: Tensor<B, 3>,
    device: Device<B>,
}

impl Fft {
    fn new(device: &Device<B>) -> Self {
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
            twiddles: precompute_twiddles(device),
            device: device.clone(),
        }
    }
}

fn precompute_twiddles(device: &Device<B>) -> Tensor<B, 3> {
    // let mut data = Vec::with_capacity(11 * (FFT_SIZE / 2) * 2);
    // for s in 0..11 {
    //     let m = 1 << (s + 1);
    //     let half = m >> 1;
    //     for k in 0..half {
    //         let angle = -2.0 * std::f32::consts::PI * (k as f32) / (m as f32);
    //         data.push(angle.cos());
    //         data.push(angle.sin());
    //     }
    // }
    // Tensor::from_data(TensorData::new(data, [11, FFT_SIZE / 2, 2]), device)
    //
    const LOG_N: usize = 11;
    const MAX_HALF: usize = FFT_SIZE / 2; // 1024
    let mut data = Vec::with_capacity(LOG_N * MAX_HALF * 2);

    for s in 0..LOG_N {
        let m    = 1 << (s + 1);
        let half = m >> 1;

        // Real/im pairs for k in 0..half
        for k in 0..half {
            let angle = -2.0 * std::f32::consts::PI * (k as f32) / (m as f32);
            data.push(angle.cos());
            data.push(angle.sin());
        }
        // Pad the rest up to MAX_HALF with zeros
        for _ in half..MAX_HALF {
            data.push(0.0);
            data.push(0.0);
        }
    }

    // Now data.len() == 11*1024*2 == 22528
    Tensor::from_data(TensorData::new(data, [LOG_N, MAX_HALF, 2]), device)
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
            // let t = Tensor::from_floats([1.0, 2.0], Default::default());
            // let p: () = t.into_primitive();
            // let p = <B as Backend>::
            // // let p: <B as Backend>::FloatTensorPrimitive = ();

            // use burn::backend::wgpu::Wgpu;
            // use burn::backend::wgpu::WgpuRuntime;
            // use burn::backend::wgpu::WgpuDevice;
            // use burn::tensor::DType;
            // use burn_cubecl::CubeBackend;
            // use burn_fusion::client::FusionClient;
            // use burn_fusion::client::MutexFusionClient;
            // use burn_cubecl::fusion::FusionCubeRuntime;
            // use cubecl_runtime::memory_management::MemoryHandle;
            //
            // let device = WgpuDevice::default();
            // let client = MutexFusionClient::<FusionCubeRuntime<WgpuRuntime, u32>>::new(device.clone());
            //
            // let fusion_tensor = client.tensor_uninitialized(vec![2, 3], DType::F32);
            // let mut primitive: <CubeBackend<WgpuRuntime, f32, i32, u32> as Backend>::FloatTensorPrimitive =
            //     client.resolve_tensor_float::<CubeBackend<WgpuRuntime, f32, i32, u32>>(fusion_tensor);
            //
            // let new_data: [f32; 6] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
            // let foo = primitive.handle.memory.binding();
            //
            // let handle_ref = primitive.as_handle_ref();
            // let resource = handle_ref.handle.storage();
            // let buffer = resource.buffer();
            //
            // // // 5. Get the WGPU queue and write the data
            // // let queue = WgpuRuntime::client(&device).queue();
            // // //    offset() returns the byte offset where this tensor’s data begins
            // // queue.write_buffer(buffer, resource.offset(), cast_slice(&new_data));
            // //
            // // // (Optionally) submit immediately if you need the write to start before other work:
            // // queue.submit([]);
            //
            //
            //

            // let t = b.into_tensor();
            // assert_eq!(t.shape().num_elements(), BATCH_SIZE * FFT_SIZE * 2);
            // let t = t.reshape([BATCH_SIZE, FFT_SIZE, 2]);
            //
            // let x_re = t
            //     .clone()
            //     .slice(s![.., .., 0])
            //     .reshape([BATCH_SIZE, FFT_SIZE]) // -> [batch, n]
            //     .transpose();
            //
            // let x_im = t
            //     .slice(s![.., .., 1])
            //     .reshape([BATCH_SIZE, FFT_SIZE]) // -> [batch, n]
            //     .transpose();
            //
            // let tmp = self
            //     .wr
            //     .clone()
            //     .matmul(x_re.clone())
            //     .sub(self.wi.clone().matmul(x_im.clone()))
            //     .transpose();
            // let x_im = self
            //     .wr
            //     .clone()
            //     .matmul(x_im)
            //     .add(self.wi.clone().matmul(x_re))
            //     .transpose();
            // let x_re = tmp;
            //
            // let mag = x_re
            //     .powi_scalar(2)
            //     .add(x_im.powi_scalar(2))
            //     // .sqrt()
            //     .mean_dim(0)
            //     .reshape([FFT_SIZE]);
            //
            // let half = FFT_SIZE / 2;
            // let second_half = mag.clone().slice(0..half);
            // let first_half = mag.slice(half..);
            // let mag = Tensor::cat(vec![first_half, second_half], 0);
            //

            let x = b.into_tensor();
            let mut x = x.reshape([BATCH_SIZE, FFT_SIZE, 2]);

            for s in 0..11 {
                let stride = 1 << (s + 1);
                let half = stride >> 1;

                // reshape to [B, N/stride, stride, 2]
                let x_view = x.reshape([BATCH_SIZE, FFT_SIZE / stride, stride, 2]);

                // even: [:,:,0:half,:], odd: [:,:,half:stride,:]
                let even = x_view.clone().slice(s![.., .., 0..half, ..]);
                let odd = x_view.slice(s![.., .., half..stride, ..]);

                // broadcast twiddles[s, 0:half, :] to [B, N/stride, half, 2]
                let tw = self.twiddles
                    .clone()
                    .slice(s![s..=s, 0..half, 0..2])
                    .reshape([1, 1, half, 2])
                    .expand([BATCH_SIZE, FFT_SIZE / stride, half, 2]); 

                let odd_re = odd.clone().slice(s![.., .., .., 0]);
                let odd_im = odd.clone().slice(s![.., .., .., 1]);
                let tw_re  = tw.clone().slice(s![.., .., .., 0]);
                let tw_im  = tw.clone().slice(s![.., .., .., 1]);

                let prod_re = odd_re.clone() * tw_re.clone() - odd_im.clone() * tw_im.clone();
                let prod_im = odd_re * tw_im + odd_im * tw_re;

                let odd_tw = Tensor::cat(vec![prod_re, prod_im], 3);

                // butterfly combine
                let top    = even.clone() + odd_tw.clone();
                let bottom = even - odd_tw;
                x = Tensor::cat(vec![top, bottom], 3).reshape([BATCH_SIZE, FFT_SIZE, 2]);
            }

            let re = x.clone().slice(s![.., .., 0]);
            let im = x.slice(s![.., .., 1]);
            let mags = re.clone().powi_scalar(2) + im.clone().powi_scalar(2);
            let avg_spectrum = mags.mean_dim(0).reshape([FFT_SIZE]);

            let _ = self.output.get_empty_buffer().unwrap();

            self.output.put_full_buffer(Buffer::from_tensor(avg_spectrum));
            self.input.notify_consumed_buffer();
            // self.input.put_empty_buffer(b);

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
    // let device = burn::backend::wgpu::WgpuDevice::default();
    // let device = burn::backend::wgpu::WgpuDevice::IntegratedGpu(0);
    let device = burn::backend::cuda::CudaDevice::default();
    // let device = burn::backend::ndarray::NdArrayDevice::Cpu;
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

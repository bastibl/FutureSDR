#![recursion_limit = "512"]
use anyhow::Result;
use burn::backend::WebGpu;
use burn::prelude::*;
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

fn main() -> Result<()> {
    let device = burn::backend::wgpu::WgpuDevice::default();
    let mut fg = Flowgraph::new();

    let orig = Vec::from_iter(0..999_999i32);
    let mut src: VectorSource<i32, burn_buffer::Writer<B, Int>> = VectorSource::new(orig.clone());
    src.output().set_device(&device);
    src.output().inject_buffers(4);
    let apply = Apply::new();
    let snk = VectorSink::new(orig.len());

    connect!(fg, src > apply > snk);
    connect!(fg, src < apply);
    connect!(fg, apply < snk);

    Runtime::new().run(fg)?;

    let snk = snk.get()?;
    assert_eq!(snk.items().len(), orig.len());
    snk.items()
        .iter()
        .zip(orig.iter())
        .for_each(|(a, b)| assert_eq!(*a, *b));

    Ok(())
}

use anyhow::Result;
use futuresdr::blocks::Head;
use futuresdr::blocks::NullSink;
use futuresdr::blocks::VectorSource;
use futuresdr::prelude::*;
use futuresdr::runtime::buffer::DefaultCpuReader;
use futuresdr::runtime::buffer::DefaultCpuWriter;
use futuresdr::runtime::buffer::LocalCpuReader;
use futuresdr::runtime::buffer::LocalCpuWriter;

fn main() -> Result<()> {
    let mut fg = Flowgraph::new();
    let snk = fg.add(NullSink::<u8, DefaultCpuReader<u8>>::new());

    {
        let mut local = fg.local_domain();

        let src = local.add(|| VectorSource::<u8, LocalCpuWriter<u8>>::new(vec![1, 2, 3, 4]));
        let head = local.add(|| Head::<u8, LocalCpuReader<u8>, DefaultCpuWriter<u8>>::new(3));

        local.stream(&src, |b| b.output(), &head, |b| b.input())?;
        local.stream_to_normal(&head, |b| b.output(), &snk, |b| b.input())?;
    }

    let fg = Runtime::new().run(fg)?;
    let snk = fg.block(&snk)?;

    println!("received {} items", snk.n_received());

    Ok(())
}

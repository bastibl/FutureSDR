use anyhow::Result;
use futuresdr::blocks::Head;
use futuresdr::blocks::NullSink;
use futuresdr::blocks::VectorSource;
use futuresdr::prelude::*;
use futuresdr::runtime::buffer::DefaultCpuReader;
use futuresdr::runtime::buffer::DefaultCpuWriter;
use futuresdr::runtime::buffer::LocalCpuReader;
use futuresdr::runtime::buffer::LocalCpuWriter;
use futuresdr::runtime::dev::BlockMeta;
use futuresdr::runtime::dev::Kernel;
use futuresdr::runtime::dev::MessageOutputs;
use futuresdr::runtime::dev::WorkIo;
use futuresdr::runtime::macros::Block;
use std::rc::Rc;

#[derive(Block)]
struct NonSendLocalBlock {
    _state: Rc<()>,
}

impl NonSendLocalBlock {
    fn new() -> Self {
        Self {
            _state: Rc::new(()),
        }
    }
}

impl Kernel for NonSendLocalBlock {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        io.finished = true;
        Ok(())
    }
}

#[test]
fn local_to_local_and_local_to_normal() -> Result<()> {
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
    assert_eq!(fg.block(&snk)?.n_received(), 3);

    Ok(())
}

#[test]
fn local_domain_accepts_non_send_blocks() -> Result<()> {
    let mut fg = Flowgraph::new();
    {
        let mut local = fg.local_domain();
        local.add(NonSendLocalBlock::new);
    }

    Runtime::new().run(fg)?;
    Ok(())
}

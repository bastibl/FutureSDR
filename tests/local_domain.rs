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
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

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

#[derive(Block)]
#[blocking]
struct BlockingNoop {
    worked: Arc<AtomicBool>,
}

impl BlockingNoop {
    fn new(worked: Arc<AtomicBool>) -> Self {
        Self { worked }
    }
}

impl Kernel for BlockingNoop {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        self.worked.store(true, Ordering::SeqCst);
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

#[test]
fn runtime_owned_flowgraph_runs_explicit_local_blocks() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = rt.flowgraph();

    let src = fg.add_local(|| VectorSource::<u8, DefaultCpuWriter<u8>>::new(vec![1, 2, 3, 4]));
    let snk = fg.add(NullSink::<u8, DefaultCpuReader<u8>>::new());

    assert!(src.with_local(&fg, |_| true)?);
    fg.stream(&src, |b| b.output(), &snk, |b| b.input())?;

    let fg = rt.run(fg)?;
    assert_eq!(fg.block(&snk)?.n_received(), 4);
    assert!(src.with_local(&fg, |_| true)?);

    Ok(())
}

#[test]
fn blocking_add_runs_in_auto_local_domain() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = rt.flowgraph();
    let worked = Arc::new(AtomicBool::new(false));
    let blk = fg.add(BlockingNoop::new(worked.clone()));

    let fg = rt.run(fg)?;

    assert!(worked.load(Ordering::SeqCst));
    assert!(blk.with_local(&fg, |_| true)?);
    Ok(())
}

#[test]
fn local_streams_reject_different_domains() -> Result<()> {
    let mut fg = Flowgraph::new();
    let mut local_a = fg.local_domain();
    let src = local_a.add(|| VectorSource::<u8, LocalCpuWriter<u8>>::new(vec![1]));
    let mut local_b = fg.local_domain();
    let snk = local_b.add(NullSink::<u8, LocalCpuReader<u8>>::new);

    assert!(
        local_a
            .stream(&src, |b| b.output(), &snk, |b| b.input())
            .is_err()
    );

    Ok(())
}

#[test]
fn runtime_rejects_flowgraph_from_other_runtime() -> Result<()> {
    let rt_a = Runtime::new();
    let rt_b = Runtime::new();
    let fg = rt_a.flowgraph();

    assert!(rt_b.run(fg).is_err());
    Ok(())
}

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
use futuresdr::runtime::dev::LocalKernel;
use futuresdr::runtime::dev::LocalWorkIo;
use futuresdr::runtime::dev::MessageOutputs;
use futuresdr::runtime::dev::WorkIo;
use futuresdr::runtime::macros::Block;
use futuresdr::runtime::macros::LocalBlock;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

#[derive(LocalBlock)]
struct NonSendLocalBlock {
    state: Rc<()>,
    waited: bool,
}

impl NonSendLocalBlock {
    fn new() -> Self {
        Self {
            state: Rc::new(()),
            waited: false,
        }
    }
}

impl LocalKernel for NonSendLocalBlock {
    async fn work(
        &mut self,
        io: &mut LocalWorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        if self.waited {
            io.finished = true;
        } else {
            self.waited = true;
            let state = self.state.clone();
            io.call_again = false;
            io.block_on(async move {
                let _state = state;
            });
        }
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

    let local = fg.local_domain();

    let src = fg.add_local(local, || {
        VectorSource::<u8, LocalCpuWriter<u8>>::new(vec![1, 2, 3, 4])
    });
    let head = fg.add_local(local, || {
        Head::<u8, LocalCpuReader<u8>, DefaultCpuWriter<u8>>::new(3)
    });

    fg.stream(&src, |b| b.output(), &head, |b| b.input())?;
    fg.stream(&head, |b| b.output(), &snk, |b| b.input())?;

    let fg = Runtime::new().run(fg)?;
    assert_eq!(fg.block(&snk)?.n_received(), 3);

    Ok(())
}

#[test]
fn local_domain_accepts_non_send_blocks() -> Result<()> {
    let mut fg = Flowgraph::new();
    let local = fg.local_domain();
    fg.add_local(local, NonSendLocalBlock::new);

    Runtime::new().run(fg)?;
    Ok(())
}

#[test]
fn flowgraph_runs_local_domain_blocks() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let local = fg.local_domain();
    let src = fg.add_local(local, || {
        VectorSource::<u8, DefaultCpuWriter<u8>>::new(vec![1, 2, 3, 4])
    });
    let snk = fg.add(NullSink::<u8, DefaultCpuReader<u8>>::new());

    assert!(src.with(&fg, |_| true)?);
    fg.stream(&src, |b| b.output(), &snk, |b| b.input())?;

    let fg = rt.run(fg)?;
    assert_eq!(fg.block(&snk)?.n_received(), 4);
    assert!(src.with(&fg, |_| true)?);

    Ok(())
}

#[test]
fn stream_dyn_connects_local_source_to_normal_blocks() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let local = fg.local_domain();
    let src = fg.add_local(local, || {
        VectorSource::<u8, DefaultCpuWriter<u8>>::new(vec![1, 2, 3, 4])
    });
    let snk0 = fg.add(NullSink::<u8, DefaultCpuReader<u8>>::new());
    let snk1 = fg.add(NullSink::<u8, DefaultCpuReader<u8>>::new());

    fg.stream_dyn(src, "output", snk0, "input")?;
    fg.stream_dyn(src, "output", snk1, "input")?;

    let fg = rt.run(fg)?;
    assert_eq!(fg.block(&snk0)?.n_received(), 4);
    assert_eq!(fg.block(&snk1)?.n_received(), 4);

    Ok(())
}

#[test]
fn blocking_add_runs_in_auto_local_domain() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = Flowgraph::new();
    let worked = Arc::new(AtomicBool::new(false));
    let blk = fg.add(BlockingNoop::new(worked.clone()));

    let fg = rt.run(fg)?;

    assert!(worked.load(Ordering::SeqCst));
    assert!(blk.with(&fg, |_| true)?);
    Ok(())
}

#[test]
fn local_streams_reject_different_domains() -> Result<()> {
    let mut fg = Flowgraph::new();
    let local_a = fg.local_domain();
    let src = fg.add_local(local_a, || {
        VectorSource::<u8, LocalCpuWriter<u8>>::new(vec![1])
    });
    let local_b = fg.local_domain();
    let snk = fg.add_local(local_b, NullSink::<u8, LocalCpuReader<u8>>::new);

    assert!(
        fg.stream(&src, |b| b.output(), &snk, |b| b.input())
            .is_err()
    );

    Ok(())
}

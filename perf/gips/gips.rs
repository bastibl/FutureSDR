use clap::Parser;
use std::iter::repeat_with;
use std::time;

use futuresdr::anyhow::{Context, Result};
use futuresdr::blocks::CopyRandBuilder;
use futuresdr::blocks::FirBuilder;
use futuresdr::blocks::Head;
use futuresdr::blocks::NullSink;
use futuresdr::blocks::NullSource;
use futuresdr::runtime::scheduler::gips::GipsGraph;
use futuresdr::runtime::scheduler::GipsScheduler;
use futuresdr::runtime::scheduler::SmolScheduler;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;

#[derive(Parser, Debug)]
struct Args {
    #[clap(short, long, default_value_t = 0)]
    run: usize,
    #[clap(short, long, default_value_t = 6)]
    stages: usize,
    #[clap(short, long, default_value_t = 5)]
    pipes: usize,
    #[clap(short = 'n', long, default_value_t = 15000000)]
    samples: usize,
    #[clap(short, long, default_value_t = 4000000000)]
    max_copy: usize,
    #[clap(short = 'S', long, default_value = "smol1")]
    scheduler: String,
}

fn main() -> Result<()> {
    let Args {
        run,
        pipes,
        stages,
        samples,
        max_copy,
        scheduler,
    } = Args::parse();

    let mut fg = Flowgraph::new();
    let mut gips = GipsGraph::new(4, 0.25);
    let taps: [f32; 64] = repeat_with(rand::random::<f32>)
        .take(64)
        .collect::<Vec<f32>>()
        .try_into()
        .unwrap();

    let mut snks = Vec::new();

    for _ in 0..pipes {
        let src = fg.add_block(NullSource::<f32>::new());
        gips.add_block(0, 1);
        let head = fg.add_block(Head::<f32>::new(samples as u64));
        gips.add_block(1, 1);
        fg.connect_stream(src, "out", head, "in")?;
        gips.connect(src, head);

        let copy = fg.add_block(CopyRandBuilder::<f32>::new().max_copy(max_copy).build());
        gips.add_block(1, 1);
        let mut last = fg.add_block(FirBuilder::new::<f32, f32, _, _>(taps.to_owned()));
        gips.add_block(10, 1);
        fg.connect_stream(head, "out", copy, "in")?;
        gips.connect(head, copy);
        fg.connect_stream(copy, "out", last, "in")?;
        gips.connect(copy, last);

        for _ in 1..stages {
            let copy = fg.add_block(CopyRandBuilder::<f32>::new().max_copy(max_copy).build());
            gips.add_block(1, 1);
            fg.connect_stream(last, "out", copy, "in")?;
            gips.connect(last, copy);
            last = fg.add_block(FirBuilder::new::<f32, f32, _, _>(taps.to_owned()));
            gips.add_block(10, 1);
            fg.connect_stream(copy, "out", last, "in")?;
            gips.connect(copy, last);
        }

        let snk = fg.add_block(NullSink::<f32>::new());
        gips.add_block(0, 1);
        fg.connect_stream(last, "out", snk, "in")?;
        gips.connect(last, snk);
        snks.push(snk);
    }

    let json = serde_json::to_string_pretty(&gips).unwrap();
    println!("graph {}", json);

    let elapsed;

    if scheduler == "smol1" {
        let runtime = Runtime::with_scheduler(SmolScheduler::new(1, false));
        let now = time::Instant::now();
        fg = runtime.run(fg)?;
        elapsed = now.elapsed();
    } else if scheduler == "smoln" {
        let runtime = Runtime::with_scheduler(SmolScheduler::default());
        let now = time::Instant::now();
        fg = runtime.run(fg)?;
        elapsed = now.elapsed();
    } else if scheduler == "gips" {
        let runtime = Runtime::with_scheduler(GipsScheduler::new());
        let now = time::Instant::now();
        fg = runtime.run(fg)?;
        elapsed = now.elapsed();
    } else {
        panic!("unknown scheduler");
    }

    for s in snks {
        let snk = fg.kernel::<NullSink<f32>>(s).context("no block")?;
        let v = snk.n_received();
        assert_eq!(v, samples - (stages * 63));
    }

    println!(
        "{},{},{},{},{},{},{}",
        run,
        pipes,
        stages,
        samples,
        max_copy,
        scheduler,
        elapsed.as_secs_f64()
    );

    Ok(())
}

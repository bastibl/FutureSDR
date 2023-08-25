#![allow(unreachable_code)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_variables)]
use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::Apply;
use futuresdr::blocks::FileSource;
use futuresdr::blocks::FirBuilder;
use futuresdr::futuredsp::firdes;
use futuresdr::futuredsp::windows;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;

use netsys::Decoder;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// File
    #[clap(short, long)]
    file: Option<String>,
    /// Sample Rate
    #[clap(short, long, default_value_t = 4e6)]
    sample_rate: f32,
    /// Seify Args
    #[clap(short, long)]
    args: Option<String>,
    /// Gain
    #[clap(short, long, default_value_t = 40.0)]
    gain: f64,
    /// Frequency
    #[clap(long, default_value_t = 2.48092e9)]
    freq: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {args:?}");

    let mut fg = Flowgraph::new();

    // read from file if a filename was specified
    // otherwise, read from SDR
    let src = match args.file {
        Some(file) => FileSource::<Complex32>::new(file, false),
        None => {
            let mut src = SourceBuilder::new().frequency(args.freq).gain(args.gain);
            if let Some(a) = args.args {
                src = src.args(a)?;
            }
            src.build()?
        }
    };

    // downsamples by a factor of 16
    let resamp = FirBuilder::new_resampling::<Complex32, Complex32>(1, 16);

    //  a block that outputs the squared magnitude for every input sample
    //  see the `Apply` block, how this can be defined on-the-fly
    let complex_to_float = todo!();

    // compute the average amplitude with an IIR filter (alpha = 0.0001)
    // and subtract it from the input samples
    // Note: Apply closures can be stateful
    let avg = todo!();

    let taps = firdes::lowpass::<f32>(15e3 / 250e3, &windows::hamming(64, false));
    // low_pass filter, using the taps
    let low_pass = todo!();

    // create a block that outputs a 1u8 if the amplitude is > 0.0
    // otherwise 0u8
    let slice = todo!();

    let decoder = Decoder::new();

    // connect the blocks in the order that they are defined
    // use the connnect! macro

    // this runs the flowgraph
    Runtime::new().run(fg)?;

    Ok(())
}

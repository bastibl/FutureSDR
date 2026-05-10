use futuresdr::runtime::dev::prelude::*;

const MIN_GAP: usize = 480;
const MAX_SAMPLES: usize = 540 * 80;
const THRESHOLD: f32 = 0.56;

#[derive(Debug)]
enum State {
    Search,
    Found,
    Copy(usize, f32, bool),
}

#[derive(Block)]
pub struct SyncShort<
    I0 = DefaultCpuReader<Complex32>,
    I1 = DefaultCpuReader<Complex32>,
    I2 = DefaultCpuReader<f32>,
    O = DefaultCpuWriter<Complex32>,
> where
    I0: CpuBufferReader<Item = Complex32>,
    I1: CpuBufferReader<Item = Complex32>,
    I2: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    #[input]
    in_sig: I0,
    #[input]
    in_abs: I1,
    #[input]
    in_cor: I2,
    #[output]
    output: O,
    state: State,
    pending_start_tag: Option<f32>,
    samples_seen: usize,
    samples_out: usize,
    starts: usize,
    resyncs: usize,
    report_max_cor: f32,
    next_report_at: usize,
}

impl<I0, I1, I2, O> SyncShort<I0, I1, I2, O>
where
    I0: CpuBufferReader<Item = Complex32>,
    I1: CpuBufferReader<Item = Complex32>,
    I2: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    pub fn new() -> Self {
        Self {
            in_sig: I0::default(),
            in_abs: I1::default(),
            in_cor: I2::default(),
            output: O::default(),
            state: State::Search,
            pending_start_tag: None,
            samples_seen: 0,
            samples_out: 0,
            starts: 0,
            resyncs: 0,
            report_max_cor: 0.0,
            next_report_at: 1_000_000,
        }
    }
}
impl<I0, I1, I2, O> Default for SyncShort<I0, I1, I2, O>
where
    I0: CpuBufferReader<Item = Complex32>,
    I1: CpuBufferReader<Item = Complex32>,
    I2: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<I0, I1, I2, O> Kernel for SyncShort<I0, I1, I2, O>
where
    I0: CpuBufferReader<Item = Complex32>,
    I1: CpuBufferReader<Item = Complex32>,
    I2: CpuBufferReader<Item = f32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let in_sig = self.in_sig.slice();
        let in_abs = self.in_abs.slice();
        let in_cor = self.in_cor.slice();
        let in_cor_len = in_cor.len();
        let (out, mut tags) = self.output.slice_with_tags();

        let n_input = std::cmp::min(std::cmp::min(in_sig.len(), in_abs.len()), in_cor.len());

        let mut o = 0;
        let mut i = 0;

        while i < n_input && o < out.len() {
            self.report_max_cor = self.report_max_cor.max(in_cor[i]);
            match self.state {
                State::Search => {
                    if in_cor[i] > THRESHOLD {
                        self.state = State::Found;
                    }
                }
                State::Found => {
                    if in_cor[i] > THRESHOLD {
                        let f_offset = -in_abs[i].arg() / 16.0;
                        self.starts += 1;
                        if self.starts <= 10 {
                            info!(
                                "sync short: start candidate {} at total sample {}, cor {:.3}, freq offset {:.6}",
                                self.starts,
                                self.samples_seen + i,
                                in_cor[i],
                                f_offset
                            );
                        }
                        self.state = State::Copy(0, f_offset, false);
                        self.pending_start_tag = Some(f_offset);
                    } else {
                        self.state = State::Search;
                    }
                }
                State::Copy(n_copied, f_offset, mut last_above_threshold) => {
                    if in_cor[i] > THRESHOLD {
                        // resync
                        if last_above_threshold && n_copied > MIN_GAP {
                            let f_offset = -in_abs[i].arg() / 16.0;
                            self.resyncs += 1;
                            if self.resyncs <= 10 {
                                info!(
                                    "sync short: resync {} at total sample {}, cor {:.3}, freq offset {:.6}",
                                    self.resyncs,
                                    self.samples_seen + i,
                                    in_cor[i],
                                    f_offset
                                );
                            }
                            self.state = State::Copy(0, f_offset, false);
                            self.pending_start_tag = Some(f_offset);
                            i += 1;
                            continue;
                        } else {
                            last_above_threshold = true;
                        }
                    } else {
                        last_above_threshold = false;
                    }

                    if n_copied == 0
                        && let Some(f_offset) = self.pending_start_tag.take()
                    {
                        tags.add_tag(o, Tag::NamedF32("wifi_start".to_string(), f_offset));
                    }

                    out[o] = in_sig[i] * Complex32::from_polar(1.0, f_offset * n_copied as f32); // accum?
                    o += 1;

                    if n_copied + 1 == MAX_SAMPLES {
                        self.state = State::Search;
                    } else {
                        self.state = State::Copy(n_copied + 1, f_offset, last_above_threshold);
                    }
                }
            }
            i += 1;
        }

        self.in_sig.consume(i);
        self.in_abs.consume(i);
        self.in_cor.consume(i);
        self.output.produce(o);
        self.samples_seen += i;
        self.samples_out += o;

        if self.samples_seen >= self.next_report_at {
            info!(
                "sync short: processed {} samples, output {}, starts {}, resyncs {}, max corr {:.3}",
                self.samples_seen, self.samples_out, self.starts, self.resyncs, self.report_max_cor
            );
            self.next_report_at = self.samples_seen + 20_000_000;
            self.report_max_cor = 0.0;
        }

        if self.in_cor.finished() && i == in_cor_len {
            io.finished = true;
        }

        Ok(())
    }
}

use any_spawner::Executor;
use futuresdr::blocks::wasm::HackRf;
use futuresdr::blocks::{Apply, Combine, Delay, Fft};
use futuresdr::prelude::*;
use futuresdr::runtime::dev::{BlockMeta, MessageOutputs, WorkIo};
use futuresdr::runtime::macros::Block;
use futuresdr::runtime::scheduler::WasmScheduler;
use gloo_timers::future::TimeoutFuture;
use leptos::html::Input;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::HtmlInputElement;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use wasm_bindgen_futures::JsFuture;

use crate::{Decoder, FrameEqualizer, MovingAverage, SyncLong, SyncShort};

const DEFAULT_CHANNEL: &str = "34";
const DEFAULT_FREQUENCY: f64 = 5_170_000_000.0;
const DEFAULT_SAMPLE_RATE: f64 = 20_000_000.0;
const FRAME_QUEUE_LIMIT: usize = 100;
const DISPLAY_FRAME_LIMIT: usize = 50;

const WLAN_CHANNELS: &[(&str, f64)] = &[
    ("1", 2412e6),
    ("2", 2417e6),
    ("3", 2422e6),
    ("4", 2427e6),
    ("5", 2432e6),
    ("6", 2437e6),
    ("7", 2442e6),
    ("8", 2447e6),
    ("9", 2452e6),
    ("10", 2457e6),
    ("11", 2462e6),
    ("12", 2467e6),
    ("13", 2472e6),
    ("14", 2484e6),
    ("34", 5170e6),
    ("36", 5180e6),
    ("38", 5190e6),
    ("40", 5200e6),
    ("42", 5210e6),
    ("44", 5220e6),
    ("46", 5230e6),
    ("48", 5240e6),
    ("50", 5250e6),
    ("52", 5260e6),
    ("54", 5270e6),
    ("56", 5280e6),
    ("58", 5290e6),
    ("60", 5300e6),
    ("62", 5310e6),
    ("64", 5320e6),
    ("100", 5500e6),
    ("102", 5510e6),
    ("104", 5520e6),
    ("106", 5530e6),
    ("108", 5540e6),
    ("110", 5550e6),
    ("112", 5560e6),
    ("114", 5570e6),
    ("116", 5580e6),
    ("118", 5590e6),
    ("120", 5600e6),
    ("122", 5610e6),
    ("124", 5620e6),
    ("126", 5630e6),
    ("128", 5640e6),
    ("132", 5660e6),
    ("134", 5670e6),
    ("136", 5680e6),
    ("138", 5690e6),
    ("140", 5700e6),
    ("142", 5710e6),
    ("144", 5720e6),
    ("149", 5745e6),
    ("151", 5755e6),
    ("153", 5765e6),
    ("155", 5775e6),
    ("157", 5785e6),
    ("159", 5795e6),
    ("161", 5805e6),
    ("165", 5825e6),
    ("172", 5860e6),
    ("174", 5870e6),
    ("176", 5880e6),
    ("178", 5890e6),
    ("180", 5900e6),
    ("182", 5910e6),
    ("184", 5920e6),
];

type FrameQueue = Arc<Mutex<VecDeque<Vec<u8>>>>;
type Shared<T> = Arc<Mutex<Option<T>>>;

#[derive(Clone)]
struct RunControl {
    flowgraph: FlowgraphHandle,
    source: BlockId,
}

struct Receiver {
    _runtime: Runtime,
    control: RunControl,
    frames: FrameQueue,
    failure: Shared<String>,
}

#[derive(Clone, Copy)]
struct ReceiverConfig {
    frequency: f64,
    sample_rate: f64,
    lna_gain: u16,
    vga_gain: u16,
    amp: bool,
    dc_offset: bool,
}

#[derive(Block)]
#[message_inputs(r#in)]
#[null_kernel]
struct FramePipe {
    frames: FrameQueue,
}

impl FramePipe {
    fn new(frames: FrameQueue) -> Self {
        Self { frames }
    }

    async fn r#in(
        &mut self,
        _io: &mut WorkIo,
        _mo: &mut MessageOutputs,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        if let Pmt::Blob(data) = p {
            let mut frames = self
                .frames
                .lock()
                .map_err(|_| anyhow::anyhow!("frame queue lock poisoned"))?;
            frames.push_back(data);
            while frames.len() > FRAME_QUEUE_LIMIT {
                frames.pop_front();
            }
            Ok(Pmt::Ok)
        } else {
            Ok(Pmt::InvalidValue)
        }
    }
}

#[component]
pub fn Gui() -> impl IntoView {
    let (status, set_status) = signal(String::from("idle"));
    let (running, set_running) = signal(false);
    let (frames, set_frames) = signal(VecDeque::<Frame>::new());
    let (control, set_control) = signal(None::<RunControl>);
    let (frequency, set_frequency) = signal(DEFAULT_FREQUENCY);
    let (sample_rate, set_sample_rate) = signal(DEFAULT_SAMPLE_RATE);
    let (lna_gain, set_lna_gain) = signal(32u16);
    let (vga_gain, set_vga_gain) = signal(24u16);
    let (amp, set_amp) = signal(true);
    let dc_offset = NodeRef::<Input>::new();

    let start = move |_| {
        if running.get_untracked() {
            return;
        }

        let config = ReceiverConfig {
            frequency: frequency.get_untracked(),
            sample_rate: sample_rate.get_untracked(),
            lna_gain: lna_gain.get_untracked(),
            vga_gain: vga_gain.get_untracked(),
            amp: amp.get_untracked(),
            dc_offset: dc_offset
                .get_untracked()
                .map(|input| input.checked())
                .unwrap_or(false),
        };

        set_running.set(true);
        set_status.set("requesting HackRF permission".to_string());
        set_frames.set(VecDeque::new());

        spawn_local(async move {
            match start_receiver(config, set_status).await {
                Ok(receiver) => {
                    set_control.set(Some(receiver.control.clone()));
                    set_status.set("running".to_string());
                    poll_frames(receiver, set_frames, set_status).await;
                }
                Err(e) => {
                    set_running.set(false);
                    set_status.set(format!("failed: {e:?}"));
                }
            }
        });
    };

    view! {
        <div class="min-h-screen bg-slate-900 text-slate-100">
            <header class="bg-slate-800 border-b border-slate-700 shadow-lg">
                <div class="flex items-center gap-3 px-4 py-3">
                    <div class="text-white font-semibold tracking-tight text-base">"FutureSDR WLAN RX"</div>
                    <div class="text-xs text-slate-400">"HackRF local domain + 3 WASM scheduler workers"</div>
                </div>
            </header>

            <main class="p-4 space-y-4">
                <section class="bg-slate-800 border border-slate-700 rounded-xl p-5 shadow-lg">
                    <div class="flex items-center justify-between mb-4">
                        <h2 class="text-white text-lg font-semibold">"Receiver"</h2>
                        <span class="text-sm text-slate-300">{move || status.get()}</span>
                    </div>

                    <div class="grid grid-cols-1 md:grid-cols-3 xl:grid-cols-6 gap-4 items-end">
                        <label class="block">
                            <span class="text-slate-300 text-sm">"WLAN Channel"</span>
                            <select
                                class="mt-1 w-full rounded bg-slate-900 border border-slate-600 text-slate-100 px-2 py-2"
                                on:change=move |ev| {
                                    let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                    let chan = input.value();
                                    if let Some((_, freq)) = WLAN_CHANNELS.iter().find(|(c, _)| *c == chan) {
                                        set_frequency.set(*freq);
                                        post_source(control, "freq", Pmt::F64(*freq));
                                    }
                                }
                            >
                                {WLAN_CHANNELS.iter().map(|(chan, _)| view! {
                                    <option value=*chan selected=*chan == DEFAULT_CHANNEL>{*chan}</option>
                                }).collect_view()}
                            </select>
                        </label>

                        <label class="block">
                            <span class="text-slate-300 text-sm">"Sample Rate"</span>
                            <select
                                class="mt-1 w-full rounded bg-slate-900 border border-slate-600 text-slate-100 px-2 py-2"
                                on:change=move |ev| {
                                    let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                    let rate = input.value().parse::<f64>().unwrap_or(DEFAULT_SAMPLE_RATE);
                                    set_sample_rate.set(rate);
                                    post_source(control, "sample_rate", Pmt::F64(rate));
                                }
                            >
                                <option value="5000000">"5 MHz"</option>
                                <option value="10000000">"10 MHz"</option>
                                <option value="20000000" selected=true>"20 MHz"</option>
                            </select>
                        </label>

                        <label class="block">
                            <span class="text-slate-300 text-sm">"LNA Gain: " {move || lna_gain.get()} " dB"</span>
                            <input
                                class="mt-2 w-full accent-cyan-400"
                                type="range"
                                min="0"
                                max="40"
                                step="8"
                                value="32"
                                on:input=move |ev| {
                                    let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                    let gain = input.value().parse::<u16>().unwrap_or(32);
                                    set_lna_gain.set(gain);
                                    post_source(control, "lna", Pmt::U32(gain.into()));
                                }
                            />
                        </label>

                        <label class="block">
                            <span class="text-slate-300 text-sm">"VGA Gain: " {move || vga_gain.get()} " dB"</span>
                            <input
                                class="mt-2 w-full accent-cyan-400"
                                type="range"
                                min="0"
                                max="62"
                                step="2"
                                value="24"
                                on:input=move |ev| {
                                    let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                    let gain = input.value().parse::<u16>().unwrap_or(24);
                                    set_vga_gain.set(gain);
                                    post_source(control, "vga", Pmt::U32(gain.into()));
                                }
                            />
                        </label>

                        <label class="inline-flex items-center gap-2 text-sm text-slate-300">
                            <input
                                class="accent-cyan-400"
                                type="checkbox"
                                checked=true
                                on:change=move |ev| {
                                    let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                    let enabled = input.checked();
                                    set_amp.set(enabled);
                                    post_source(control, "amp", Pmt::Bool(enabled));
                                }
                            />
                            "RF Amp"
                        </label>

                        <label class="inline-flex items-center gap-2 text-sm text-slate-300">
                            <input class="accent-cyan-400" type="checkbox" node_ref=dc_offset />
                            "DC Offset Correction"
                        </label>
                    </div>

                    <button
                        class="mt-5 rounded bg-cyan-600 hover:bg-cyan-500 disabled:bg-slate-600 text-white px-4 py-2 font-semibold"
                        on:click=start
                        disabled=running
                    >
                        {move || if running.get() { "Running" } else { "Start RX" }}
                    </button>
                </section>

                <section class="bg-slate-800 border border-slate-700 rounded-xl p-5 shadow-lg">
                    <div class="flex items-center justify-between mb-3">
                        <h2 class="text-white text-lg font-semibold">"Received Frames"</h2>
                        <span class="text-sm text-slate-400">{move || format!("{} shown", frames.get().len())}</span>
                    </div>
                    <div class="font-mono text-sm bg-slate-950 border border-slate-700 rounded-lg p-3 min-h-48 overflow-auto">
                        {move || {
                            if frames.get().is_empty() {
                                view! { <div class="text-slate-500">"No frames received yet."</div> }.into_any()
                            } else {
                                view! {
                                    <ul class="space-y-1">
                                        {frames.get().into_iter().map(|frame| view! {
                                            <li class="text-slate-200">{frame.render()}</li>
                                        }).collect_view()}
                                    </ul>
                                }.into_any()
                            }
                        }}
                    </div>
                </section>
            </main>
        </div>
    }
}

fn post_source(control: ReadSignal<Option<RunControl>>, handler: &'static str, p: Pmt) {
    if let Some(control) = control.get_untracked() {
        spawn_local(async move {
            if let Err(e) = control.flowgraph.post(control.source, handler, p).await {
                futuresdr::tracing::warn!("failed to post {handler}: {e:?}");
            }
        });
    }
}

async fn start_receiver(
    config: ReceiverConfig,
    set_status: WriteSignal<String>,
) -> anyhow::Result<Receiver> {
    request_hackrf().await?;
    set_status.set("starting flowgraph".to_string());

    let rt = Runtime::with_scheduler(WasmScheduler::new(3));
    let rt_handle = rt.handle();
    let frames = FrameQueue::default();
    let frames_for_pipe = frames.clone();
    let started = Shared::<FlowgraphHandle>::default();
    let started_for_task = started.clone();
    let failure = Shared::<String>::default();
    let failure_for_task = failure.clone();

    let mut fg = Flowgraph::new();
    let local = fg.local_domain();
    let src = fg.add_local(local, move || {
        HackRf::new()
            .frequency(config.frequency as u64)
            .initial_sample_rate(config.sample_rate)
            .lna_gain(config.lna_gain)
            .vga_gain(config.vga_gain)
            .amp_enable(config.amp)
    });
    let source = src.id();

    let start = rt.spawn(async move {
        let result = async move {
            build_rx_flowgraph(&mut fg, src, frames_for_pipe, config.dc_offset)?;
            let running = rt_handle.start(fg).await?;
            let (task, flowgraph) = running.split();
            *started_for_task
                .lock()
                .map_err(|_| anyhow::anyhow!("started lock poisoned"))? = Some(flowgraph);
            task.await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(e) = result {
            if let Ok(mut failure) = failure_for_task.lock() {
                *failure = Some(format!("{e:?}"));
            }
        }
    });
    start.detach();

    loop {
        if let Some(flowgraph) = started.lock().ok().and_then(|mut h| h.take()) {
            return Ok(Receiver {
                _runtime: rt,
                control: RunControl { flowgraph, source },
                frames,
                failure,
            });
        }
        if let Some(error) = failure.lock().ok().and_then(|e| e.clone()) {
            anyhow::bail!(error);
        }
        TimeoutFuture::new(10).await;
    }
}

fn build_rx_flowgraph(
    fg: &mut Flowgraph,
    src: BlockRef<HackRf>,
    frames: FrameQueue,
    dc_offset: bool,
) -> Result<()> {
    let (prev, output): (BlockId, &'static str) = if dc_offset {
        let mut avg_real = 0.0;
        let mut avg_img = 0.0;
        let ratio = 1.0e-5;
        let dc = Apply::new(move |c: &Complex32| -> Complex32 {
            avg_real = ratio * (c.re - avg_real) + avg_real;
            avg_img = ratio * (c.im - avg_img) + avg_img;
            Complex32::new(c.re - avg_real, c.im - avg_img)
        });

        connect!(fg, src > dc);
        (dc.into(), "output")
    } else {
        (src.into(), "output")
    };

    let delay = fg.add(Delay::<Complex32>::new(16));
    fg.stream_dyn(prev, output, delay, "input")?;

    let complex_to_mag_2 = fg.add(Apply::new(|i: &Complex32| i.norm_sqr()));
    let float_avg = MovingAverage::<f32>::new(64);
    fg.stream_dyn(prev, output, complex_to_mag_2, "input")?;
    connect!(fg, complex_to_mag_2 > float_avg);

    let mult_conj = fg.add(Combine::new(|a: &Complex32, b: &Complex32| a * b.conj()));
    let complex_avg = MovingAverage::<Complex32>::new(48);
    fg.stream_dyn(prev, output, mult_conj, "in0")?;
    connect!(fg, mult_conj > complex_avg;
                 delay > in1.mult_conj);

    let divide_mag = Combine::new(|a: &Complex32, b: &f32| a.norm() / b);
    connect!(fg, complex_avg > in0.divide_mag; float_avg > in1.divide_mag);

    let sync_short: SyncShort = SyncShort::new();
    connect!(fg, delay > in_sig.sync_short;
                 complex_avg > in_abs.sync_short;
                 divide_mag > in_cor.sync_short);

    let sync_long: SyncLong = SyncLong::new();
    let fft = Fft::new(64);
    let frame_equalizer: FrameEqualizer = FrameEqualizer::new();
    let decoder: Decoder = Decoder::new();
    let frame_pipe = FramePipe::new(frames);

    connect!(fg, sync_short > sync_long > fft > frame_equalizer > decoder;
                 decoder.rx_frames | frame_pipe);

    Ok(())
}

async fn poll_frames(receiver: Receiver, set_frames: WriteSignal<VecDeque<Frame>>, set_status: WriteSignal<String>) {
    loop {
        if let Some(error) = receiver.failure.lock().ok().and_then(|e| e.clone()) {
            set_status.set(format!("flowgraph stopped: {error}"));
            return;
        }

        let mut pending = Vec::new();
        if let Ok(mut queue) = receiver.frames.lock() {
            while let Some(data) = queue.pop_front() {
                pending.push(data);
            }
        }

        if pending.is_empty() {
            TimeoutFuture::new(50).await;
            continue;
        }

        set_frames.update(|frames| {
            for data in pending {
                frames.push_front(Frame::new(data));
                while frames.len() > DISPLAY_FRAME_LIMIT {
                    frames.pop_back();
                }
            }
        });
    }
}

async fn request_hackrf() -> anyhow::Result<()> {
    let window = leptos::web_sys::window().expect("No global `window` exists");
    let usb = window.navigator().usb();

    let filter = leptos::web_sys::UsbDeviceFilter::new();
    filter.set_vendor_id(7504);
    let filters = [filter];
    let options = leptos::web_sys::UsbDeviceRequestOptions::new(&filters);

    let devices: js_sys::Array<leptos::web_sys::UsbDevice> = JsFuture::from(usb.get_devices())
        .await
        .map_err(|e| anyhow::anyhow!("USB get_devices failed: {e:?}"))?;

    if devices.length() == 0 {
        let _ = JsFuture::from(usb.request_device(&options))
            .await
            .map_err(|e| anyhow::anyhow!("USB request_device failed: {e:?}"))?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct Frame {
    len: usize,
    payload: String,
}

impl Frame {
    fn new(data: Vec<u8>) -> Self {
        let len = data.len();
        let payload = data
            .iter()
            .take(32)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        Self { len, payload }
    }

    fn render(&self) -> String {
        format!("received frame ({} bytes): {}", self.len, self.payload)
    }
}

pub fn frontend() {
    console_error_panic_hook::set_once();
    futuresdr::runtime::init();
    Executor::init_wasm_bindgen().unwrap();
    mount_to_body(|| view! { <Gui /> })
}

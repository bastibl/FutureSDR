use any_spawner::Executor;
use futuresdr::futures::StreamExt;
use futuresdr::runtime::FlowgraphDescription;
use futuresdr::runtime::FlowgraphId;
use futuresdr::runtime::Pmt;
use gloo_net::websocket::Message;
use gloo_net::websocket::futures::WebSocket;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::HtmlInputElement;
use leptos::web_sys::HtmlSelectElement;
use prophecy::FlowgraphCanvas;
use prophecy::FlowgraphHandle;
use prophecy::FlowgraphTable;
use prophecy::RuntimeHandle;
use std::collections::VecDeque;

const DEFAULT_CHANNEL: &str = "26";
const DEFAULT_GAIN: f64 = 30.0;
const DISPLAY_FRAME_LIMIT: usize = 50;
const FRAME_WS_PORT: u16 = 9001;

const ZIGBEE_CHANNELS: &[(&str, f64)] = &[
    ("11", 2_405_000_000.0),
    ("12", 2_410_000_000.0),
    ("13", 2_415_000_000.0),
    ("14", 2_420_000_000.0),
    ("15", 2_425_000_000.0),
    ("16", 2_430_000_000.0),
    ("17", 2_435_000_000.0),
    ("18", 2_440_000_000.0),
    ("19", 2_445_000_000.0),
    ("20", 2_450_000_000.0),
    ("21", 2_455_000_000.0),
    ("22", 2_460_000_000.0),
    ("23", 2_465_000_000.0),
    ("24", 2_470_000_000.0),
    ("25", 2_475_000_000.0),
    ("26", 2_480_000_000.0),
];

fn find_source_block_id(desc: &FlowgraphDescription) -> Option<usize> {
    desc.blocks
        .iter()
        .find(|b| {
            b.message_inputs.iter().any(|h| h == "sample_rate")
                && b.message_inputs.iter().any(|h| h == "freq")
                && b.message_inputs.iter().any(|h| h == "gain")
        })
        .or_else(|| {
            desc.blocks.iter().find(|b| {
                let name = b.type_name.to_ascii_lowercase();
                name.contains("seify") && name.contains("source")
            })
        })
        .map(|b| b.id.0)
}

#[component]
pub fn Gui() -> impl IntoView {
    let rt_handle = RuntimeHandle::from_url("http://127.0.0.1:1337");
    let fg_handle = LocalResource::new(move || {
        let rt_handle = rt_handle.clone();
        async move { rt_handle.get_flowgraph(FlowgraphId(0)).await.ok() }
    });

    view! {
        <div class="min-h-screen bg-slate-900 text-slate-100">
            <header class="bg-slate-800 border-b border-slate-700 shadow-lg">
                <div class="flex items-center justify-between gap-3 px-4 py-3">
                    <div>
                        <div class="text-white font-semibold tracking-tight text-base">"FutureSDR ZigBee RX"</div>
                        <div class="text-xs text-slate-400">"Native flowgraph control + frame monitor"</div>
                    </div>
                </div>
            </header>

            {move || match fg_handle.get() {
                Some(Some(handle)) => view! { <ZigBee fg_handle=handle /> }.into_any(),
                Some(None) => view! { <div class="p-6 text-red-300">"Failed to attach flowgraph."</div> }.into_any(),
                None => view! { <div class="p-6 text-slate-300">"Connecting to native flowgraph..."</div> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn ZigBee(fg_handle: FlowgraphHandle) -> impl IntoView {
    let fg_desc = {
        let fg_handle = fg_handle.clone();
        LocalResource::new(move || {
            let fg_handle = fg_handle.clone();
            async move { fg_handle.describe().await.ok() }
        })
    };

    let source_block_id = Memo::new(move |_| {
        fg_desc
            .get()
            .and_then(|x| x)
            .and_then(|desc| find_source_block_id(&desc))
    });

    let (status, set_status) = signal(String::from("connecting frame websocket"));
    let (n_frames, set_n_frames) = signal(0usize);
    let (frames, set_frames) = signal(VecDeque::<Frame>::new());
    let (gain, set_gain) = signal(DEFAULT_GAIN);

    Effect::new(move |_| {
        spawn_local(connect_frame_websocket(
            set_status,
            set_n_frames,
            set_frames,
        ));
    });

    let fg_for_channel = fg_handle.clone();
    let fg_for_gain = fg_handle.clone();
    view! {
        <main class="p-4 space-y-4">
            <section class="bg-slate-800 border border-slate-700 rounded-xl p-5 shadow-lg">
                <div class="flex flex-col md:flex-row md:items-center justify-between gap-3 mb-4">
                    <div>
                        <h2 class="text-white text-lg font-semibold">"Receiver"</h2>
                        <div class="text-sm text-slate-400">
                            {move || source_block_id
                                .get()
                                .map(|id| format!("source block: {id}"))
                                .unwrap_or_else(|| "source block: n/a".to_string())}
                        </div>
                    </div>
                    <span class="rounded-full bg-slate-900 border border-slate-700 px-3 py-1 text-xs text-slate-300">
                        {move || status.get()}
                    </span>
                </div>

                <div class="grid grid-cols-1 md:grid-cols-3 gap-4 items-stretch">
                    <label class="block rounded-lg bg-slate-900 border border-slate-700 p-4 h-full">
                        <span class="text-slate-300 text-sm">"ZigBee Channel"</span>
                        <select
                            class="mt-2 w-full rounded bg-slate-950 border border-slate-600 text-slate-100 px-2 py-2"
                            on:change=move |ev| {
                                let input: HtmlSelectElement = ev.target().unwrap().dyn_into().unwrap();
                                let freq = input.value().parse::<f64>().unwrap_or(2_480_000_000.0);
                                if let Some(source) = source_block_id.get_untracked() {
                                    let fg = fg_for_channel.clone();
                                    spawn_local(async move {
                                        let _ = fg.post(source, "freq", Pmt::F64(freq)).await;
                                    });
                                }
                            }
                        >
                            {ZIGBEE_CHANNELS.iter().map(|(chan, freq)| view! {
                                <option value=freq.to_string() selected=*chan == DEFAULT_CHANNEL>{*chan}</option>
                            }).collect_view()}
                        </select>
                    </label>

                    <label class="block rounded-lg bg-slate-900 border border-slate-700 p-4 h-full">
                        <span class="text-slate-300 text-sm">"RF Gain: " {move || format!("{:.0} dB", gain.get())}</span>
                        <input
                            class="mt-2 w-full accent-cyan-400"
                            type="range"
                            min="0"
                            max="80"
                            step="1"
                            value=DEFAULT_GAIN.to_string()
                            on:input=move |ev| {
                                let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
                                let new_gain = input.value().parse::<f64>().unwrap_or(DEFAULT_GAIN);
                                set_gain.set(new_gain);
                                if let Some(source) = source_block_id.get_untracked() {
                                    let fg = fg_for_gain.clone();
                                    spawn_local(async move {
                                        let _ = fg.post(source, "gain", Pmt::F64(new_gain)).await;
                                    });
                                }
                            }
                        />
                    </label>

                    <div class="rounded-lg bg-slate-900 border border-slate-700 p-4 h-full">
                        <div class="text-slate-400 text-sm">"Decoded frames"</div>
                        <div class="text-3xl font-semibold text-cyan-300 mt-1">{n_frames}</div>
                    </div>
                </div>
            </section>

            <section class="bg-slate-800 border border-slate-700 rounded-xl p-5 shadow-lg">
                <div class="flex items-center justify-between mb-3">
                    <h2 class="text-white text-lg font-semibold">"Received Frames"</h2>
                    <span class="text-sm text-slate-400">{move || format!("{} shown", frames.get().len())}</span>
                </div>
                <div class="font-mono text-sm bg-slate-950 border border-slate-700 rounded-lg p-3 min-h-48 overflow-auto">
                    {move || {
                        let frames = frames.get();
                        if frames.is_empty() {
                            view! { <div class="text-slate-500">"No ZigBee frames received yet."</div> }.into_any()
                        } else {
                            view! {
                                <ul class="space-y-2">
                                    {frames.into_iter().map(|frame| view! {
                                        <li class="rounded border border-slate-800 bg-slate-900/70 p-3">
                                            <div class="flex flex-wrap gap-2 text-xs text-slate-400 mb-1">
                                                <span class="rounded bg-slate-800 px-2 py-1">"PAN " <span class="text-slate-200">{frame.dst_pan}</span></span>
                                                <span class="rounded bg-slate-800 px-2 py-1">"DST " <span class="text-slate-200">{frame.dst_addr}</span></span>
                                            </div>
                                            <div class="text-slate-100 break-all">{frame.payload}</div>
                                        </li>
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }
                    }}
                </div>
            </section>

            <section class="bg-slate-800 border border-slate-700 rounded-xl p-5 shadow-lg">
                <h2 class="text-white text-lg font-semibold mb-3">"Flowgraph"</h2>
                {move || match fg_desc.get() {
                    Some(Some(desc)) => view! {
                        <FlowgraphCanvas fg=desc.clone() on_message_input_click=Callback::new(|_| ()) />
                        <div class="-mx-4">
                            <FlowgraphTable fg=desc on_message_input_click=Callback::new(|_| ()) />
                        </div>
                    }.into_any(),
                    _ => view! {}.into_any(),
                }}
            </section>
        </main>
    }
}

async fn connect_frame_websocket(
    set_status: WriteSignal<String>,
    set_n_frames: WriteSignal<usize>,
    set_frames: WriteSignal<VecDeque<Frame>>,
) {
    let ws_url = frame_ws_url();
    let Ok(mut ws) = WebSocket::open(&ws_url) else {
        set_status.set(format!("failed to open frame websocket {ws_url}"));
        return;
    };

    set_status.set("frame websocket connected".to_string());
    let mut total_frames = 0usize;
    let mut displayed = VecDeque::<Frame>::new();

    while let Some(msg) = ws.next().await {
        match msg {
            Ok(Message::Bytes(data)) => {
                total_frames += 1;
                displayed.push_front(Frame::new(data));
                while displayed.len() > DISPLAY_FRAME_LIMIT {
                    displayed.pop_back();
                }
                set_n_frames.set(total_frames);
                set_frames.set(displayed.clone());
                set_status.set(format!("running ({total_frames} decoded frames)"));
            }
            Ok(Message::Text(_)) => {}
            Err(e) => {
                set_status.set(format!("frame websocket error: {e:?}"));
                return;
            }
        }
    }

    set_status.set("frame websocket closed".to_string());
}

fn frame_ws_url() -> String {
    let location = window().location();
    let proto = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .hostname()
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let scheme = if proto == "https:" { "wss" } else { "ws" };
    format!("{scheme}://{host}:{FRAME_WS_PORT}")
}

#[derive(Clone, Debug)]
struct Frame {
    dst_addr: String,
    dst_pan: String,
    payload: String,
}

impl Frame {
    fn new(data: Vec<u8>) -> Self {
        const HEADER_LEN: usize = 7;
        const TRAILER_LEN: usize = 4;

        let dst_pan = get_u16(&data, 3)
            .map(|v| format!("{v:#06x}"))
            .unwrap_or_else(|| "n/a".to_string());
        let dst_addr = get_u16(&data, 5)
            .map(|v| format!("{v:#06x}"))
            .unwrap_or_else(|| "n/a".to_string());
        let payload = if data.len() >= HEADER_LEN + TRAILER_LEN {
            String::from_utf8_lossy(&data[HEADER_LEN..data.len() - TRAILER_LEN]).to_string()
        } else {
            String::new()
        };

        Self {
            dst_addr,
            dst_pan,
            payload,
        }
    }
}

fn get_u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
}

pub fn frontend() {
    console_error_panic_hook::set_once();
    Executor::init_wasm_bindgen().unwrap();
    mount_to_body(|| view! { <Gui /> })
}

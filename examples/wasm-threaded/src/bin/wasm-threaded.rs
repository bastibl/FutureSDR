#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
fn main() {
    app::main();
}

#[cfg(target_arch = "wasm32")]
mod app {
    use any_spawner::Executor;
    use futuresdr::blocks::Apply;
    use futuresdr::blocks::MessagePipe;
    use futuresdr::blocks::NullSink;
    use futuresdr::blocks::wasm::HackRf;
    use futuresdr::prelude::*;
    use futuresdr::runtime::scheduler::WasmScheduler;
    use leptos::prelude::*;
    use leptos::task::spawn_local;
    use std::collections::VecDeque;
    use wasm_bindgen_futures::JsFuture;
    use zigbee::ClockRecoveryMm;
    use zigbee::Decoder;
    use zigbee::Mac;

    const WORKER_SCRIPT: &str = "./futuresdr-wasm-scheduler-worker.js";

    pub fn main() {
        console_error_panic_hook::set_once();
        futuresdr::runtime::init();
        Executor::init_wasm_bindgen().unwrap();
        mount_to_body(|| view! { <App /> });
    }

    #[component]
    fn App() -> impl IntoView {
        let (running, set_running) = signal(false);
        let (status, set_status) = signal(String::from("idle"));
        let (frames, set_frames) = signal(VecDeque::<Frame>::new());

        let start = move |_| {
            if running.get_untracked() {
                return;
            }
            set_running.set(true);
            set_status.set("requesting HackRF".to_string());
            spawn_local(async move {
                if let Err(e) = run_receiver(set_status, set_frames).await {
                    set_status.set(format!("error: {e:?}"));
                    set_running.set(false);
                }
            });
        };

        view! {
            <main style="font-family: sans-serif; max-width: 900px; margin: 2rem auto;">
                <h1>"FutureSDR threaded WASM ZigBee RX"</h1>
                <p>"HackRF runs locally; DSP/MAC blocks are scheduled on four web workers."</p>
                <button on:click=start disabled=move || running()>
                    {move || if running() { "running" } else { "start" }}
                </button>
                <p><b>"status: "</b>{move || status()}</p>
                <p><b>"frames: "</b>{move || frames().len()}</p>
                <ul>
                    {move || frames().into_iter().map(|f| view! { <li><code>{f.render()}</code></li> }).collect_view()}
                </ul>
            </main>
        }
    }

    async fn run_receiver(
        set_status: WriteSignal<String>,
        set_frames: WriteSignal<VecDeque<Frame>>,
    ) -> anyhow::Result<()> {
        request_hackrf().await?;
        set_status.set("starting flowgraph".to_string());

        let mut fg = Flowgraph::new();

        let src = fg.add_local(HackRf::new());
        let source = src.id();

        let mut last = Complex32::new(0.0, 0.0);
        let mut iir = 0.0f32;
        let alpha = 0.00016;
        let avg = Apply::new(move |i: &Complex32| -> f32 {
            let phase = (last.conj() * i).arg();
            last = *i;
            iir = (1.0 - alpha) * iir + alpha * phase;
            phase - iir
        });

        let mm: ClockRecoveryMm = ClockRecoveryMm::new(2.0, 0.000225, 0.5, 0.03, 0.0002);
        let decoder: Decoder = Decoder::new(6);
        let mac: Mac = Mac::new();
        let snk = NullSink::<u8>::new();

        let (tx_frame, rx_frame) = mpsc::channel::<Pmt>(100);
        let message_pipe = MessagePipe::new(tx_frame);

        connect!(fg,
            src > avg > mm > decoder;
            mac > snk;
            decoder | rx.mac;
            mac.rxed | message_pipe
        );

        let rt = Runtime::with_scheduler(WasmScheduler::with_worker_script(4, WORKER_SCRIPT));
        let running = rt.start_async(fg).await?;
        let (task, flowgraph) = running.split();

        flowgraph
            .post(source, "freq", Pmt::U64(2_480_000_000))
            .await?;
        set_status.set("running".to_string());

        spawn_local(async move {
            if let Err(e) = task.await {
                futuresdr::tracing::info!("flowgraph terminated with error: {:?}", e);
            }
        });

        while let Some(msg) = rx_frame.recv().await {
            if let Pmt::Blob(data) = msg {
                set_frames.update(|frames| {
                    frames.push_front(Frame::new(data));
                    if frames.len() > 20 {
                        frames.pop_back();
                    }
                });
            }
        }

        Ok(())
    }

    async fn request_hackrf() -> anyhow::Result<()> {
        let window = web_sys::window().expect("No global `window` exists");
        let usb = window.navigator().usb();

        let filter = web_sys::UsbDeviceFilter::new();
        filter.set_vendor_id(7504);
        let filters = [filter];
        let options = web_sys::UsbDeviceRequestOptions::new(&filters);

        let devices: js_sys::Array<web_sys::UsbDevice> = JsFuture::from(usb.get_devices())
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
        dst_addr: String,
        dst_pan: String,
        payload: String,
    }

    impl Frame {
        fn render(&self) -> String {
            format!(
                "dst_pan={} dst_addr={} payload={}",
                self.dst_pan, self.dst_addr, self.payload
            )
        }

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

            Frame {
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
}

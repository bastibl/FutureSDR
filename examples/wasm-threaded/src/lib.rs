#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start_app() {
    app::main();
}

#[cfg(target_arch = "wasm32")]
mod app {
    use any_spawner::Executor;
    use futuresdr::blocks::Apply;
    use futuresdr::blocks::NullSink;
    use futuresdr::blocks::wasm::HackRf;
    use futuresdr::prelude::*;
    use futuresdr::runtime::dev::BlockMeta;
    use futuresdr::runtime::dev::MessageOutputs;
    use futuresdr::runtime::dev::WorkIo;
    use futuresdr::runtime::macros::Block;
    use futuresdr::runtime::scheduler::WasmScheduler;
    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use leptos::task::spawn_local;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use wasm_bindgen_futures::JsFuture;
    use zigbee::ClockRecoveryMm;
    use zigbee::Decoder;
    use zigbee::Mac;

    const WORKER_SCRIPT: &str = "./futuresdr-wasm-scheduler-worker.js";
    const FRAME_QUEUE_LIMIT: usize = 100;

    type FrameQueue = Arc<Mutex<VecDeque<Vec<u8>>>>;

    #[derive(Block)]
    #[message_inputs(r#in)]
    #[null_kernel]
    struct FramePipe {
        queue: FrameQueue,
    }

    impl FramePipe {
        fn new(queue: FrameQueue) -> Self {
            Self { queue }
        }

        async fn r#in(
            &mut self,
            _io: &mut WorkIo,
            _mo: &mut MessageOutputs,
            _meta: &mut BlockMeta,
            p: Pmt,
        ) -> Result<Pmt> {
            if let Pmt::Blob(data) = p {
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|_| anyhow::anyhow!("frame queue lock poisoned"))?;
                queue.push_back(data);
                while queue.len() > FRAME_QUEUE_LIMIT {
                    queue.pop_front();
                }
                Ok(Pmt::Ok)
            } else {
                Ok(Pmt::InvalidValue)
            }
        }
    }

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
                <p>"HackRF runs in a local-domain worker; DSP/MAC blocks are scheduled on two web workers."</p>
                <button on:click=start disabled=running>
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

        let frame_queue = FrameQueue::default();
        let frame_queue_writer = frame_queue.clone();
        let rt = Runtime::with_scheduler(WasmScheduler::with_worker_script(2, WORKER_SCRIPT));
        let handle = rt.handle();

        // Create the local-domain worker from the browser thread. The remaining
        // flowgraph construction runs on a scheduler worker, where synchronous
        // local-domain port wiring can use Atomics.wait without freezing the UI.
        let mut fg = Flowgraph::new();
        let local = fg.local_domain();
        let src = fg.add_local(local, HackRf::new);

        let start = rt.spawn(async move {
            let result = async move {
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
                let frame_pipe = FramePipe::new(frame_queue_writer);

                connect!(fg,
                    src > avg > mm > decoder;
                    mac > snk;
                    decoder | rx.mac;
                    mac.rxed | frame_pipe
                );

                let running = handle.start(fg).await?;
                let (task, _flowgraph) = running.split();
                task.await?;
                Ok::<_, futuresdr::runtime::Error>(())
            }
            .await;

            if let Err(e) = result {
                futuresdr::tracing::error!("wasm-threaded flowgraph failed: {:?}", e);
            }
        });
        start.detach();
        set_status.set("running".to_string());

        loop {
            let mut pending = Vec::new();
            {
                let mut queue = frame_queue
                    .lock()
                    .map_err(|_| anyhow::anyhow!("frame queue lock poisoned"))?;
                while let Some(data) = queue.pop_front() {
                    pending.push(data);
                }
            }

            if pending.is_empty() {
                TimeoutFuture::new(10).await;
                continue;
            }

            for data in pending {
                set_frames.update(|frames| {
                    frames.push_front(Frame::new(data));
                    if frames.len() > 20 {
                        frames.pop_back();
                    }
                });
            }
        }
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

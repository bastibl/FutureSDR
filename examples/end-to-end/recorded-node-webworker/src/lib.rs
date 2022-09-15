#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_docs_in_private_items)]

//! The node implementation for the end-to-end example.

use byteorder::ReadBytesExt;
use control_worker::ControlWorker;
use fft_worker::FftWorker;
use gloo_worker::Spawnable;
use gloo_worker::WorkerBridge;
use shared_utils::{
    DataTypeMarker, FromControlWorkerMsg, NodeConfig, ToControlWorkerMsg, ToProcessorWorker,
};
use std::cell::RefCell;
use std::io::Cursor;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// Recorded SDR data
static RECORDED_I8: &[u8] = include_bytes!("../../record.raw");

/// Struct to use recorded data in JS.
#[wasm_bindgen]
pub struct Data {
    /// Cursor to recorded data.
    cursor: Cursor<&'static [u8]>,
}

#[wasm_bindgen]
impl Data {
    /// Create new Data struct.
    pub fn new() -> Self {
        Data::default()
    }

    /// Read n values.
    pub fn read_n(&mut self, n: usize) -> Vec<i8> {
        let mut data_vector = Vec::with_capacity(n);
        for _ in 0..n {
            if self.cursor.position() >= (RECORDED_I8.len() - 1) as u64 {
                self.cursor.set_position(0);
            }
            data_vector.push(self.cursor.read_i8().unwrap());
        }
        data_vector
    }
}

impl Default for Data {
    fn default() -> Self {
        Data {
            cursor: Cursor::new(RECORDED_I8),
        }
    }
}

thread_local! {
    static CONTROL_WORKER: RefCell<Option<WorkerBridge<ControlWorker>>> = RefCell::new(None);
    static FFT_WORKER: RefCell<Option<WorkerBridge<FftWorker>>> = RefCell::new(None);
    //static ZIG_BEE_WORKER: RefCell<Option<WorkerBridge<ZigBeeWorker>>> = RefCell::new(None);

    // Add globals for more workers here
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen]
    fn print_from_rust(s: &str);
    #[wasm_bindgen]
    fn set_freq_from_rust(freq: u64);
    #[wasm_bindgen]
    fn set_amp_from_rust(amp: u8);
    #[wasm_bindgen]
    fn set_lna_from_rust(lna: u8);
    #[wasm_bindgen]
    fn set_vga_from_rust(vga: u8);
    #[wasm_bindgen]
    fn set_sample_rate_from_rust(sample_rate: u64);
}

/// Takes the input samples and sends them to every active processor worker.
#[wasm_bindgen]
pub fn input_samples(samples: Vec<i8>) {
    FFT_WORKER.with(|bridge| {
        if let Some(bridge) = bridge.borrow_mut().as_ref() {
            bridge.send(ToProcessorWorker::Data {
                data: samples.clone(),
            });
        }
    });
    // Add data input for more workers here
}

/// Stops all processor workers.
pub fn stop_processor_workers() {
    // Replacing the FFT_WORKER content drops the WorkerBridge with terminates the worker.
    FFT_WORKER.with(|inner| {
        inner.replace(None);
    });
    // Add stop code for more workers here
}

/// Stops the control worker.
pub fn stop_control_worker() {
    CONTROL_WORKER.with(|inner| {
        inner.replace(None);
    });
}

/// Starts processor workers according to `node_config`
pub fn start_processor_workers(node_config: &NodeConfig) {
    for entry in node_config.data_types.keys() {
        match entry {
            DataTypeMarker::Fft => {
                FFT_WORKER.with(|inner| {
                    inner.replace(Some(FftWorker::spawner().spawn("fft_worker.js")));
                    if let Some(bridge) = &*inner.borrow_mut() {
                        bridge.send(ToProcessorWorker::ApplyConfig {
                            config: node_config.clone(),
                        });
                    }
                });
            }
            DataTypeMarker::ZigBee => {}
        }
    }
    // Add start code for more workers here
}

/// Applies the `node_config` to the node.
/// Sets SDR config.
/// Starts appropriate processor workers and configures them.
pub fn apply_config(node_config: NodeConfig) {
    stop_processor_workers();

    set_freq_from_rust(node_config.sdr_config.freq);
    set_amp_from_rust(node_config.sdr_config.amp);
    set_lna_from_rust(node_config.sdr_config.lna);
    set_vga_from_rust(node_config.sdr_config.vga);
    set_sample_rate_from_rust(node_config.sdr_config.sample_rate);

    start_processor_workers(&node_config);
}

/// Restarts the control worker and initializes it.
pub fn restart_control_worker() {
    create_control_worker();
    CONTROL_WORKER.with(|inner| {
        if let Some(bridge) = &*inner.borrow_mut() {
            bridge.send(ToControlWorkerMsg::GetInitialConfig);
        }
    });
    initialize_config();
}

/// Creates the control worker.
#[wasm_bindgen]
pub fn create_control_worker() {
    let bridge = ControlWorker::spawner()
        .callback(move |m| match m {
            FromControlWorkerMsg::ReceivedConfig { config } => {
                print_from_rust(&format!("Applying new config: {config:?}"));
                apply_config(config.clone());

                CONTROL_WORKER.with(|inner| {
                    let bridge_option = inner.borrow();
                    let bridge = bridge_option.as_ref().unwrap();
                    bridge.send(ToControlWorkerMsg::AckConfig { config });
                })
            }
            FromControlWorkerMsg::PrintToScreen { msg } => {
                print_from_rust(&msg);
            }
            FromControlWorkerMsg::Terminate { msg } => {
                print_from_rust(&msg);
                stop_processor_workers();
                stop_control_worker();
            }
            FromControlWorkerMsg::Disconnected => {
                stop_control_worker();
                stop_processor_workers();
                print_from_rust(
                    "No connection to backend, restarting workers after 10 seconds to reconnect",
                );
                spawn_local(async {
                    gloo_timers::future::TimeoutFuture::new(10_000).await;
                    restart_control_worker();
                });
            }
        })
        .spawn("control_worker.js");

    CONTROL_WORKER.with(|inner| {
        inner.replace(Some(bridge));
    });
}

/// Initializes the control worker.
#[wasm_bindgen]
pub fn initialize_config() {
    CONTROL_WORKER.with(|inner| {
        if let Some(bridge) = &*inner.borrow_mut() {
            bridge.send(ToControlWorkerMsg::Initialize);
            bridge.send(ToControlWorkerMsg::GetInitialConfig);
        }
    });
}

/// Generate a new UUID and return it in String representation.
#[wasm_bindgen]
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Set the console error panic hook.
#[wasm_bindgen]
pub fn set_console_error_panic_hook() {
    console_error_panic_hook::set_once();
}

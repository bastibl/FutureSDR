#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_docs_in_private_items)]

//! The fft processor web worker component of the end-to-end example.
#[allow(clippy::missing_docs_in_private_items)]
pub mod fft_shift;
#[allow(clippy::missing_docs_in_private_items)]
pub mod keep_1_in_n;
#[allow(clippy::missing_docs_in_private_items)]
pub mod wasm;

use gloo_worker::{HandlerId, Worker, WorkerScope};
use shared_utils::{DataTypeConfig, ToProcessorWorker};
use wasm_bindgen_futures::spawn_local;

#[cfg(feature = "include_main")]
use gloo_worker::Registrable;
use shared_utils::DataTypeMarker::Fft;

#[cfg(feature = "include_main")]
use wasm_bindgen::prelude::*;

/// The fft worker implementation.
pub struct FftWorker {
    /// Whether the worker has been initialized or not.
    is_initialized: bool,
}

impl Worker for FftWorker {
    type Message = ();

    type Input = ToProcessorWorker;

    type Output = ();

    fn create(_scope: &WorkerScope<Self>) -> Self {
        Self {
            is_initialized: false,
        }
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {}

    fn received(&mut self, _scope: &WorkerScope<Self>, msg: Self::Input, _who: HandlerId) {
        match msg {
            ToProcessorWorker::Data { data } => {
                if self.is_initialized {
                    spawn_local(async move {
                        futuresdr::blocks::wasm_sdr::push_samples(data).await;
                    });
                }
            }
            ToProcessorWorker::ApplyConfig { config } => {
                if let Some(DataTypeConfig::Fft {
                    fft_chunks_per_ws_transfer,
                }) = config.data_types.get(&Fft)
                {
                    let ws_url = format!(
                        "ws://127.0.0.1:3000/node/api/data/fft/{}/{}/{}/{}/{}",
                        config.sdr_config.freq,
                        config.sdr_config.amp,
                        config.sdr_config.lna,
                        config.sdr_config.vga,
                        config.sdr_config.sample_rate
                    );
                    let fft_chunks_per_ws_transfer = *fft_chunks_per_ws_transfer;
                    spawn_local(async move {
                        wasm::run(ws_url, fft_chunks_per_ws_transfer)
                            .await
                            .expect("Failed to run flowgraph");
                    });

                    self.is_initialized = true;
                }
            }
        }
    }
}

#[cfg(feature = "include_main")]
#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();

    FftWorker::registrar().register();
}

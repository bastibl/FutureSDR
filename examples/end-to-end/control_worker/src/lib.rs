#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_docs_in_private_items)]

//! The control worker component of the end-to-end example.

/// Handles the WebSocket receiving and sending tasks.
mod ws_tasks;

use futures::{SinkExt, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_worker::{HandlerId, Worker, WorkerScope};
use shared_utils::{FromControlWorkerMsg, NodeToBackend, ToControlWorkerMsg};
use wasm_bindgen_futures::spawn_local;

#[cfg(feature = "include_main")]
use gloo_worker::Registrable;
#[cfg(feature = "include_main")]
use wasm_bindgen::prelude::*;

/// Used to communicate from the worker to ws sender task.
#[derive(Debug)]
pub enum ToWsSenderTask {
    /// Request config from backend.
    RequestConfig,
    /// Acknowledge config to backend.
    AckConfig,
}

/// The control worker implementation.
pub struct ControlWorker {
    /// Channel to send messages to the WebSocket task to send to the backend.
    ws_sender: futures::channel::mpsc::Sender<NodeToBackend>,
    /// Channel to receive incoming messages from the backend from the WebSocket Task.
    ws_receiver: Option<futures::channel::mpsc::Receiver<NodeToBackend>>,
}

impl Worker for ControlWorker {
    type Message = ();

    type Input = ToControlWorkerMsg;

    type Output = FromControlWorkerMsg;

    fn create(_scope: &WorkerScope<Self>) -> Self {
        let (ws_sender, ws_receiver) = futures::channel::mpsc::channel(5);
        Self {
            ws_sender,
            ws_receiver: Some(ws_receiver),
        }
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {}

    fn received(&mut self, scope: &WorkerScope<Self>, msg: Self::Input, who: HandlerId) {
        match msg {
            ToControlWorkerMsg::Initialize => {
                let scope_clone = scope.clone();
                let scope_clone2 = scope.clone();
                match WebSocket::open("ws://127.0.0.1:3000/node/api/control") {
                    Ok(ws) => {
                        let (write, read) = ws.split();

                        spawn_local(
                            async move { ws_tasks::run_receiver(read, scope_clone, who).await },
                        );
                        let receiver = self.ws_receiver.take().unwrap();
                        spawn_local(async move {
                            ws_tasks::run_sender(write, receiver, scope_clone2, who).await
                        });
                    }
                    Err(e) => scope.respond(
                        who,
                        FromControlWorkerMsg::Terminate {
                            msg: format!("Failed to open WebSocket connection: {e}"),
                        },
                    ),
                }
            }
            ToControlWorkerMsg::AckConfig { config } => {
                let mut sender_clone = self.ws_sender.clone();
                let scope_clone = scope.clone();
                spawn_local(async move {
                    if let Err(e) = sender_clone.send(NodeToBackend::AckConfig { config }).await {
                        scope_clone
                            .respond(who, FromControlWorkerMsg::Terminate { msg: e.to_string() })
                    }
                });
            }
            ToControlWorkerMsg::GetInitialConfig => {
                let mut sender_clone = self.ws_sender.clone();
                let scope_clone = scope.clone();
                spawn_local(async move {
                    if let Err(e) = sender_clone.send(NodeToBackend::RequestConfig).await {
                        scope_clone
                            .respond(who, FromControlWorkerMsg::Terminate { msg: e.to_string() })
                    }
                });
            }
        }
    }
}

#[cfg(feature = "include_main")]
#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();

    ControlWorker::registrar().register();
}

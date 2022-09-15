use crate::application::{NodeId, State};
use crate::frontend_api;
use axum::extract::ws::WebSocket;
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use shared_utils::DataTypeMarker;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error};

/// Handles requests for streaming data from the frontend.
/// If no node with `node_id` is currently in the `state`, a HTTP 400 status code is returned.
pub async fn frontend_streaming_data_ws_handler(
    state: Arc<State>,
    node_id: NodeId,
    data_type: DataTypeMarker,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    debug!("frontend connected");

    let receiver = {
        let nodes_lock = state.nodes.lock().await;
        if let Some(node) = nodes_lock.get(&node_id) {
            if let Some(sender) = node.data_streams.get(&data_type) {
                sender.subscribe()
            } else {
                return StatusCode::NOT_FOUND.into_response();
            }
        } else {
            return StatusCode::NOT_FOUND.into_response();
        }
    };

    ws.on_upgrade(move |socket| frontend_streaming_data_ws_loop(socket, receiver, data_type))
}

/// Relays node data to the interested frontends. The method depends on the `data_type` requested.
pub async fn frontend_streaming_data_ws_loop(
    mut socket: WebSocket,
    mut receiver: broadcast::Receiver<Arc<Vec<u8>>>,
    data_type: DataTypeMarker,
) {
    match data_type {
        DataTypeMarker::Fft => {
            debug!("Starting Ftt data loop");
            // Data is sent in chunks since the frontend only accepts input of 2048 f32.
            loop {
                match receiver.recv().await {
                    Ok(data) => {
                        if let Err(e) = frontend_api::process_fft_data(data, &mut socket).await {
                            error!(%e);
                            return;
                        }
                    }
                    Err(e) => {
                        error!("Receive error in frontend fft data ws loop: {e}");
                        return;
                    }
                }
            }
        }
        DataTypeMarker::ZigBee => {
            unimplemented!()
        }
    }
}

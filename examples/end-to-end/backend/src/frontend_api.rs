/// Historical data frontend API methods.
pub(crate) mod historical;
/// Nodes data and config frontend API methods.
pub(crate) mod nodes;
/// Streaming data frontend API methods.
pub(crate) mod streaming;

use crate::application::{NodeId, State};
use crate::frontend_api::historical::{frontend_historical_data_ws_handler, TimestampQuery};
use crate::frontend_api::streaming::frontend_streaming_data_ws_handler;
use anyhow::bail;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::Extension;
use futures::SinkExt;
use shared_utils::DataTypeMarker;
use std::sync::Arc;

/// Redirects the data request to the historical handler if a timestamp was found, if not, the
/// streaming data handler is used.
pub async fn frontend_data_api_ws_handler(
    Extension(state): Extension<Arc<State>>,
    Path((node_id, data_type)): Path<(NodeId, DataTypeMarker)>,
    timestamp: Option<Query<TimestampQuery>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if let Some(timestamp) = timestamp {
        frontend_historical_data_ws_handler(state, node_id, data_type, timestamp.0, ws)
            .await
            .into_response()
    } else {
        frontend_streaming_data_ws_handler(state, node_id, data_type, ws)
            .await
            .into_response()
    }
}

/// Process fft data before sending it to the frontend. The frontend currently only accepts chunks
/// of 2048 `f32` values and panics if the data does not conform to that.
async fn process_fft_data(data: Arc<Vec<u8>>, socket: &mut WebSocket) -> anyhow::Result<()> {
    let chunks_vec: Vec<Result<Message, _>> = data
        // The frontend expects 2048 f32 at once per msg and one f32 is 4 bytes (4 * u8.len() = 32)
        .chunks(2048 * 4)
        // The data in the Arc is not modified as `to_vec` copies the data.
        // Data currently needs to be copied since tungstenite does not support
        // shared data: https://github.com/snapview/tungstenite-rs/issues/96
        .map(|v| Ok(Message::Binary(v.to_vec())))
        .collect();
    let mut chunks_stream = futures::stream::iter(chunks_vec);
    // Send_all returns a tungstenite error. We just close the WebSocket if an error is encountered.
    // If a more granular error handling is needed, use axum-tungstenite
    if let Err(e) = socket.send_all(&mut chunks_stream).await {
        let error_msg = format!("Frontend websocket encountered error: {e}");
        bail!(error_msg);
    };
    Ok(())
}

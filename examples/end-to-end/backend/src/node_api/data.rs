use crate::application::{NodeId, State};
use crate::node_api::{
    extract_node_id_cookie, get_last_seen_and_terminate_from_state, update_last_seen,
};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension};
use serde::{Deserialize, Serialize};
use shared_utils::DataTypeMarker;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_cookies::Cookies;
use tracing::{debug, error};

/// SDR parameters used to collect parameters for incoming node data from the request path.
/// Data types are i16 and i64 since Postgresql and with it SQLx with Postgresql driver do not
/// support unsigned values. i16 instead of i8 is used since Postgresql does not allow comparisons
/// between CHAR and an integer.
/// steps and ranges taken from https://hackrf.readthedocs.io/en/latest/faq.html#what-gain-controls-are-provided-by-hackrf
/// Sample rate and frequency taken from https://hackrf.readthedocs.io/en/latest/hackrf_one.html
#[derive(Debug, Deserialize, Serialize)]
pub struct IncomingDataParameters {
    /// Which data type is received.
    pub data_type: DataTypeMarker,
    /// The frequency.
    /// 1MHz to 6 GHz (1.000.000 - 6.000.000.000)
    pub freq: i64,
    /// Amplification on or off.
    /// on or off (0 or 1)
    pub amp: i16,
    /// LNA
    /// 0-40 in steps of 8
    pub lna: i16,
    /// VGA:
    /// 0-62 in steps of 2
    pub vga: i16,
    /// The sample rate
    /// 1 Msps to 20 Msps (million samples per second) (1000000 - 20000000)
    pub sample_rate: i64,
}

/// Handles a new data connection from a node. If no cookie with a node ID is found or no
/// control worker corresponds to the node ID, a HTTP 400 status code is returned.
pub async fn data_node_ws_handler(
    Extension(state): Extension<Arc<State>>,
    Path(node_sdr_parameters): Path<IncomingDataParameters>,
    cookies: Cookies,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    debug!("Processor worker connected");

    let node_id = match extract_node_id_cookie(cookies) {
        Ok(node_id) => node_id,
        Err(_) => return (StatusCode::BAD_REQUEST, "No node ID").into_response(),
    };

    // Check if the control worker for this node id has connected before the data stream.
    {
        let state_lock = state.nodes.lock().await;
        if !state_lock.contains_key(&node_id) {
            error!("Node without control worker connected tried to connect: {node_id}");
            return (
                StatusCode::BAD_REQUEST,
                "No control worker with same node ID",
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| data_node_ws_loop(socket, node_id, node_sdr_parameters, state))
}

/// Receives data from nodes. Received data is stored in the database and, if at least one frontend
/// is connected, send to all listing frontends via a broadcast channel.
///
/// # Panics
/// If the broadcast channel sender returns an error.
async fn data_node_ws_loop(
    mut socket: WebSocket,
    node_id: NodeId,
    node_sdr_parameters: IncomingDataParameters,
    state: Arc<State>,
) {
    let sender = {
        match process_data_node_connection(node_id, node_sdr_parameters.data_type, state.clone())
            .await
        {
            Ok(sender) => sender,
            Err(_) => return,
        }
    };

    // Counter to reduce the amount of output when debugging.
    #[cfg(feature = "incoming_data_counter")]
    let mut counter = 0;

    // Use an Arc to the last_seen value to prevent locking the whole node state structure
    // every time we receive node data.
    // We can't hold the last_seen lock the whole time since data from multiple connections of
    // one node can come in and the control connection also modifies last_seen.
    // `terminate_data` is used to clean up all data streams as removing the channels from the storage
    // does not terminate active channels. If only the control worker but not the processor worker
    // would be disconnected, the processor worker would continue sending data as the connection is intact
    // and all connected frontends and the database would keep receiving data.
    // This would lead to "ghost" data as no control_worker is associated with the node.
    let (last_seen_mutex, terminate_data) =
        match get_last_seen_and_terminate_from_state(state.clone(), node_id).await {
            Ok((last_seen_mutex, terminate_data)) => (last_seen_mutex, terminate_data),
            Err(_) => return,
        };
    debug!("Start receiving data from: {node_id}");
    while let Some(msg) = socket.recv().await {
        if *terminate_data.read().await {
            return;
        }
        if let Ok(msg) = msg {
            match msg {
                Message::Binary(data) => {
                    let timestamp = chrono::Utc::now();
                    update_last_seen(&last_seen_mutex, timestamp).await;

                    // `data_type as _` is described here: https://github.com/launchbadge/sqlx/issues/1004#issuecomment-764964043
                    let NodeId(node_id_inner) = node_id;
                    if let Err(e) =  sqlx::query!(
                            "INSERT INTO data_storage (node_id, data_type, freq, amp, lna, vga, sample_rate, timestamp, data) VALUES ($1, $2, $3, $4, $5 , $6, $7, $8, $9)", 
                            node_id_inner, node_sdr_parameters.data_type as _,
                                node_sdr_parameters.freq, node_sdr_parameters.amp,
                                node_sdr_parameters.lna, node_sdr_parameters.vga,
                                node_sdr_parameters.sample_rate, timestamp, data)
                            .execute(&state.db_pool)
                            .await {
                        error!("Failed to insert data into database: {e}");
                    }

                    let data = Arc::new(data);
                    if sender.receiver_count() >= 1 {
                        // According to tokio docs: "A send operation can only fail if there are no
                        // active receivers, implying that the message could never be received.
                        // The error contains the message being sent as a payload so it can be
                        // recovered."
                        sender
                            .send(data)
                            .expect("Broadcast channel send failed. This can't happen.");
                    }

                    #[cfg(feature = "incoming_data_counter")]
                    {
                        counter += 1;
                        if counter >= 100 {
                            debug!("Processor worker sent data: {}", node_id);
                            counter = 0;
                        }
                    }
                }
                Message::Close(_) => {
                    debug!("Processor worker client disconnected: {}", node_id);
                    return;
                }
                _ => {
                    error!("Processor worker behaved unexpectedly");
                }
            }
        } else {
            debug!("Processor worker disconnected unexpectedly: {}", node_id);
            return;
        }
    }
}

/// Processes a newly connected processor worker and returns a broadcast channel sender to which the
/// received data should be sent.
async fn process_data_node_connection(
    node_id: NodeId,
    data_type: DataTypeMarker,
    state: Arc<State>,
) -> anyhow::Result<broadcast::Sender<Arc<Vec<u8>>>> {
    Ok({
        let mut state_lock = state.nodes.lock().await;
        if let Entry::Occupied(mut entry) = state_lock.entry(node_id) {
            *entry.get_mut().last_seen.lock().await = chrono::Utc::now();

            // Only one active node per NodeId is assumed. Keeping the same sender allows to
            // resume data transfer over frontend web sockets without disruption.
            if let Some(sender) = entry.get().data_streams.get(&data_type) {
                sender.clone()
            } else {
                let (sender, _) = broadcast::channel(10);
                entry
                    .get_mut()
                    .data_streams
                    .insert(data_type, sender.clone());
                sender
            }
        } else {
            let error_msg = format!(
                "New processor worker connected without corresponding control worker: {node_id}"
            );
            error!(error_msg);
            anyhow::bail!(error_msg);
        }
    })
}

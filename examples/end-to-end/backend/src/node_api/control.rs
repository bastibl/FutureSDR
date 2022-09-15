use crate::application::{NodeControlConnection, NodeId, NodeState, State};
use crate::node_api::{
    extract_node_id_cookie, get_last_seen_and_terminate_from_state, update_last_seen,
};
use crate::DEFAULT_NODE_CONFIG;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::{response::IntoResponse, Extension};
use futures::{sink::SinkExt, stream::StreamExt};
use shared_utils::{BackendToNode, NodeConfig, NodeToBackend};
use sqlx::PgPool;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_cookies::Cookies;
use tracing::{debug, error};

/// Removes the node from the [State].
/// Sets the `terminate_data` variable for that node to true to terminate all data streams.
///
/// # Panics
/// If the node to be removed is not in the node state.
async fn node_cleanup(state: Arc<State>, node_id: NodeId) {
    debug!("Removing node: {node_id}");
    let mut state_lock = state.nodes.lock().await;
    // `terminate_data` is an Arc<RwLock<bool>>, so removing the node does not drop the variable
    // but only one reference to it.
    *state_lock
        .get_mut(&node_id)
        .expect("Node was not present in state")
        .terminate_data
        .write()
        .await = true;
    state_lock.remove(&node_id);
}

/// Handles a control node connecting. If no `node_id`is present or if the `node_id` is already
/// used by any other active node, a HTTP 400 status code is returned.
pub async fn control_node_ws_handler(
    Extension(state): Extension<Arc<State>>,
    cookies: Cookies,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    debug!("Control node connected");

    let node_id = match extract_node_id_cookie(cookies) {
        Ok(node_id) => node_id,
        Err(_) => return (StatusCode::BAD_REQUEST, "No node ID").into_response(),
    };

    // Check if the control worker for this node id is already present.
    {
        let state_lock = state.nodes.lock().await;
        if state_lock.contains_key(&node_id) {
            error!("Node with control worker already connected tried to connect: {node_id}");
            return (StatusCode::BAD_REQUEST, "Node ID already taken").into_response();
        }
    }

    ws.on_upgrade(move |socket| control_node_ws_loop(socket, node_id, state))
}

/// Handles the control node websocket loop. This means receiving messages and sending messages
/// from and to the control node.
///
/// # Panics
/// - If a message to the node cannot be serialized.
/// - If sending data over the `tokio::sync::mpsc::Sender<BackendToNode>` channel fails.
async fn control_node_ws_loop(socket: WebSocket, node_id: NodeId, state: Arc<State>) {
    let (to_node_sender, mut to_node_receiver) =
        match process_control_node_connection(node_id, state.clone()).await {
            Ok((to_node_sender, to_node_receiver)) => (to_node_sender, to_node_receiver),
            Err(_) => return,
        };

    let (mut ws_sender, mut ws_receiver) = socket.split();

    tokio::spawn(async move {
        while let Some(msg) = to_node_receiver.recv().await {
            let msg_serialized =
                bincode::serialize(&msg).expect("Failed to serialize BackendToNode msg");

            if let Err(e) = ws_sender.send(Message::Binary(msg_serialized)).await {
                error!("Failed to send data to control worker: {e}");
            }
        }
    });

    let (last_seen_mutex, _) =
        match get_last_seen_and_terminate_from_state(state.clone(), node_id).await {
            Ok((last_seen_mutex, terminate_data)) => (last_seen_mutex, terminate_data),
            Err(_) => return,
        };

    tokio::spawn(update_last_seen_task(state.clone(), node_id));

    while let Some(msg) = ws_receiver.next().await {
        if let Ok(msg) = msg {
            update_last_seen(&last_seen_mutex, chrono::Utc::now()).await;
            match msg {
                Message::Binary(v) => match bincode::deserialize::<NodeToBackend>(&v) {
                    Ok(NodeToBackend::AckConfig { config }) => {
                        debug!("Ack config: {node_id} - {config:?}");
                    }
                    Ok(NodeToBackend::RequestConfig) => {
                        debug!("Request config: {node_id}");

                        let config =
                            match retrieve_config_or_use_default(node_id, &state.db_pool).await {
                                Ok(config_msg) => config_msg,
                                Err(e) => {
                                    send_error_to_node(to_node_sender.clone(), e.to_string()).await;
                                    return;
                                }
                            };
                        to_node_sender
                            .send(BackendToNode::SendConfig { config })
                            .await
                            .expect("Failed to send data to to-node-sender task");
                    }
                    Err(e) => {
                        error!("Failed to deserialize control worker msg: {e}");
                    }
                },
                Message::Close(_) => {
                    debug!("Control worker disconnected");
                    node_cleanup(state, node_id).await;
                    return;
                }
                _ => {
                    error!("Control worker behaved unexpectedly");
                    node_cleanup(state, node_id).await;
                    return;
                }
            }
        } else {
            debug!("Control worker disconnected unexpectedly");
            node_cleanup(state, node_id).await;
            return;
        }
    }
}

/// Every second the `last_seen` member of the node is checked and the config_storage is updated if
/// it changed in that time. This tasks checks `terminate_data` every run to check whether it should
/// terminate too.
async fn update_last_seen_task(state: Arc<State>, node_id: NodeId) {
    let (last_seen_mutex, terminate_data) =
        match get_last_seen_and_terminate_from_state(state.clone(), node_id).await {
            Ok((last_seen_mutex, terminate_data)) => (last_seen_mutex, terminate_data),
            Err(_) => return,
        };
    let NodeId(node_id_inner) = node_id;
    let mut last_seen = *last_seen_mutex.lock().await;

    while !*terminate_data.read().await {
        let new_last_seen = *last_seen_mutex.lock().await;
        if last_seen != new_last_seen {
            if let Err(e) = sqlx::query!(
                "UPDATE config_storage SET last_seen = $1 WHERE node_id = $2",
                new_last_seen,
                node_id_inner
            )
            .execute(&state.db_pool)
            .await
            {
                error!("Failed to update last_seen in config_storage: {e}");
            }
        }
        last_seen = *last_seen_mutex.lock().await;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

/// Used the `to_node_sender` channel sender to send the `error_msg` to the node and terminates the
/// node.
///
/// # Panics
///
/// Panics if the `to_node_sender` returns an error.
async fn send_error_to_node(
    to_node_sender: tokio::sync::mpsc::Sender<BackendToNode>,
    error_msg: String,
) {
    error!(error_msg);
    to_node_sender
        .send(BackendToNode::Error {
            msg: error_msg,
            terminate: true,
        })
        .await
        .expect("Failed to send data to to-node-sender task");
}

/// Processes a newly connected control node into the `state` and returns two channels.
/// The `Sender<BackendToNode>` is to send messages to the control node from other parts of the application.
/// The `Receiver<BackendToNode>` is for the `control_node_ws_loop` to receive messages to be
/// sent to the node.
async fn process_control_node_connection(
    node_id: NodeId,
    state: Arc<State>,
) -> anyhow::Result<(
    tokio::sync::mpsc::Sender<BackendToNode>,
    tokio::sync::mpsc::Receiver<BackendToNode>,
)> {
    Ok({
        let mut state_lock = state.nodes.lock().await;

        if let Entry::Vacant(entry) = state_lock.entry(node_id) {
            let (to_node_sender, to_node_receiver) = tokio::sync::mpsc::channel(5);

            let control_connection = NodeControlConnection {
                to_node: to_node_sender.clone(),
            };

            let data_streams_hashmap = HashMap::new();

            entry.insert(NodeState {
                control_connection,
                data_streams: data_streams_hashmap,
                last_seen: Arc::new(Mutex::new(chrono::Utc::now())),
                terminate_data: Arc::new(RwLock::new(false)),
            });
            (to_node_sender, to_node_receiver)
        } else {
            let error_msg =
                format!("New control worker connected while old one still present: {node_id}");
            error!(error_msg);
            anyhow::bail!(error_msg)
        }
    })
}

/// Retrieves the config for the specified `node_id` from the database or returns the default config.
/// # Panics
/// If the default config cannot be serialized.
async fn retrieve_config_or_use_default(
    node_id: NodeId,
    db_pool: &PgPool,
) -> anyhow::Result<NodeConfig> {
    let NodeId(node_id_inner) = node_id;
    let db_result = sqlx::query!(
        "SELECT config_serialized FROM config_storage WHERE node_id = $1",
        node_id_inner
    )
    .fetch_one(db_pool)
    .await;
    let config_msg = match db_result {
        Ok(db_result) => {
            debug!("Successfully fetched config from database");

            match serde_json::from_str::<NodeConfig>(&db_result.config_serialized) {
                Ok(config) => {
                    debug!("Successfully deserialized config");
                    config
                }
                Err(e) => {
                    let error_msg = format!("Failed to deserialize config from database: {e}");
                    error!(error_msg);
                    anyhow::bail!(error_msg);
                }
            }
        }
        Err(sqlx::error::Error::RowNotFound) => {
            debug!("No config in database for this node, providing default config");

            let config_serialized =
                serde_json::to_string(&*DEFAULT_NODE_CONFIG).expect("Failed to serialize config");

            if let Err(e) = sqlx::query!(
                    "INSERT INTO config_storage (node_id, last_seen, config_serialized) VALUES ($1, $2, $3)",
                    node_id_inner, chrono::Utc::now(), config_serialized)
                .execute(db_pool)
                .await {
                let error_msg =
                    format!("Failed to insert config into database: {e}");
                error!(error_msg);
                anyhow::bail!(error_msg);
            }

            DEFAULT_NODE_CONFIG.clone()
        }
        Err(e) => {
            let error_msg = format!("Database error: {e}");
            error!(error_msg);
            anyhow::bail!(error_msg);
        }
    };
    Ok(config_msg)
}

use crate::application::{NodeId, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::Utc;
use futures::StreamExt;
use shared_utils::{BackendToNode, NodeConfigRequest, NodeMetaDataResponse};
use std::collections::hash_map::Entry;
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

/// A helper to retrieve node metadata from the database.
#[derive(Debug)]
struct NodeMetaData {
    /// Node ID
    node_id: Uuid,
    /// Last seen timestamp
    last_seen: chrono::DateTime<Utc>,
    /// The serialized config string
    config_serialized: String,
}

/// Retrieves node metadata from the database and returns it as JSON to the client.
/// If no data is found, an empty JSON response is given.
pub async fn frontend_nodes_metadata(
    Extension(state): Extension<Arc<State>>,
) -> Json<Vec<NodeMetaDataResponse>> {
    let mut stream = sqlx::query_as!(
        NodeMetaData,
        "
                                SELECT *
                                FROM config_storage
                                "
    )
    .fetch(&state.db_pool);
    let mut response: Vec<NodeMetaDataResponse> = Vec::new();

    while let Some(Ok(data)) = stream.next().await {
        response.push(NodeMetaDataResponse {
            node_id: data.node_id,
            last_seen: data.last_seen,
            config: serde_json::from_str(&data.config_serialized)
                .expect("Failed to deserialize config"),
        });
    }
    Json(response)
}

/// Takes a JSON representation of a `NodeConfigRequest` and applies it to the specified node.
///
/// # Panics
/// - If the ' to_node' sender of the 'NodeState' returns an error.
/// - If the supplied config cannot be serialized.
pub async fn frontend_nodes_set_config(
    Extension(state): Extension<Arc<State>>,
    Json(config): Json<NodeConfigRequest>,
) -> impl IntoResponse {
    if let Err(e) = config.config.sdr_config.check_plausibility() {
        error!("Bad config received: {e}");
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    let config_serialized =
        serde_json::to_string(&config.config).expect("Failed to serialize config");

    if let Err(e) = sqlx::query!(
        "UPDATE config_storage SET config_serialized = $1 WHERE node_id = $2",
        config_serialized,
        config.node_id
    )
    .execute(&state.db_pool)
    .await
    {
        error!("Failed to update config: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let node_id = NodeId(config.node_id);
    {
        let mut state_lock = state.nodes.lock().await;
        if let Entry::Occupied(x) = state_lock.entry(node_id) {
            x.get()
                .control_connection
                .to_node
                .send(BackendToNode::SendConfig {
                    config: config.config,
                })
                .await
                .expect("Sending failed");
        }
    }
    StatusCode::OK.into_response()
}

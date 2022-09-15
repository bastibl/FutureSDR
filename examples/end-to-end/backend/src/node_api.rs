/// Control worker API methods.
pub(crate) mod control;
/// Processor worker API methods.
pub(crate) mod data;

use crate::application::{NodeId, State};
use anyhow::bail;
use chrono::{DateTime, Utc};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_cookies::Cookies;
use tracing::error;

/// Extract the cookie out of the `tower_cookies::Cookies` struct.
fn extract_node_id_cookie(cookies: Cookies) -> anyhow::Result<NodeId> {
    if let Some(node_id_cookie) = cookies.get("node_id") {
        let node_id_uuid = uuid::Uuid::from_str(node_id_cookie.value())?;
        Ok(NodeId(node_id_uuid))
    } else {
        error!("No cookie \"node_id\"");
        bail!("No cookie \"node_id\"");
    }
}

/// Get the `last_seen` and `terminate_data` variables from the state for the specified `node_id`.
async fn get_last_seen_and_terminate_from_state(
    state: Arc<State>,
    node_id: NodeId,
) -> anyhow::Result<(Arc<Mutex<DateTime<Utc>>>, Arc<RwLock<bool>>)> {
    let mut state_lock = state.nodes.lock().await;
    if let Some(node) = state_lock.get_mut(&node_id) {
        Ok((node.last_seen.clone(), node.terminate_data.clone()))
    } else {
        let error_msg = "Node vanished from HashMap while in use";
        error!(error_msg);
        bail!("Node vanished from HashMap while in use");
    }
}

/// Update the `last_seen` value behind the `last_seen_mutex` to the current time.
async fn update_last_seen(last_seen_mutex: &Arc<Mutex<DateTime<Utc>>>, timestamp: DateTime<Utc>) {
    {
        let mut last_seen_lock = last_seen_mutex.lock().await;
        *last_seen_lock = timestamp;
    }
}

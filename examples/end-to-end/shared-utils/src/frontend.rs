#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_docs_in_private_items)]

//! Shared utilities used by multiple components of the end-to-end example.

use crate::NodeConfig;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Used by the backend to supply node metadata to the frontend.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeMetaDataResponse {
    /// Node ID.
    pub node_id: Uuid,
    /// Last seen timestamp.
    pub last_seen: chrono::DateTime<Utc>,
    /// The current config.
    pub config: NodeConfig,
}

/// Used by the frontend to send and by the backend to receive a new config for the `node_id`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeConfigRequest {
    /// The node ID.
    pub node_id: Uuid,
    /// The new config.
    pub config: NodeConfig,
}

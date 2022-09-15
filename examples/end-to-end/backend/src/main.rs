#![deny(clippy::missing_panics_doc)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_docs_in_private_items)]

//! The backend implementation for the FutureSDR end-to-end example.

/// The application code, the axum router and start methods.
pub mod application;
/// The frontend API methods.
pub mod frontend_api;
/// The node API methods.
pub mod node_api;

use lazy_static::lazy_static;
use shared_utils::{DataTypeConfig, DataTypeMarker, NodeConfig, SdrConfig};
use std::collections::HashMap;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Configure the backend server here
/// The connection string used to connect to the PostgreSQL database.
static PG_CONNECTION_STRING: &str = "postgres://postgres:password@localhost:5432/backend";
lazy_static! {
    /// The default configuration sent to a node without prior config in the database.
    static ref DEFAULT_NODE_CONFIG: NodeConfig = {
        let mut data_types = HashMap::new();
        let fft_config = DataTypeConfig::Fft{fft_chunks_per_ws_transfer: 20};
        let sdr_config = SdrConfig {
            freq: 1000000,
            amp: 1,
            lna: 0,
            vga: 0,
            sample_rate: 4000000,
        };
        data_types.insert(DataTypeMarker::Fft, fft_config);
        NodeConfig{
            sdr_config,
            data_types,
        }
    };
    /// Address to which the backend should bind
    static ref BIND_ADDR: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 3000));
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            // If no `RUST_LOG` env variable is set, we default to log level `trace` for the backend and tower_http crate.
            std::env::var("RUST_LOG").unwrap_or_else(|_| "backend=trace,tower_http=trace".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    application::start_up().await
}

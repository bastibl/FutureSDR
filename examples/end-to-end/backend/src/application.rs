use crate::frontend_api::frontend_data_api_ws_handler;
use crate::frontend_api::nodes::{frontend_nodes_metadata, frontend_nodes_set_config};
use crate::node_api::{control::control_node_ws_handler, data::data_node_ws_handler};
use crate::{BIND_ADDR, PG_CONNECTION_STRING};
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get_service;
use axum::{routing::get, routing::post, Extension, Router};
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use shared_utils::{BackendToNode, DataTypeMarker};
use sqlx::PgPool;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tower_cookies::CookieManagerLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::debug;
use tracing::error;

/// Contains the applications state:
/// - Database connection pool
/// - Node state
#[derive(Debug)]
pub struct State {
    /// PostgreSQL database pool
    pub db_pool: PgPool,
    /// All connected nodes with their state.
    pub nodes: Arc<Mutex<HashMap<NodeId, NodeState>>>,
}

/// New type for [uuid::Uuid].
#[derive(Debug, Deserialize, Serialize, Hash, Eq, PartialEq, Copy, Clone)]
pub struct NodeId(pub uuid::Uuid);

impl Display for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Type alias to make clippy happy (https://rust-lang.github.io/rust-clippy/master/index.html#type_complexity)
pub type DataStreamStorage = HashMap<DataTypeMarker, broadcast::Sender<Arc<Vec<u8>>>>;

/// Allows sending control messages to the node.
#[derive(Debug)]
pub struct NodeControlConnection {
    /// MPSC channel to send messages to the node.
    pub to_node: mpsc::Sender<BackendToNode>,
}

/// Contains all state needed to handle a single node.
#[derive(Debug)]
pub struct NodeState {
    /// Send messages to the control node.
    pub control_connection: NodeControlConnection,
    /// All data streams coming from this node.
    pub data_streams: DataStreamStorage,
    /// When the node was last seen.
    pub last_seen: Arc<Mutex<chrono::DateTime<Utc>>>,
    /// Indicator that all data streams should terminate.
    pub terminate_data: Arc<RwLock<bool>>,
}

/// NodeId and DataTypeMarker from the request path. Used to identify which node and what data stream
/// is requested.
#[derive(Debug, Deserialize, Serialize)]
struct FrontendPathParameter {
    /// The node ID.
    node_id: NodeId,
    /// A marker to specify the data type.
    mode: DataTypeMarker,
}

/// Handles file serve errors, logs the error and returns a HTTP 500 status code.
pub async fn handle_file_serve_error(err: std::io::Error) -> impl IntoResponse {
    error!(%err);
    (StatusCode::INTERNAL_SERVER_ERROR, "Failure to serve file")
}

/// Builds the router and applies all needed middleware layers.
/// Middleware will set CORS, COEP and COOP headers.
/// Creates file serving services and defines API surface.
async fn build_router() -> Router {
    let db_pool = PgPool::connect(PG_CONNECTION_STRING)
        .await
        .expect("Failed to create database connection");

    sqlx::migrate!("./migrations")
        .run(&db_pool)
        .await
        .expect("Failed to migrate database");

    let state = Arc::new(State {
        db_pool,
        nodes: Arc::new(Mutex::new(HashMap::new())),
    });
    let cors = CorsLayer::new().allow_origin(Any);
    let coep_header = SetResponseHeaderLayer::if_not_present(
        headers::HeaderName::from_str("cross-origin-embedder-policy").unwrap(),
        HeaderValue::from_static("require-corp"),
    );
    let coop_header = SetResponseHeaderLayer::if_not_present(
        headers::HeaderName::from_str("cross-origin-opener-policy").unwrap(),
        HeaderValue::from_static("same-origin"),
    );

    let file_server_service = ServeDir::new("serve");

    let fft_server_service = ServeDir::new("serve/frontend/fft");
    let zigbee_server_service = ServeDir::new("serve/frontend/zigbee");

    Router::new()
        // node data api
        .route(
            "/node/api/data/:data_type/:freq/:amp/:lna/:vga/:sample_rate",
            get(data_node_ws_handler),
        )
        // node control api
        .route("/node/api/control", get(control_node_ws_handler))
        // frontend api
        .route(
            "/frontend_api/data/:node_id/:data_type",
            get(frontend_data_api_ws_handler),
        )
        .route("/frontend_api/config", post(frontend_nodes_set_config))
        .route("/frontend_api/nodes", get(frontend_nodes_metadata))
        // Visualizers get served separately because the path parameters need to be removed
        // from the path before serving the files.
        .nest(
            "/frontend/fft/:node_id",
            get_service(fft_server_service).handle_error(handle_file_serve_error),
        )
        .nest(
            "/frontend/zigbee/:node_id",
            get_service(zigbee_server_service).handle_error(handle_file_serve_error),
        )
        // File serving
        .fallback(get_service(file_server_service).handle_error(handle_file_serve_error))
        .layer(Extension(state))
        .layer(cors)
        .layer(coep_header)
        .layer(coop_header)
        .layer(CookieManagerLayer::new())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
}

/// Starts the backend and awaits the server.
///
/// # Panics
/// This function panics if the server cannot bind to the `BIND_ADDR` constant.
pub async fn start_up() {
    let app = build_router().await;

    let addr = *BIND_ADDR;
    debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

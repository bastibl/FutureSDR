use crate::application::{NodeId, State};
use crate::frontend_api;
use axum::extract::ws::WebSocket;
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shared_utils::DataTypeMarker;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error};

/// A helper to extract historical data from a database query.
#[derive(Debug)]
struct HistoricalData {
    /// The data received from the database.
    pub data: Vec<u8>,
}

/// A helper to extract whether or not historical data exists from a database query.
#[derive(Debug)]
struct HistoricalDataExists {
    /// The boolean value received from the database.
    pub exists: Option<bool>,
}

/// A helper to extract the timestamp out of a HTTP GET request query.
#[derive(Debug, Serialize, Deserialize)]
pub struct TimestampQuery {
    /// The timestamp extracted from the query.
    pub timestamp: chrono::DateTime<Utc>,
}

/// Handles requests by the frontend for historical data.
/// Checks whether historical data is present and if so, starts the serving process.
pub async fn frontend_historical_data_ws_handler(
    state: Arc<State>,
    node_id: NodeId,
    data_type: DataTypeMarker,
    timestamp: TimestampQuery,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    debug!("historical connected");
    debug!(?timestamp);

    let NodeId(node_id_inner) = node_id;
    let receiver = match data_type {
        DataTypeMarker::Fft => {
            let result = sqlx::query_as!(
                HistoricalDataExists,
                "
                SELECT EXISTS (
                    SELECT data 
                    FROM data_storage 
                    WHERE node_id = $1 
                    AND data_type = $2
                    AND timestamp >= $3
                ) AS \"exists\"
            ",
                node_id_inner,
                data_type as _,
                timestamp.timestamp
            )
            .fetch_one(&state.db_pool)
            .await;
            match result {
                Ok(historical_data_exists) => {
                    // Check whether the query returned a value for `exists` and if so, whether it
                    // is `true`.
                    if historical_data_exists.exists.is_some()
                        && historical_data_exists.exists.unwrap()
                    {
                        let (sender, receiver) = mpsc::channel::<Arc<Vec<u8>>>(10);
                        tokio::spawn(async move {
                            let mut stream = sqlx::query_as!(
                                HistoricalData,
                                "
                                SELECT data 
                                FROM data_storage 
                                WHERE node_id = $1 
                                AND data_type = $2
                                AND timestamp >= $3
                                ",
                                node_id_inner,
                                data_type as _,
                                timestamp.timestamp
                            )
                            .fetch(&state.db_pool);

                            while let Some(Ok(data)) = stream.next().await {
                                match sender.send(Arc::new(data.data)).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        error! {%e}
                                        return;
                                    }
                                }
                            }
                        });
                        receiver
                    } else {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                }
                Err(sqlx::error::Error::RowNotFound) => {
                    return StatusCode::NOT_FOUND.into_response();
                }
                Err(e) => {
                    error!("Failed to check whether historical data exists: {e}");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        DataTypeMarker::ZigBee => {
            unimplemented!()
        }
    };

    ws.on_upgrade(move |socket| frontend_historical_data_ws_loop(socket, receiver, data_type))
}

/// Sends the historical data to the frontend. The method depends on the `data_type`.
///
/// # Notes:
/// We need to use an mpsc here since the database overwhelms `process_fft_data` with a broadcast
/// channel and we get a huge amount of "channel lagged by x" errors.
pub async fn frontend_historical_data_ws_loop(
    mut socket: WebSocket,
    mut receiver: mpsc::Receiver<Arc<Vec<u8>>>,
    data_type: DataTypeMarker,
) {
    match data_type {
        DataTypeMarker::Fft => {
            debug!("Starting Ftt data loop");
            // Data is sent in chunks since the fft visualizer only accepts input of 2048 f32.
            while let Some(data) = receiver.recv().await {
                if let Err(e) = frontend_api::process_fft_data(data, &mut socket).await {
                    error!(%e);
                    return;
                }
            }
        }
        DataTypeMarker::ZigBee => {
            unimplemented!()
        }
    }
}

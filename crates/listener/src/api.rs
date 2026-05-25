//! Read-only HTTP API over the in-memory pool store.
//!
//! Exposes the same reserve state the listener mirrors to Redis, for callers
//! that want a one-shot HTTP read instead of subscribing to `pool_updates`.

use crate::store::PoolStore;
use alloy::primitives::Address;
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub async fn start(host: &str, port: u16, store: PoolStore) -> Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/pools", get(get_pools))
        .route("/pools/:address", get(get_pool))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(store);

    let bind_addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Listener API listening on http://{bind_addr}");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(store): State<PoolStore>) -> impl IntoResponse {
    Json(json!({ "status": "ok", "pools": store.len() }))
}

/// All pool state currently tracked in memory.
async fn get_pools(State(store): State<PoolStore>) -> impl IntoResponse {
    Json(store.get_all())
}

/// A single pool's state by pair address (checksummed or lowercase).
async fn get_pool(
    State(store): State<PoolStore>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    let addr = match address.parse::<Address>() {
        Ok(a) => a,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid address: {address}") })),
            )
                .into_response();
        }
    };
    match store.get(&addr) {
        Some(state) => Json(state).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "pool not tracked" })),
        )
            .into_response(),
    }
}

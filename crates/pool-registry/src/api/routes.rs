use crate::db;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

// ── request / response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PoolsQuery {
    #[serde(default = "default_min_tvl")]
    pub min_tvl: f64,
    #[serde(default)]
    pub include_null: bool,
}

fn default_min_tvl() -> f64 {
    0.0
}

#[derive(Debug, Deserialize)]
pub struct TokensQuery {
    #[serde(default = "default_token_limit")]
    pub limit: i64,
}

fn default_token_limit() -> i64 {
    1000
}

#[derive(Debug, Deserialize)]
pub struct RegisterEntry {
    pub pool_address: String,
    pub protocol: String,
}

#[derive(Debug, Serialize)]
struct RegisterResponse {
    registered: u64,
}

// ── handlers ──────────────────────────────────────────────────────────────────

async fn health(State(db): State<PgPool>) -> impl IntoResponse {
    match db::count_pools(&db).await {
        Ok((total, with_tvl)) => Json(json!({
            "status": "ok",
            "total_pools": total,
            "pools_with_tvl": with_tvl,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_pools(
    State(db): State<PgPool>,
    Query(params): Query<PoolsQuery>,
) -> impl IntoResponse {
    match db::get_pools_by_min_tvl(&db, params.min_tvl, params.include_null).await {
        Ok(pools) => Json(pools).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn register_pools(
    State(db): State<PgPool>,
    Json(entries): Json<Vec<RegisterEntry>>,
) -> impl IntoResponse {
    let pairs: Vec<(String, String)> = entries
        .into_iter()
        .map(|e| (e.pool_address, e.protocol))
        .collect();

    let mut total_registered = 0u64;

    for chunk in pairs.chunks(500) {
        match db::upsert_pools(&db, chunk).await {
            Ok(n) => {
                total_registered += n;
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
    }

    tracing::info!(registered = total_registered, "Pools registered");
    Json(RegisterResponse {
        registered: total_registered,
    })
    .into_response()
}

async fn get_stats(State(db): State<PgPool>) -> impl IntoResponse {
    let pools = db::count_pools(&db).await;
    let tokens = db::count_token_metadata(&db).await;
    match (pools, tokens) {
        (Ok((total, with_tvl)), Ok((tokens_referenced, tokens_resolved))) => Json(json!({
            "pools": {
                "total": total,
                "with_tvl": with_tvl,
                "without_tvl": total - with_tvl,
            },
            "token_metadata": {
                "referenced": tokens_referenced,
                "resolved": tokens_resolved,
                "pending": tokens_referenced - tokens_resolved,
            },
        }))
        .into_response(),
        (Err(e), _) | (_, Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_tokens(
    State(db): State<PgPool>,
    Query(params): Query<TokensQuery>,
) -> impl IntoResponse {
    let limit = params.limit.clamp(1, 10_000);
    match db::list_token_metadata(&db, limit).await {
        Ok(tokens) => Json(tokens).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── router ────────────────────────────────────────────────────────────────────

pub fn router() -> Router<PgPool> {
    Router::new()
        .route("/health", get(health))
        .route("/stats", get(get_stats))
        .route("/pools", get(get_pools))
        .route("/pools/register", post(register_pools))
        .route("/tokens", get(get_tokens))
}

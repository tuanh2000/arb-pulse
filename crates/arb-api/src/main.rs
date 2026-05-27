mod db;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
struct AppState {
    db: sqlx::PgPool,
}

#[derive(Deserialize)]
struct TokensQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Serialize)]
struct TokensResponse {
    tokens: Vec<db::TokenRecord>,
    total: i64,
    limit: i64,
    offset: i64,
}

async fn list_tokens(
    State(state): State<AppState>,
    Query(params): Query<TokensQuery>,
) -> Result<Json<TokensResponse>, StatusCode> {
    let limit = params.limit.clamp(1, 500);
    let offset = params.offset.max(0);

    let (tokens, total) = db::list_tokens(
        &state.db,
        db::TokenQuery { search: &params.q, limit, offset },
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list_tokens failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(TokensResponse { tokens, total, limit, offset }))
}

async fn get_token(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<db::TokenRecord>, StatusCode> {
    match db::get_token(&state.db, &address).await {
        Ok(Some(token)) => Ok(Json(token)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(error = %e, address = %address, "get_token failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arb_api=info,sqlx=warn".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL env var is required"))?;
    let host = std::env::var("ARB_API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("ARB_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4000);
    let cors_origin = std::env::var("ARB_API_CORS_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3100".to_string());

    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&database_url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to PostgreSQL: {}", e))?;
    tracing::info!("Connected to PostgreSQL");

    let cors_allow_origin: AllowOrigin = match cors_origin.parse::<axum::http::HeaderValue>() {
        Ok(origin) => AllowOrigin::exact(origin),
        Err(_) => AllowOrigin::any(),
    };
    let cors = CorsLayer::new()
        .allow_origin(cors_allow_origin)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = AppState { db };
    let app = Router::new()
        .route("/api/tokens", get(list_tokens))
        .route("/api/tokens/:address", get(get_token))
        .route("/health", get(health))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!(%addr, "arb-api listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

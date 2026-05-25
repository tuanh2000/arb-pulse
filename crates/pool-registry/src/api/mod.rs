pub mod routes;

use anyhow::Result;
use axum::Router;
use sqlx::PgPool;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub async fn start(host: &str, port: u16, db: PgPool) -> Result<()> {
    let app = Router::new()
        .merge(routes::router())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(db);

    let bind_addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Pool-registry API listening on http://{}", bind_addr);

    axum::serve(listener, app).await?;
    Ok(())
}

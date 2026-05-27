//! pool-registry-price: tiered on-chain price oracle.
//! Prices stablecoins, WPLS, majors, and all other tokens from DEX reserves.

use anyhow::Result;
use pool_registry::{api, config, db, price_updater, startup};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pool_registry=info,sqlx=warn".into()),
        )
        .init();

    arb_metrics::init(arb_metrics::ports::POOL_REGISTRY_PRICE);

    let cfg = config::AppConfig::load()?;
    let api_port = startup::api_port_override()?.unwrap_or(cfg.api.port);
    tracing::info!(api_port, "pool-registry-price starting");

    let db_pool = startup::connect_db(&cfg.database).await?;
    tracing::info!("Connected to PostgreSQL");

    db::run_migrations(&db_pool).await?;
    tracing::info!("Database migrations applied");

    // Run initial oracle pass then periodic refresh — entirely in background
    // so the API server starts immediately and health checks pass at once.
    let min_liq = cfg.price_updater.min_anchor_liquidity_usd;
    let interval = cfg.price_updater.refresh_interval_secs;
    {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let anchors = cfg.filter.anchor_tokens.clone();
        tokio::spawn(async move {
            tracing::info!(min_anchor_liquidity_usd = min_liq, "Running initial price oracle pass…");
            if let Err(e) =
                price_updater::update_once(&pool_clone, &rpc_url, &anchors, min_liq).await
            {
                tracing::warn!(error = %e, "Initial oracle pass failed — prices will be null until next cycle");
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            price_updater::run(pool_clone, rpc_url, anchors, interval, min_liq).await;
        });
    }

    api::start(&cfg.api.host, api_port, db_pool).await?;
    Ok(())
}

//! pool-registry-metadata: resolves ERC-20 token metadata (symbol/name/decimals)
//! and runs the fee-on-transfer screener. FoT tokens are automatically labelled
//! as meme and excluded from the /pools API served by this process.

use anyhow::Result;
use pool_registry::{api, config, db, fot_screener, startup, token_metadata};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pool_registry=info,sqlx=warn".into()),
        )
        .init();

    arb_metrics::init(arb_metrics::ports::POOL_REGISTRY_METADATA);

    let cfg = config::AppConfig::load()?;
    let api_port = startup::api_port_override()?.unwrap_or(cfg.api.port);
    tracing::info!(api_port, rpc = %cfg.network.rpc_http, "pool-registry-metadata starting");

    let db_pool = startup::connect_db(&cfg.database).await?;
    tracing::info!("Connected to PostgreSQL");

    db::run_migrations(&db_pool).await?;
    tracing::info!("Database migrations applied");

    if !cfg.filter.denylist.is_empty() {
        match db::flag_tokens_fot(&db_pool, &cfg.filter.denylist).await {
            Ok(n) => tracing::info!(rows = n, "Applied token denylist (is_fot + is_meme)"),
            Err(e) => tracing::warn!(error = %e, "Failed to apply token denylist"),
        }
        match db::flag_tokens_meme(&db_pool, &cfg.filter.denylist).await {
            Ok(n) => tracing::info!(rows = n, "Applied token denylist (is_meme)"),
            Err(e) => tracing::warn!(error = %e, "Failed to apply meme denylist"),
        }
    }

    // Token metadata worker: resolves symbol/name/decimals for every pool token.
    {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let batch_size = cfg.worker.batch_size;
        let idle = cfg.worker.idle_sleep_secs;
        tokio::spawn(async move {
            token_metadata::run(pool_clone, rpc_url, batch_size, idle).await;
        });
    }

    // FoT screener: detects transfer-tax tokens via eth_call state overrides.
    // A token flagged as FoT is also labelled meme — it is the sole criterion.
    if cfg.fot_screener.enabled {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let screener_cfg = cfg.fot_screener.clone();
        tokio::spawn(async move {
            fot_screener::run(pool_clone, rpc_url, screener_cfg).await;
        });
    }

    // Metrics gauge loop.
    {
        let metrics_pool = db_pool.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                ticker.tick().await;
                if let Ok((referenced, resolved)) = db::count_token_metadata(&metrics_pool).await {
                    metrics::gauge!("registry_tokens_resolved").set(resolved as f64);
                    metrics::gauge!("registry_tokens_pending").set((referenced - resolved) as f64);
                }
            }
        });
    }

    api::start(&cfg.api.host, api_port, db_pool).await?;
    Ok(())
}

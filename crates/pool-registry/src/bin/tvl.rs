//! pool-registry-tvl: seeds pool addresses from DEX factories and maintains
//! per-pool USD TVL by reading on-chain reserves against the price oracle DB.

use anyhow::Result;
use pool_registry::{api, chain_seeder, config, db, startup, tvl_worker};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pool_registry=info,sqlx=warn".into()),
        )
        .init();

    arb_metrics::init(arb_metrics::ports::POOL_REGISTRY_TVL);

    let cfg = config::AppConfig::load()?;
    let api_port = startup::api_port_override()?.unwrap_or(cfg.api.port);
    tracing::info!(api_port, rpc = %cfg.network.rpc_http, "pool-registry-tvl starting");

    let db_pool = startup::connect_db(&cfg.database).await?;
    tracing::info!("Connected to PostgreSQL");

    db::run_migrations(&db_pool).await?;
    tracing::info!("Database migrations applied");

    if !cfg.filter.denylist.is_empty() {
        match db::flag_tokens_fot(&db_pool, &cfg.filter.denylist).await {
            Ok(n) => tracing::info!(rows = n, "Applied token denylist (is_fot)"),
            Err(e) => tracing::warn!(error = %e, "Failed to apply token denylist"),
        }
    }

    // Chain seeding (~5 min first run) and TVL worker run entirely in the
    // background so the API server starts immediately and health checks pass.
    {
        let pool_clone = db_pool.clone();
        let cfg_clone = cfg.clone();
        tokio::spawn(async move {
            let existing = match db::count_total(&pool_clone).await {
                Ok(n) => n,
                Err(e) => { tracing::warn!(error = %e, "count_total failed"); 0 }
            };
            tracing::info!(existing_pools = existing, "Enumerating pairs for all enabled DEXes (~5 min on first run)");
            match chain_seeder::enumerate_all_pairs(&cfg_clone).await {
                Ok(pairs) if pairs.is_empty() => {
                    tracing::warn!("Chain seeder returned 0 pairs — RPC may be unreachable");
                }
                Ok(pairs) => {
                    match db::upsert_pools(&pool_clone, &pairs).await {
                        Ok(inserted) => tracing::info!(enumerated = pairs.len(), newly_inserted = inserted, "Pools seeded"),
                        Err(e) => tracing::warn!(error = %e, "upsert_pools failed"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "Chain seeder failed — continuing with existing pools"),
            }
            if let Err(e) = chain_seeder::populate_tokens(&pool_clone, &cfg_clone).await {
                tracing::warn!(error = %e, "Seed token-fill failed — TVL worker will fill lazily");
            }
        });
    }

    // TVL worker: reads reserves, looks up prices, writes TVL per pool.
    {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let worker_cfg = cfg.worker.clone();
        tokio::spawn(async move {
            tvl_worker::run(pool_clone, rpc_url, worker_cfg).await;
        });
    }

    // Metrics gauge loop.
    {
        let metrics_pool = db_pool.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                ticker.tick().await;
                if let Ok((total, with_tvl)) = db::count_pools(&metrics_pool).await {
                    metrics::gauge!("registry_pools_total").set(total as f64);
                    metrics::gauge!("registry_pools_with_tvl").set(with_tvl as f64);
                }
            }
        });
    }

    api::start(&cfg.api.host, api_port, db_pool).await?;
    Ok(())
}

mod api;
mod chain_seeder;
mod config;
mod db;
mod fot_screener;
mod meme_screener;
mod price;
mod price_updater;
mod reserve_fetcher;
mod token_metadata;
mod tvl_worker;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;

/// Which of the registry's three population modes (plus `All`) this process runs.
/// Selected via `--mode <metadata|price|tvl|all>` or the `REGISTRY_MODE` env var.
/// Defaults to `All`, which runs every worker in a single process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Metadata,
    Price,
    Tvl,
    All,
}

impl Mode {
    fn from_args_and_env() -> Result<Self> {
        // --mode <x> takes precedence over REGISTRY_MODE
        let mut args = std::env::args().skip(1);
        let raw = loop {
            match args.next() {
                Some(a) if a == "--mode" => break args.next(),
                Some(a) if a.starts_with("--mode=") => {
                    break Some(a["--mode=".len()..].to_string())
                }
                Some(_) => continue,
                None => break std::env::var("REGISTRY_MODE").ok(),
            }
        };
        match raw.as_deref().map(str::trim).map(str::to_lowercase).as_deref() {
            None | Some("") | Some("all") => Ok(Mode::All),
            Some("metadata") => Ok(Mode::Metadata),
            Some("price") => Ok(Mode::Price),
            Some("tvl") => Ok(Mode::Tvl),
            Some(other) => Err(anyhow::anyhow!(
                "Unknown mode '{}'. Expected one of: metadata, price, tvl, all",
                other
            )),
        }
    }

    fn runs_seeder(self) -> bool {
        matches!(self, Mode::Tvl | Mode::All)
    }
    fn runs_price(self) -> bool {
        matches!(self, Mode::Price | Mode::All)
    }
    fn runs_tvl(self) -> bool {
        matches!(self, Mode::Tvl | Mode::All)
    }
    fn runs_metadata(self) -> bool {
        matches!(self, Mode::Metadata | Mode::All)
    }
}

/// Optional API-port override from `--api-port <n>` / `--api-port=n` or the
/// `REGISTRY_API_PORT` env var (flag wins). Lets each mode bind its own port so
/// multiple modes can run on one host without colliding on `config.api.port`.
fn api_port_override() -> Result<Option<u16>> {
    let mut args = std::env::args().skip(1);
    let raw = loop {
        match args.next() {
            Some(a) if a == "--api-port" => break args.next(),
            Some(a) if a.starts_with("--api-port=") => {
                break Some(a["--api-port=".len()..].to_string())
            }
            Some(_) => continue,
            None => break std::env::var("REGISTRY_API_PORT").ok(),
        }
    };
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(p) => p
            .parse::<u16>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Invalid API port '{}': must be 1-65535", p)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialise structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pool_registry=info,sqlx=warn".into()),
        )
        .init();

    // 2. Load config + resolve run mode + API port
    let mode = Mode::from_args_and_env()?;
    let cfg = config::AppConfig::load()?;
    let api_port = api_port_override()?.unwrap_or(cfg.api.port);
    tracing::info!(
        mode = ?mode,
        api_port,
        rpc = %cfg.network.rpc_http,
        batch_size = cfg.worker.batch_size,
        price_refresh_secs = cfg.price_updater.refresh_interval_secs,
        "Config loaded"
    );

    // 3. Connect to PostgreSQL — retry until ready (gives docker-compose time to start)
    let db_pool = {
        const MAX_RETRIES: u32 = 12;
        const RETRY_DELAY_SECS: u64 = 5;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match PgPoolOptions::new()
                .max_connections(cfg.database.max_connections)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&cfg.database.url)
                .await
            {
                Ok(pool) => break pool,
                Err(e) if attempt < MAX_RETRIES => {
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        retry_in_secs = RETRY_DELAY_SECS,
                        error = %e,
                        "PostgreSQL not ready, retrying... (hint: run `docker-compose up -d`)"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to connect to PostgreSQL after {} attempts: {}\n\
                         Make sure Postgres is running: docker-compose up -d",
                        MAX_RETRIES,
                        e
                    ));
                }
            }
        }
    };
    tracing::info!("Connected to PostgreSQL");

    // 4. Run migrations
    db::run_migrations(&db_pool).await?;
    tracing::info!("Database migrations applied");

    // 4b. Apply the config token denylist (fee-on-transfer / gas-heavy / scam).
    //     Flagged tokens' pools are excluded from /pools, so they never reach the
    //     listener, finder, or broadcaster. Runs in every mode (cheap).
    if !cfg.filter.denylist.is_empty() {
        match db::flag_tokens_fot(&db_pool, &cfg.filter.denylist).await {
            Ok(n) => tracing::info!(
                denylisted = cfg.filter.denylist.len(),
                rows = n,
                "Applied token denylist (is_fot)"
            ),
            Err(e) => tracing::warn!(error = %e, "Failed to apply token denylist"),
        }
    }

    // 4c. Apply the meme-coin denylist from config. These addresses are hard-flagged
    //     as meme regardless of keywords; their pools are excluded from /pools.
    if !cfg.meme_screener.denylist.is_empty() {
        match db::flag_tokens_meme(&db_pool, &cfg.meme_screener.denylist).await {
            Ok(n) => tracing::info!(
                denylisted = cfg.meme_screener.denylist.len(),
                rows = n,
                "Applied meme token denylist (is_meme)"
            ),
            Err(e) => tracing::warn!(error = %e, "Failed to apply meme token denylist"),
        }
    }

    // 5. Seed pools table from chain (enumerates pair addresses via Multicall3 for
    //    every enabled DEX factory). Runs on every startup — inserts are idempotent
    //    (ON CONFLICT DO NOTHING), so newly-enabled factories get picked up without
    //    wiping the DB; existing pools are kept. Only TVL/All modes seed.
    if mode.runs_seeder() {
        let existing = db::count_total(&db_pool).await?;
        tracing::info!(existing_pools = existing, "Enumerating pairs for all enabled DEXes (~5 min)");
        match chain_seeder::enumerate_all_pairs(&cfg).await {
            Ok(pairs) if pairs.is_empty() => {
                tracing::warn!("Chain seeder returned 0 pairs — RPC may be unreachable");
            }
            Ok(pairs) => {
                let inserted = db::upsert_pools(&db_pool, &pairs).await?;
                tracing::info!(
                    enumerated = pairs.len(),
                    newly_inserted = inserted,
                    "Pools seeded from chain"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "Chain seeder failed — continuing with existing pools");
            }
        }

    // Fill static token identity (token0/token1/decimals) for any pools missing it,
    // so the price oracle can identify anchor pools immediately after a fresh seed
    // instead of waiting for the TVL worker's slow round-robin.
    if let Err(e) = chain_seeder::populate_tokens(&db_pool, &cfg).await {
        tracing::warn!(error = %e, "Seed token-fill failed — TVL worker will fill lazily");
    }
    }

    // 6. PRICE MODE — initial on-chain oracle pass + periodic refresh.
    if mode.runs_price() {
        let min_liq = cfg.price_updater.min_anchor_liquidity_usd;
        tracing::info!(min_anchor_liquidity_usd = min_liq, "Running initial on-chain price oracle pass…");
        if let Err(e) = price_updater::update_once(
            &db_pool,
            &cfg.network.rpc_http,
            &cfg.filter.anchor_tokens,
            min_liq,
        )
        .await
        {
            tracing::warn!(error = %e, "Initial oracle pass failed — TVL will be null until next cycle");
        }

        // Spawn periodic oracle (sleeps first interval, since the initial run already happened)
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let anchors = cfg.filter.anchor_tokens.clone();
        let interval = cfg.price_updater.refresh_interval_secs;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            price_updater::run(pool_clone, rpc_url, anchors, interval, min_liq).await;
        });
    }

    // 7. TVL MODE — read reserves from chain, prices from DB (oracle), write TVL.
    if mode.runs_tvl() {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let worker_cfg = cfg.worker.clone();
        tokio::spawn(async move {
            tvl_worker::run(pool_clone, rpc_url, worker_cfg).await;
        });
    }

    // 8. METADATA MODE — resolve symbol/name/decimals on-chain for pool tokens.
    if mode.runs_metadata() {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let batch_size = cfg.worker.batch_size;
        let idle = cfg.worker.idle_sleep_secs;
        tokio::spawn(async move {
            token_metadata::run(pool_clone, rpc_url, batch_size, idle).await;
        });
    }

    // 8b. FOT SCREENER — auto-detect fee-on-transfer / gas-heavy tokens on-chain
    //     (via eth_call state overrides) and flag them so their pools drop from
    //     /pools. Default-OFF: only spawned when [fot_screener].enabled is true.
    //     Runs alongside metadata population (Metadata/All modes).
    if mode.runs_metadata() && cfg.fot_screener.enabled {
        let pool_clone = db_pool.clone();
        let rpc_url = cfg.network.rpc_http.clone();
        let screener_cfg = cfg.fot_screener.clone();
        tokio::spawn(async move {
            fot_screener::run(pool_clone, rpc_url, screener_cfg).await;
        });
    }

    // 8c. MEME SCREENER — classify tokens as meme coins by keyword matching
    //     against their symbol / name (no RPC needed). Flags their pools via
    //     `has_meme_token` so they are excluded from /pools. Default-OFF.
    //     Runs alongside metadata population (Metadata/All modes) since it
    //     depends on token_metadata rows being present.
    if mode.runs_metadata() && cfg.meme_screener.enabled {
        let pool_clone = db_pool.clone();
        let screener_cfg = cfg.meme_screener.clone();
        tokio::spawn(async move {
            meme_screener::run(pool_clone, screener_cfg).await;
        });
    }

    // 9. Start axum HTTP server (blocks until shutdown)
    api::start(&cfg.api.host, api_port, db_pool).await?;

    Ok(())
}

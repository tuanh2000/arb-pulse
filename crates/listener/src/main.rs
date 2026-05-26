mod api;
mod config;
mod types;
mod store;
mod registry_client;
mod sink;
mod protocols;
mod listener;
mod mempool;

use anyhow::Result;
use registry_client::RegistryClient;
use sink::RedisSink;
use std::sync::Arc;
use store::PoolStore;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "listener=info".into()),
        )
        .init();

    arb_metrics::init(arb_metrics::ports::LISTENER);

    let config = config::AppConfig::load()?;
    info!(
        chain_id = config.network.chain_id,
        registry_url = %config.registry.url,
        redis_url = %config.redis.url,
        min_tvl_usd = config.registry.min_tvl_usd,
        "Config loaded"
    );

    // Connect to Redis (state output for PathFinder).
    let sink = RedisSink::connect(&config.redis.url).await?;
    info!("Connected to Redis");

    // Load the curated valid-TVL pool set from pool-registry.
    let registry = RegistryClient::new(config.registry.url.clone());
    let pools = registry.load_pools(config.registry.min_tvl_usd).await?;
    if pools.is_empty() {
        warn!("pool-registry returned 0 pools above the TVL threshold — nothing to track");
    }
    info!(count = pools.len(), "Loaded valid-TVL pools from pool-registry");
    let pools = Arc::new(pools);

    // Initialize reserve state for ONLY the curated pools via Multicall3.
    info!("Initializing pool state from chain...");
    let states = listener::fetch_states_for_pools(&config, &pools).await?;
    let store = PoolStore::new();
    for s in states {
        store.insert(s);
    }
    metrics::gauge!("listener_pools_tracked").set(store.len() as f64);
    info!(pools = store.len(), "Pool state initialized");

    // Seed Redis with the initial snapshot before going live.
    sink.write_snapshot(&store.get_all()).await?;
    info!("Initial state snapshot written to Redis");

    // Serve the read-only HTTP API alongside the WS listener. Both run forever;
    // if either returns (always an error), propagate it and shut down.
    let api_store = store.clone();
    let api_host = config.api.host.clone();
    let api_port = config.api.port;

    // Phase 2 (additive, default OFF): spawn the mempool watcher only when the
    // [mempool] section is present AND enabled. It shares the same Redis sink and
    // pool set but publishes only to the separate `pending_updates` channel, so
    // the confirmed Sync path is entirely unaffected whether on or off.
    let mempool_handle = match &config.mempool {
        Some(m) if m.enabled => {
            info!(channel = %m.channel, "Mempool watcher enabled (predicted updates)");
            let mp_store = store.clone();
            let mp_sink = sink.clone();
            let mp_config = config.clone();
            Some(tokio::spawn(async move {
                mempool::run(&mp_config, mp_store, mp_sink).await
            }))
        }
        _ => None,
    };

    let core = tokio::try_join!(
        // Maintain state from WebSocket Sync events (no RPC polling in steady state).
        listener::start_ws_listener(&config, store, sink, pools),
        api::start(&api_host, api_port, api_store),
    );

    if let Some(h) = mempool_handle {
        h.abort();
    }
    core?;

    Ok(())
}

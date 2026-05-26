use crate::config::AppConfig;
use crate::protocols::ProtocolRegistry;
use crate::registry_client::RegistryPool;
use crate::sink::RedisSink;
use crate::store::PoolStore;
use alloy::{
    providers::{Provider, ProviderBuilder, WsConnect},
    rpc::types::{BlockNumberOrTag, Filter},
};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use std::sync::Arc;

const MAX_RETRIES: u32 = 10;
const RETRY_DELAY_SECS: u64 = 2;

async fn run_inner(config: &AppConfig, store: &PoolStore, sink: &RedisSink) -> Result<()> {
    let ws = WsConnect::new(config.network.rpc_ws.clone());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;

    let registry = ProtocolRegistry::new();

    // Subscribe to the union of every protocol's state-change topics, with no
    // address filter — catch all matching events globally and route each log to
    // its pool's protocol in-app. Avoids any address-array limit on the subscription.
    let filter = Filter::new()
        .event_signature(registry.all_topics())
        .from_block(BlockNumberOrTag::Latest);

    let sub = provider.subscribe_logs(&filter).await?;
    let mut stream = sub.into_stream();

    // Block-head subscription: drives the per-block "block_complete" signal the
    // PathFinder evaluates on. When head N arrives, block N-1's logs are all in.
    let block_sub = provider.subscribe_blocks().await?;
    let mut block_stream = block_sub.into_stream();
    // Highest block we have already signalled as complete (0 = none yet).
    let mut last_completed: u64 = 0;
    // Timestamp (seconds since epoch) of the most recently received block header.
    // When we signal block N complete we pass N's timestamp, which we captured
    // one iteration ago (block N+1 triggers the signal for N).
    let mut prev_block_timestamp: u64 = 0;
    // block number -> on-chain timestamp, populated from block heads. Used as a
    // fallback to stamp Sync updates when the log itself lacks `block_timestamp`,
    // so the finder can measure block-creation -> detection latency. Pruned to a
    // recent window to stay bounded.
    let mut block_ts_map: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();

    tracing::info!("WebSocket state-change subscription active");

    // Liveness counters. `seen` counts all Sync logs on chain; `matched` counts
    // updates applied to our curated pools. The heartbeat reports both so a quiet
    // log doesn't look like a dead socket — and a live socket with 0 matches still
    // proves events are flowing.
    let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(30));
    ticker.tick().await; // consume the immediate first tick
    let mut seen: u64 = 0;
    let mut matched: u64 = 0;
    let mut total_matched: u64 = 0;
    let mut first_update_logged = false;

    loop {
        tokio::select! {
            maybe_log = stream.next() => {
                let Some(log) = maybe_log else {
                    return Err(anyhow!("WebSocket Sync stream ended unexpectedly"));
                };
                seen += 1;
                let pair_addr = log.address();

                // Single lookup doubles as the membership check and gives us the
                // pool's protocol (to pick the decoder) and metadata (for logging).
                let Some(pool) = store.get(&pair_addr) else {
                    continue;
                };

                // Route the log to the decoder for this pool's protocol.
                let Some(proto) = registry.get(&pool.dex_type) else {
                    continue;
                };
                let Some(update) = proto.decode_update(&log) else {
                    continue;
                };

                store.update_reserves(update.address, update.reserve0, update.reserve1, update.block);

                // Prefer the timestamp the node attaches to the log; fall back to the
                // header-derived map. 0 means unknown (finder then omits latency).
                let block_ts = log
                    .block_timestamp
                    .or_else(|| block_ts_map.get(&update.block).copied())
                    .unwrap_or(0);

                if let Err(e) = sink
                    .update_reserves(update.address, update.reserve0, update.reserve1, update.block, block_ts)
                    .await
                {
                    tracing::warn!(pair = %pair_addr, error = %e, "Failed to write reserve update to Redis");
                }

                matched += 1;
                total_matched += 1;

                // Confirm the pipeline end-to-end on the very first matched update,
                // so liveness is visible immediately instead of after 30s.
                if !first_update_logged {
                    first_update_logged = true;
                    tracing::info!(
                        pair = %pair_addr,
                        block = update.block,
                        "First state update applied to a curated pool — WS maintenance live"
                    );
                }

                tracing::info!(
                    pool = %pair_addr,
                    dex = %pool.dex_name,
                    token0 = %pool.token0,
                    token1 = %pool.token1,
                    reserve0 = %update.reserve0,
                    reserve1 = %update.reserve1,
                    block = update.block,
                    "Pool reserves updated"
                );
            }
            maybe_header = block_stream.next() => {
                let Some(header) = maybe_header else {
                    return Err(anyhow!("WebSocket newHeads stream ended unexpectedly"));
                };
                // On head N, block N-1 is fully delivered. Signal it (and skip any
                // gap if heads jumped — one ping for the latest completed block
                // is enough, the finder evaluates its whole dirty set at once).
                let completed = header.number.saturating_sub(1);
                if completed > last_completed {
                    if let Err(e) = sink.publish_block_complete(completed, prev_block_timestamp).await {
                        tracing::warn!(block = completed, error = %e, "Failed to publish block_complete");
                    } else {
                        tracing::debug!(head = header.number, block_complete = completed, "Signalled block complete");
                    }
                    last_completed = completed;
                }
                prev_block_timestamp = header.timestamp;
                block_ts_map.insert(header.number, header.timestamp);
                // Keep only a recent window so the map can't grow unbounded.
                let cutoff = header.number.saturating_sub(300);
                block_ts_map.retain(|&n, _| n >= cutoff);
            }
            _ = ticker.tick() => {
                tracing::info!(
                    seen_30s = seen,
                    matched_30s = matched,
                    total_matched,
                    tracked_pools = store.len(),
                    last_completed_block = last_completed,
                    "WS heartbeat: Sync events in last 30s"
                );
                seen = 0;
                matched = 0;
            }
        }
    }
}

/// Re-fetch reserves for the full curated pool set from chain and refresh both
/// the in-memory store and Redis. Called after a reconnect to repair any gap in
/// events that occurred while the socket was down.
async fn resync(
    config: &AppConfig,
    store: &PoolStore,
    sink: &RedisSink,
    pools: &[RegistryPool],
) -> Result<()> {
    let states = crate::listener::fetch_states_for_pools(config, pools).await?;
    for s in &states {
        store.update_reserves(s.pair_address, s.reserve0, s.reserve1, s.last_updated_block);
    }
    sink.write_snapshot(&states).await?;
    tracing::info!(pools = states.len(), "Re-synced pool state after reconnect");
    Ok(())
}

pub async fn run(
    config: &AppConfig,
    store: PoolStore,
    sink: RedisSink,
    pools: Arc<Vec<RegistryPool>>,
) -> Result<()> {
    let mut attempt = 0u32;

    loop {
        match run_inner(config, &store, &sink).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempt += 1;
                if attempt >= MAX_RETRIES {
                    tracing::error!(
                        attempt,
                        error = %e,
                        "WebSocket listener failed after max retries, giving up"
                    );
                    return Err(e);
                }

                tracing::warn!(
                    attempt,
                    max = MAX_RETRIES,
                    error = %e,
                    "WebSocket listener error, retrying in {}s",
                    RETRY_DELAY_SECS
                );

                tokio::time::sleep(tokio::time::Duration::from_secs(RETRY_DELAY_SECS)).await;

                // Repair any gap that opened while disconnected.
                if let Err(re) = resync(config, &store, &sink, &pools).await {
                    tracing::warn!(error = %re, "Re-sync after reconnect failed (continuing)");
                }
            }
        }
    }
}

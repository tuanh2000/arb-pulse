use crate::types::PoolState;
use alloy::primitives::Address;
use anyhow::Result;
use redis::AsyncCommands;
use std::collections::HashMap;

/// Channel the listener publishes incremental reserve updates on.
pub const UPDATE_CHANNEL: &str = "pool_updates";

/// Channel the listener pings once per fully-processed block. This is the
/// evaluation trigger: reserve deltas arrive on `UPDATE_CHANNEL` and are applied
/// immediately, but cycles are only re-evaluated when this fires.
pub const BLOCK_COMPLETE_CHANNEL: &str = "block_complete";

/// Load the full pool snapshot from Redis (`pool:*` hashes written by the listener).
pub async fn load_snapshot(url: &str) -> Result<Vec<PoolState>> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;

    let keys: Vec<String> = conn.keys("pool:*").await?;
    let mut pools = Vec::with_capacity(keys.len());
    for key in keys {
        let map: HashMap<String, String> = conn.hgetall(&key).await?;
        match parse_pool(&key, &map) {
            Some(pool) => pools.push(pool),
            None => tracing::warn!(key = %key, "skipping malformed pool hash"),
        }
    }
    Ok(pools)
}

/// Subscribe to the reserve-update and block-complete channels on one connection
/// (so publish order is preserved: a block's updates are delivered before its
/// block_complete trigger). When `pending_channel` is set (Phase 2 speculative
/// mode) also subscribe to it. Caller drives `on_message`.
pub async fn subscribe(url: &str, pending_channel: Option<&str>) -> Result<redis::aio::PubSub> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(UPDATE_CHANNEL).await?;
    pubsub.subscribe(BLOCK_COMPLETE_CHANNEL).await?;
    if let Some(ch) = pending_channel {
        pubsub.subscribe(ch).await?;
    }
    Ok(pubsub)
}

fn parse_pool(key: &str, m: &HashMap<String, String>) -> Option<PoolState> {
    let pair = key.strip_prefix("pool:")?.parse::<Address>().ok()?;
    Some(PoolState {
        pair,
        token0: m.get("token0")?.parse().ok()?,
        token1: m.get("token1")?.parse().ok()?,
        reserve0: m.get("reserve0")?.parse().ok()?,
        reserve1: m.get("reserve1")?.parse().ok()?,
        fee_bps: m.get("fee_bps")?.parse().ok()?,
        dex: m.get("dex").cloned().unwrap_or_default(),
        block: m.get("block").and_then(|b| b.parse().ok()).unwrap_or(0),
    })
}

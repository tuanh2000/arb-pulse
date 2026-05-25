//! Redis sink for mirroring in-memory pool reserve state.
//!
//! The [`RedisSink`] writes each pool's state to a Redis hash keyed by
//! `pool:{address:#x}` and publishes incremental reserve updates on the
//! `pool_updates` channel so the downstream PathFinder service can react in
//! real time. The underlying [`redis::aio::ConnectionManager`] auto-reconnects
//! and is cheaply cloneable, so this type is `Clone`.

use crate::types::PoolState;
use alloy::primitives::{Address, U256};
use anyhow::Result;
use redis::AsyncCommands;

/// Channel that incremental reserve updates are published to.
const UPDATE_CHANNEL: &str = "pool_updates";

/// Channel that one "block fully processed" signal is published to per block,
/// so the PathFinder runs a single coherent evaluation pass per block rather
/// than reacting to every individual reserve update.
const BLOCK_COMPLETE_CHANNEL: &str = "block_complete";

/// Maximum number of pools batched into a single snapshot pipeline.
const SNAPSHOT_CHUNK: usize = 500;

#[derive(Clone)]
pub struct RedisSink {
    conn: redis::aio::ConnectionManager,
}

impl RedisSink {
    /// Open a Redis connection manager (auto-reconnecting).
    pub async fn connect(url: &str) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = redis::aio::ConnectionManager::new(client).await?;
        Ok(Self { conn })
    }

    /// Write/overwrite the full state of every pool. Used for the initial
    /// snapshot and for re-sync after a WebSocket reconnect.
    pub async fn write_snapshot(&self, pools: &[PoolState]) -> Result<()> {
        for chunk in pools.chunks(SNAPSHOT_CHUNK) {
            let mut conn = self.conn.clone();
            let mut pipe = redis::pipe();

            for pool in chunk {
                let key = format!("pool:{:#x}", pool.pair_address);
                pipe.hset_multiple(
                    &key,
                    &[
                        ("dex", pool.dex_name.clone()),
                        ("token0", format!("{:#x}", pool.token0)),
                        ("token1", format!("{:#x}", pool.token1)),
                        ("reserve0", pool.reserve0.to_string()),
                        ("reserve1", pool.reserve1.to_string()),
                        ("fee_bps", pool.fee_bps.to_string()),
                        ("token0_decimals", pool.token0_decimals.to_string()),
                        ("token1_decimals", pool.token1_decimals.to_string()),
                        ("block", pool.last_updated_block.to_string()),
                    ],
                )
                .ignore();
            }

            pipe.query_async::<()>(&mut conn).await?;
        }

        tracing::debug!(count = pools.len(), "wrote pool snapshot to redis");
        Ok(())
    }

    /// Update the reserves + block on an existing pool key, then publish a
    /// notification so PathFinder reacts in real time.
    pub async fn update_reserves(
        &self,
        address: Address,
        reserve0: U256,
        reserve1: U256,
        block: u64,
    ) -> Result<()> {
        let mut conn = self.conn.clone();

        let key = format!("pool:{address:#x}");
        let reserve0 = reserve0.to_string();
        let reserve1 = reserve1.to_string();
        let block_str = block.to_string();

        let _: () = conn
            .hset_multiple(
                &key,
                &[
                    ("reserve0", reserve0.as_str()),
                    ("reserve1", reserve1.as_str()),
                    ("block", block_str.as_str()),
                ],
            )
            .await?;

        let payload = serde_json::json!({
            "address": format!("{address:#x}"),
            "reserve0": reserve0,
            "reserve1": reserve1,
            "block": block,
        })
        .to_string();

        let _: () = conn.publish(UPDATE_CHANNEL, payload).await?;

        Ok(())
    }

    /// Publish a single "block `block` fully processed" signal. The PathFinder
    /// uses this as its evaluation trigger: it applies reserve deltas from
    /// `pool_updates` as they arrive, then evaluates once per block on this ping.
    /// `block_timestamp_s` is the block's on-chain timestamp (seconds since epoch)
    /// so downstream can measure end-to-end latency from block creation.
    pub async fn publish_block_complete(&self, block: u64, block_timestamp_s: u64) -> Result<()> {
        let mut conn = self.conn.clone();
        let payload = serde_json::json!({ "block": block, "block_ts": block_timestamp_s }).to_string();
        let _: () = conn.publish(BLOCK_COMPLETE_CHANNEL, payload).await?;
        Ok(())
    }
}

mod amm;
mod config;
mod db;
mod emitter;
mod finder;
mod graph;
mod source;
mod store;
mod types;

use alloy::primitives::Address;
use anyhow::Result;
use emitter::Emitter;
use finder::{evaluate, EvalParams};
use futures::StreamExt;
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::PoolStore;
use tracing::{debug, info, warn};
use types::{Opportunity, PoolUpdate};

/// Emit a structured info log for one discovered opportunity.
fn log_opportunity(opp: &Opportunity, decimals: u8, db_id: Option<i64>, phase: &str) {
    let scale = 10f64.powi(decimals as i32);
    let size_in = amm::u256_to_f64(opp.amount_in) / scale;
    let path = opp
        .hops
        .iter()
        .map(|h| h.dex.as_str())
        .collect::<Vec<_>>()
        .join(" -> ");
    info!(
        phase,
        net_profit = opp.net_profit_token_in,
        gross_profit = opp.profit_token_in,
        size_in,
        hops = opp.hops.len(),
        block = opp.block,
        db_id = ?db_id,
        path = %path,
        "opportunity found"
    );
}

/// How often the live update loop logs an aggregate heartbeat.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "opportunity_finder=info".into()),
        )
        .init();

    let cfg = config::AppConfig::load()?;
    let token_in = cfg.token_in_address()?;
    info!(
        redis_url = %cfg.redis.url,
        token_in = %format!("{token_in:#x}"),
        max_hops = cfg.finder.max_hops,
        "Config loaded"
    );

    // Optional PostgreSQL connection — degrades gracefully if not configured.
    let db_pool: Option<sqlx::PgPool> = if let Some(db_cfg) = &cfg.database {
        match sqlx::PgPool::connect(&db_cfg.url).await {
            Ok(pool) => {
                info!("Connected to PostgreSQL");
                Some(pool)
            }
            Err(e) => {
                warn!(error = %e, "Failed to connect to PostgreSQL — DB persistence disabled");
                None
            }
        }
    } else {
        info!("No database config — DB persistence disabled");
        None
    };

    // 1. Load the pool snapshot the listener wrote to Redis.
    let pools = source::load_snapshot(&cfg.redis.url).await?;
    if pools.is_empty() {
        warn!("no pools found in Redis — is the listener running?");
    }
    let store = PoolStore::from_pools(pools.clone());
    info!(pools = store.len(), "Loaded pool snapshot");

    // 2. Enumerate candidate cycles once (structure changes only when pools are added/removed).
    let cycles = graph::enumerate_cycles(
        &pools,
        token_in,
        cfg.finder.max_hops,
        cfg.finder.max_cycles,
    );
    let index = graph::build_pool_cycle_index(&cycles);
    info!(cycles = cycles.len(), "Enumerated candidate cycles");
    if cycles.len() >= cfg.finder.max_cycles {
        warn!(
            cap = cfg.finder.max_cycles,
            "cycle enumeration hit the cap — some cycles were skipped"
        );
    }

    let params = EvalParams {
        repay_factor: cfg.repay_factor(),
        loan_fee_bps: cfg.finder.loan_fee_bps,
        max_trade_in: cfg.max_trade_in_raw(),
        token_in_decimals: cfg.finder.token_in_decimals,
        min_profit: cfg.finder.min_profit,
        gas_cost: cfg.finder.gas_cost,
    };

    let emitter = Emitter::connect(&cfg.redis.url, cfg.finder.output_channel.clone()).await?;

    // 3. Initial full evaluation.
    info!(
        cycles = cycles.len(),
        pools = store.len(),
        "Initial scan starting — evaluating all cycles for profitable paths"
    );
    let scan_start = Instant::now();
    let mut emitted = 0usize;
    for cycle in &cycles {
        if let Some(opp) = evaluate(cycle, &store, &params) {
            let db_id = if let Some(pool) = &db_pool {
                match db::insert_opportunity(pool, &opp).await {
                    Ok(id) => Some(id),
                    Err(e) => {
                        warn!(error = %e, "failed to persist opportunity to DB");
                        None
                    }
                }
            } else {
                None
            };
            log_opportunity(&opp, params.token_in_decimals, db_id, "initial");
            emitter.emit(&opp, db_id).await?;
            emitted += 1;
        }
    }
    let scan_ms = scan_start.elapsed().as_millis();
    info!(
        emitted,
        cycles = cycles.len(),
        pools = store.len(),
        elapsed_ms = scan_ms as u64,
        "Initial scan complete"
    );

    // 4. Block-coherent evaluation. Reserve deltas on `pool_updates` keep the store
    //    fresh and queue the touched cycles; the listener's `block_complete` ping
    //    triggers a single evaluation pass over all cycles dirtied during that block.
    //    Both channels share one connection, so a block's updates always arrive
    //    before its trigger.
    let mut pubsub = source::subscribe(&cfg.redis.url).await?;
    let mut stream = pubsub.on_message();
    info!("Listening for pool updates + per-block evaluation triggers...");

    // Aggregate counters for the periodic heartbeat.
    let mut updates_processed: u64 = 0;
    let mut updates_ignored: u64 = 0;
    let mut opps_found: u64 = 0;
    let mut blocks_evaluated: u64 = 0;
    // Cycle indices touched since the last block_complete; deduped across pools.
    let mut dirty_cycles: HashSet<usize> = HashSet::new();
    let mut last_heartbeat = Instant::now();

    while let Some(msg) = stream.next().await {
        let channel = msg.get_channel_name().to_string();
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "bad pubsub payload");
                continue;
            }
        };

        // ── Data path: apply the reserve delta and queue affected cycles. ──
        if channel == source::UPDATE_CHANNEL {
            let update: PoolUpdate = match serde_json::from_str(&payload) {
                Ok(u) => u,
                Err(e) => {
                    warn!(error = %e, "failed to decode pool update");
                    continue;
                }
            };
            let Ok(addr) = update.address.parse::<Address>() else {
                warn!(addr = %update.address, "invalid pool address in update");
                continue;
            };
            let (Ok(r0), Ok(r1)) = (update.reserve0.parse(), update.reserve1.parse()) else {
                warn!("invalid reserves in update");
                continue;
            };
            store.update_reserves(addr, r0, r1, update.block);
            updates_processed += 1;

            match index.get(&addr) {
                Some(cycle_idxs) => {
                    for &i in cycle_idxs {
                        dirty_cycles.insert(i);
                    }
                }
                // Reserve update for a pool that is not part of any cycle.
                None => updates_ignored += 1,
            }
            continue;
        }

        // ── Trigger path: evaluate every dirtied cycle once for this block. ──
        if channel == source::BLOCK_COMPLETE_CHANNEL {
            let parsed = serde_json::from_str::<serde_json::Value>(&payload).ok();
            let block = parsed.as_ref().and_then(|v| v["block"].as_u64()).unwrap_or(0);
            let block_ts = parsed.as_ref().and_then(|v| v["block_ts"].as_u64()).unwrap_or(0);

            // Measure wall-clock lag from when this block was created on-chain.
            // Block timestamps are seconds; we compute ms precision from our own clock.
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let block_age_ms = if block_ts > 0 {
                now_ms.saturating_sub(block_ts * 1000)
            } else {
                0
            };

            let dirty = dirty_cycles.len();
            if dirty > 0 {
                info!(
                    block,
                    block_age_ms,
                    dirty_cycles = dirty,
                    "search start — scanning dirty cycles for profitable paths"
                );
            } else {
                debug!(block, block_age_ms, "block complete — no dirty cycles to scan");
            }
            let eval_start = Instant::now();
            // Pure cycle-math time (excludes DB persist + Redis emit I/O below).
            let mut compute_us: u128 = 0;
            let mut found_this_block: u64 = 0;
            for &i in &dirty_cycles {
                let t = Instant::now();
                let maybe_opp = evaluate(&cycles[i], &store, &params);
                compute_us += t.elapsed().as_micros();
                if let Some(opp) = maybe_opp {
                    let db_id = if let Some(pool) = &db_pool {
                        match db::insert_opportunity(pool, &opp).await {
                            Ok(id) => Some(id),
                            Err(e) => {
                                warn!(error = %e, "failed to persist opportunity to DB");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    opps_found += 1;
                    found_this_block += 1;
                    log_opportunity(&opp, params.token_in_decimals, db_id, "live");
                    if let Err(e) = emitter.emit(&opp, db_id).await {
                        warn!(error = %e, "failed to emit opportunity");
                    }
                }
            }
            dirty_cycles.clear();
            blocks_evaluated += 1;
            // Per-block timing so we can see how long the finder takes per block.
            // `compute_us` is the cycle math alone; `total_us` includes persist+emit.
            let total_us = eval_start.elapsed().as_micros() as u64;
            if dirty > 0 {
                info!(
                    block,
                    block_age_ms,
                    dirty_cycles = dirty,
                    opps = found_this_block,
                    compute_us = compute_us as u64,
                    total_us,
                    "search done — finished scanning dirty cycles for profitable paths"
                );
            }

            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                info!(
                    blocks_evaluated,
                    updates_processed,
                    updates_ignored,
                    opps_found,
                    pools = store.len(),
                    "finder heartbeat"
                );
                last_heartbeat = Instant::now();
            }
            continue;
        }

        debug!(channel = %channel, "ignoring message on unexpected channel");
    }

    Ok(())
}

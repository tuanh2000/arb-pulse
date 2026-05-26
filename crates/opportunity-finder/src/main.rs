mod amm;
mod config;
mod db;
mod emitter;
mod finder;
mod graph;
mod source;
mod store;
mod types;

use alloy::primitives::{Address, U256};
use anyhow::Result;
use emitter::Emitter;
use finder::{evaluate, evaluate_with_view, EvalParams, ReserveView};
use futures::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::PoolStore;
use tracing::{debug, info, warn};
use types::{Opportunity, PendingUpdate, PoolUpdate};

/// Emit a structured info log for one discovered opportunity. `detect_latency_ms`
/// is the lag from the block's on-chain creation to this detection (None = the
/// update carried no block timestamp).
fn log_opportunity(
    opp: &Opportunity,
    decimals: u8,
    db_id: Option<i64>,
    phase: &str,
    detect_latency_ms: Option<u64>,
) {
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
        detect_latency_ms = ?detect_latency_ms,
        db_id = ?db_id,
        path = %path,
        "opportunity found"
    );
}

/// Persist (if DB present), log, and emit a confirmed opportunity. Returns the db id.
async fn handle_confirmed(
    opp: &Opportunity,
    db_pool: &Option<sqlx::PgPool>,
    emitter: &Emitter,
    decimals: u8,
    phase: &str,
    detect_latency_ms: Option<u64>,
) {
    let db_id = if let Some(pool) = db_pool {
        match db::insert_opportunity(pool, opp).await {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(error = %e, "failed to persist opportunity to DB");
                None
            }
        }
    } else {
        None
    };
    log_opportunity(opp, decimals, db_id, phase, detect_latency_ms);
    if let Err(e) = emitter.emit(opp, db_id).await {
        warn!(error = %e, "failed to emit opportunity");
    }
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

    arb_metrics::init(arb_metrics::ports::OPPORTUNITY_FINDER);

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
    // Retry until non-empty: the listener's Multicall3 seed can take 30-120 s,
    // so the finder may start before `pool:*` keys exist.
    const SNAPSHOT_RETRY_SECS: u64 = 5;
    const SNAPSHOT_MAX_ATTEMPTS: usize = 24; // up to 2 min total
    let mut attempt = 0usize;
    let pools = loop {
        let p = source::load_snapshot(&cfg.redis.url).await?;
        if !p.is_empty() {
            break p;
        }
        attempt += 1;
        if attempt >= SNAPSHOT_MAX_ATTEMPTS {
            anyhow::bail!(
                "No pools in Redis after {} attempts ({}s). Is the listener running?",
                SNAPSHOT_MAX_ATTEMPTS,
                SNAPSHOT_MAX_ATTEMPTS as u64 * SNAPSHOT_RETRY_SECS
            );
        }
        warn!(
            attempt,
            max = SNAPSHOT_MAX_ATTEMPTS,
            retry_secs = SNAPSHOT_RETRY_SECS,
            "no pools in Redis snapshot — listener still seeding, retrying..."
        );
        tokio::time::sleep(Duration::from_secs(SNAPSHOT_RETRY_SECS)).await;
    };
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
            handle_confirmed(&opp, &db_pool, &emitter, params.token_in_decimals, "initial", None)
                .await;
            metrics::counter!("finder_opportunities_total", "phase" => "initial").increment(1);
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

    // 4. Live evaluation. Each reserve delta on `pool_updates` is applied to the store
    //    and the affected cycles are evaluated immediately (no waiting for the next
    //    block header). `block_complete` no longer drives evaluation — it is kept only
    //    for the latency log, the heartbeat, and pruning the dedup map.
    let speculative = cfg.finder.speculative_enabled;
    let pending_channel = speculative.then(|| cfg.finder.pending_updates_channel.clone());
    let mut pubsub = source::subscribe(&cfg.redis.url, pending_channel.as_deref()).await?;
    let mut stream = pubsub.on_message();
    info!(
        speculative,
        "Listening for pool updates (evaluate-on-update)..."
    );

    // Aggregate counters for the periodic heartbeat.
    let mut updates_processed: u64 = 0;
    let mut updates_ignored: u64 = 0;
    let mut opps_found: u64 = 0;
    let mut blocks_evaluated: u64 = 0;
    // cycle index -> last block at which it was emitted (per-block dedup).
    let mut last_emitted: HashMap<usize, u64> = HashMap::new();
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

        // ── Data path: apply the reserve delta and evaluate affected cycles now. ──
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
            metrics::counter!("finder_updates_processed_total").increment(1);
            metrics::gauge!("finder_pools_tracked").set(store.len() as f64);

            // Latency from the block's on-chain creation to this detection. Measured
            // now (when we react to the update), not later after DB/emit I/O. None
            // when the listener couldn't attach a block timestamp.
            let detect_latency_ms = if update.block_ts > 0 {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let lag = now_ms.saturating_sub(update.block_ts * 1000);
                // Pipeline latency for EVERY processed update (block creation -> the
                // finder reacting), independent of whether an opportunity is found.
                metrics::histogram!("finder_block_age_ms").record(lag as f64);
                Some(lag)
            } else {
                None
            };

            let Some(cycle_idxs) = index.get(&addr) else {
                // Reserve update for a pool that is not part of any cycle.
                updates_ignored += 1;
                metrics::counter!("finder_updates_ignored_total").increment(1);
                continue;
            };

            for &i in cycle_idxs {
                if let Some(opp) = evaluate(&cycles[i], &store, &params) {
                    // Per-(cycle, block) dedup: skip if already emitted this block.
                    if last_emitted.get(&i) == Some(&update.block) {
                        continue;
                    }
                    last_emitted.insert(i, update.block);
                    opps_found += 1;
                    // Page-3 metric: latency from block creation to opportunity
                    // detection. Recorded only when the listener attached a block
                    // timestamp; observed here, before DB/emit I/O.
                    if let Some(lat) = detect_latency_ms {
                        metrics::histogram!("finder_opportunity_detect_latency_ms")
                            .record(lat as f64);
                    }
                    metrics::counter!("finder_opportunities_total", "phase" => "live").increment(1);
                    metrics::gauge!("finder_last_net_profit").set(opp.net_profit_token_in);
                    handle_confirmed(
                        &opp,
                        &db_pool,
                        &emitter,
                        params.token_in_decimals,
                        "live",
                        detect_latency_ms,
                    )
                    .await;
                }
            }
            continue;
        }

        // ── Speculative path: evaluate against confirmed state with this pool's
        //    predicted reserves overridden (Phase 2; only when enabled). ──
        if speculative && pending_channel.as_deref() == Some(channel.as_str()) {
            let update: PendingUpdate = match serde_json::from_str(&payload) {
                Ok(u) => u,
                Err(e) => {
                    warn!(error = %e, "failed to decode pending update");
                    continue;
                }
            };
            let Ok(addr) = update.address.parse::<Address>() else {
                warn!(addr = %update.address, "invalid pool address in pending update");
                continue;
            };
            let (Ok(r0), Ok(r1)) = (update.reserve0.parse(), update.reserve1.parse()) else {
                warn!("invalid reserves in pending update");
                continue;
            };
            let Some(cycle_idxs) = index.get(&addr) else {
                continue;
            };

            let mut overrides: HashMap<Address, (U256, U256)> = HashMap::with_capacity(1);
            overrides.insert(addr, (r0, r1));
            let view = ReserveView::with_overrides(&store, &overrides);

            for &i in cycle_idxs {
                if let Some(opp) = evaluate_with_view(&cycles[i], &view, &params) {
                    log_opportunity(&opp, params.token_in_decimals, None, "speculative", None);
                    if let Err(e) = emitter
                        .emit_speculative(&cfg.finder.speculative_channel, &opp, &update.tx_hash)
                        .await
                    {
                        warn!(error = %e, "failed to emit speculative opportunity");
                    }
                }
            }
            continue;
        }

        // ── Trigger path: latency log, heartbeat, dedup pruning (no evaluation). ──
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
            debug!(block, block_age_ms, "block complete");

            // Drop dedup entries for blocks older than the one just completed.
            if block > 0 {
                last_emitted.retain(|_, &mut b| b >= block);
            }
            blocks_evaluated += 1;

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

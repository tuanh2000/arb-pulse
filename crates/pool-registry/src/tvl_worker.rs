use crate::{config::WorkerConfig, db, price, reserve_fetcher};
use alloy::providers::ProviderBuilder;
use sqlx::PgPool;
use std::time::Duration;

/// Reads pool reserves from chain and computes each pool's TVL using token prices
/// from the `token_price` table (populated by the price-mode oracle). It does not
/// derive prices itself — pricing is owned entirely by the oracle.
pub async fn run(pool: PgPool, rpc_url: String, cfg: WorkerConfig) {
    let rpc_url_parsed = match rpc_url.parse::<alloy::transports::http::reqwest::Url>() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "Invalid RPC URL — TVL worker cannot start");
            return;
        }
    };

    let provider = ProviderBuilder::new().connect_http(rpc_url_parsed);

    loop {
        let batch = match db::get_oldest_pools(&pool, cfg.batch_size).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(error = %e, "Failed to fetch pool batch from DB");
                tokio::time::sleep(Duration::from_secs(cfg.idle_sleep_secs)).await;
                continue;
            }
        };

        if batch.is_empty() {
            tracing::info!(idle_secs = cfg.idle_sleep_secs, "No pools to process — sleeping");
            tokio::time::sleep(Duration::from_secs(cfg.idle_sleep_secs)).await;
            continue;
        }

        // ── Phase 1: fetch reserves + token info from chain via Multicall3 ───
        let pool_refs: Vec<&db::PoolRecord> = batch.iter().collect();
        let states = match reserve_fetcher::fetch_batch(&provider, &pool_refs).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    batch = batch.len(),
                    "Multicall3 batch failed — bumping updated_at for entire batch"
                );
                for record in &batch {
                    let _ = db::update_tvl(&pool, &record.pool_address, None).await;
                }
                tokio::time::sleep(Duration::from_secs(cfg.idle_sleep_secs)).await;
                continue;
            }
        };

        // ── Phase 2: look up token prices from token_price table ─────────────
        let token_addrs: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            states
                .iter()
                .filter_map(|s| s.as_ref())
                .flat_map(|s| [s.token0.clone(), s.token1.clone()])
                .filter(|a| seen.insert(a.clone()))
                .collect()
        };
        let token_refs: Vec<&str> = token_addrs.iter().map(String::as_str).collect();
        let prices = db::get_token_prices(&pool, &token_refs).await.unwrap_or_default();

        // ── Phase 3: compute TVL and persist ─────────────────────────────────
        let mut updated = 0usize;
        let mut no_state = 0usize;
        let mut no_price = 0usize;
        let mut failed = 0usize;

        for (record, state) in batch.iter().zip(states.iter()) {
            let result = match state {
                Some(s) => {
                    let tvl = price::compute_tvl(
                        &prices,
                        s.reserve0,
                        s.reserve1,
                        &s.token0,
                        &s.token1,
                        s.token0_decimals,
                        s.token1_decimals,
                    );
                    if tvl.is_none() {
                        no_price += 1;
                    }
                    db::update_pool_state(
                        &pool,
                        &record.pool_address,
                        &s.token0,
                        &s.token1,
                        s.token0_decimals,
                        s.token1_decimals,
                        tvl,
                    )
                    .await
                    .map(|_| tvl)
                }
                None => {
                    no_state += 1;
                    db::update_tvl(&pool, &record.pool_address, None)
                        .await
                        .map(|_| None)
                }
            };

            match result {
                Ok(tvl) => {
                    tracing::debug!(
                        pool = %record.pool_address,
                        tvl = tvl.map(|t| format!("{:.2}", t)).unwrap_or("null".into()),
                        "updated"
                    );
                    updated += 1;
                }
                Err(e) => {
                    tracing::warn!(pool = %record.pool_address, error = %e, "DB write failed");
                    let _ = db::update_tvl(&pool, &record.pool_address, None).await;
                    failed += 1;
                }
            }
        }

        tracing::info!(
            updated,
            no_state,
            no_price,
            failed,
            batch = batch.len(),
            "TVL worker batch complete"
        );

        tokio::time::sleep(Duration::from_millis(cfg.batch_delay_ms)).await;
    }
}

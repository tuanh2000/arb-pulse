//! On-chain price oracle with a tiered trusted-anchor set.
//!
//! A token's USD price is read from the reserves of its deepest pool against a
//! trusted, already-priced anchor — far more reliable than subgraph `derivedUSD`.
//! Anchors are priced in tiers, each from the tier below:
//!   Tier 0  stablecoins (USDC/USDT/DAI)        = $1 (hardcoded base)
//!   Tier 1  WPLS                               ← deepest WPLS/stablecoin pool
//!   Tier 2  majors (PLSX/HEX/WETH/PHUX/INC/…)  ← deepest stable/WPLS pool
//!   Tier 3  every other token                  ← deepest pool vs ANY trusted anchor
//!
//! The trusted set is config-driven (`filter.anchor_tokens`): entries with a
//! hardcoded price are stablecoins, the `WPLS` symbol is tier 1, the rest are
//! tier-2 majors. A pool's anchor side must hold at least the configured liquidity
//! floor for its price to be trusted, which keeps dust pools from poisoning prices.
//! Tokens with no qualifying anchor pool get no price.

use crate::{
    config::AnchorToken,
    db::{self, AnchorPricingPool, TokenPriceInput, SOURCE_HARDCODED, SOURCE_RESERVE_ORACLE},
    price, reserve_fetcher,
};
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::Result;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Reject dust-pool prices above this — no legitimate PulseChain token is worth this much.
const MAX_PRICE_USD: f64 = 1_000_000.0;

/// Pools per on-chain reserve sub-batch and the throttle between sub-batches.
const ORACLE_BATCH: usize = 1500;
const THROTTLE_MS: u64 = 100;

fn dec_or_18(d: Option<i16>) -> u8 {
    d.unwrap_or(18) as u8
}

/// Trust tier of an anchor: stablecoin (0) > WPLS (1) > major (2). Lower is preferred.
fn tier_of(addr: &str, stables: &HashSet<String>, wpls: &str) -> u8 {
    if stables.contains(addr) {
        0
    } else if addr == wpls {
        1
    } else {
        2
    }
}

/// A priced candidate: derived price, anchor-side USD liquidity, and the anchor's tier.
struct Priced {
    price: f64,
    anchor_liq_usd: f64,
    tier: u8,
}

/// `a` is a better source than `b`: lower tier wins; within a tier, deeper liquidity.
fn better(a: &Priced, b: &Priced) -> bool {
    if a.tier != b.tier {
        a.tier < b.tier
    } else {
        a.anchor_liq_usd >= b.anchor_liq_usd
    }
}

/// Derive a token's price from one candidate pool, applying the liquidity floor.
fn price_candidate(
    c: &AnchorPricingPool,
    reserves: &HashMap<String, (u128, u128)>,
    anchor_prices: &HashMap<String, f64>,
    stables: &HashSet<String>,
    wpls: &str,
    floor: f64,
) -> Option<Priced> {
    let (reserve0, reserve1) = *reserves.get(&c.pool_address)?;
    let anchor_price = *anchor_prices.get(&c.anchor)?;

    let dec0 = dec_or_18(c.token0_decimals);
    let dec1 = dec_or_18(c.token1_decimals);
    let (anchor_reserve, anchor_dec, token_reserve, token_dec) = if c.anchor_is_token0 {
        (reserve0, dec0, reserve1, dec1)
    } else {
        (reserve1, dec1, reserve0, dec0)
    };

    let anchor_liq_usd = price::to_float(anchor_reserve, anchor_dec) * anchor_price;
    if anchor_liq_usd < floor {
        return None;
    }
    let price = price::compute_spot_price(anchor_reserve, anchor_dec, anchor_price, token_reserve, token_dec)?;
    if price <= 0.0 || price > MAX_PRICE_USD {
        return None;
    }
    Some(Priced { price, anchor_liq_usd, tier: tier_of(&c.anchor, stables, wpls) })
}

/// Pick each token's best pool (tier then depth). Returns (token→price, unpriced tokens).
fn select_prices(
    cands: &[AnchorPricingPool],
    reserves: &HashMap<String, (u128, u128)>,
    anchor_prices: &HashMap<String, f64>,
    stables: &HashSet<String>,
    wpls: &str,
    floor: f64,
) -> (HashMap<String, f64>, Vec<String>) {
    let mut best: HashMap<String, Priced> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    for c in cands {
        seen.insert(c.token.clone());
        if let Some(p) = price_candidate(c, reserves, anchor_prices, stables, wpls, floor) {
            match best.get(&c.token) {
                Some(b) if better(b, &p) => {}
                _ => {
                    best.insert(c.token.clone(), p);
                }
            }
        }
    }
    let priced: HashMap<String, f64> = best.iter().map(|(t, p)| (t.clone(), p.price)).collect();
    let unpriced: Vec<String> = seen.into_iter().filter(|t| !priced.contains_key(t)).collect();
    (priced, unpriced)
}

/// Read reserves for a set of candidate pools, throttled, into a pool→reserves map.
async fn read_reserves<P: Provider>(
    provider: &P,
    cands: &[AnchorPricingPool],
) -> Result<HashMap<String, (u128, u128)>> {
    let mut map = HashMap::with_capacity(cands.len());
    for chunk in cands.chunks(ORACLE_BATCH) {
        let addrs: Vec<String> = chunk.iter().map(|c| c.pool_address.clone()).collect();
        let res = reserve_fetcher::fetch_reserves_only(provider, &addrs).await?;
        for (c, r) in chunk.iter().zip(res.into_iter()) {
            if let Some(rr) = r {
                map.insert(c.pool_address.clone(), rr);
            }
        }
        tokio::time::sleep(Duration::from_millis(THROTTLE_MS)).await;
    }
    Ok(map)
}

/// Run a single oracle cycle.
pub async fn update_once(
    pool: &PgPool,
    rpc_url: &str,
    anchors: &[AnchorToken],
    min_anchor_liq_usd: f64,
) -> Result<()> {
    // ── Classify the configured trusted-anchor set into tiers ───────────────────
    let hardcoded: Vec<TokenPriceInput> = anchors
        .iter()
        .filter_map(|a| {
            a.hardcoded_price_usd.map(|p| TokenPriceInput {
                token_address: a.address.to_lowercase(),
                symbol: Some(a.symbol.clone()),
                name: None,
                price_usd: p,
                source: SOURCE_HARDCODED,
            })
        })
        .collect();
    if !hardcoded.is_empty() {
        db::upsert_token_prices(pool, &hardcoded).await?;
    }

    let stables: Vec<String> = hardcoded.iter().map(|h| h.token_address.clone()).collect();
    if stables.is_empty() {
        tracing::warn!("No hardcoded stablecoins configured — oracle has no USD base, skipping");
        return Ok(());
    }
    let stable_set: HashSet<String> = stables.iter().cloned().collect();

    let wpls = match anchors.iter().find(|a| a.symbol.eq_ignore_ascii_case("WPLS")) {
        Some(a) => a.address.to_lowercase(),
        None => {
            tracing::warn!("No WPLS anchor configured — cannot price WPLS-paired tokens");
            return Ok(());
        }
    };

    // Tier-2 majors: configured anchors that are neither a stablecoin nor WPLS.
    let majors: Vec<String> = anchors
        .iter()
        .map(|a| a.address.to_lowercase())
        .filter(|a| !stable_set.contains(a) && a != &wpls)
        .collect();

    let provider = ProviderBuilder::new()
        .connect_http(rpc_url.parse::<alloy::transports::http::reqwest::Url>()?);

    // ── Build the anchor price map, tier by tier ────────────────────────────────
    let mut anchor_prices: HashMap<String, f64> = stables.iter().map(|s| (s.clone(), 1.0)).collect();

    // Tier 1: WPLS from its deepest WPLS/stablecoin pool.
    if let Some(p) = price_wpls(pool, &provider, &stables, &wpls, min_anchor_liq_usd).await? {
        anchor_prices.insert(wpls.clone(), p);
        db::upsert_oracle_prices(pool, &[oracle_input(&wpls, p)]).await?;
        tracing::info!(wpls_usd = p, "WPLS priced");
    } else {
        tracing::warn!("Could not price WPLS — WPLS-paired tokens will be skipped this cycle");
    }

    // Tier 2: majors from their deepest stable/WPLS pool (bounded to one extra hop).
    if !majors.is_empty() {
        let bases: Vec<String> = anchor_prices.keys().cloned().collect();
        let cands = db::get_pools_pairing(pool, &majors, &bases).await?;
        let reserves = read_reserves(&provider, &cands).await?;
        let (priced, _unpriced) =
            select_prices(&cands, &reserves, &anchor_prices, &stable_set, &wpls, min_anchor_liq_usd);
        let inputs: Vec<TokenPriceInput> = priced
            .iter()
            .map(|(t, p)| {
                anchor_prices.insert(t.clone(), *p);
                oracle_input(t, *p)
            })
            .collect();
        db::upsert_oracle_prices(pool, &inputs).await?;
        tracing::info!(majors = majors.len(), priced = inputs.len(), "Tier-2 majors priced");
    }

    // ── Tier 3: price every token against any trusted anchor ────────────────────
    let all_anchors: Vec<String> = anchor_prices.keys().cloned().collect();
    let cands = db::get_anchor_pricing_pools(pool, &all_anchors).await?;
    let reserves = read_reserves(&provider, &cands).await?;
    let (priced, unpriced) =
        select_prices(&cands, &reserves, &anchor_prices, &stable_set, &wpls, min_anchor_liq_usd);

    let inputs: Vec<TokenPriceInput> = priced.iter().map(|(t, p)| oracle_input(t, *p)).collect();
    let mut priced_count = 0u64;
    for batch in inputs.chunks(2000) {
        priced_count += db::upsert_oracle_prices(pool, batch).await?;
    }

    // Candidate tokens with no qualifying pool: clear any stale price they may carry.
    let mut cleared = 0u64;
    for batch in unpriced.chunks(2000) {
        cleared += db::delete_token_prices(pool, batch).await?;
    }

    let purged = db::delete_stale_prices(pool).await?;

    tracing::info!(
        priced = priced_count,
        unpriced = unpriced.len(),
        cleared,
        candidate_pools = cands.len(),
        anchors = all_anchors.len(),
        purged_stale = purged,
        "Oracle cycle complete"
    );
    Ok(())
}

fn oracle_input(token: &str, price: f64) -> TokenPriceInput {
    TokenPriceInput {
        token_address: token.to_string(),
        symbol: None,
        name: None,
        price_usd: price,
        source: SOURCE_RESERVE_ORACLE,
    }
}

/// Read reserves of all WPLS/stablecoin pools and return WPLS's USD price from the
/// one with the deepest stablecoin side that clears the liquidity floor.
async fn price_wpls<P: Provider>(
    pool: &PgPool,
    provider: &P,
    stables: &[String],
    wpls: &str,
    floor: f64,
) -> Result<Option<f64>> {
    let pools = db::get_wpls_stable_pools(pool, stables, wpls).await?;
    if pools.is_empty() {
        return Ok(None);
    }
    let addrs: Vec<String> = pools.iter().map(|p| p.pool_address.clone()).collect();
    let reserves = reserve_fetcher::fetch_reserves_only(provider, &addrs).await?;

    let mut best: Option<(f64, f64)> = None; // (stable_usd_liquidity, wpls_price)
    for (p, res) in pools.iter().zip(reserves.iter()) {
        let Some((reserve0, reserve1)) = *res else { continue };
        let dec0 = dec_or_18(p.token0_decimals);
        let dec1 = dec_or_18(p.token1_decimals);
        let (stable_reserve, stable_dec, wpls_reserve, wpls_dec) = if p.wpls_is_token0 {
            (reserve1, dec1, reserve0, dec0)
        } else {
            (reserve0, dec0, reserve1, dec1)
        };
        let stable_liq = price::to_float(stable_reserve, stable_dec); // USD, stable=$1
        if stable_liq < floor {
            continue;
        }
        if let Some(wp) = price::compute_spot_price(stable_reserve, stable_dec, 1.0, wpls_reserve, wpls_dec) {
            if best.map_or(true, |(liq, _)| stable_liq > liq) {
                best = Some((stable_liq, wp));
            }
        }
    }
    Ok(best.map(|(_, wp)| wp))
}

/// Periodic background task: runs `update_once` then sleeps `refresh_interval_secs`.
pub async fn run(
    pool: PgPool,
    rpc_url: String,
    anchors: Vec<AnchorToken>,
    refresh_interval_secs: u64,
    min_anchor_liq_usd: f64,
) {
    loop {
        if let Err(e) = update_once(&pool, &rpc_url, &anchors, min_anchor_liq_usd).await {
            tracing::error!(error = %e, "Oracle cycle failed");
        }
        tokio::time::sleep(Duration::from_secs(refresh_interval_secs)).await;
    }
}

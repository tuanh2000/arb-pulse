use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct PoolRecord {
    pub pool_address: String,
    pub protocol: String,
    pub token0: Option<String>,
    pub token1: Option<String>,
    pub token0_decimals: Option<i16>,
    pub token1_decimals: Option<i16>,
    pub tvl: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Bulk-insert pools (address + protocol only). Ignores conflicts.
/// Token data is NULL initially and filled in by the TVL worker.
/// Returns the number of newly inserted rows.
pub async fn upsert_pools(pool: &PgPool, pools: &[(String, String)]) -> Result<u64> {
    let mut inserted = 0u64;
    for (address, protocol) in pools {
        let rows = sqlx::query(
            "INSERT INTO pools (pool_address, protocol) VALUES ($1, $2) ON CONFLICT (pool_address) DO NOTHING"
        )
        .bind(address)
        .bind(protocol)
        .execute(pool)
        .await?
        .rows_affected();
        inserted += rows;
    }
    Ok(inserted)
}

/// All pools that still need their static token identity (token0 IS NULL).
/// Used by the seed-time token fill so the price oracle works immediately.
pub async fn get_pools_missing_tokens(pool: &PgPool) -> Result<Vec<PoolRecord>> {
    let rows = sqlx::query_as::<_, PoolRecord>(
        "SELECT pool_address, protocol, token0, token1, token0_decimals, token1_decimals, tvl, updated_at
         FROM pools WHERE token0 IS NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Set a pool's static token identity (token0/token1/decimals) only. Does not
/// touch tvl or updated_at, so it doesn't disturb the TVL worker's scheduling.
pub async fn update_pool_tokens(
    pool: &PgPool,
    pool_address: &str,
    token0: &str,
    token1: &str,
    token0_decimals: u8,
    token1_decimals: u8,
) -> Result<()> {
    sqlx::query(
        "UPDATE pools SET token0 = $1, token1 = $2, token0_decimals = $3, token1_decimals = $4
         WHERE pool_address = $5",
    )
    .bind(token0)
    .bind(token1)
    .bind(token0_decimals as i16)
    .bind(token1_decimals as i16)
    .bind(pool_address)
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns pools ordered by updated_at ASC (oldest first) up to `limit`.
pub async fn get_oldest_pools(pool: &PgPool, limit: i64) -> Result<Vec<PoolRecord>> {
    let rows = sqlx::query_as::<_, PoolRecord>(
        "SELECT pool_address, protocol, token0, token1, token0_decimals, token1_decimals, tvl, updated_at
         FROM pools ORDER BY updated_at ASC LIMIT $1"
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Update TVL and bump updated_at without touching token columns.
/// Used when the listener returns no data for a pool (404) or on processing error.
pub async fn update_tvl(pool: &PgPool, pool_address: &str, tvl: Option<f64>) -> Result<()> {
    sqlx::query(
        "UPDATE pools SET tvl = $1, updated_at = NOW() WHERE pool_address = $2"
    )
    .bind(tvl)
    .bind(pool_address)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update token info, TVL, and updated_at together.
/// Called by the TVL worker whenever the listener returns full pool state.
pub async fn update_pool_state(
    pool: &PgPool,
    pool_address: &str,
    token0: &str,
    token1: &str,
    token0_decimals: u8,
    token1_decimals: u8,
    tvl: Option<f64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE pools
         SET token0 = $1, token1 = $2, token0_decimals = $3, token1_decimals = $4,
             tvl = $5, updated_at = NOW()
         WHERE pool_address = $6"
    )
    .bind(token0)
    .bind(token1)
    .bind(token0_decimals as i16)
    .bind(token1_decimals as i16)
    .bind(tvl)
    .bind(pool_address)
    .execute(pool)
    .await?;
    Ok(())
}

/// Return all pools where tvl >= min_tvl, EXCLUDING any pool whose token0 or
/// token1 is flagged fee-on-transfer/gas-heavy (`token_metadata.is_fot`) OR
/// whose `has_meme_token` flag is set. This is the single choke point: flagged
/// pools never reach the listener, so they can't enter a cycle or the broadcaster.
pub async fn get_pools_by_min_tvl(
    pool: &PgPool,
    min_tvl: f64,
    include_null: bool,
) -> Result<Vec<PoolRecord>> {
    // Subquery: exclude pools where either token is fee-on-transfer.
    const FOT_EXCLUDE: &str = "NOT EXISTS (\
        SELECT 1 FROM token_metadata m \
        WHERE m.is_fot AND m.token_address IN (lower(token0), lower(token1)))";
    // Direct column check: exclude pools containing a meme token.
    const MEME_EXCLUDE: &str = "NOT has_meme_token";

    let rows = if include_null {
        sqlx::query_as::<_, PoolRecord>(
            &format!("SELECT pool_address, protocol, token0, token1, token0_decimals, token1_decimals, tvl, updated_at
             FROM pools WHERE (tvl >= $1 OR tvl IS NULL) AND {FOT_EXCLUDE} AND {MEME_EXCLUDE} ORDER BY tvl DESC NULLS LAST")
        )
        .bind(min_tvl)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, PoolRecord>(
            &format!("SELECT pool_address, protocol, token0, token1, token0_decimals, token1_decimals, tvl, updated_at
             FROM pools WHERE tvl >= $1 AND {FOT_EXCLUDE} AND {MEME_EXCLUDE} ORDER BY tvl DESC")
        )
        .bind(min_tvl)
        .fetch_all(pool)
        .await?
    };
    Ok(rows)
}

pub async fn count_pools(pool: &PgPool) -> Result<(i64, i64)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pools")
        .fetch_one(pool)
        .await?;
    let with_tvl: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pools WHERE tvl IS NOT NULL")
        .fetch_one(pool)
        .await?;
    Ok((total, with_tvl))
}

pub async fn count_total(pool: &PgPool) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pools")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

// ── token_price ───────────────────────────────────────────────────────────────

pub const SOURCE_HARDCODED: &str = "hardcoded";
pub const SOURCE_RESERVE_ORACLE: &str = "reserve_oracle";

/// Price sources, highest priority first: hardcoded stablecoins ($1 base), then the
/// on-chain reserve oracle. These are the only two trusted sources — anything else
/// in the table is stale and purged each oracle cycle.

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct TokenPriceRecord {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub price_usd: f64,
    pub source: String,
    pub updated_at: DateTime<Utc>,
}

pub struct TokenPriceInput {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub price_usd: f64,
    pub source: &'static str,
}

/// Upsert token prices in a single transaction. On conflict updates price, symbol, name, source.
pub async fn upsert_token_prices(pool: &PgPool, prices: &[TokenPriceInput]) -> Result<u64> {
    if prices.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut upserted = 0u64;
    for p in prices {
        let rows = sqlx::query(
            "INSERT INTO token_price (token_address, symbol, name, price_usd, source)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (token_address) DO UPDATE
             SET price_usd  = EXCLUDED.price_usd,
                 symbol     = EXCLUDED.symbol,
                 name       = EXCLUDED.name,
                 source     = EXCLUDED.source,
                 updated_at = NOW()",
        )
        .bind(&p.token_address)
        .bind(&p.symbol)
        .bind(&p.name)
        .bind(p.price_usd)
        .bind(p.source)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        upserted += rows;
    }
    tx.commit().await?;
    Ok(upserted)
}

/// A candidate pricing pool: a `token` paired with an `anchor` (the anchor side's
/// address), used by the oracle which knows the anchor's USD price.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AnchorPricingPool {
    pub token: String,
    pub pool_address: String,
    /// The anchor-side token address (lowercase).
    pub anchor: String,
    /// True if the anchor is token0 (so `token` is token1).
    pub anchor_is_token0: bool,
    pub token0_decimals: Option<i16>,
    pub token1_decimals: Option<i16>,
}

/// Every pool that pairs a non-anchor token with one of `anchors`, one row per
/// (token, pool). `token` is the non-anchor side, `anchor` the anchor side. Pools
/// where both sides are anchors are excluded (priced separately). The oracle reads
/// reserves and selects each token's best pool itself. `anchors` are lowercase hex.
pub async fn get_anchor_pricing_pools(
    pool: &PgPool,
    anchors: &[String],
) -> Result<Vec<AnchorPricingPool>> {
    let rows = sqlx::query_as::<_, AnchorPricingPool>(
        "SELECT pool_address,
                lower(CASE WHEN lower(token0) = ANY($1) THEN token1 ELSE token0 END) AS token,
                lower(CASE WHEN lower(token0) = ANY($1) THEN token0 ELSE token1 END) AS anchor,
                (lower(token0) = ANY($1)) AS anchor_is_token0,
                token0_decimals, token1_decimals
         FROM pools
         WHERE (lower(token0) = ANY($1)) <> (lower(token1) = ANY($1))
           AND token0 IS NOT NULL",
    )
    .bind(anchors)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Pools pairing a `targets` token with a `bases` token, one row per (token, pool):
/// `token` is the target side, `anchor` the base side. Used to price tier-2 majors
/// (targets) from their stablecoin/WPLS (bases) pools. Both lists are lowercase hex.
pub async fn get_pools_pairing(
    pool: &PgPool,
    targets: &[String],
    bases: &[String],
) -> Result<Vec<AnchorPricingPool>> {
    let rows = sqlx::query_as::<_, AnchorPricingPool>(
        "SELECT pool_address,
                lower(CASE WHEN lower(token0) = ANY($1) THEN token0 ELSE token1 END) AS token,
                lower(CASE WHEN lower(token0) = ANY($1) THEN token1 ELSE token0 END) AS anchor,
                (lower(token1) = ANY($1)) AS anchor_is_token0,
                token0_decimals, token1_decimals
         FROM pools
         WHERE ((lower(token0) = ANY($1) AND lower(token1) = ANY($2))
             OR (lower(token1) = ANY($1) AND lower(token0) = ANY($2)))
           AND token0 IS NOT NULL",
    )
    .bind(targets)
    .bind(bases)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// A WPLS/stablecoin pool used to price WPLS itself.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WplsStablePool {
    pub pool_address: String,
    /// True if WPLS is token0 (so the stable is token1).
    pub wpls_is_token0: bool,
    pub token0_decimals: Option<i16>,
    pub token1_decimals: Option<i16>,
}

/// All pools pairing WPLS directly with a stablecoin. The oracle reads their
/// reserves and prices WPLS from the deepest one.
pub async fn get_wpls_stable_pools(
    pool: &PgPool,
    stables: &[String],
    wpls: &str,
) -> Result<Vec<WplsStablePool>> {
    let rows = sqlx::query_as::<_, WplsStablePool>(
        "SELECT pool_address,
                (lower(token0) = $2) AS wpls_is_token0,
                token0_decimals, token1_decimals
         FROM pools
         WHERE ((lower(token0) = $2 AND lower(token1) = ANY($1))
             OR (lower(token1) = $2 AND lower(token0) = ANY($1)))",
    )
    .bind(stables)
    .bind(wpls)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Purge every price row that isn't hardcoded or oracle-derived. The oracle is the
/// sole trusted source, so leftovers (old subgraph or legacy reserve_derived rows,
/// e.g. dust-pool prices the oracle no longer produces) must not linger and feed TVL.
pub async fn delete_stale_prices(pool: &PgPool) -> Result<u64> {
    let n = sqlx::query(
        "DELETE FROM token_price WHERE source NOT IN ('hardcoded','reserve_oracle')",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// Delete prices for the given tokens (except hardcoded). Used by the oracle to
/// clear a token that no longer has any pool clearing the liquidity floor, so a
/// previously-written price can't go stale and keep poisoning TVL.
pub async fn delete_token_prices(pool: &PgPool, addrs: &[String]) -> Result<u64> {
    if addrs.is_empty() {
        return Ok(0);
    }
    let n = sqlx::query(
        "DELETE FROM token_price WHERE token_address = ANY($1) AND source <> 'hardcoded'",
    )
    .bind(addrs)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// Upsert oracle-derived prices. Overrides every source except `hardcoded`.
pub async fn upsert_oracle_prices(pool: &PgPool, prices: &[TokenPriceInput]) -> Result<u64> {
    if prices.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut upserted = 0u64;
    for p in prices {
        let rows = sqlx::query(
            "INSERT INTO token_price (token_address, symbol, name, price_usd, source)
             VALUES ($1, $2, $3, $4, 'reserve_oracle')
             ON CONFLICT (token_address) DO UPDATE
             SET price_usd  = EXCLUDED.price_usd,
                 source     = EXCLUDED.source,
                 updated_at = NOW()
             WHERE token_price.source <> 'hardcoded'",
        )
        .bind(&p.token_address)
        .bind(&p.symbol)
        .bind(&p.name)
        .bind(p.price_usd)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        upserted += rows;
    }
    tx.commit().await?;
    Ok(upserted)
}

// ── token_metadata ──────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct TokenMetadataRecord {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub decimals: Option<i16>,
    pub updated_at: DateTime<Utc>,
}

pub struct TokenMetadataInput {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub decimals: Option<u8>,
}

/// Distinct token addresses that appear in `pools` (token0/token1) but are not yet
/// present in `token_metadata`. These are the tokens the metadata worker must resolve.
pub async fn get_tokens_missing_metadata(pool: &PgPool, limit: i64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT t.addr FROM (
             SELECT token0 AS addr FROM pools WHERE token0 IS NOT NULL
             UNION
             SELECT token1 AS addr FROM pools WHERE token1 IS NOT NULL
         ) t
         LEFT JOIN token_metadata m ON m.token_address = LOWER(t.addr)
         WHERE m.token_address IS NULL
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(a,)| a).collect())
}

/// Upsert token metadata. On conflict, only overwrites a column when the new value
/// is non-null, so a later partial fetch never clobbers a good earlier value.
pub async fn upsert_token_metadata(pool: &PgPool, rows: &[TokenMetadataInput]) -> Result<u64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut upserted = 0u64;
    for r in rows {
        let affected = sqlx::query(
            "INSERT INTO token_metadata (token_address, symbol, name, decimals)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (token_address) DO UPDATE
             SET symbol     = COALESCE(EXCLUDED.symbol, token_metadata.symbol),
                 name       = COALESCE(EXCLUDED.name, token_metadata.name),
                 decimals   = COALESCE(EXCLUDED.decimals, token_metadata.decimals),
                 updated_at = NOW()",
        )
        .bind(r.token_address.to_lowercase())
        .bind(&r.symbol)
        .bind(&r.name)
        .bind(r.decimals.map(|d| d as i16))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        upserted += affected;
    }
    tx.commit().await?;
    Ok(upserted)
}

/// (total distinct tokens referenced by pools, tokens resolved in token_metadata).
pub async fn count_token_metadata(pool: &PgPool) -> Result<(i64, i64)> {
    let referenced: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
             SELECT token0 AS addr FROM pools WHERE token0 IS NOT NULL
             UNION
             SELECT token1 AS addr FROM pools WHERE token1 IS NOT NULL
         ) t",
    )
    .fetch_one(pool)
    .await?;
    let resolved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM token_metadata")
        .fetch_one(pool)
        .await?;
    Ok((referenced, resolved))
}

/// Flag a set of token addresses as fee-on-transfer (`is_fot = TRUE`). Used to
/// apply the config denylist at startup. Inserts a metadata row if the token has
/// none yet. Addresses are lowercased to match the table's convention.
pub async fn flag_tokens_fot(pool: &PgPool, addrs: &[String]) -> Result<u64> {
    if addrs.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for a in addrs {
        n += sqlx::query(
            "INSERT INTO token_metadata (token_address, is_fot, screened_at)
             VALUES (lower($1), TRUE, NOW())
             ON CONFLICT (token_address) DO UPDATE
             SET is_fot = TRUE, screened_at = NOW()",
        )
        .bind(a)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}

/// One token to screen for fee-on-transfer, with a pool that pairs it against a
/// known base token (whose balance storage slot we can override in `eth_call`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScreenCandidate {
    pub token: String,
    pub pool_address: String,
    pub base: String,
}

/// Unscreened tokens (no `screened_at`) that have a pool pairing them with exactly
/// one of `bases`. One row per token (the first such pool). Bases are lowercase hex.
pub async fn get_unscreened_base_pools(
    pool: &PgPool,
    bases: &[String],
    limit: i64,
) -> Result<Vec<ScreenCandidate>> {
    let rows = sqlx::query_as::<_, ScreenCandidate>(
        "SELECT DISTINCT ON (token) token, pool_address, base FROM (
             SELECT lower(CASE WHEN lower(token0) = ANY($1) THEN token1 ELSE token0 END) AS token,
                    pool_address,
                    lower(CASE WHEN lower(token0) = ANY($1) THEN token0 ELSE token1 END) AS base
             FROM pools
             WHERE ((lower(token0) = ANY($1)) <> (lower(token1) = ANY($1)))
               AND token0 IS NOT NULL
         ) c
         WHERE NOT EXISTS (
             SELECT 1 FROM token_metadata m
             WHERE m.token_address = c.token AND m.screened_at IS NOT NULL
         )
         LIMIT $2",
    )
    .bind(bases)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Record a screening result: set `is_fot` and `screened_at` for one token.
pub async fn mark_token_screened(pool: &PgPool, token: &str, is_fot: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO token_metadata (token_address, is_fot, screened_at)
         VALUES (lower($1), $2, NOW())
         ON CONFLICT (token_address) DO UPDATE
         SET is_fot = EXCLUDED.is_fot, screened_at = NOW()",
    )
    .bind(token)
    .bind(is_fot)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read token metadata, newest first, up to `limit`.
pub async fn list_token_metadata(pool: &PgPool, limit: i64) -> Result<Vec<TokenMetadataRecord>> {
    let rows = sqlx::query_as::<_, TokenMetadataRecord>(
        "SELECT token_address, symbol, name, decimals, updated_at
         FROM token_metadata ORDER BY updated_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ── meme_screener ───────────────────────────────────────────────────────────────

/// A token that has metadata resolved but has not yet been classified for meme
/// status. Fed to the meme screener for keyword-based classification.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingMemeToken {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
}

/// Tokens that have a symbol or name in `token_metadata` but whose
/// `meme_screened_at` is NULL (not yet classified). Ordered by address for
/// stable batching so repeated runs make forward progress.
pub async fn get_tokens_pending_meme_classification(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<PendingMemeToken>> {
    let rows = sqlx::query_as::<_, PendingMemeToken>(
        "SELECT token_address, symbol, name FROM token_metadata
         WHERE meme_screened_at IS NULL
           AND (symbol IS NOT NULL OR name IS NOT NULL)
         ORDER BY token_address
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Record a meme classification result for one token. If the token is flagged,
/// also sets `has_meme_token = TRUE` on every pool that contains it. Both
/// writes happen in a single transaction so they are always consistent.
pub async fn mark_token_meme(pool: &PgPool, token: &str, is_meme: bool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO token_metadata (token_address, is_meme, meme_screened_at)
         VALUES (lower($1), $2, NOW())
         ON CONFLICT (token_address) DO UPDATE
         SET is_meme = EXCLUDED.is_meme, meme_screened_at = NOW()",
    )
    .bind(token)
    .bind(is_meme)
    .execute(&mut *tx)
    .await?;

    if is_meme {
        sqlx::query(
            "UPDATE pools SET has_meme_token = TRUE
             WHERE NOT has_meme_token
               AND (lower(token0) = lower($1) OR lower(token1) = lower($1))",
        )
        .bind(token)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Sync `has_meme_token` on the pools table. Sets the flag TRUE for any pool
/// where either token is a meme coin but the pool flag is stale (e.g. pool
/// token identity was set after the screener ran). Called after each screener
/// batch to converge on correct state without re-classifying every token.
pub async fn sync_pool_meme_flags(pool: &PgPool) -> Result<u64> {
    let n = sqlx::query(
        "UPDATE pools
         SET has_meme_token = TRUE
         WHERE NOT has_meme_token
           AND token0 IS NOT NULL
           AND EXISTS (
             SELECT 1 FROM token_metadata m
             WHERE m.is_meme
               AND m.token_address IN (lower(token0), lower(token1))
           )",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// Flag a set of token addresses as meme coins (from config denylist). Inserts
/// a metadata row if none exists. Also propagates to affected pools. Addresses
/// are lowercased to match the table's convention.
pub async fn flag_tokens_meme(pool: &PgPool, addrs: &[String]) -> Result<u64> {
    if addrs.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for a in addrs {
        n += sqlx::query(
            "INSERT INTO token_metadata (token_address, is_meme, meme_screened_at)
             VALUES (lower($1), TRUE, NOW())
             ON CONFLICT (token_address) DO UPDATE
             SET is_meme = TRUE, meme_screened_at = NOW()",
        )
        .bind(a)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    // Propagate to pools outside the per-row transaction.
    sync_pool_meme_flags(pool).await?;
    Ok(n)
}

/// Look up prices for a specific set of token addresses.
/// Returns a map of lowercase_address → price_usd for all tokens found in the table.
pub async fn get_token_prices(pool: &PgPool, addrs: &[&str]) -> Result<HashMap<String, f64>> {
    if addrs.is_empty() {
        return Ok(HashMap::new());
    }
    let lower: Vec<String> = addrs.iter().map(|a| a.to_lowercase()).collect();
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT token_address, price_usd FROM token_price WHERE token_address = ANY($1)",
    )
    .bind(&lower)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}


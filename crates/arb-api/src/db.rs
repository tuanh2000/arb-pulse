use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TokenRecord {
    pub token_address: String,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub decimals: Option<i16>,
    pub is_fot: bool,
    pub is_meme: bool,
    pub transfer_fee_bps: Option<i16>,
    pub screened_at: Option<DateTime<Utc>>,
    pub price_usd: Option<f64>,
    pub pool_count: i64,
    pub updated_at: DateTime<Utc>,
}

pub struct TokenQuery<'a> {
    pub search: &'a str,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list_tokens(pool: &PgPool, q: TokenQuery<'_>) -> Result<(Vec<TokenRecord>, i64)> {
    let search = q.search.trim();

    let tokens = sqlx::query_as::<_, TokenRecord>(
        r#"
        WITH pc AS (
            SELECT token_address, SUM(cnt)::BIGINT AS pool_count
            FROM (
                SELECT lower(token0) AS token_address, COUNT(*) AS cnt
                FROM pools WHERE token0 IS NOT NULL GROUP BY 1
                UNION ALL
                SELECT lower(token1) AS token_address, COUNT(*) AS cnt
                FROM pools WHERE token1 IS NOT NULL GROUP BY 1
            ) sub
            GROUP BY 1
        )
        SELECT
            m.token_address,
            m.symbol,
            m.name,
            m.decimals,
            m.is_fot,
            m.is_meme,
            m.transfer_fee_bps,
            m.screened_at,
            m.updated_at,
            tp.price_usd,
            COALESCE(pc.pool_count, 0) AS pool_count
        FROM token_metadata m
        LEFT JOIN token_price tp ON tp.token_address = m.token_address
        LEFT JOIN pc ON pc.token_address = m.token_address
        WHERE (
            $1 = ''
            OR m.token_address ILIKE '%' || $1 || '%'
            OR m.symbol ILIKE '%' || $1 || '%'
            OR m.name ILIKE '%' || $1 || '%'
        )
        ORDER BY pool_count DESC, m.updated_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(search)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(pool)
    .await?;

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM token_metadata m
        WHERE (
            $1 = ''
            OR m.token_address ILIKE '%' || $1 || '%'
            OR m.symbol ILIKE '%' || $1 || '%'
            OR m.name ILIKE '%' || $1 || '%'
        )
        "#,
    )
    .bind(search)
    .fetch_one(pool)
    .await?;

    Ok((tokens, total))
}

pub async fn get_token(pool: &PgPool, address: &str) -> Result<Option<TokenRecord>> {
    let token = sqlx::query_as::<_, TokenRecord>(
        r#"
        WITH pc AS (
            SELECT token_address, SUM(cnt)::BIGINT AS pool_count
            FROM (
                SELECT lower(token0) AS token_address, COUNT(*) AS cnt
                FROM pools WHERE token0 IS NOT NULL GROUP BY 1
                UNION ALL
                SELECT lower(token1) AS token_address, COUNT(*) AS cnt
                FROM pools WHERE token1 IS NOT NULL GROUP BY 1
            ) sub
            WHERE token_address = lower($1)
            GROUP BY 1
        )
        SELECT
            m.token_address,
            m.symbol,
            m.name,
            m.decimals,
            m.is_fot,
            m.is_meme,
            m.transfer_fee_bps,
            m.screened_at,
            m.updated_at,
            tp.price_usd,
            COALESCE(pc.pool_count, 0) AS pool_count
        FROM token_metadata m
        LEFT JOIN token_price tp ON tp.token_address = m.token_address
        LEFT JOIN pc ON pc.token_address = m.token_address
        WHERE m.token_address = lower($1)
        "#,
    )
    .bind(address)
    .fetch_optional(pool)
    .await?;

    Ok(token)
}

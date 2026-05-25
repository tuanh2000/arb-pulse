use crate::types::Opportunity;
use anyhow::Result;
use sqlx::PgPool;

/// Insert one opportunity row and return its generated `id`.
pub async fn insert_opportunity(pool: &PgPool, opp: &Opportunity) -> Result<i64> {
    let json = opp.to_json();
    let hops: sqlx::types::JsonValue = json["hops"].clone();

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO opportunities
             (block_number, token_in, amount_in_raw, expected_out_raw,
              profit_raw, profit_human, net_profit_human, hops)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(opp.block as i64)
    .bind(format!("{:#x}", opp.token_in))
    .bind(opp.amount_in.to_string())
    .bind(opp.expected_out.to_string())
    .bind(opp.profit_raw.to_string())
    .bind(opp.profit_token_in)
    .bind(opp.net_profit_token_in)
    .bind(hops)
    .fetch_one(pool)
    .await?;

    Ok(id)
}

use anyhow::Result;
use sqlx::PgPool;

/// Insert a new `arb_transactions` row with status='pending'.
/// Returns the generated row id.
pub async fn insert_arb_tx(db: &PgPool, opportunity_id: Option<i64>) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO arb_transactions (opportunity_id, status) VALUES ($1, 'pending') RETURNING id",
    )
    .bind(opportunity_id)
    .fetch_one(db)
    .await?;

    use sqlx::Row;
    let id: i64 = row.try_get("id")?;
    Ok(id)
}

/// Record that the transaction was submitted: set tx_hash and status='sent'.
pub async fn update_arb_tx_sent(db: &PgPool, id: i64, tx_hash: &str) -> Result<()> {
    sqlx::query(
        "UPDATE arb_transactions \
         SET tx_hash = $1, status = 'sent', updated_at = NOW() \
         WHERE id = $2",
    )
    .bind(tx_hash)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a successfully mined transaction: set status='success', block_number, gas_used.
pub async fn update_arb_tx_success(db: &PgPool, id: i64, block: u64, gas_used: u64) -> Result<()> {
    sqlx::query(
        "UPDATE arb_transactions \
         SET status = 'success', block_number = $1, gas_used = $2, updated_at = NOW() \
         WHERE id = $3",
    )
    .bind(block as i64)
    .bind(gas_used as i64)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a failed or reverted transaction: set status and error_msg.
/// Pass `status` as "reverted" (on-chain failure) or "failed" (submission error).
pub async fn update_arb_tx_failed(db: &PgPool, id: i64, status: &str, err: &str) -> Result<()> {
    sqlx::query(
        "UPDATE arb_transactions \
         SET status = $1, error_msg = $2, updated_at = NOW() \
         WHERE id = $3",
    )
    .bind(status)
    .bind(err)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

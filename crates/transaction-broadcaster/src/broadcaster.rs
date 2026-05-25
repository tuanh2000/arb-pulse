use crate::abi::{executeArbitrageCall, Hop};
use crate::config::BroadcasterConfig;
use crate::db;
use crate::types::Opportunity;
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, TxHash, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;
use tracing::{info, warn};

/// Validate the opportunity and ABI-encode the contract calldata.
/// Pure — no I/O.
pub fn build_calldata(opp: &Opportunity, min_profit: U256) -> Result<Vec<u8>> {
    let token = Address::from_str(&opp.token_in).context("token_in")?;
    let amount = U256::from_str(&opp.amount_in).context("amount_in")?;
    let mut hops = Vec::with_capacity(opp.hops.len());
    for h in &opp.hops {
        hops.push(Hop {
            pair: Address::from_str(&h.pool).context("hop.pool")?,
            tokenIn: Address::from_str(&h.token_in).context("hop.token_in")?,
            feeBps: h.fee_bps as u16,
        });
    }
    Ok(executeArbitrageCall { token, amount, hops, minProfit: min_profit }.abi_encode())
}

/// eth_call pre-flight: returns true if the tx succeeds on current chain state.
/// A false return means the tx would revert — skip it before spending gas.
pub async fn simulate<P: Provider>(
    provider: &P,
    sender: Address,
    contract: Address,
    data: Vec<u8>,
) -> bool {
    let sim = TransactionRequest::default()
        .with_from(sender)
        .with_to(contract)
        .with_input(data);
    provider.call(sim).await.is_ok()
}

/// Build, sign, and submit the tx. Returns the tx hash immediately — does NOT
/// wait for the receipt. The caller handles receipt polling separately so the
/// send loop is never blocked on inclusion.
pub async fn submit<P: Provider>(
    provider: &P,
    cfg: &BroadcasterConfig,
    contract: Address,
    chain_id: u64,
    data: Vec<u8>,
) -> Result<TxHash> {
    let est = provider
        .estimate_eip1559_fees()
        .await
        .context("estimate_eip1559_fees")?;
    let priority_floor = (cfg.priority_fee_gwei * 1e9) as u128;
    let priority = est.max_priority_fee_per_gas.max(priority_floor);
    let cap = (cfg.max_fee_gwei * 1e9) as u128;
    let max_fee = (est.max_fee_per_gas + priority).min(cap).max(priority);

    let tx = TransactionRequest::default()
        .with_to(contract)
        .with_input(data)
        .with_chain_id(chain_id)
        .with_gas_limit(cfg.gas_limit)
        .with_max_priority_fee_per_gas(priority)
        .with_max_fee_per_gas(max_fee);

    let pending = provider.send_transaction(tx).await.context("send_transaction")?;
    Ok(*pending.tx_hash())
}

/// Poll for a receipt in a background task. Calls `get_transaction_receipt`
/// every 3 s until mined or `timeout_secs` elapses, then updates the DB.
/// Designed to be spawned with `tokio::spawn` — takes owned values.
pub async fn poll_receipt<P>(
    provider: P,
    tx_hash: TxHash,
    timeout_secs: u64,
    db: Option<PgPool>,
    arb_id: Option<i64>,
) where
    P: Provider + Send + Sync + 'static,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() >= deadline {
            warn!(
                tx = %tx_hash,
                timeout_secs,
                "receipt not found within timeout — tx still pending"
            );
            if let (Some(pool), Some(id)) = (&db, arb_id) {
                if let Err(e) =
                    db::update_arb_tx_failed(&pool, id, "timeout", "receipt timeout").await
                {
                    warn!(error = %e, "db: update to 'timeout' failed");
                }
            }
            return;
        }

        match provider.get_transaction_receipt(tx_hash).await {
            Ok(Some(receipt)) => {
                if receipt.status() {
                    info!(tx = %tx_hash, block = ?receipt.block_number, "arb tx mined OK");
                    if let (Some(pool), Some(id)) = (&db, arb_id) {
                        let block = receipt.block_number.unwrap_or(0);
                        let gas_used = receipt.gas_used;
                        if let Err(e) =
                            db::update_arb_tx_success(&pool, id, block, gas_used).await
                        {
                            warn!(error = %e, "db: update to 'success' failed");
                        }
                    }
                } else {
                    warn!(tx = %tx_hash, block = ?receipt.block_number, "arb tx reverted on-chain");
                    if let (Some(pool), Some(id)) = (&db, arb_id) {
                        if let Err(e) =
                            db::update_arb_tx_failed(&pool, id, "reverted", "on-chain revert")
                                .await
                        {
                            warn!(error = %e, "db: update to 'reverted' failed");
                        }
                    }
                }
                return;
            }
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(e) => {
                warn!(tx = %tx_hash, error = %e, "error polling receipt, retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

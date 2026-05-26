use crate::abi::{executeArbitrageCall, Hop};
use crate::config::BroadcasterConfig;
use crate::types::Opportunity;
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, TxHash, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use std::str::FromStr;

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

/// EIP-1559 fee pair, in wei.
#[derive(Clone, Copy)]
pub struct Fees {
    pub priority: u128,
    pub max_fee: u128,
}

/// Network fee estimate, floored by `priority_fee_gwei` and capped by `max_fee_gwei`.
/// Logged so an un-includable price (used_max_fee below the live base fee) or a
/// too-low tip for the priority-gas auction is visible.
pub async fn compute_fees<P: Provider>(provider: &P, cfg: &BroadcasterConfig) -> Result<Fees> {
    let est = provider
        .estimate_eip1559_fees()
        .await
        .context("estimate_eip1559_fees")?;
    let priority_floor = (cfg.priority_fee_gwei * 1e9) as u128;
    let priority = est.max_priority_fee_per_gas.max(priority_floor);
    let cap = (cfg.max_fee_gwei * 1e9) as u128;
    let max_fee = (est.max_fee_per_gas + priority).min(cap).max(priority);

    tracing::info!(
        est_max_fee_gwei = est.max_fee_per_gas as f64 / 1e9,
        est_priority_gwei = est.max_priority_fee_per_gas as f64 / 1e9,
        used_priority_gwei = priority as f64 / 1e9,
        used_max_fee_gwei = max_fee as f64 / 1e9,
        cap_gwei = cfg.max_fee_gwei,
        gas_limit = cfg.gas_limit,
        "computed gas fees"
    );
    Ok(Fees { priority, max_fee })
}

/// Increase a fee by 12.5% — comfortably above the 10% minimum a node requires to
/// accept a replacement tx at the same nonce.
pub fn bump_fee(old: u128) -> u128 {
    old + old / 8
}

/// True if the error means a tx already occupies this nonce in the mempool.
pub fn is_replacement_conflict(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("underpriced") || e.contains("already known") || e.contains("replacement")
}

/// True if the error means our nonce is behind the chain (a tx at it already mined).
pub fn is_nonce_too_low(err: &str) -> bool {
    err.to_ascii_lowercase().contains("nonce too low")
}

/// Send one tx at an explicit `nonce` with explicit `fees`. Returns its hash.
pub async fn send_tx<P: Provider>(
    provider: &P,
    contract: Address,
    chain_id: u64,
    nonce: u64,
    fees: Fees,
    gas_limit: u64,
    data: Vec<u8>,
) -> Result<TxHash> {
    let tx = TransactionRequest::default()
        .with_to(contract)
        .with_input(data)
        .with_chain_id(chain_id)
        .with_nonce(nonce)
        .with_gas_limit(gas_limit)
        .with_max_priority_fee_per_gas(fees.priority)
        .with_max_fee_per_gas(fees.max_fee);

    let pending = provider.send_transaction(tx).await.context("send_transaction")?;
    Ok(*pending.tx_hash())
}

/// Send at `nonce`, bumping the fee and retrying if the node reports a tx already
/// at that nonce ("replacement underpriced" / "already known"). This flushes a
/// pre-existing stuck tx occupying the nonce. Returns the hash and the fees that
/// were actually used (>= input). Non-conflict errors (nonce too low, insufficient
/// funds, ...) return immediately.
#[allow(clippy::too_many_arguments)]
pub async fn send_replacing<P: Provider>(
    provider: &P,
    contract: Address,
    chain_id: u64,
    nonce: u64,
    mut fees: Fees,
    gas_limit: u64,
    cap: u128,
    max_tries: u32,
    data: Vec<u8>,
) -> Result<(TxHash, Fees)> {
    for _ in 0..max_tries.max(1) {
        match send_tx(provider, contract, chain_id, nonce, fees, gas_limit, data.clone()).await {
            Ok(hash) => return Ok((hash, fees)),
            Err(e) => {
                let chain = format!("{e:#}");
                if !is_replacement_conflict(&chain) {
                    return Err(e);
                }
                let next_max = bump_fee(fees.max_fee).min(cap);
                let next_priority = bump_fee(fees.priority).min(next_max);
                // Can't achieve the required ≥10% bump within the cap — give up.
                if next_max < fees.max_fee + fees.max_fee / 10 {
                    return Err(anyhow::anyhow!(
                        "tx at nonce {nonce} stuck at fee cap; raise max_fee_gwei ({chain})"
                    ));
                }
                fees = Fees { priority: next_priority, max_fee: next_max };
            }
        }
    }
    Err(anyhow::anyhow!("exhausted replacement attempts at nonce {nonce}"))
}

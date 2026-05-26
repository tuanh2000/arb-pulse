use alloy::primitives::{Address, U256};
use serde::Deserialize;

/// Local mirror of one pool, parsed from the listener's Redis `pool:{addr}` hash.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub pair: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub fee_bps: u32,
    pub dex: String,
    pub block: u64,
}

impl PoolState {
    /// The other token in the pair, given one side. None if `t` isn't in the pair.
    pub fn other_token(&self, t: Address) -> Option<Address> {
        if t == self.token0 {
            Some(self.token1)
        } else if t == self.token1 {
            Some(self.token0)
        } else {
            None
        }
    }

    /// Reserves oriented for a swap whose input is `token_in`: (reserve_in, reserve_out).
    pub fn reserves_for(&self, token_in: Address) -> Option<(U256, U256)> {
        if token_in == self.token0 {
            Some((self.reserve0, self.reserve1))
        } else if token_in == self.token1 {
            Some((self.reserve1, self.reserve0))
        } else {
            None
        }
    }
}

/// One swap in a cycle: swap `token_in` -> `token_out` through `pool`.
#[derive(Debug, Clone)]
pub struct Hop {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
}

/// A cyclic path that starts and ends in the base token.
#[derive(Debug, Clone)]
pub struct Cycle {
    pub hops: Vec<Hop>,
}

/// Incremental reserve update published by the listener on `pool_updates`.
#[derive(Debug, Deserialize)]
pub struct PoolUpdate {
    pub address: String,
    pub reserve0: String,
    pub reserve1: String,
    pub block: u64,
    /// On-chain timestamp (s) of the block that produced this update. 0 = unknown.
    #[serde(default)]
    pub block_ts: u64,
}

/// Predicted reserve state for a pool after a pending mempool tx, published by the
/// listener on `pending_updates` (Phase 2). Evaluated against confirmed state without
/// mutating it.
#[derive(Debug, Deserialize)]
pub struct PendingUpdate {
    pub address: String,
    pub reserve0: String,
    pub reserve1: String,
    /// Block the prediction is anchored to; part of the wire contract but not used
    /// in evaluation (speculative opps are not deduped per-block).
    #[allow(dead_code)]
    pub block: u64,
    pub tx_hash: String,
}

/// A hop as reported in an emitted opportunity.
#[derive(Debug, Clone)]
pub struct OppHop {
    pub pool: Address,
    pub dex: String,
    pub fee_bps: u32,
    pub token_in: Address,
    pub token_out: Address,
}

/// A profitable arbitrage opportunity emitted to the Sender.
#[derive(Debug, Clone)]
pub struct Opportunity {
    pub token_in: Address,
    pub amount_in: U256,
    pub expected_out: U256,
    /// Amount that must be repaid to the loan provider (= amount_in for 0% loans).
    pub repay: U256,
    pub profit_raw: U256,
    pub profit_token_in: f64,
    pub net_profit_token_in: f64,
    /// Suggested on-chain revert floor: proceeds must clear at least this.
    pub min_out: U256,
    pub hops: Vec<OppHop>,
    pub block: u64,
}

impl Opportunity {
    pub fn to_json(&self) -> serde_json::Value {
        let hops: Vec<serde_json::Value> = self
            .hops
            .iter()
            .map(|h| {
                serde_json::json!({
                    "pool": format!("{:#x}", h.pool),
                    "dex": h.dex,
                    "fee_bps": h.fee_bps,
                    "token_in": format!("{:#x}", h.token_in),
                    "token_out": format!("{:#x}", h.token_out),
                })
            })
            .collect();
        serde_json::json!({
            "token_in": format!("{:#x}", self.token_in),
            "amount_in": self.amount_in.to_string(),
            "expected_out": self.expected_out.to_string(),
            "repay": self.repay.to_string(),
            "min_out": self.min_out.to_string(),
            "profit_raw": self.profit_raw.to_string(),
            "profit_token_in": self.profit_token_in,
            "net_profit_token_in": self.net_profit_token_in,
            "block": self.block,
            "hops": hops,
        })
    }
}

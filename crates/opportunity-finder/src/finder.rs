use crate::amm::{get_amount_out, u256_to_f64, VirtualPool};
use crate::store::PoolStore;
use crate::types::{Cycle, OppHop, Opportunity, PoolState};
use alloy::primitives::{Address, U256};
use std::collections::HashMap;

/// Read-only view of pool reserves: confirmed store plus an optional per-pool
/// overlay of predicted (reserve0, reserve1) used for speculative evaluation.
/// The overlay is consulted first; everything else falls back to the store, so
/// the confirmed `PoolStore` is never mutated.
pub struct ReserveView<'a> {
    store: &'a PoolStore,
    overrides: &'a HashMap<Address, (U256, U256)>,
}

impl<'a> ReserveView<'a> {
    pub fn confirmed(store: &'a PoolStore) -> Self {
        Self {
            store,
            overrides: EMPTY_OVERRIDES.get_or_init(HashMap::new),
        }
    }

    pub fn with_overrides(
        store: &'a PoolStore,
        overrides: &'a HashMap<Address, (U256, U256)>,
    ) -> Self {
        Self { store, overrides }
    }

    /// Pool state with any overridden reserves applied.
    fn get(&self, pair: &Address) -> Option<PoolState> {
        let mut pool = self.store.get(pair)?;
        if let Some(&(r0, r1)) = self.overrides.get(pair) {
            pool.reserve0 = r0;
            pool.reserve1 = r1;
        }
        Some(pool)
    }
}

static EMPTY_OVERRIDES: std::sync::OnceLock<HashMap<Address, (U256, U256)>> =
    std::sync::OnceLock::new();

/// Parameters shared across all cycle evaluations.
pub struct EvalParams {
    /// Repay factor c (1.0 for 0% loan / own capital).
    pub repay_factor: f64,
    /// Loan fee in bps used for the exact integer repay amount.
    pub loan_fee_bps: u32,
    /// Trade-size cap in raw token_in units. ZERO = unbounded.
    pub max_trade_in: U256,
    pub token_in_decimals: u8,
    /// Minimum net profit (token_in human units) to accept.
    pub min_profit: f64,
    /// Gas cost estimate (token_in human units) subtracted from profit.
    pub gas_cost: f64,
}

/// Evaluate one cycle against the confirmed store. Returns an Opportunity if it clears
/// the profit threshold after the loan fee and gas.
pub fn evaluate(cycle: &Cycle, store: &PoolStore, p: &EvalParams) -> Option<Opportunity> {
    evaluate_with_view(cycle, &ReserveView::confirmed(store), p)
}

/// Like `evaluate`, but reads reserves through a `ReserveView` so callers can supply
/// predicted overrides without mutating the confirmed store.
pub fn evaluate_with_view(cycle: &Cycle, view: &ReserveView, p: &EvalParams) -> Option<Opportunity> {
    if cycle.hops.is_empty() {
        return None;
    }

    // 1. Compose the path into a single fee-less virtual pool (f64) to size the trade.
    let mut vp: Option<VirtualPool> = None;
    for hop in &cycle.hops {
        let pool = view.get(&hop.pool)?;
        let (r_in, r_out) = pool.reserves_for(hop.token_in)?;
        let (ri, ro) = (u256_to_f64(r_in), u256_to_f64(r_out));
        if ri <= 0.0 || ro <= 0.0 {
            return None;
        }
        vp = Some(match vp {
            None => VirtualPool::first(ri, ro, pool.fee_bps),
            Some(v) => v.extend(ri, ro, pool.fee_bps),
        });
    }
    let x_opt = vp?.optimal_input(p.repay_factor)?;

    // 2. Clamp the optimal size to the trade cap.
    let max_in = u256_to_f64(p.max_trade_in);
    let x = if max_in > 0.0 { x_opt.min(max_in) } else { x_opt };
    if x < 1.0 {
        return None;
    }
    let amount_in = U256::from(x as u128);

    // 3. Exact integer simulation across the real path (this is what the chain sees).
    let mut amount = amount_in;
    for hop in &cycle.hops {
        let pool = view.get(&hop.pool)?;
        let (r_in, r_out) = pool.reserves_for(hop.token_in)?;
        amount = get_amount_out(amount, r_in, r_out, pool.fee_bps);
        if amount.is_zero() {
            return None;
        }
    }
    let gross_out = amount;

    // 4. Repay = amount_in * (10000 + loan_fee_bps) / 10000.
    let repay = amount_in * U256::from(10_000u32 + p.loan_fee_bps) / U256::from(10_000u32);
    if gross_out <= repay {
        return None;
    }
    let profit_raw = gross_out - repay;

    let scale = 10f64.powi(p.token_in_decimals as i32);
    let profit_human = u256_to_f64(profit_raw) / scale;
    let net = profit_human - p.gas_cost;
    if net < p.min_profit {
        return None;
    }

    let hops = cycle
        .hops
        .iter()
        .map(|h| {
            let pool = view.get(&h.pool);
            OppHop {
                pool: h.pool,
                dex: pool.as_ref().map(|p| p.dex.clone()).unwrap_or_default(),
                fee_bps: pool.as_ref().map(|p| p.fee_bps).unwrap_or(0),
                token_in: h.token_in,
                token_out: h.token_out,
            }
        })
        .collect();

    let block = cycle
        .hops
        .iter()
        .filter_map(|h| view.get(&h.pool).map(|p| p.block))
        .max()
        .unwrap_or(0);

    Some(Opportunity {
        token_in: cycle.hops[0].token_in,
        amount_in,
        expected_out: gross_out,
        repay,
        profit_raw,
        profit_token_in: profit_human,
        net_profit_token_in: net,
        min_out: repay,
        hops,
        block,
    })
}

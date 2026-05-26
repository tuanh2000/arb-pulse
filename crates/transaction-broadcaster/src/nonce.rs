//! Local nonce management + fee-bump/replace for the single-key sender.
//!
//! The node's auto-nonce jams the whole pipeline if one tx is un-includable: the
//! mined nonce stops advancing and every later tx queues behind the stuck one.
//! Here we assign nonces locally, track each in-flight tx by nonce, and a monitor
//! task either resolves it (mined -> receipt -> DB) or fee-bumps and resends it at
//! the same nonce until it lands — so a single under-priced tx can't freeze sends.

use crate::broadcaster::{self, Fees};
use crate::config::BroadcasterConfig;
use crate::db;
use alloy::primitives::{Address, TxHash};
use alloy::providers::Provider;
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// One submitted tx awaiting confirmation at a fixed nonce.
struct Inflight {
    hash: TxHash,
    data: Vec<u8>,
    fees: Fees,
    /// Chain tip when this version was (re)submitted; drives the bump timer.
    first_block: u64,
    /// Number of submissions for this nonce (1 = original, +1 per fee bump).
    attempts: u32,
    arb_id: Option<i64>,
}

struct Inner {
    next: u64,
    inflight: BTreeMap<u64, Inflight>,
}

#[derive(Clone)]
pub struct NonceManager {
    inner: Arc<Mutex<Inner>>,
}

impl NonceManager {
    pub fn new(start_nonce: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next: start_nonce,
                inflight: BTreeMap::new(),
            })),
        }
    }

    /// The nonce to assign to the next new tx. Does not advance — only a
    /// successful `commit` does, so a failed send never leaves a nonce gap.
    pub fn peek(&self) -> u64 {
        self.inner.lock().unwrap().next
    }

    /// Record a successful send and advance `next`.
    pub fn commit(
        &self,
        nonce: u64,
        hash: TxHash,
        data: Vec<u8>,
        fees: Fees,
        tip: u64,
        arb_id: Option<i64>,
    ) {
        let mut s = self.inner.lock().unwrap();
        if s.next <= nonce {
            s.next = nonce + 1;
        }
        s.inflight.insert(
            nonce,
            Inflight { hash, data, fees, first_block: tip, attempts: 1, arb_id },
        );
    }

    /// Pull `next` forward to the chain's mined count (after a "nonce too low").
    pub fn resync(&self, mined: u64) {
        let mut s = self.inner.lock().unwrap();
        if mined > s.next {
            s.next = mined;
        }
    }
}

/// Background task: every 3 s, confirm/fail or fee-bump in-flight txs by nonce.
/// - A nonce below the chain's mined count is resolved via its receipt and removed.
/// - A still-pending nonce older than `replace_after_blocks` is resent at a higher
///   fee (same nonce), up to `max_replacements` times, capped at `max_fee_gwei`.
#[allow(clippy::too_many_arguments)]
pub async fn run_monitor<P>(
    provider: P,
    cfg: BroadcasterConfig,
    contract: Address,
    chain_id: u64,
    sender: Address,
    nm: NonceManager,
    tip: Arc<AtomicU64>,
    db: Option<PgPool>,
) where
    P: Provider + Clone,
{
    let cap = (cfg.max_fee_gwei * 1e9) as u128;
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mined = match provider.get_transaction_count(sender).await {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "nonce monitor: get_transaction_count failed");
                continue;
            }
        };
        nm.resync(mined);
        let now_tip = tip.load(Ordering::Relaxed);

        // Snapshot the work to do, then act without holding the lock across awaits.
        #[allow(clippy::type_complexity)]
        let (confirmed, stale): (
            Vec<(u64, TxHash, Option<i64>)>,
            Vec<(u64, Vec<u8>, Fees, u32, Option<i64>)>,
        ) = {
            let s = nm.inner.lock().unwrap();
            let mut confirmed = Vec::new();
            let mut stale = Vec::new();
            for (&nonce, inf) in s.inflight.iter() {
                if nonce < mined {
                    confirmed.push((nonce, inf.hash, inf.arb_id));
                } else if now_tip > 0
                    && now_tip.saturating_sub(inf.first_block) >= cfg.replace_after_blocks
                    && inf.attempts <= cfg.max_replacements
                {
                    stale.push((nonce, inf.data.clone(), inf.fees, inf.attempts, inf.arb_id));
                }
            }
            (confirmed, stale)
        };

        for (nonce, hash, arb_id) in confirmed {
            resolve_confirmed(&provider, &db, nonce, hash, arb_id).await;
            nm.inner.lock().unwrap().inflight.remove(&nonce);
        }

        for (nonce, data, fees, attempts, arb_id) in stale {
            let next_max = broadcaster::bump_fee(fees.max_fee).min(cap);
            let next_priority = broadcaster::bump_fee(fees.priority).min(next_max);
            if next_max < fees.max_fee + fees.max_fee / 10 {
                warn!(
                    nonce,
                    cap_gwei = cfg.max_fee_gwei,
                    "tx stuck at fee cap and not mined — raise max_fee_gwei to clear the nonce"
                );
                continue;
            }
            let new_fees = Fees { priority: next_priority, max_fee: next_max };
            match broadcaster::send_tx(&provider, contract, chain_id, nonce, new_fees, cfg.gas_limit, data)
                .await
            {
                Ok(new_hash) => {
                    info!(
                        nonce,
                        attempt = attempts + 1,
                        new_max_fee_gwei = new_fees.max_fee as f64 / 1e9,
                        new_priority_gwei = new_fees.priority as f64 / 1e9,
                        tx = %new_hash,
                        "fee-bumped pending tx (replaced at same nonce)"
                    );
                    {
                        let mut s = nm.inner.lock().unwrap();
                        if let Some(inf) = s.inflight.get_mut(&nonce) {
                            inf.hash = new_hash;
                            inf.fees = new_fees;
                            inf.attempts = attempts + 1;
                            inf.first_block = now_tip;
                        }
                    }
                    if let (Some(pool), Some(id)) = (&db, arb_id) {
                        let h = format!("{new_hash:?}");
                        if let Err(e) = db::update_arb_tx_sent(pool, id, &h).await {
                            warn!(error = %e, "db: update to 'sent' (bump) failed");
                        }
                    }
                }
                Err(e) => {
                    let chain = format!("{e:#}");
                    // "nonce too low" => it actually mined; next cycle resolves it.
                    if !broadcaster::is_nonce_too_low(&chain) {
                        warn!(nonce, error = %chain, "fee-bump resend failed");
                    }
                }
            }
        }
    }
}

/// Fetch the receipt for a mined nonce and record success/revert in the DB.
async fn resolve_confirmed<P: Provider>(
    provider: &P,
    db: &Option<PgPool>,
    nonce: u64,
    hash: TxHash,
    arb_id: Option<i64>,
) {
    match provider.get_transaction_receipt(hash).await {
        Ok(Some(r)) => {
            if r.status() {
                metrics::counter!("broadcaster_tx_total", "status" => "success").increment(1);
                metrics::histogram!("broadcaster_gas_used").record(r.gas_used as f64);
                info!(nonce, tx = %hash, block = ?r.block_number, "arb tx mined OK");
                if let (Some(pool), Some(id)) = (db, arb_id) {
                    if let Err(e) =
                        db::update_arb_tx_success(pool, id, r.block_number.unwrap_or(0), r.gas_used)
                            .await
                    {
                        warn!(error = %e, "db: success update failed");
                    }
                }
            } else {
                metrics::counter!("broadcaster_tx_total", "status" => "reverted").increment(1);
                metrics::histogram!("broadcaster_gas_used").record(r.gas_used as f64);
                warn!(nonce, tx = %hash, block = ?r.block_number, "arb tx reverted on-chain");
                if let (Some(pool), Some(id)) = (db, arb_id) {
                    if let Err(e) =
                        db::update_arb_tx_failed(pool, id, "reverted", "on-chain revert").await
                    {
                        warn!(error = %e, "db: reverted update failed");
                    }
                }
            }
        }
        Ok(None) => {
            // The nonce advanced but our hash isn't the tx that mined — a
            // replacement or an external tx took the slot.
            metrics::counter!("broadcaster_tx_total", "status" => "superseded").increment(1);
            warn!(nonce, tx = %hash, "nonce confirmed but our hash not found (superseded)");
            if let (Some(pool), Some(id)) = (db, arb_id) {
                if let Err(e) =
                    db::update_arb_tx_failed(pool, id, "superseded", "nonce taken by another tx")
                        .await
                {
                    warn!(error = %e, "db: superseded update failed");
                }
            }
        }
        Err(e) => {
            warn!(nonce, tx = %hash, error = %e, "receipt fetch failed for confirmed nonce");
        }
    }
}

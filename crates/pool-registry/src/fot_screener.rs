//! Fee-on-transfer / gas-heavy token screener.
//!
//! Detects reflection / tax / hook tokens on-chain so their pools are excluded
//! from `/pools` (which filters on `token_metadata.is_fot`). Works entirely via
//! `eth_call` with STATE OVERRIDES against the public RPC: it injects the
//! `FotDetector` runtime bytecode at a scratch address and funds that address
//! with the pair's base token by overriding the base token's `balanceOf` slot,
//! then asks the detector to pull a slice of the token under test out of the
//! pair. A clean ERC-20 delivers exactly what was requested; a fee-on-transfer
//! token delivers less. No real deployment, no funds at risk, no writes on-chain.

use crate::config::{FotBase, FotScreenerConfig};
use crate::db::{self, ScreenCandidate};
use alloy::{
    primitives::{keccak256, Address, Bytes, B256, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{
        state::{AccountOverride, StateOverride},
        TransactionInput, TransactionRequest,
    },
    sol,
    sol_types::SolCall,
};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

sol! {
    #[allow(missing_docs)]
    interface IFotDetector {
        function measure(address pair, address base, address tokenOut) external returns (uint256 requested, uint256 received);
    }
}

/// Deployed runtime bytecode of `contracts/src/FotDetector.sol`
/// (`contracts/out/FotDetector.sol/FotDetector.json` `.deployedBytecode.object`).
/// Embedded as a const so the worker has no runtime file dependency.
const FOT_DETECTOR_DEPLOYED_BYTECODE: &str = "0x608060405234801561000f575f80fd5b5060043610610029575f3560e01c8063ea7dcd9d1461002d575b5f80fd5b61004061003b366004610368565b610059565b6040805192835260208301919091520160405180910390f35b5f805f80866001600160a01b0316630902f1ac6040518163ffffffff1660e01b8152600401606060405180830381865afa158015610099573d5f803e3d5ffd5b505050506040513d601f19601f820116820180604052508101906100bd91906103d2565b50915091505f856001600160a01b0316886001600160a01b0316630dfe16816040518163ffffffff1660e01b8152600401602060405180830381865afa158015610109573d5f803e3d5ffd5b505050506040513d601f19601f8201168201806040525081019061012d9190610413565b6001600160a01b03161490505f816101455782610147565b835b6dffffffffffffffffffffffffffff1690505f826101655784610167565b835b6dffffffffffffffffffffffffffff1690505f6101866103e884610435565b9050805f03610193575060015b6040517fa9059cbb0000000000000000000000000000000000000000000000000000000081526001600160a01b038c81166004830152602482018490528b169063a9059cbb906044016020604051808303815f875af11580156101f8573d5f803e3d5ffd5b505050506040513d601f19601f8201168201806040525081019061021c919061046d565b505f808561022b575f8361022e565b825f5b6040517f022c0d9f0000000000000000000000000000000000000000000000000000000081526004810183905260248101829052306044820152608060648201525f608482015291935091506001600160a01b038e169063022c0d9f9060a4015f604051808303815f87803b1580156102a5575f80fd5b505af11580156102b7573d5f803e3d5ffd5b50506040517f70a08231000000000000000000000000000000000000000000000000000000008152306004820152949b508b946001600160a01b038e1692506370a082319150602401602060405180830381865afa15801561031b573d5f803e3d5ffd5b505050506040513d601f19601f8201168201806040525081019061033f919061048c565b98505050505050505050935093915050565b6001600160a01b0381168114610365575f80fd5b50565b5f805f6060848603121561037a575f80fd5b833561038581610351565b9250602084013561039581610351565b915060408401356103a581610351565b809150509250925092565b80516dffffffffffffffffffffffffffff811681146103cd575f80fd5b919050565b5f805f606084860312156103e4575f80fd5b6103ed846103b0565b92506103fb602085016103b0565b9150604084015163ffffffff811681146103a5575f80fd5b5f60208284031215610423575f80fd5b815161042e81610351565b9392505050565b5f82610468577f4e487b71000000000000000000000000000000000000000000000000000000005f52601260045260245ffd5b500490565b5f6020828403121561047d575f80fd5b8151801515811461042e575f80fd5b5f6020828403121561049c575f80fd5b505191905056fea2646970667358221220ff0833c3c66c0a06352d5b23c673120e988f4bf8cbc6769ff091837e515d78dd64736f6c63430008180033";

/// Scratch address where the detector code is injected. Arbitrary; just needs to
/// be an address we don't otherwise touch.
const SCRATCH_ADDR: &str = "0x000000000000000000000000000000000000F0F0";

/// Generous gas cap for the probe `eth_call` (two external swaps + reads).
const PROBE_GAS_CAP: u64 = 30_000_000;

/// A FoT token delivers strictly less than requested; allow 0.1% slack so normal
/// integer-division rounding in the AMM math is not mistaken for a transfer tax.
fn is_short_delivery(requested: U256, received: U256) -> bool {
    received < requested * U256::from(999u64) / U256::from(1000u64)
}

/// Outcome of probing one candidate. `Indeterminate` means the `eth_call`
/// reverted; we do NOT mark such tokens so they are retried on a later cycle.
enum Probe {
    Clean,
    Fot { fee_bps: u16 },
    Indeterminate,
}

/// Storage slot for `balanceOf[holder]` of a Solidity `mapping(address=>uint)`
/// declared at slot `slot`: `keccak256(pad32(holder) ++ pad32(slot))`.
fn balance_slot_key(holder: Address, slot: u64) -> B256 {
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(holder.as_slice());
    buf[56..64].copy_from_slice(&slot.to_be_bytes());
    keccak256(buf)
}

/// Build the state override: detector code at the scratch address, plus a
/// `state_diff` on the base token giving the scratch address a near-infinite
/// balance so the detector can deposit the pair's whole base reserve.
fn build_overrides(scratch: Address, base: Address, balance_slot: u64) -> StateOverride {
    let code = Bytes::from_str(FOT_DETECTOR_DEPLOYED_BYTECODE).expect("valid detector bytecode");

    let mut ov = StateOverride::default();
    ov.insert(scratch, AccountOverride::default().with_code(code));

    let slot = balance_slot_key(scratch, balance_slot);
    ov.insert(
        base,
        AccountOverride::default().with_state_diff([(slot, B256::from(U256::MAX))]),
    );
    ov
}

/// Probe one (pair, base, tokenOut) triple. Returns the classification.
async fn probe<P: Provider>(
    provider: &P,
    scratch: Address,
    pair: Address,
    base: Address,
    token_out: Address,
    balance_slot: u64,
    gas_threshold: u64,
) -> Probe {
    let overrides = build_overrides(scratch, base, balance_slot);
    let calldata = IFotDetector::measureCall {
        pair,
        base,
        tokenOut: token_out,
    }
    .abi_encode();

    let tx = TransactionRequest::default()
        .from(scratch)
        .to(scratch)
        .gas_limit(PROBE_GAS_CAP)
        .input(TransactionInput::new(Bytes::copy_from_slice(&calldata)));

    let raw = match provider.call(tx.clone()).overrides(overrides.clone()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(token = %token_out, error = %e, "FoT probe reverted — leaving unscreened");
            return Probe::Indeterminate;
        }
    };

    let ret = match IFotDetector::measureCall::abi_decode_returns(raw.as_ref()) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(token = %token_out, error = %e, "FoT probe decode failed — leaving unscreened");
            return Probe::Indeterminate;
        }
    };

    if is_short_delivery(ret.requested, ret.received) {
        let fee_bps = if ret.requested > U256::ZERO {
            ((ret.requested - ret.received) * U256::from(10_000u64) / ret.requested)
                .try_into()
                .unwrap_or(10_000u16)
        } else {
            0u16
        };
        return Probe::Fot { fee_bps };
    }

    // Clean delivery: also flag gas-heavy / hook tokens by the probe's gas cost.
    match provider.estimate_gas(tx).overrides(overrides).await {
        Ok(gas) if gas > gas_threshold => {
            tracing::debug!(token = %token_out, gas, threshold = gas_threshold, "FoT screener: gas-heavy token");
            Probe::Fot { fee_bps: 0 }
        }
        Ok(_) => Probe::Clean,
        // Gas estimation failing on an otherwise-clean swap is unusual; treat as
        // indeterminate rather than mis-flagging a good token.
        Err(e) => {
            tracing::debug!(token = %token_out, error = %e, "FoT gas estimate failed — leaving unscreened");
            Probe::Indeterminate
        }
    }
}

/// Periodic worker. Every `interval_secs` it fetches a batch of unscreened tokens
/// (paired against a configured base) and screens each, marking the result.
/// Gated by a startup self-test: a misconfigured balance slot can't mass-mis-flag.
pub async fn run(pool: sqlx::PgPool, rpc_url: String, cfg: FotScreenerConfig) {
    let rpc = match rpc_url.parse::<alloy::transports::http::reqwest::Url>() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "Invalid RPC URL — FoT screener cannot start");
            return;
        }
    };
    let provider = ProviderBuilder::new().connect_http(rpc);

    let scratch: Address = match SCRATCH_ADDR.parse() {
        Ok(a) => a,
        Err(_) => {
            tracing::error!("Invalid scratch address — FoT screener cannot start");
            return;
        }
    };

    // Map each configured base address (lowercase) → its balance slot, so a
    // candidate's `base` column resolves to the slot we must override.
    let mut slot_by_base: HashMap<String, u64> = HashMap::new();
    for FotBase { address, balance_slot } in &cfg.bases {
        slot_by_base.insert(address.to_lowercase(), *balance_slot);
    }
    let bases: Vec<String> = slot_by_base.keys().cloned().collect();
    if bases.is_empty() {
        tracing::error!("FoT screener has no configured bases — disabling");
        return;
    }

    // ── Startup self-test ────────────────────────────────────────────────────
    // If a known-CLEAN token is configured, prove the override math is right
    // before touching any real candidate. A revert or short delivery here means
    // the balance slot is wrong; disable rather than risk mass-mis-flagging.
    if let Some(st) = &cfg.self_test {
        let parsed = (
            st.pool.parse::<Address>(),
            st.base.parse::<Address>(),
            st.token.parse::<Address>(),
        );
        match parsed {
            (Ok(pair), Ok(base), Ok(token)) => {
                let slot = match slot_by_base.get(&st.base.to_lowercase()) {
                    Some(s) => *s,
                    None => {
                        tracing::error!(
                            base = %st.base,
                            "FoT self-test base not in configured bases — disabling screener"
                        );
                        return;
                    }
                };
                // Self-test must NOT flag the known-clean token. Both a revert
                // (Indeterminate) and a Fot result indicate a broken override.
                match probe(&provider, scratch, pair, base, token, slot, cfg.gas_threshold).await {
                    Probe::Clean => {
                        tracing::info!(token = %st.token, "FoT screener self-test passed");
                    }
                    _ => {
                        tracing::error!(
                            token = %st.token,
                            "FoT screener self-test FAILED (clean token did not verify) — \
                             balance-slot override is likely wrong. DISABLING screener."
                        );
                        return;
                    }
                }
            }
            _ => {
                tracing::error!("FoT self-test has an invalid address — disabling screener");
                return;
            }
        }
    } else {
        tracing::warn!(
            "FoT screener running without a self_test — a wrong balance_slot could mis-flag good tokens"
        );
    }

    tracing::info!(
        bases = bases.len(),
        gas_threshold = cfg.gas_threshold,
        batch_size = cfg.batch_size,
        interval_secs = cfg.interval_secs,
        "FoT screener started"
    );

    loop {
        let candidates = match db::get_unscreened_base_pools(&pool, &bases, cfg.batch_size).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "FoT screener: failed to fetch candidates");
                tokio::time::sleep(Duration::from_secs(cfg.interval_secs)).await;
                continue;
            }
        };

        if candidates.is_empty() {
            tracing::info!(sleep_secs = cfg.interval_secs, "FoT screener: nothing to screen — sleeping");
            tokio::time::sleep(Duration::from_secs(cfg.interval_secs)).await;
            continue;
        }

        let (mut flagged, mut clean, mut skipped) = (0u64, 0u64, 0u64);
        for ScreenCandidate { token, pool_address, base } in &candidates {
            let slot = match slot_by_base.get(&base.to_lowercase()) {
                Some(s) => *s,
                None => {
                    skipped += 1;
                    continue;
                }
            };
            let (pair, base_a, token_a) = match (
                pool_address.parse::<Address>(),
                base.parse::<Address>(),
                token.parse::<Address>(),
            ) {
                (Ok(p), Ok(b), Ok(t)) => (p, b, t),
                _ => {
                    skipped += 1;
                    continue;
                }
            };

            let probe_result = probe(
                &provider, scratch, pair, base_a, token_a, slot, cfg.gas_threshold,
            )
            .await;

            let (is_fot, fee_bps) = match probe_result {
                Probe::Fot { fee_bps } => (true, Some(fee_bps)),
                Probe::Clean => (false, None),
                Probe::Indeterminate => {
                    skipped += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };

            if let Err(e) = db::mark_token_screened(&pool, token, is_fot, fee_bps).await {
                tracing::warn!(token = %token, error = %e, "FoT screener: mark_token_screened failed");
            } else {
                if is_fot {
                    if let Err(e) = db::mark_token_meme(&pool, token, true).await {
                        tracing::warn!(token = %token, error = %e, "FoT screener: mark_token_meme failed");
                    }
                    flagged += 1;
                } else {
                    clean += 1;
                }
            }

            // Be gentle on the RPC between probes.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Sync pool flags for tokens whose pool identity was resolved after their
        // initial screening (handles race between chain seeder and screener).
        match db::sync_pool_meme_flags(&pool).await {
            Ok(synced) if synced > 0 => {
                tracing::info!(synced, "FoT screener: synced stale pool meme flags");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "FoT screener: pool meme flag sync failed"),
        }

        tracing::info!(
            batch = candidates.len(),
            flagged,
            clean,
            skipped,
            "FoT screener batch complete"
        );
    }
}

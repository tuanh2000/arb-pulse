//! Mempool watcher (Phase 2, additive — default OFF).
//!
//! Subscribes to PENDING swap transactions and predicts the reserve change each
//! one will cause BEFORE it is mined, so the downstream finder can react a block
//! earlier. Predicted updates are published to a SEPARATE Redis channel
//! (`pending_updates`) tagged `predicted: true`; the confirmed Sync path is
//! untouched. This task is only spawned when `[mempool] enabled = true`.
//!
//! NOTE: PulseChain public nodes frequently reject pending-tx subscriptions.
//! Subscription setup is wrapped in graceful error handling: on rejection we log
//! a warning and return cleanly so the listener process keeps running.

use crate::config::AppConfig;
use crate::sink::RedisSink;
use crate::store::PoolStore;
use alloy::{
    consensus::Transaction as _,
    network::TransactionResponse,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder, WsConnect},
    sol,
    sol_types::SolCall,
};
use std::collections::HashMap;

sol! {
    #[allow(missing_docs)]
    interface IUniswapV2Router {
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
        function swapExactETHForTokens(uint256 amountOutMin, address[] path, address to, uint256 deadline) external payable returns (uint256[] amounts);
        function swapExactETHForTokensSupportingFeeOnTransferTokens(uint256 amountOutMin, address[] path, address to, uint256 deadline) external payable;
        function swapExactTokensForETH(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
        function swapExactTokensForETHSupportingFeeOnTransferTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external;
    }
}

/// What a decoded swap gives us: the input amount and the token hop path.
struct DecodedSwap {
    amount_in: U256,
    path: Vec<Address>,
}

/// router address -> DEX (name, fee_bps), from enabled config entries with a
/// non-empty router. Used to recognize a pending tx as a swap on a known DEX.
type RouterIndex = HashMap<Address, (String, u32)>;

/// (token_lo, token_hi) -> list of candidate pools for that unordered pair.
/// Multiple DEXes can share a pair, so we keep all candidates and disambiguate
/// by the router's DEX name at simulation time when possible.
type PairIndex = HashMap<(Address, Address), Vec<PoolRef>>;

#[derive(Clone)]
struct PoolRef {
    address: Address,
    token0: Address,
    fee_bps: u32,
    dex_name: String,
}

/// Build the router lookup from enabled DEX config entries (skip empty routers).
fn build_router_index(config: &AppConfig) -> RouterIndex {
    let mut idx = RouterIndex::new();
    for d in &config.dexes {
        if !d.enabled || d.router.trim().is_empty() {
            continue;
        }
        match d.router.parse::<Address>() {
            Ok(addr) => {
                idx.insert(addr, (d.name.clone(), d.fee_bps));
            }
            Err(e) => {
                tracing::warn!(dex = %d.name, router = %d.router, error = %e, "Skipping DEX with unparseable router");
            }
        }
    }
    idx
}

/// Build the unordered-pair -> pools index from the current pool set.
fn build_pair_index(store: &PoolStore) -> PairIndex {
    let mut idx = PairIndex::new();
    for p in store.get_all() {
        let key = pair_key(p.token0, p.token1);
        idx.entry(key).or_default().push(PoolRef {
            address: p.pair_address,
            token0: p.token0,
            fee_bps: p.fee_bps,
            dex_name: p.dex_name,
        });
    }
    idx
}

/// Order-independent key for a token pair.
fn pair_key(a: Address, b: Address) -> (Address, Address) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Decode a router swap calldata into (amount_in, path), matching on the 4-byte
/// selector. ETH-in variants take amount_in from tx.value. Returns None for any
/// selector we don't handle.
fn decode_swap(input: &[u8], value: U256) -> Option<DecodedSwap> {
    if input.len() < 4 {
        return None;
    }
    let sel: [u8; 4] = input[0..4].try_into().ok()?;

    // tokens-in variants: amount_in is the first arg.
    if sel == IUniswapV2Router::swapExactTokensForTokensCall::SELECTOR {
        let c = IUniswapV2Router::swapExactTokensForTokensCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: c.amountIn, path: c.path });
    }
    if sel == IUniswapV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::SELECTOR {
        let c = IUniswapV2Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: c.amountIn, path: c.path });
    }
    if sel == IUniswapV2Router::swapExactTokensForETHCall::SELECTOR {
        let c = IUniswapV2Router::swapExactTokensForETHCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: c.amountIn, path: c.path });
    }
    if sel == IUniswapV2Router::swapExactTokensForETHSupportingFeeOnTransferTokensCall::SELECTOR {
        let c = IUniswapV2Router::swapExactTokensForETHSupportingFeeOnTransferTokensCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: c.amountIn, path: c.path });
    }

    // ETH-in variants: amount_in = tx.value, path arg has no amountIn.
    if sel == IUniswapV2Router::swapExactETHForTokensCall::SELECTOR {
        let c = IUniswapV2Router::swapExactETHForTokensCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: value, path: c.path });
    }
    if sel == IUniswapV2Router::swapExactETHForTokensSupportingFeeOnTransferTokensCall::SELECTOR {
        let c = IUniswapV2Router::swapExactETHForTokensSupportingFeeOnTransferTokensCall::abi_decode(input).ok()?;
        return Some(DecodedSwap { amount_in: value, path: c.path });
    }

    None
}

/// Constant-product output with fee, all in U256 (same math as the finder's
/// amm.rs `get_amount_out`; reimplemented locally to avoid a cross-crate dep).
fn get_amount_out(amount_in: U256, reserve_in: U256, reserve_out: U256, fee_bps: u32) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::ZERO;
    }
    let fee_factor = U256::from(10_000u32 - fee_bps);
    let amount_in_with_fee = amount_in * fee_factor;
    let numerator = amount_in_with_fee * reserve_out;
    let denominator = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
    if denominator.is_zero() {
        return U256::ZERO;
    }
    numerator / denominator
}

/// Resolve the pool to use for a hop `(token_in, token_out)`, preferring one
/// whose DEX matches the router. Returns the pool plus its CURRENT reserves
/// from the store (predicted simulation reads live state each tx).
fn resolve_pool(
    pair_index: &PairIndex,
    store: &PoolStore,
    token_in: Address,
    token_out: Address,
    dex_name: &str,
) -> Option<(PoolRef, U256, U256)> {
    let candidates = pair_index.get(&pair_key(token_in, token_out))?;
    // Prefer the pool on the same DEX as the router; otherwise take the first.
    let chosen = candidates
        .iter()
        .find(|p| p.dex_name == dex_name)
        .or_else(|| candidates.first())?;
    let state = store.get(&chosen.address)?;
    Some((chosen.clone(), state.reserve0, state.reserve1))
}

/// A single pool's predicted reserves after applying one tx's hops.
struct PredictedReserves {
    address: Address,
    reserve0: U256,
    reserve1: U256,
}

/// Simulate a multi-hop swap over the path against CURRENT store reserves,
/// producing predicted reserve0/reserve1 for each affected pool. Reserves are
/// read fresh from the store per hop, so chained hops on distinct pools compose
/// correctly. We do not mutate the confirmed store.
fn simulate_swap(
    swap: &DecodedSwap,
    pair_index: &PairIndex,
    store: &PoolStore,
    dex_name: &str,
) -> Vec<PredictedReserves> {
    let mut out = Vec::new();
    let mut amount_in = swap.amount_in;

    for window in swap.path.windows(2) {
        let token_in = window[0];
        let token_out = window[1];

        let Some((pool, reserve0, reserve1)) =
            resolve_pool(pair_index, store, token_in, token_out, dex_name)
        else {
            // Path leaves our tracked pool set — stop, we can't predict further.
            break;
        };

        // Orient reserves to the swap direction by token ordering.
        let (reserve_in, reserve_out, in_is_token0) = if token_in == pool.token0 {
            (reserve0, reserve1, true)
        } else {
            (reserve1, reserve0, false)
        };

        let amount_out = get_amount_out(amount_in, reserve_in, reserve_out, pool.fee_bps);
        if amount_out.is_zero() {
            break;
        }

        let reserve_in_new = reserve_in + amount_in;
        let reserve_out_new = reserve_out.saturating_sub(amount_out);

        let (r0, r1) = if in_is_token0 {
            (reserve_in_new, reserve_out_new)
        } else {
            (reserve_out_new, reserve_in_new)
        };

        out.push(PredictedReserves { address: pool.address, reserve0: r0, reserve1: r1 });

        // Chain the output as the next hop's input.
        amount_in = amount_out;
    }

    out
}

async fn run_inner(config: &AppConfig, store: &PoolStore, sink: &RedisSink) -> anyhow::Result<()> {
    let ws = WsConnect::new(config.network.rpc_ws.clone());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;

    let router_index = build_router_index(config);
    if router_index.is_empty() {
        tracing::warn!("Mempool watcher: no enabled DEX has a router configured — nothing to watch");
        return Ok(());
    }
    let pair_index = build_pair_index(store);
    tracing::info!(
        routers = router_index.len(),
        pairs = pair_index.len(),
        "Mempool watcher indexes built"
    );

    // Try the full-body subscription first; PulseChain public nodes may reject it.
    // On rejection, log a WARNING and return cleanly (never panic/kill the process).
    let sub = match provider.subscribe_full_pending_transactions().await {
        Ok(sub) => sub,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Node rejected full pending-tx subscription; mempool watcher disabled for this run \
                 (public nodes often lack this — likely needs an owned node)"
            );
            return Ok(());
        }
    };
    let mut stream = sub.into_stream();
    tracing::info!("Mempool watcher: pending-tx subscription active");

    use futures::StreamExt;
    while let Some(tx) = stream.next().await {
        // `to` must be a known enabled router.
        let Some(to) = tx.to() else { continue };
        let Some((dex_name, _fee)) = router_index.get(&to) else { continue };
        let dex_name = dex_name.clone();

        let Some(swap) = decode_swap(tx.input(), tx.value()) else { continue };
        if swap.path.len() < 2 || swap.amount_in.is_zero() {
            continue;
        }

        let predicted = simulate_swap(&swap, &pair_index, store, &dex_name);
        if predicted.is_empty() {
            continue;
        }

        // Best-effort chain tip; 0 if unavailable.
        let block = provider.get_block_number().await.unwrap_or(0);
        let tx_hash = tx.tx_hash();

        for pr in predicted {
            if let Err(e) = sink
                .publish_pending_update(pr.address, pr.reserve0, pr.reserve1, block, tx_hash)
                .await
            {
                tracing::warn!(pool = %pr.address, error = %e, "Failed to publish predicted update");
            } else {
                tracing::debug!(
                    pool = %pr.address,
                    dex = %dex_name,
                    block,
                    tx = %tx_hash,
                    "Published predicted reserve update"
                );
            }
        }
    }

    Err(anyhow::anyhow!("Mempool pending-tx stream ended"))
}

/// Run the mempool watcher. Never panics; on any failure it logs and returns Ok
/// so it cannot bring down the listener (the confirmed path is independent).
pub async fn run(config: &AppConfig, store: PoolStore, sink: RedisSink) -> anyhow::Result<()> {
    if let Err(e) = run_inner(config, &store, &sink).await {
        tracing::warn!(error = %e, "Mempool watcher stopped (continuing — confirmed path unaffected)");
    }
    Ok(())
}

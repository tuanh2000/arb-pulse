use crate::db::PoolRecord;
use alloy::{
    primitives::{Address, Bytes, U256},
    providers::Provider,
    rpc::types::{TransactionInput, TransactionRequest},
    sol,
    sol_types::SolCall,
};
use anyhow::{anyhow, Result};

sol! {
    #[allow(missing_docs)]
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }
        struct McResult {
            bool success;
            bytes returnData;
        }
        function aggregate3(Call3[] calldata calls) external payable returns (McResult[] memory returnResults);
    }
}

sol! {
    #[allow(missing_docs)]
    interface IUniswapV2Pair {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }
}

sol! {
    #[allow(missing_docs)]
    interface IERC20 {
        function decimals() external view returns (uint8);
    }
}

const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

// 3 calls/pool × 60 = 180 calls per aggregate3 — well within RPC limits.
// Phase B adds 2 calls/pool × 60 = 120 more, both batches are safe.
const CHUNK_SIZE: usize = 60;

pub struct PoolOnchainState {
    pub token0: String,
    pub token1: String,
    pub reserve0: u128,
    pub reserve1: u128,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
}

/// Fetch on-chain state (token0, token1, reserves, decimals) for a batch of pools.
/// Returns one `Option<PoolOnchainState>` per input pool in the same order.
/// `None` means the pool's contract calls failed (e.g. not a valid pair address).
/// The outer `Result` fails only on a network-level RPC error.
pub async fn fetch_batch<P: Provider>(
    provider: &P,
    pools: &[&PoolRecord],
) -> Result<Vec<Option<PoolOnchainState>>> {
    let multicall3_addr: Address = MULTICALL3_ADDRESS
        .parse()
        .map_err(|_| anyhow!("Invalid Multicall3 address"))?;

    let token0_data = Bytes::copy_from_slice(&IUniswapV2Pair::token0Call {}.abi_encode());
    let token1_data = Bytes::copy_from_slice(&IUniswapV2Pair::token1Call {}.abi_encode());
    let reserves_data =
        Bytes::copy_from_slice(&IUniswapV2Pair::getReservesCall {}.abi_encode());
    let decimals_data = Bytes::copy_from_slice(&IERC20::decimalsCall {}.abi_encode());

    let mut output: Vec<Option<PoolOnchainState>> = (0..pools.len()).map(|_| None).collect();

    for (chunk_idx, chunk) in pools.chunks(CHUNK_SIZE).enumerate() {
        let global_offset = chunk_idx * CHUNK_SIZE;

        // ── Phase A: token0, token1, getReserves for every pool in chunk ─────
        let calls_a: Vec<IMulticall3::Call3> = chunk
            .iter()
            .flat_map(|r| {
                let addr: Address = r.pool_address.parse().unwrap_or(Address::ZERO);
                [
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: token0_data.clone(),
                    },
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: token1_data.clone(),
                    },
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: reserves_data.clone(),
                    },
                ]
            })
            .collect();

        let results_a = multicall(provider, multicall3_addr, calls_a).await?;

        // Decode token addresses for Phase B
        let mut token_pairs: Vec<(Address, Address)> = Vec::with_capacity(chunk.len());
        for i in 0..chunk.len() {
            let base = i * 3;
            let t0 = decode_addr(&results_a[base]).unwrap_or(Address::ZERO);
            let t1 = decode_addr(&results_a[base + 1]).unwrap_or(Address::ZERO);
            token_pairs.push((t0, t1));
        }

        // ── Phase B: decimals for each token pair ─────────────────────────────
        let calls_b: Vec<IMulticall3::Call3> = token_pairs
            .iter()
            .flat_map(|(t0, t1)| {
                [
                    IMulticall3::Call3 {
                        target: *t0,
                        allowFailure: true,
                        callData: decimals_data.clone(),
                    },
                    IMulticall3::Call3 {
                        target: *t1,
                        allowFailure: true,
                        callData: decimals_data.clone(),
                    },
                ]
            })
            .collect();

        let results_b = multicall(provider, multicall3_addr, calls_b).await?;

        // ── Assemble output ───────────────────────────────────────────────────
        for (local_i, _pool_record) in chunk.iter().enumerate() {
            let base_a = local_i * 3;
            let base_b = local_i * 2;

            let r_t0 = &results_a[base_a];
            let r_t1 = &results_a[base_a + 1];
            let r_res = &results_a[base_a + 2];

            // Skip pools where core calls failed or returned short data
            if !r_t0.success
                || !r_t1.success
                || !r_res.success
                || r_t0.returnData.len() < 32
                || r_t1.returnData.len() < 32
                || r_res.returnData.len() < 64
            {
                continue;
            }

            let (token0, token1) = token_pairs[local_i];
            // getReserves returns (uint112 r0, uint112 r1, uint32 ts) — each slot is 32 bytes
            let reserve0 = U256::from_be_slice(&r_res.returnData[0..32]).to::<u128>();
            let reserve1 = U256::from_be_slice(&r_res.returnData[32..64]).to::<u128>();

            // Default decimals to 18 if the call failed
            let dec0 = decode_decimals(&results_b[base_b]).unwrap_or(18);
            let dec1 = decode_decimals(&results_b[base_b + 1]).unwrap_or(18);

            output[global_offset + local_i] = Some(PoolOnchainState {
                token0: format!("{:?}", token0),
                token1: format!("{:?}", token1),
                reserve0,
                reserve1,
                token0_decimals: dec0,
                token1_decimals: dec1,
            });
        }
    }

    Ok(output)
}

// getReserves is a single call/pool, so we can pack more pools per aggregate3.
const RESERVES_CHUNK: usize = 150;

/// Fetch only (reserve0, reserve1) for a list of pool addresses via Multicall3.
/// Used by the price oracle, which already knows token sides/decimals from the DB
/// and so doesn't need the heavier token0/token1/decimals calls.
/// Returns one `Option<(u128, u128)>` per input pool, in order. `None` = call failed.
pub async fn fetch_reserves_only<P: Provider>(
    provider: &P,
    pools: &[String],
) -> Result<Vec<Option<(u128, u128)>>> {
    let multicall3_addr: Address = MULTICALL3_ADDRESS
        .parse()
        .map_err(|_| anyhow!("Invalid Multicall3 address"))?;

    let reserves_data =
        Bytes::copy_from_slice(&IUniswapV2Pair::getReservesCall {}.abi_encode());

    let mut output: Vec<Option<(u128, u128)>> = (0..pools.len()).map(|_| None).collect();

    for (chunk_idx, chunk) in pools.chunks(RESERVES_CHUNK).enumerate() {
        let global_offset = chunk_idx * RESERVES_CHUNK;

        let calls: Vec<IMulticall3::Call3> = chunk
            .iter()
            .map(|addr_str| {
                let addr: Address = addr_str.parse().unwrap_or(Address::ZERO);
                IMulticall3::Call3 {
                    target: addr,
                    allowFailure: true,
                    callData: reserves_data.clone(),
                }
            })
            .collect();

        let results = multicall(provider, multicall3_addr, calls).await?;

        for (local_i, r) in results.iter().enumerate() {
            if r.success && r.returnData.len() >= 64 {
                let reserve0 = U256::from_be_slice(&r.returnData[0..32]).to::<u128>();
                let reserve1 = U256::from_be_slice(&r.returnData[32..64]).to::<u128>();
                output[global_offset + local_i] = Some((reserve0, reserve1));
            }
        }
    }

    Ok(output)
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn decode_addr(r: &IMulticall3::McResult) -> Option<Address> {
    if r.success && r.returnData.len() >= 32 {
        Some(Address::from_slice(&r.returnData[12..32]))
    } else {
        None
    }
}

fn decode_decimals(r: &IMulticall3::McResult) -> Option<u8> {
    if r.success && r.returnData.len() >= 32 {
        let d = r.returnData[31];
        // Treat 0 as a failed/malformed response — fall back to the 18-decimal default.
        // A successful call returning 0 almost always means a non-standard token encoding,
        // not a genuine 0-decimal token; using it would leave reserves unscaled (×10^18 error).
        if d == 0 { None } else { Some(d) }
    } else {
        None
    }
}

async fn multicall<P: Provider>(
    provider: &P,
    multicall3: Address,
    calls: Vec<IMulticall3::Call3>,
) -> Result<Vec<IMulticall3::McResult>> {
    let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();
    let tx = TransactionRequest::default()
        .to(multicall3)
        .input(TransactionInput::new(Bytes::copy_from_slice(&calldata)));

    let raw = provider
        .call(tx)
        .await
        .map_err(|e| anyhow!("Multicall3 failed: {}", e))?;

    IMulticall3::aggregate3Call::abi_decode_returns(raw.as_ref())
        .map_err(|e| anyhow!("Multicall3 decode failed: {}", e))
}

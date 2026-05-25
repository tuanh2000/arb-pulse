//! Uniswap V2 (constant-product) protocol. Covers PulseX V1/V2, 9inch V2, 9mm V2,
//! and any other `x*y=k` fork: state is `token0/token1/getReserves/decimals`, and a
//! `Sync(uint112 reserve0, uint112 reserve1)` event signals every reserve change.

use super::{Protocol, ReserveUpdate};
use crate::config::{AppConfig, DexType};
use crate::registry_client::RegistryPool;
use crate::types::PoolState;
use alloy::{
    primitives::{Address, Bytes, B256, U256},
    providers::{DynProvider, Provider},
    rpc::types::{Log, TransactionInput, TransactionRequest},
    sol,
    sol_types::SolCall,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashSet;
use std::time::Instant;

sol! {
    #[allow(missing_docs)]
    interface IMulticall3 {
        struct Call3 { address target; bool allowFailure; bytes callData; }
        struct McResult { bool success; bytes returnData; }
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
    interface IERC20 { function decimals() external view returns (uint8); }
}

const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
// 3 calls/pool in the first multicall (token0, token1, reserves) + 2 in the
// follow-up (dec0, dec1) — comfortably within RPC limits.
const INFO_CHUNK: usize = 60;
/// keccak256("Sync(uint112,uint112)")
const SYNC_TOPIC: &str = "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1";

pub struct UniswapV2;

impl UniswapV2 {
    pub fn new() -> Self {
        UniswapV2
    }
}

#[async_trait]
impl Protocol for UniswapV2 {
    fn dex_type(&self) -> DexType {
        DexType::UniswapV2
    }

    fn state_change_topics(&self) -> Vec<B256> {
        vec![SYNC_TOPIC.parse().expect("valid Sync topic")]
    }

    fn decode_update(&self, log: &Log) -> Option<ReserveUpdate> {
        let data = log.data().data.as_ref();
        if data.len() < 64 {
            tracing::warn!(pair = %log.address(), "Sync log data too short ({} bytes), skipping", data.len());
            return None;
        }
        Some(ReserveUpdate {
            address: log.address(),
            reserve0: U256::from_be_slice(&data[0..32]),
            reserve1: U256::from_be_slice(&data[32..64]),
            block: log.block_number.unwrap_or(0),
        })
    }

    async fn fetch_states(
        &self,
        provider: &DynProvider,
        config: &AppConfig,
        pools: &[RegistryPool],
        block: u64,
    ) -> Result<Vec<PoolState>> {
        if pools.is_empty() {
            return Ok(Vec::new());
        }

        let multicall3_addr: Address = MULTICALL3_ADDRESS
            .parse()
            .map_err(|_| anyhow!("Invalid Multicall3 address"))?;

        let token0_data = Bytes::copy_from_slice(&IUniswapV2Pair::token0Call {}.abi_encode());
        let token1_data = Bytes::copy_from_slice(&IUniswapV2Pair::token1Call {}.abi_encode());
        let reserves_data = Bytes::copy_from_slice(&IUniswapV2Pair::getReservesCall {}.abi_encode());
        let decimals_data = Bytes::copy_from_slice(&IERC20::decimalsCall {}.abi_encode());

        let total_pools = pools.len();
        let total_info_chunks = total_pools.div_ceil(INFO_CHUNK);
        let mut states: Vec<PoolState> = Vec::with_capacity(total_pools);
        let mut loaded = 0usize;
        let mut skipped = 0usize;
        let mut warned_protocols: HashSet<String> = HashSet::new();
        let info_start = Instant::now();

        for (chunk_idx, chunk) in pools.chunks(INFO_CHUNK).enumerate() {
            // First multicall: token0, token1, getReserves (3 calls per pool).
            let mut calls: Vec<IMulticall3::Call3> = Vec::with_capacity(chunk.len() * 3);
            for pool in chunk {
                calls.push(IMulticall3::Call3 { target: pool.address, allowFailure: true, callData: token0_data.clone() });
                calls.push(IMulticall3::Call3 { target: pool.address, allowFailure: true, callData: token1_data.clone() });
                calls.push(IMulticall3::Call3 { target: pool.address, allowFailure: true, callData: reserves_data.clone() });
            }
            let results_a = multicall(provider, multicall3_addr, calls).await?;

            // Decode token addresses, then a follow-up decimals multicall.
            let mut calls_b: Vec<IMulticall3::Call3> = Vec::with_capacity(chunk.len() * 2);
            let mut chain_tokens: Vec<(Option<Address>, Option<Address>)> = Vec::with_capacity(chunk.len());
            for (i, pool) in chunk.iter().enumerate() {
                let base = i * 3;
                let chain_t0 = decode_addr(&results_a[base]);
                let chain_t1 = decode_addr(&results_a[base + 1]);
                chain_tokens.push((chain_t0, chain_t1));

                let dec_t0 = chain_t0.or(pool.token0).unwrap_or(Address::ZERO);
                let dec_t1 = chain_t1.or(pool.token1).unwrap_or(Address::ZERO);
                calls_b.push(IMulticall3::Call3 { target: dec_t0, allowFailure: true, callData: decimals_data.clone() });
                calls_b.push(IMulticall3::Call3 { target: dec_t1, allowFailure: true, callData: decimals_data.clone() });
            }
            let results_b = multicall(provider, multicall3_addr, calls_b).await?;

            // Assemble PoolState for each pool in the chunk.
            for (i, pool) in chunk.iter().enumerate() {
                let base_a = i * 3;
                let base_b = i * 2;
                let r_res = &results_a[base_a + 2];
                let (chain_t0, chain_t1) = chain_tokens[i];

                let token0 = match pool.token0.or(chain_t0) {
                    Some(a) => a,
                    None => { skipped += 1; continue; }
                };
                let token1 = match pool.token1.or(chain_t1) {
                    Some(a) => a,
                    None => { skipped += 1; continue; }
                };

                if !r_res.success || r_res.returnData.len() < 96 {
                    tracing::debug!(pair = %pool.address, "getReserves failed/short, skipping");
                    skipped += 1;
                    continue;
                }
                let reserve0 = U256::from_be_slice(&r_res.returnData[0..32]);
                let reserve1 = U256::from_be_slice(&r_res.returnData[32..64]);

                let token0_decimals = pool.token0_decimals.or_else(|| decode_decimals(&results_b[base_b])).unwrap_or(18);
                let token1_decimals = pool.token1_decimals.or_else(|| decode_decimals(&results_b[base_b + 1])).unwrap_or(18);

                let (dex_type, fee_bps) = match config.dex_for_protocol(&pool.protocol) {
                    Some(d) => (d.dex_type.clone(), d.fee_bps),
                    None => {
                        if warned_protocols.insert(pool.protocol.clone()) {
                            tracing::warn!(protocol = %pool.protocol, "Unknown protocol; defaulting to UniswapV2 / fee_bps=30");
                        }
                        (DexType::UniswapV2, 30)
                    }
                };

                states.push(PoolState {
                    pair_address: pool.address,
                    token0,
                    token1,
                    reserve0,
                    reserve1,
                    dex_name: pool.protocol.clone(),
                    dex_type,
                    fee_bps,
                    token0_decimals,
                    token1_decimals,
                    last_updated_block: block,
                });
                loaded += 1;
            }

            if (chunk_idx + 1) % 50 == 0 || chunk_idx + 1 == total_info_chunks {
                let processed = ((chunk_idx + 1) * INFO_CHUNK).min(total_pools);
                let pct = processed * 100 / total_pools;
                let elapsed = info_start.elapsed().as_secs_f32();
                let rate = processed as f32 / elapsed.max(0.001);
                let eta = (total_pools - processed) as f32 / rate.max(0.001);
                tracing::info!(
                    chunk = format!("{}/{}", chunk_idx + 1, total_info_chunks),
                    loaded, skipped, pct,
                    elapsed_s = format!("{:.1}", elapsed),
                    eta_s = format!("{:.0}", eta),
                    "UniswapV2: fetching pool info..."
                );
            }
        }

        tracing::info!(
            loaded, skipped,
            elapsed_s = format!("{:.1}", info_start.elapsed().as_secs_f32()),
            "UniswapV2 pool state fetch complete"
        );
        Ok(states)
    }
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
    if !r.success || r.returnData.len() < 32 {
        return None;
    }
    Some(r.returnData[31])
}

async fn multicall(
    provider: &DynProvider,
    multicall3: Address,
    calls: Vec<IMulticall3::Call3>,
) -> Result<Vec<IMulticall3::McResult>> {
    let calldata = IMulticall3::aggregate3Call { calls }.abi_encode();
    let tx = TransactionRequest::default()
        .to(multicall3)
        .input(TransactionInput::new(Bytes::copy_from_slice(&calldata)));
    let raw = provider.call(tx).await.map_err(|e| anyhow!("Multicall3 failed: {}", e))?;
    IMulticall3::aggregate3Call::abi_decode_returns(raw.as_ref())
        .map_err(|e| anyhow!("Multicall3 decode failed: {}", e))
}

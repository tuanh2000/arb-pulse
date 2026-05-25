use crate::config::{AppConfig, ChainDexConfig};
use crate::db::{self, PoolRecord};
use crate::reserve_fetcher;
use alloy::{
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{TransactionInput, TransactionRequest},
    sol,
    sol_types::SolCall,
};
use anyhow::{anyhow, Result};
use sqlx::PgPool;
use std::time::Instant;

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
    interface IUniswapV2Factory {
        function allPairsLength() external view returns (uint256);
        function allPairs(uint256 index) external view returns (address);
    }
}

const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
const ENUM_CHUNK: usize = 300;

/// Enumerate all pair addresses from the blockchain for all enabled DEXes.
/// Returns (pair_address_hex, dex_name) tuples ready for DB upsert.
/// Takes ~5 minutes for 180k pools, with no dependency on the listener.
pub async fn enumerate_all_pairs(cfg: &AppConfig) -> Result<Vec<(String, String)>> {
    let rpc_url = cfg
        .network
        .rpc_http
        .parse::<alloy::transports::http::reqwest::Url>()
        .map_err(|e| anyhow!("Invalid RPC URL: {}", e))?;

    let provider = ProviderBuilder::new().connect_http(rpc_url);

    let multicall3_addr: Address = MULTICALL3_ADDRESS
        .parse()
        .map_err(|_| anyhow!("Invalid Multicall3 address"))?;

    let mut all_pairs: Vec<(String, String)> = Vec::new();

    for dex in cfg.enabled_dexes() {
        match enumerate_dex(&provider, multicall3_addr, dex).await {
            Ok(pairs) => {
                tracing::info!(dex = %dex.name, count = pairs.len(), "DEX enumerated");
                all_pairs.extend(pairs);
            }
            Err(e) => {
                tracing::warn!(dex = %dex.name, error = %e, "Failed to enumerate DEX, skipping");
            }
        }
    }

    Ok(all_pairs)
}

const TOKEN_FILL_BATCH: usize = 600;

/// Populate token0/token1/decimals for every pool that still lacks them (a freshly
/// seeded set). Token identity is static, so doing it once here lets the price
/// oracle identify anchor pools immediately instead of waiting for the TVL worker's
/// round-robin. Pools whose on-chain calls fail are left NULL and retried later.
pub async fn populate_tokens(db_pool: &PgPool, cfg: &AppConfig) -> Result<()> {
    let pools = db::get_pools_missing_tokens(db_pool).await?;
    if pools.is_empty() {
        return Ok(());
    }

    let rpc_url = cfg
        .network
        .rpc_http
        .parse::<alloy::transports::http::reqwest::Url>()
        .map_err(|e| anyhow!("Invalid RPC URL: {}", e))?;
    let provider = ProviderBuilder::new().connect_http(rpc_url);

    let total = pools.len();
    let mut filled = 0u64;
    let start = Instant::now();
    tracing::info!(total, "Seed token-fill: fetching token0/token1/decimals on-chain");

    for (i, chunk) in pools.chunks(TOKEN_FILL_BATCH).enumerate() {
        let refs: Vec<&PoolRecord> = chunk.iter().collect();
        match reserve_fetcher::fetch_batch(&provider, &refs).await {
            Ok(states) => {
                for (rec, st) in chunk.iter().zip(states.iter()) {
                    if let Some(s) = st {
                        if let Err(e) = db::update_pool_tokens(
                            db_pool,
                            &rec.pool_address,
                            &s.token0,
                            &s.token1,
                            s.token0_decimals,
                            s.token1_decimals,
                        )
                        .await
                        {
                            tracing::warn!(pool = %rec.pool_address, error = %e, "update_pool_tokens failed");
                        } else {
                            filled += 1;
                        }
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "Token-fill batch failed (RPC) — skipping"),
        }

        if (i + 1) % 20 == 0 || (i + 1) * TOKEN_FILL_BATCH >= total {
            tracing::info!(
                filled,
                total,
                elapsed_s = format!("{:.0}", start.elapsed().as_secs_f32()),
                "Seed token-fill progress"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    tracing::info!(filled, total, "Seed token-fill complete");
    Ok(())
}

async fn enumerate_dex(
    provider: &impl Provider,
    multicall3_addr: Address,
    dex: &ChainDexConfig,
) -> Result<Vec<(String, String)>> {
    let factory_addr: Address = dex
        .factory
        .parse()
        .map_err(|e| anyhow!("Invalid factory for {}: {}", dex.name, e))?;

    // Step 1: allPairsLength()
    let length_calldata = IUniswapV2Factory::allPairsLengthCall {}.abi_encode();
    let length_tx = TransactionRequest::default()
        .to(factory_addr)
        .input(TransactionInput::new(Bytes::copy_from_slice(&length_calldata)));

    let length_raw = provider
        .call(length_tx)
        .await
        .map_err(|e| anyhow!("allPairsLength() for {}: {}", dex.name, e))?;

    let pair_count = U256::from_be_slice(length_raw.as_ref()).to::<u64>() as usize;
    let total_chunks = pair_count.div_ceil(ENUM_CHUNK);

    tracing::info!(
        dex = %dex.name,
        pairs = pair_count,
        chunks = total_chunks,
        "Chain seeder: enumerating pair addresses"
    );

    // Step 2: batch allPairs(i) via Multicall3
    let mut pair_addresses: Vec<Address> = Vec::with_capacity(pair_count);
    let start = Instant::now();

    for (chunk_idx, chunk_start) in (0..pair_count).step_by(ENUM_CHUNK).enumerate() {
        let chunk_end = (chunk_start + ENUM_CHUNK).min(pair_count);

        let calls: Vec<IMulticall3::Call3> = (chunk_start..chunk_end)
            .map(|i| IMulticall3::Call3 {
                target: factory_addr,
                allowFailure: false,
                callData: Bytes::copy_from_slice(
                    &IUniswapV2Factory::allPairsCall {
                        index: U256::from(i),
                    }
                    .abi_encode(),
                ),
            })
            .collect();

        let results = multicall(provider, multicall3_addr, calls).await?;

        for r in &results {
            if r.returnData.len() >= 32 {
                pair_addresses.push(Address::from_slice(&r.returnData[12..32]));
            }
        }

        if (chunk_idx + 1) % 50 == 0 || chunk_end == pair_count {
            let enumerated = pair_addresses.len();
            let pct = enumerated * 100 / pair_count.max(1);
            let elapsed = start.elapsed().as_secs_f32();
            let rate = enumerated as f32 / elapsed.max(0.001);
            let eta = (pair_count.saturating_sub(enumerated)) as f32 / rate;
            tracing::info!(
                dex = %dex.name,
                enumerated,
                total = pair_count,
                pct,
                elapsed_s = format!("{:.1}", elapsed),
                eta_s = format!("{:.0}", eta),
                "Chain seeder progress"
            );
        }
    }

    Ok(pair_addresses
        .into_iter()
        .map(|addr| (format!("{:?}", addr), dex.name.clone()))
        .collect())
}

async fn multicall(
    provider: &impl Provider,
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

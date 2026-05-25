use crate::db::{self, TokenMetadataInput};
use alloy::{
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::{TransactionInput, TransactionRequest},
    sol,
    sol_types::SolCall,
};
use anyhow::{anyhow, Result};
use sqlx::PgPool;
use std::time::Duration;

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
    interface IERC20Metadata {
        function symbol() external view returns (string);
        function name() external view returns (string);
        function decimals() external view returns (uint8);
    }
}

const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

// 3 calls/token × 60 = 180 calls per aggregate3 — well within RPC limits.
const CHUNK_SIZE: usize = 60;

pub struct TokenMeta {
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub decimals: Option<u8>,
}

/// Fetch symbol / name / decimals for a batch of token addresses via Multicall3.
/// Returns one `Option<TokenMeta>` per input in the same order. `None` means none
/// of the three calls returned usable data (likely not an ERC-20 contract).
pub async fn fetch_batch<P: Provider>(
    provider: &P,
    tokens: &[String],
) -> Result<Vec<Option<TokenMeta>>> {
    let multicall3_addr: Address = MULTICALL3_ADDRESS
        .parse()
        .map_err(|_| anyhow!("Invalid Multicall3 address"))?;

    let symbol_data = Bytes::copy_from_slice(&IERC20Metadata::symbolCall {}.abi_encode());
    let name_data = Bytes::copy_from_slice(&IERC20Metadata::nameCall {}.abi_encode());
    let decimals_data = Bytes::copy_from_slice(&IERC20Metadata::decimalsCall {}.abi_encode());

    let mut output: Vec<Option<TokenMeta>> = (0..tokens.len()).map(|_| None).collect();

    for (chunk_idx, chunk) in tokens.chunks(CHUNK_SIZE).enumerate() {
        let global_offset = chunk_idx * CHUNK_SIZE;

        let calls: Vec<IMulticall3::Call3> = chunk
            .iter()
            .flat_map(|addr_str| {
                let addr: Address = addr_str.parse().unwrap_or(Address::ZERO);
                [
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: symbol_data.clone(),
                    },
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: name_data.clone(),
                    },
                    IMulticall3::Call3 {
                        target: addr,
                        allowFailure: true,
                        callData: decimals_data.clone(),
                    },
                ]
            })
            .collect();

        let results = multicall(provider, multicall3_addr, calls).await?;

        for local_i in 0..chunk.len() {
            let base = local_i * 3;
            let symbol = decode_string(&results[base]);
            let name = decode_string(&results[base + 1]);
            let decimals = decode_decimals(&results[base + 2]);

            // Only record the token if at least one field resolved.
            if symbol.is_some() || name.is_some() || decimals.is_some() {
                output[global_offset + local_i] = Some(TokenMeta {
                    symbol,
                    name,
                    decimals,
                });
            }
        }
    }

    Ok(output)
}

/// Periodic worker: resolves metadata for tokens referenced by pools that are not
/// yet in `token_metadata`, in batches, until none remain, then idles.
pub async fn run(pool: PgPool, rpc_url: String, batch_size: i64, idle_sleep_secs: u64) {
    let rpc_url_parsed = match rpc_url.parse::<alloy::transports::http::reqwest::Url>() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "Invalid RPC URL — token-metadata worker cannot start");
            return;
        }
    };
    let provider = ProviderBuilder::new().connect_http(rpc_url_parsed);

    loop {
        let tokens = match db::get_tokens_missing_metadata(&pool, batch_size).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "Failed to query tokens missing metadata");
                tokio::time::sleep(Duration::from_secs(idle_sleep_secs)).await;
                continue;
            }
        };

        if tokens.is_empty() {
            tracing::info!(idle_secs = idle_sleep_secs, "No tokens need metadata — sleeping");
            tokio::time::sleep(Duration::from_secs(idle_sleep_secs)).await;
            continue;
        }

        let metas = match fetch_batch(&provider, &tokens).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, batch = tokens.len(), "Metadata Multicall3 batch failed");
                tokio::time::sleep(Duration::from_secs(idle_sleep_secs)).await;
                continue;
            }
        };

        // Always write a row per token (even when fields are null) so the worker
        // does not re-query the same dead/non-ERC20 address on every cycle.
        let inputs: Vec<TokenMetadataInput> = tokens
            .iter()
            .zip(metas.iter())
            .map(|(addr, meta)| match meta {
                Some(m) => TokenMetadataInput {
                    token_address: addr.clone(),
                    symbol: m.symbol.clone(),
                    name: m.name.clone(),
                    decimals: m.decimals,
                },
                None => TokenMetadataInput {
                    token_address: addr.clone(),
                    symbol: None,
                    name: None,
                    decimals: None,
                },
            })
            .collect();

        let resolved = metas.iter().filter(|m| m.is_some()).count();
        match db::upsert_token_metadata(&pool, &inputs).await {
            Ok(_) => tracing::info!(
                batch = tokens.len(),
                resolved,
                "Token-metadata batch complete"
            ),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to upsert token metadata");
                tokio::time::sleep(Duration::from_secs(idle_sleep_secs)).await;
            }
        }
    }
}

// ── decoding helpers ────────────────────────────────────────────────────────────

/// Decode an ERC-20 string field, handling both the standard `string` return and
/// the legacy `bytes32` return (e.g. MKR, early tokens).
fn decode_string(r: &IMulticall3::McResult) -> Option<String> {
    if !r.success {
        return None;
    }
    let data = r.returnData.as_ref();

    // Dynamic `string`: [offset(32)][length(32)][utf8 bytes...]
    if data.len() >= 64 {
        let offset = U256::from_be_slice(&data[0..32]).to::<usize>();
        if offset == 32 {
            let len = U256::from_be_slice(&data[32..64]).to::<usize>();
            if len > 0 && 64 + len <= data.len() {
                if let Ok(s) = std::str::from_utf8(&data[64..64 + len]) {
                    let t = s.trim_matches('\0').trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }

    // Legacy `bytes32`: left-aligned, zero-padded.
    if data.len() == 32 {
        let end = data.iter().position(|&b| b == 0).unwrap_or(32);
        if let Ok(s) = std::str::from_utf8(&data[..end]) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }

    None
}

fn decode_decimals(r: &IMulticall3::McResult) -> Option<u8> {
    if r.success && r.returnData.len() >= 32 {
        let d = r.returnData[31];
        if d == 0 {
            None
        } else {
            Some(d)
        }
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

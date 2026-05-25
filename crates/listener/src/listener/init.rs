//! Initial pool-state load. Dispatches each pool to its protocol's `fetch_states`
//! based on the configured `dex_type` (unknown protocols default to UniswapV2).

use crate::config::{AppConfig, DexType};
use crate::protocols::ProtocolRegistry;
use crate::registry_client::RegistryPool;
use crate::types::PoolState;
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Fetch current state for the given curated pools, grouped and dispatched by
/// protocol. Returns the successfully-loaded states across all protocols.
pub async fn fetch_states_for_pools(
    config: &AppConfig,
    pools: &[RegistryPool],
) -> Result<Vec<PoolState>> {
    if pools.is_empty() {
        tracing::info!("No pools provided; nothing to fetch");
        return Ok(Vec::new());
    }

    let rpc_url = config
        .network
        .rpc_http
        .parse::<alloy::transports::http::reqwest::Url>()
        .map_err(|e| anyhow!("Invalid HTTP RPC URL: {}", e))?;
    let provider = ProviderBuilder::new().connect_http(rpc_url).erased();

    let current_block = provider.get_block_number().await?;
    tracing::info!(block = current_block, "Connected to chain");

    let registry = ProtocolRegistry::new();

    // Group pools by resolved dex_type; unknown protocols default to UniswapV2.
    let mut by_type: HashMap<DexType, Vec<RegistryPool>> = HashMap::new();
    for pool in pools {
        let dt = config
            .dex_for_protocol(&pool.protocol)
            .map(|d| d.dex_type.clone())
            .unwrap_or(DexType::UniswapV2);
        by_type.entry(dt).or_default().push(pool.clone());
    }

    let mut states = Vec::with_capacity(pools.len());
    for (dex_type, group) in &by_type {
        match registry.get(dex_type) {
            Some(proto) => {
                let fetched = proto
                    .fetch_states(&provider, config, group, current_block)
                    .await?;
                states.extend(fetched);
            }
            None => tracing::warn!(
                dex_type = ?dex_type,
                count = group.len(),
                "No protocol implementation for dex_type; skipping these pools"
            ),
        }
    }

    tracing::info!(
        requested = pools.len(),
        loaded = states.len(),
        "Pool state fetch complete (all protocols)"
    );
    Ok(states)
}

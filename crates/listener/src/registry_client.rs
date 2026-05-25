//! Client for the `pool-registry` HTTP service.
//!
//! Fetches the curated set of valid-TVL liquidity pools that the Listener
//! tracks, mapping the registry's JSON representation into strongly-typed
//! [`RegistryPool`] values.

use alloy::primitives::Address;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct RegistryPool {
    pub address: Address,
    pub protocol: String,
    pub token0: Option<Address>,
    pub token1: Option<Address>,
    pub token0_decimals: Option<u8>,
    pub token1_decimals: Option<u8>,
}

/// Raw pool row as returned by `GET /pools`. Fields not needed by the Listener
/// (e.g. `tvl`, `updated_at`) are simply omitted — serde ignores them.
#[derive(Debug, Deserialize)]
struct RawPool {
    pool_address: String,
    protocol: String,
    token0: Option<String>,
    token1: Option<String>,
    token0_decimals: Option<i64>,
    token1_decimals: Option<i64>,
}

pub struct RegistryClient {
    base_url: String,
    client: reqwest::Client,
}

impl RegistryClient {
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client");
        Self { base_url, client }
    }

    /// Fetch the curated pool list from `GET /pools?min_tvl=<min_tvl>`.
    pub async fn load_pools(&self, min_tvl: f64) -> Result<Vec<RegistryPool>> {
        let url = format!("{}/pools?min_tvl={}", self.base_url, min_tvl);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("GET /pools failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "pool-registry returned HTTP {} for /pools?min_tvl={}",
                resp.status(),
                min_tvl
            ));
        }

        let rows: Vec<RawPool> = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse /pools response: {}", e))?;

        let pools: Vec<RegistryPool> = rows
            .into_iter()
            .filter_map(|row| {
                let address = match row.pool_address.parse::<Address>() {
                    Ok(addr) => addr,
                    Err(e) => {
                        tracing::warn!(
                            pool_address = %row.pool_address,
                            error = %e,
                            "Skipping pool with unparseable address"
                        );
                        return None;
                    }
                };

                Some(RegistryPool {
                    address,
                    protocol: row.protocol,
                    token0: parse_token(row.token0),
                    token1: parse_token(row.token1),
                    token0_decimals: to_u8(row.token0_decimals),
                    token1_decimals: to_u8(row.token1_decimals),
                })
            })
            .collect();

        tracing::info!(count = pools.len(), "Loaded pools from registry");
        Ok(pools)
    }
}

/// Parse an optional token address; present-but-unparseable becomes `None`.
fn parse_token(value: Option<String>) -> Option<Address> {
    let value = value?;
    match value.parse::<Address>() {
        Ok(addr) => Some(addr),
        Err(e) => {
            tracing::debug!(token = %value, error = %e, "Ignoring unparseable token address");
            None
        }
    }
}

/// Convert optional `i64` decimals into `u8`, dropping nulls and out-of-range values.
fn to_u8(value: Option<i64>) -> Option<u8> {
    value.and_then(|v| u8::try_from(v).ok())
}

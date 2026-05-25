use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub rpc_http: String,
    pub rpc_ws: String,
    pub chain_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub url: String,
    /// Minimum TVL (USD) a pool must have to be included in GET /pools responses.
    /// Applied when the caller does not pass an explicit ?min_tvl= query param.
    pub min_tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum DexType {
    UniswapV2,
    UniswapV3,
    StableSwap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexConfig {
    pub name: String,
    pub enabled: bool,
    pub factory: String,
    pub router: String,
    pub dex_type: DexType,
    pub fee_bps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub redis: RedisConfig,
    pub registry: RegistryConfig,
    pub api: ApiConfig,
    pub dexes: Vec<DexConfig>,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
        let contents = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Look up DEX config by protocol name (the `protocol` string returned by
    /// pool-registry maps to a DEX `name` here). Used to resolve dex_type/fee_bps
    /// for a curated pool.
    pub fn dex_for_protocol(&self, protocol: &str) -> Option<&DexConfig> {
        self.dexes.iter().find(|d| d.name == protocol)
    }
}

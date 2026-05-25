use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub rpc_http: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainDexConfig {
    pub name: String,
    pub enabled: bool,
    pub factory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub batch_size: i64,
    pub idle_sleep_secs: u64,
    /// Milliseconds to sleep between batches to avoid hammering the RPC.
    pub batch_delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorToken {
    pub address: String,
    pub symbol: String,
    pub hardcoded_price_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    pub min_tvl_usd: f64,
    pub anchor_tokens: Vec<AnchorToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceUpdaterConfig {
    /// How often the oracle re-prices all tokens (seconds).
    pub refresh_interval_secs: u64,
    /// Minimum USD on a pool's anchor side for its price to be trusted. Lower =
    /// more (smaller) tokens priced but noisier; higher = only deep pools.
    #[serde(default = "default_min_anchor_liquidity_usd")]
    pub min_anchor_liquidity_usd: f64,
}

fn default_min_anchor_liquidity_usd() -> f64 {
    100.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub dexes: Vec<ChainDexConfig>,
    pub database: DatabaseConfig,
    pub api: ApiConfig,
    pub worker: WorkerConfig,
    pub filter: FilterConfig,
    pub price_updater: PriceUpdaterConfig,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = std::env::var("REGISTRY_CONFIG_PATH")
            .unwrap_or_else(|_| "pool-registry-config.toml".to_string());
        let contents = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read config '{}': {}", path, e))?;
        Ok(toml::from_str(&contents)?)
    }

    pub fn enabled_dexes(&self) -> Vec<&ChainDexConfig> {
        self.dexes.iter().filter(|d| d.enabled).collect()
    }
}

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
    /// Token addresses to hard-exclude (fee-on-transfer / gas-heavy / scam). Any
    /// pool containing one is dropped from /pools so it never reaches the listener.
    /// Seeded into `token_metadata.is_fot` at startup.
    #[serde(default)]
    pub denylist: Vec<String>,
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

/// A base token the screener can fund the detector with. `balance_slot` is the
/// declared storage-slot index of the token's `mapping(address=>uint) balanceOf`,
/// used to override the detector's balance via `eth_call` state overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FotBase {
    pub address: String,
    pub balance_slot: u64,
}

/// Known-CLEAN token + its pair + base. Probed at startup: if it fails to behave
/// like a clean token, the balance-slot override is wrong and the screener self-disables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FotSelfTest {
    pub token: String,
    pub pool: String,
    pub base: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FotScreenerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_fot_bases")]
    pub bases: Vec<FotBase>,
    #[serde(default = "default_fot_gas_threshold")]
    pub gas_threshold: u64,
    #[serde(default = "default_fot_batch_size")]
    pub batch_size: i64,
    #[serde(default = "default_fot_interval_secs")]
    pub interval_secs: u64,
    #[serde(default)]
    pub self_test: Option<FotSelfTest>,
}

impl Default for FotScreenerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bases: default_fot_bases(),
            gas_threshold: default_fot_gas_threshold(),
            batch_size: default_fot_batch_size(),
            interval_secs: default_fot_interval_secs(),
            self_test: None,
        }
    }
}

/// WPLS is a WETH9-style contract whose `balanceOf` mapping lives at slot 3.
fn default_fot_bases() -> Vec<FotBase> {
    vec![FotBase {
        address: "0xA1077a294dDE1B09bB078844df40758a5D0f9a27".to_string(),
        balance_slot: 3,
    }]
}
fn default_fot_gas_threshold() -> u64 {
    400_000
}
fn default_fot_batch_size() -> i64 {
    200
}
fn default_fot_interval_secs() -> u64 {
    300
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
    /// Absence in the TOML = disabled (serde default).
    #[serde(default)]
    pub fot_screener: FotScreenerConfig,
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

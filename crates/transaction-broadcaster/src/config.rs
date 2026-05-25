use alloy::primitives::{Address, U256};
use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::str::FromStr;

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    pub rpc_http: String,
    pub chain_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BroadcasterConfig {
    /// Deployed ArbExecutor address.
    pub contract: String,
    #[serde(default = "default_channel")]
    pub opportunities_channel: String,
    #[serde(default = "default_gas_limit")]
    pub gas_limit: u64,
    #[serde(default = "default_priority")]
    pub priority_fee_gwei: f64,
    #[serde(default = "default_max_fee")]
    pub max_fee_gwei: f64,
    #[serde(default = "default_age")]
    pub max_opportunity_age_blocks: u64,
    #[serde(default)]
    pub min_profit_raw: String,
    /// Max seconds to wait for a sent tx's receipt before abandoning the wait.
    /// Prevents one un-includable tx (e.g. a base-fee spike above `max_fee_gwei`)
    /// from stalling the single-in-flight broadcaster indefinitely.
    #[serde(default = "default_receipt_timeout")]
    pub receipt_timeout_secs: u64,
    /// Pre-send `eth_call` simulation. When true (default), an opportunity whose
    /// tx would revert on-chain (fee-on-transfer token, stale reserves, etc.) is
    /// skipped before any gas is spent. Costs one extra RPC round-trip per opp.
    #[serde(default = "default_simulate")]
    pub simulate: bool,
}

fn default_channel() -> String {
    "opportunities".to_string()
}
fn default_gas_limit() -> u64 {
    800_000
}
fn default_priority() -> f64 {
    5.0
}
fn default_max_fee() -> f64 {
    500_000.0
}
fn default_age() -> u64 {
    3
}
fn default_simulate() -> bool {
    true
}
fn default_receipt_timeout() -> u64 {
    45
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub redis: RedisConfig,
    pub broadcaster: BroadcasterConfig,
    pub database: Option<DatabaseConfig>,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
        let contents = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn contract_address(&self) -> Result<Address> {
        self.broadcaster
            .contract
            .parse::<Address>()
            .map_err(|e| anyhow::anyhow!("invalid broadcaster.contract '{}': {}", self.broadcaster.contract, e))
    }

    pub fn min_profit(&self) -> U256 {
        let s = self.broadcaster.min_profit_raw.trim();
        if s.is_empty() {
            return U256::ZERO;
        }
        U256::from_str(s).unwrap_or(U256::ZERO)
    }
}

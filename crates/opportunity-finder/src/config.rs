use alloy::primitives::{Address, U256};
use anyhow::Result;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FinderConfig {
    /// Base token every cycle starts and ends in (e.g. USDC).
    pub token_in: String,
    pub token_in_decimals: u8,
    /// Max swaps per cycle.
    pub max_hops: usize,
    #[serde(default)]
    pub min_profit: f64,
    /// Trade-size cap in token_in human units. 0 = unbounded.
    #[serde(default)]
    pub max_trade_size: f64,
    /// Flash-loan fee in bps on the borrowed notional. 0 = PHUX / own capital.
    #[serde(default)]
    pub loan_fee_bps: u32,
    /// Rough per-tx gas cost in token_in human units.
    #[serde(default)]
    pub gas_cost: f64,
    #[serde(default = "default_max_cycles")]
    pub max_cycles: usize,
    #[serde(default = "default_channel")]
    pub output_channel: String,
    /// Phase 2: enable consuming predicted reserves and emitting speculative opps.
    #[serde(default)]
    pub speculative_enabled: bool,
    #[serde(default = "default_pending_updates_channel")]
    pub pending_updates_channel: String,
    #[serde(default = "default_speculative_channel")]
    pub speculative_channel: String,
}

fn default_max_cycles() -> usize {
    200_000
}

fn default_channel() -> String {
    "opportunities".to_string()
}

fn default_pending_updates_channel() -> String {
    "pending_updates".to_string()
}

fn default_speculative_channel() -> String {
    "opportunities_speculative".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub redis: RedisConfig,
    pub finder: FinderConfig,
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

    pub fn token_in_address(&self) -> Result<Address> {
        self.finder.token_in.parse::<Address>().map_err(|e| {
            anyhow::anyhow!("invalid finder.token_in '{}': {}", self.finder.token_in, e)
        })
    }

    /// Repay factor `c`: 1.0 for a 0% loan / own capital, 1 + fee for paid loans.
    pub fn repay_factor(&self) -> f64 {
        1.0 + self.finder.loan_fee_bps as f64 / 10_000.0
    }

    /// max_trade_size (human units) converted to raw token_in units. 0 = unbounded.
    pub fn max_trade_in_raw(&self) -> U256 {
        if self.finder.max_trade_size <= 0.0 {
            return U256::ZERO;
        }
        let raw = self.finder.max_trade_size * 10f64.powi(self.finder.token_in_decimals as i32);
        U256::from(raw as u128)
    }
}

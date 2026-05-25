//! Per-protocol abstraction.
//!
//! Each supported AMM lives in its own module and implements [`Protocol`], which
//! the listener uses uniformly to (a) read a pool's current reserve state from
//! chain and (b) turn a state-change log into a reserve update. New DEX families
//! (V3 concentrated liquidity, Balancer, …) are added by writing a new module and
//! registering it in [`ProtocolRegistry::new`] — the init/ws paths stay unchanged.

pub mod uniswap_v2;

use crate::config::{AppConfig, DexType};
use crate::registry_client::RegistryPool;
use crate::types::PoolState;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::DynProvider;
use alloy::rpc::types::Log;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

/// A reserve-state change decoded from an on-chain log.
///
/// Reserves are modeled the Uniswap-V2 way (reserve0/reserve1). When a non-V2
/// protocol is added this will generalize to a protocol-tagged enum; for now every
/// supported protocol is a V2 fork.
#[derive(Debug, Clone)]
pub struct ReserveUpdate {
    pub address: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub block: u64,
}

/// One AMM protocol family. Implementations are stateless and dispatched by [`DexType`].
#[async_trait]
pub trait Protocol: Send + Sync {
    /// The dex_type this protocol handles.
    fn dex_type(&self) -> DexType;

    /// topic0 hashes whose logs signal a reserve/state change for this protocol's pools.
    /// The listener subscribes to the union across all protocols and routes by pool address.
    fn state_change_topics(&self) -> Vec<B256>;

    /// Read full current state for a batch of this protocol's pools via `provider`.
    /// `config` resolves per-DEX metadata (dex_type/fee_bps) by protocol name.
    async fn fetch_states(
        &self,
        provider: &DynProvider,
        config: &AppConfig,
        pools: &[RegistryPool],
        block: u64,
    ) -> Result<Vec<PoolState>>;

    /// Decode a state-change log into a reserve update, or `None` if not applicable.
    fn decode_update(&self, log: &Log) -> Option<ReserveUpdate>;
}

/// Holds one [`Protocol`] per [`DexType`] and dispatches by type.
pub struct ProtocolRegistry {
    by_type: HashMap<DexType, Box<dyn Protocol>>,
}

impl ProtocolRegistry {
    /// Build the registry with every supported protocol.
    pub fn new() -> Self {
        let mut by_type: HashMap<DexType, Box<dyn Protocol>> = HashMap::new();
        Self::register(&mut by_type, Box::new(uniswap_v2::UniswapV2::new()));
        Self { by_type }
    }

    fn register(map: &mut HashMap<DexType, Box<dyn Protocol>>, p: Box<dyn Protocol>) {
        map.insert(p.dex_type(), p);
    }

    /// The protocol for a given dex_type, if supported.
    pub fn get(&self, dex_type: &DexType) -> Option<&dyn Protocol> {
        self.by_type.get(dex_type).map(|b| b.as_ref())
    }

    /// Union of every protocol's state-change topics, for the WS subscription.
    pub fn all_topics(&self) -> Vec<B256> {
        let mut topics: Vec<B256> = self
            .by_type
            .values()
            .flat_map(|p| p.state_change_topics())
            .collect();
        topics.sort();
        topics.dedup();
        topics
    }
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

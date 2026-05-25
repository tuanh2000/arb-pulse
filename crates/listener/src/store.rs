use crate::types::PoolState;
use alloy::primitives::{Address, U256};
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PoolStore {
    // pair_address -> PoolState
    pools: Arc<DashMap<Address, PoolState>>,
}

impl PoolStore {
    pub fn new() -> Self {
        Self {
            pools: Arc::new(DashMap::new()),
        }
    }

    pub fn insert(&self, state: PoolState) {
        self.pools.insert(state.pair_address, state);
    }

    pub fn update_reserves(&self, pair: Address, reserve0: U256, reserve1: U256, block: u64) {
        if let Some(mut entry) = self.pools.get_mut(&pair) {
            entry.reserve0 = reserve0;
            entry.reserve1 = reserve1;
            entry.last_updated_block = block;
        }
    }

    pub fn get_all(&self) -> Vec<PoolState> {
        self.pools.iter().map(|e| e.value().clone()).collect()
    }

    pub fn get(&self, pair: &Address) -> Option<PoolState> {
        self.pools.get(pair).map(|e| e.value().clone())
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }
}

impl Default for PoolStore {
    fn default() -> Self {
        Self::new()
    }
}

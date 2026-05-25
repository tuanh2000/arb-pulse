use crate::types::PoolState;
use alloy::primitives::{Address, U256};
use dashmap::DashMap;
use std::sync::Arc;

/// In-memory mirror of pool state, kept fresh from the listener's `pool_updates`.
#[derive(Clone, Default)]
pub struct PoolStore {
    pools: Arc<DashMap<Address, PoolState>>,
}

impl PoolStore {
    pub fn from_pools(pools: Vec<PoolState>) -> Self {
        let map = DashMap::new();
        for p in pools {
            map.insert(p.pair, p);
        }
        Self {
            pools: Arc::new(map),
        }
    }

    pub fn get(&self, pair: &Address) -> Option<PoolState> {
        self.pools.get(pair).map(|e| e.value().clone())
    }

    pub fn update_reserves(&self, pair: Address, reserve0: U256, reserve1: U256, block: u64) {
        if let Some(mut e) = self.pools.get_mut(&pair) {
            e.reserve0 = reserve0;
            e.reserve1 = reserve1;
            e.block = block;
        }
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }
}

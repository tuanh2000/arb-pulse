use crate::types::{Cycle, Hop, PoolState};
use alloy::primitives::Address;
use std::collections::{HashMap, HashSet};

/// token -> indices of pools that include it.
fn build_adjacency(pools: &[PoolState]) -> HashMap<Address, Vec<usize>> {
    let mut adj: HashMap<Address, Vec<usize>> = HashMap::new();
    for (i, p) in pools.iter().enumerate() {
        adj.entry(p.token0).or_default().push(i);
        adj.entry(p.token1).or_default().push(i);
    }
    adj
}

/// Enumerate all simple cycles `token_in -> ... -> token_in` of length 2..=max_hops.
/// A pool is used at most once per cycle; intermediate tokens are not revisited.
/// Both traversal directions of an undirected cycle are returned (they are distinct trades).
pub fn enumerate_cycles(
    pools: &[PoolState],
    token_in: Address,
    max_hops: usize,
    max_cycles: usize,
) -> Vec<Cycle> {
    let adj = build_adjacency(pools);
    let mut ctx = Ctx {
        pools,
        adj: &adj,
        token_in,
        max_hops,
        max_cycles,
        out: Vec::new(),
    };
    let mut hops = Vec::new();
    let mut used_pools = HashSet::new();
    let mut visited = HashSet::new();
    visited.insert(token_in);
    ctx.dfs(token_in, &mut hops, &mut used_pools, &mut visited);
    ctx.out
}

struct Ctx<'a> {
    pools: &'a [PoolState],
    adj: &'a HashMap<Address, Vec<usize>>,
    token_in: Address,
    max_hops: usize,
    max_cycles: usize,
    out: Vec<Cycle>,
}

impl Ctx<'_> {
    fn dfs(
        &mut self,
        current: Address,
        hops: &mut Vec<Hop>,
        used_pools: &mut HashSet<Address>,
        visited: &mut HashSet<Address>,
    ) {
        if self.out.len() >= self.max_cycles {
            return;
        }
        let Some(neighbors) = self.adj.get(&current) else {
            return;
        };
        for &idx in neighbors {
            let pool = &self.pools[idx];
            if used_pools.contains(&pool.pair) {
                continue;
            }
            let Some(next) = pool.other_token(current) else {
                continue;
            };
            let depth = hops.len() + 1;
            let hop = Hop {
                pool: pool.pair,
                token_in: current,
                token_out: next,
            };

            if next == self.token_in {
                // Closing the cycle. Need at least 2 hops.
                if depth >= 2 {
                    hops.push(hop);
                    self.out.push(Cycle { hops: hops.clone() });
                    hops.pop();
                    if self.out.len() >= self.max_cycles {
                        return;
                    }
                }
                continue;
            }

            // Recurse into an intermediate token only if there's room to still close.
            if depth < self.max_hops && !visited.contains(&next) {
                used_pools.insert(pool.pair);
                visited.insert(next);
                hops.push(hop);
                self.dfs(next, hops, used_pools, visited);
                hops.pop();
                visited.remove(&next);
                used_pools.remove(&pool.pair);
            }
        }
    }
}

/// Map each pool address to the indices of cycles that traverse it, so a reserve
/// update only re-evaluates the affected cycles.
pub fn build_pool_cycle_index(cycles: &[Cycle]) -> HashMap<Address, Vec<usize>> {
    let mut index: HashMap<Address, Vec<usize>> = HashMap::new();
    for (i, cycle) in cycles.iter().enumerate() {
        for hop in &cycle.hops {
            let entry = index.entry(hop.pool).or_default();
            if entry.last() != Some(&i) {
                entry.push(i);
            }
        }
    }
    index
}

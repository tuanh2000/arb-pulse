pub mod init;
pub mod ws;

use crate::config::AppConfig;
use crate::registry_client::RegistryPool;
use crate::sink::RedisSink;
use crate::store::PoolStore;
use anyhow::Result;
use std::sync::Arc;

pub use init::fetch_states_for_pools;

/// Start the WebSocket Sync subscription loop. Maintains `store` and mirrors
/// every reserve change to `sink` (Redis). On disconnect it re-syncs the full
/// curated pool set from chain to repair any gap before resubscribing.
/// Runs indefinitely.
pub async fn start_ws_listener(
    config: &AppConfig,
    store: PoolStore,
    sink: RedisSink,
    pools: Arc<Vec<RegistryPool>>,
) -> Result<()> {
    ws::run(config, store, sink, pools).await
}

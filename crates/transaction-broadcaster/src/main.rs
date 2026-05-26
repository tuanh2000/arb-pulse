mod abi;
mod broadcaster;
mod config;
mod db;
mod nonce;
mod types;

use alloy::network::EthereumWallet;
use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, Result};
use futures::StreamExt;
use nonce::NonceManager;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, warn};
use types::Opportunity;

const OPPORTUNITY_CHANNEL_FALLBACK: &str = "opportunities";

/// Max concurrent eth_call simulations. Each consumes one RPC connection slot.
const SIM_CONCURRENCY: usize = 8;

/// Capacity of the channel from simulation tasks to the send loop.
/// Bounded so a slow sender doesn't accumulate an unbounded backlog of stale opps.
const SEND_QUEUE_CAP: usize = 32;

struct ReadyToSend {
    opp: Opportunity,
    data: Vec<u8>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "transaction_broadcaster=info".into()),
        )
        .init();

    let cfg = config::AppConfig::load()?;
    let contract = cfg.contract_address()?;
    let chain_id = cfg.network.chain_id;
    let min_profit = cfg.min_profit();

    let db_pool: Option<PgPool> = if let Some(db_cfg) = &cfg.database {
        match PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_cfg.url)
            .await
        {
            Ok(pool) => {
                info!("Connected to PostgreSQL for transaction persistence");
                Some(pool)
            }
            Err(e) => {
                warn!(error = %e, "Failed to connect to PostgreSQL — continuing without DB persistence");
                None
            }
        }
    } else {
        info!("No [database] config — transaction persistence disabled");
        None
    };

    let key = std::env::var("PRIVATE_KEY")
        .context("PRIVATE_KEY env var is required")?;
    let signer: PrivateKeySigner = key.trim().parse().context("invalid PRIVATE_KEY")?;
    let sender_addr = signer.address();
    let wallet = EthereumWallet::from(signer);

    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(cfg.network.rpc_http.parse().context("invalid network.rpc_http")?);

    info!(
        sender = %sender_addr,
        contract = %contract,
        chain_id,
        channel = %cfg.broadcaster.opportunities_channel,
        simulate = cfg.broadcaster.simulate,
        max_opportunity_age_blocks = cfg.broadcaster.max_opportunity_age_blocks,
        "Broadcaster starting"
    );

    // ── Stage 0: local chain-tip tracker ────────────────────────────────────
    // Polls get_block_number every 2 s so staleness checks cost zero RPC per
    // opportunity (PulseChain blocks arrive every ~10 s, so 2 s is plenty).
    let current_tip = Arc::new(AtomicU64::new(0));
    {
        let tip = Arc::clone(&current_tip);
        let p = provider.clone();
        tokio::spawn(async move { track_tip(p, tip).await });
    }

    // Nonce manager seeded from the chain's mined nonce. Starting at the mined
    // (not pending) count lets fresh, properly-priced txs replace any stuck txs
    // left in the mempool, so the broadcaster self-heals from a nonce jam.
    let start_nonce = provider.get_transaction_count(sender_addr).await.unwrap_or(0);
    let nm = NonceManager::new(start_nonce);
    info!(start_nonce, "Nonce manager initialized");

    // ── Stage 1→2 channel: simulation tasks → send loop ─────────────────────
    let (ready_tx, ready_rx) = mpsc::channel::<ReadyToSend>(SEND_QUEUE_CAP);

    // ── Nonce monitor: confirms / fee-bumps / replaces in-flight txs ─────────
    {
        let p = provider.clone();
        let cfg_b = cfg.broadcaster.clone();
        let db = db_pool.clone();
        let tip = Arc::clone(&current_tip);
        let nm2 = nm.clone();
        tokio::spawn(async move {
            nonce::run_monitor(p, cfg_b, contract, chain_id, sender_addr, nm2, tip, db).await;
        });
    }

    // ── Stage 2: sequential send loop ────────────────────────────────────────
    {
        let p = provider.clone();
        let cfg_b = cfg.broadcaster.clone();
        let db = db_pool.clone();
        let tip = Arc::clone(&current_tip);
        let nm2 = nm.clone();
        tokio::spawn(async move {
            send_loop(p, cfg_b, contract, chain_id, sender_addr, tip, db, nm2, ready_rx).await;
        });
    }

    // ── Stage 1: Redis consume loop + concurrent simulation tasks ────────────
    let channel = if cfg.broadcaster.opportunities_channel.is_empty() {
        OPPORTUNITY_CHANNEL_FALLBACK.to_string()
    } else {
        cfg.broadcaster.opportunities_channel.clone()
    };
    let client = redis::Client::open(cfg.redis.url.clone())?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(&channel).await?;
    let mut stream = pubsub.on_message();
    info!("Listening for opportunities on channel '{channel}'...");

    let max_age = cfg.broadcaster.max_opportunity_age_blocks;
    let do_simulate = cfg.broadcaster.simulate;
    let sim_sem = Arc::new(Semaphore::new(SIM_CONCURRENCY));

    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "bad opportunity payload");
                continue;
            }
        };
        let opp: Opportunity = match serde_json::from_str(&payload) {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "failed to decode opportunity");
                continue;
            }
        };

        // Local staleness check — no RPC round-trip.
        let current = current_tip.load(Ordering::Relaxed);
        if opp.block != 0 && current > 0 && current > opp.block + max_age {
            warn!(
                opp_block = opp.block,
                tip = current,
                "dropped: stale on arrival"
            );
            continue;
        }

        // Spawn a simulation task. The semaphore caps concurrent eth_calls so
        // we don't flood the RPC node. The main loop returns to stream.next()
        // immediately — it is never blocked by simulation or sending.
        let permit = Arc::clone(&sim_sem).acquire_owned().await.unwrap();
        let ready_tx2 = ready_tx.clone();
        let p2 = provider.clone();

        tokio::spawn(async move {
            let _permit = permit; // released when task ends

            let data = match broadcaster::build_calldata(&opp, min_profit) {
                Ok(d) => d,
                Err(e) => {
                    warn!(error = %e, "calldata build failed");
                    return;
                }
            };

            if do_simulate {
                if !broadcaster::simulate(&p2, sender_addr, contract, data.clone()).await {
                    warn!(
                        hops = opp.hops.len(),
                        profit = opp.net_profit_token_in,
                        "skipping: pre-send simulation reverted"
                    );
                    return;
                }
            }

            if ready_tx2.send(ReadyToSend { opp, data }).await.is_err() {
                warn!("send channel closed — broadcaster shutting down");
            }
        });
    }

    Ok(())
}

/// Stage 2: drain the ready-to-send channel one opportunity at a time.
/// Sends are sequential (single private key). Each tx is assigned an explicit
/// nonce from the `NonceManager`; the nonce monitor handles confirmation and
/// fee-bumps, so this loop is never blocked on inclusion.
#[allow(clippy::too_many_arguments)]
async fn send_loop<P>(
    provider: P,
    cfg: config::BroadcasterConfig,
    contract: Address,
    chain_id: u64,
    sender: Address,
    tip: Arc<AtomicU64>,
    db: Option<PgPool>,
    nm: NonceManager,
    mut rx: mpsc::Receiver<ReadyToSend>,
) where
    P: Provider + Clone + Send + Sync + 'static,
{
    let cap = (cfg.max_fee_gwei * 1e9) as u128;
    while let Some(ready) = rx.recv().await {
        // Second staleness gate — the opp may have aged while waiting in the
        // simulation queue.
        let current = tip.load(Ordering::Relaxed);
        if ready.opp.block != 0
            && current > 0
            && current > ready.opp.block + cfg.max_opportunity_age_blocks
        {
            warn!(
                opp_block = ready.opp.block,
                tip = current,
                "dropped: stale by send time"
            );
            continue;
        }

        // Insert a pending DB row before sending so we have a record even if
        // the submission fails.
        let arb_id: Option<i64> = if let Some(pool) = &db {
            match db::insert_arb_tx(pool, ready.opp.db_id).await {
                Ok(id) => Some(id),
                Err(e) => {
                    warn!(error = %e, "db: insert_arb_tx failed");
                    None
                }
            }
        } else {
            None
        };

        let fees = match broadcaster::compute_fees(&provider, &cfg).await {
            Ok(f) => f,
            Err(e) => {
                let chain = format!("{e:#}");
                error!(error = %chain, "fee computation failed");
                if let (Some(pool), Some(id)) = (&db, arb_id) {
                    let _ = db::update_arb_tx_failed(pool, id, "failed", &chain).await;
                }
                continue;
            }
        };

        let nonce = nm.peek();
        match broadcaster::send_replacing(
            &provider,
            contract,
            chain_id,
            nonce,
            fees,
            cfg.gas_limit,
            cap,
            cfg.max_replacements,
            ready.data.clone(),
        )
        .await
        {
            Ok((tx_hash, used_fees)) => {
                info!(
                    tx = %tx_hash,
                    nonce,
                    hops = ready.opp.hops.len(),
                    profit = ready.opp.net_profit_token_in,
                    opp_block = ready.opp.block,
                    "broadcast arb tx"
                );
                nm.commit(nonce, tx_hash, ready.data, used_fees, current, arb_id);
                if let (Some(pool), Some(id)) = (&db, arb_id) {
                    let hash_str = format!("{tx_hash:?}");
                    if let Err(e) = db::update_arb_tx_sent(pool, id, &hash_str).await {
                        warn!(error = %e, "db: update to 'sent' failed");
                    }
                }
            }
            Err(e) => {
                // `{:#}` prints the full anyhow cause chain on one line — the bare
                // Display only shows the top context ("send_transaction").
                let err_chain = format!("{e:#}");
                if broadcaster::is_nonce_too_low(&err_chain) {
                    // Our nonce is behind the chain — resync and drop this (stale) opp.
                    if let Ok(mined) = provider.get_transaction_count(sender).await {
                        nm.resync(mined);
                    }
                    warn!(nonce, error = %err_chain, "send rejected (nonce too low) — resynced; dropping opp");
                } else {
                    error!(nonce, error = %err_chain, "send_transaction failed");
                }
                if let (Some(pool), Some(id)) = (&db, arb_id) {
                    if let Err(db_err) =
                        db::update_arb_tx_failed(pool, id, "failed", &err_chain).await
                    {
                        warn!(error = %db_err, "db: update to 'failed' failed");
                    }
                }
            }
        }
    }
}

/// Background task: polls get_block_number every 2 s and stores the result in
/// `tip`. Runs forever — errors are logged and retried, never fatal.
async fn track_tip<P: Provider>(provider: P, tip: Arc<AtomicU64>) {
    loop {
        match provider.get_block_number().await {
            Ok(block) => {
                tip.store(block, Ordering::Relaxed);
            }
            Err(e) => {
                warn!(error = %e, "tip tracker: get_block_number failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

mod db;
mod health;
mod supervisor;

use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

// ── Service catalogue ─────────────────────────────────────────────────────────

pub struct ServiceSpec {
    pub name: &'static str,
    /// Binary filename under `target/debug/`.
    pub bin: &'static str,
    /// HTTP URL to GET for a health check. None = process-alive only.
    pub health_url: Option<&'static str>,
    /// Names of services that must be healthy before this one starts.
    pub depends_on: &'static [&'static str],
    /// Seconds to wait for the health URL on first start.
    pub startup_timeout_secs: u64,
    /// Seconds between steady-state health polls.
    pub health_interval_secs: u64,
    /// Whether to inject the PRIVATE_KEY env var into this process.
    pub pass_private_key: bool,
}

// Declared in dependency order; this order is also the startup order.
static SERVICES: &[ServiceSpec] = &[
    ServiceSpec {
        name: "pool-registry",
        bin: "pool-registry",
        health_url: Some("http://127.0.0.1:3001/health"),
        depends_on: &[],
        startup_timeout_secs: 600,
        health_interval_secs: 30,
        pass_private_key: false,
    },
    ServiceSpec {
        name: "listener",
        bin: "listener",
        health_url: Some("http://127.0.0.1:3000/health"),
        depends_on: &["pool-registry"],
        startup_timeout_secs: 180,
        health_interval_secs: 30,
        pass_private_key: false,
    },
    ServiceSpec {
        name: "opportunity-finder",
        bin: "opportunity-finder",
        health_url: None,
        depends_on: &["listener"],
        startup_timeout_secs: 60,
        health_interval_secs: 60,
        pass_private_key: false,
    },
    ServiceSpec {
        name: "broadcaster",
        bin: "transaction-broadcaster",
        health_url: None,
        depends_on: &["opportunity-finder"],
        startup_timeout_secs: 30,
        health_interval_secs: 60,
        pass_private_key: true,
    },
];

// ── Minimal config ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OrchestratorConfig {
    database: DbSection,
}

#[derive(Deserialize)]
struct DbSection {
    url: String,
}

fn load_db_url() -> Result<String> {
    // Accept DATABASE_URL env override; fall back to config.toml [database].url.
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return Ok(url);
    }
    let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", path, e))?;
    let cfg: OrchestratorConfig = toml::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("Cannot parse [database] from '{}': {}", path, e))?;
    Ok(cfg.database.url)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "orchestrator=info".into()),
        )
        .init();

    // Locate the project root (directory containing config.toml / this binary).
    let bin_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("target/debug"));

    // Walk up from the binary to find the workspace root (contains config.toml).
    let root = bin_dir
        .ancestors()
        .find(|p| p.join("config.toml").exists())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    info!(root = %root.display(), bin_dir = %bin_dir.display(), "Orchestrator starting");

    // Load PRIVATE_KEY (broadcater needs it at spawn time).
    let private_key = load_private_key(&root);
    if private_key.is_none() {
        tracing::warn!(
            "PRIVATE_KEY not set — broadcaster will be skipped\n  \
             Set it in .env or export PRIVATE_KEY=0x..."
        );
    }

    // Connect to PostgreSQL for event logging.
    let db_url = load_db_url()?;
    info!("Connecting to PostgreSQL for event logging...");
    let db = Arc::new(db::connect(&db_url).await?);
    info!("PostgreSQL connected");

    // Check infra (postgres + redis) before starting any Rust service.
    check_infra(&db_url).await?;

    // Build a watch channel per service: false = not ready, true = healthy.
    let mut ready_txs: std::collections::HashMap<&str, watch::Sender<bool>> =
        std::collections::HashMap::new();
    let mut ready_rxs: std::collections::HashMap<&str, watch::Receiver<bool>> =
        std::collections::HashMap::new();

    for spec in SERVICES {
        let (tx, rx) = watch::channel(false);
        ready_txs.insert(spec.name, tx);
        ready_rxs.insert(spec.name, rx);
    }

    // Spawn a supervisor task per service.
    let mut handles = Vec::new();
    for spec in SERVICES {
        let dep_rxs: Vec<watch::Receiver<bool>> = spec
            .depends_on
            .iter()
            .filter_map(|n| ready_rxs.get(n).cloned())
            .collect();

        let ready_tx = ready_txs.remove(spec.name).unwrap();
        let db_clone = Arc::clone(&db);
        let bin_dir_clone = bin_dir.clone();
        let pk = private_key.clone();

        let handle = tokio::spawn(async move {
            supervisor::run(spec, bin_dir_clone, db_clone, ready_tx, dep_rxs, pk).await;
        });
        handles.push(handle);
    }

    info!("All agent supervisors launched — press Ctrl+C to stop");

    // Wait for shutdown signal.
    tokio::signal::ctrl_c().await?;
    info!("Shutting down — sending SIGTERM to all children...");
    db::log_event(&db, "orchestrator", "shutdown", None, Some("SIGINT received")).await;

    // Abort all supervisor tasks (kill_on_drop handles the child processes).
    for h in handles {
        h.abort();
    }

    info!("Orchestrator stopped");
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Load PRIVATE_KEY from env first, then .env file.
fn load_private_key(root: &PathBuf) -> Option<String> {
    if let Ok(k) = std::env::var("PRIVATE_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            let k = if k.starts_with("0x") { k } else { format!("0x{k}") };
            return Some(k);
        }
    }
    let env_path = root.join(".env");
    if let Ok(contents) = std::fs::read_to_string(&env_path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.starts_with("PRIVATE_KEY=") {
                let k = line["PRIVATE_KEY=".len()..].trim().to_string();
                if !k.is_empty() {
                    let k = if k.starts_with("0x") { k } else { format!("0x{k}") };
                    return Some(k);
                }
            }
        }
    }
    None
}

/// Verify postgres and redis are reachable before starting any Rust service.
async fn check_infra(db_url: &str) -> Result<()> {
    // Postgres: already connected (db::connect would have failed).
    info!("PostgreSQL: OK");

    // Redis: attempt a simple TCP connection to 127.0.0.1:6379.
    use tokio::net::TcpStream;
    let redis_ok = TcpStream::connect("127.0.0.1:6379")
        .await
        .map(|_| true)
        .unwrap_or(false);

    if !redis_ok {
        return Err(anyhow::anyhow!(
            "Redis is not reachable on 127.0.0.1:6379\n  \
             Hint: run `docker-compose up -d`"
        ));
    }
    info!("Redis: OK");
    let _ = db_url; // already used above
    Ok(())
}

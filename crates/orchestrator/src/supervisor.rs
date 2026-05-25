use crate::{db, health, ServiceSpec};
use anyhow::Result;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::watch;

/// Supervises one service: starts it, waits for it to become healthy, signals
/// readiness to dependents, then monitors and auto-restarts on crash.
/// `ready_tx` is set to true once this service is healthy (dependents watch this).
/// `dep_rxs` are receivers that must all be true before this service starts.
/// `private_key` is injected into the process env if `spec.pass_private_key` is set.
pub async fn run(
    spec: &'static ServiceSpec,
    bin_dir: PathBuf,
    db: Arc<PgPool>,
    ready_tx: watch::Sender<bool>,
    mut dep_rxs: Vec<watch::Receiver<bool>>,
    private_key: Option<String>,
) {
    // Wait for all dependencies to signal ready.
    for rx in &mut dep_rxs {
        let _ = rx.wait_for(|v| *v).await;
    }

    let mut backoff_secs: u64 = 2;

    loop {
        match spawn_and_monitor(spec, &bin_dir, &db, &ready_tx, private_key.as_deref()).await {
            Ok(()) => {
                // Process exited cleanly (shouldn't happen for long-running services).
                tracing::warn!(service = spec.name, "exited cleanly — restarting in {backoff_secs}s");
            }
            Err(e) => {
                tracing::error!(service = spec.name, error = %e, "crashed — restarting in {backoff_secs}s");
            }
        }

        db::log_event(
            &db,
            spec.name,
            "restarting",
            None,
            Some(&format!("backoff={}s", backoff_secs)),
        )
        .await;

        // Signal not-ready while restarting, so any newly launched dependents wait.
        let _ = ready_tx.send(false);

        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn spawn_and_monitor(
    spec: &ServiceSpec,
    bin_dir: &PathBuf,
    db: &PgPool,
    ready_tx: &watch::Sender<bool>,
    private_key: Option<&str>,
) -> Result<()> {
    let bin = bin_dir.join(spec.bin);
    if !bin.exists() {
        return Err(anyhow::anyhow!(
            "binary not found: {} — run `cargo build -p {}`",
            bin.display(),
            spec.bin
        ));
    }

    let log_path = bin_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(bin_dir)
        .join("logs")
        .join(format!("{}.log", spec.name));

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stdout = std::process::Stdio::from(log_file.try_clone()?);
    let stderr = std::process::Stdio::from(log_file);

    let mut cmd = Command::new(&bin);
    cmd.stdout(stdout).stderr(stderr).kill_on_drop(true);

    // Pass PRIVATE_KEY only to the broadcaster.
    if spec.pass_private_key {
        if let Some(key) = private_key {
            cmd.env("PRIVATE_KEY", key);
        }
    }

    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);

    tracing::info!(service = spec.name, pid, "started");
    db::log_event(db, spec.name, "started", Some(pid), None).await;

    // Wait for health or process death.
    let ready = match spec.health_url {
        Some(url) => {
            let timeout = Duration::from_secs(spec.startup_timeout_secs);
            tokio::select! {
                healthy = health::wait_healthy(url, timeout) => healthy,
                status = child.wait() => {
                    let code = status.map(|s| s.to_string()).unwrap_or_default();
                    db::log_event(db, spec.name, "crashed", Some(pid), Some(&code)).await;
                    return Err(anyhow::anyhow!("{} died during startup: {}", spec.name, code));
                }
            }
        }
        // No health URL: consider it ready as soon as it's alive for 3 seconds.
        None => {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => true,
                status = child.wait() => {
                    let code = status.map(|s| s.to_string()).unwrap_or_default();
                    db::log_event(db, spec.name, "crashed", Some(pid), Some(&code)).await;
                    return Err(anyhow::anyhow!("{} died during startup: {}", spec.name, code));
                }
            }
        }
    };

    if !ready {
        child.kill().await.ok();
        db::log_event(db, spec.name, "startup_timeout", Some(pid), None).await;
        return Err(anyhow::anyhow!("{} timed out waiting for health", spec.name));
    }

    tracing::info!(service = spec.name, pid, "healthy");
    db::log_event(db, spec.name, "healthy", Some(pid), None).await;
    let _ = ready_tx.send(true);

    // Monitor: periodic health check + wait for process death.
    let mut health_interval = tokio::time::interval(Duration::from_secs(spec.health_interval_secs));
    health_interval.tick().await; // skip the immediate tick

    loop {
        tokio::select! {
            status = child.wait() => {
                let code = status.map(|s| s.to_string()).unwrap_or_default();
                tracing::error!(service = spec.name, pid, %code, "process died");
                db::log_event(db, spec.name, "crashed", Some(pid), Some(&code)).await;
                return Err(anyhow::anyhow!("{} crashed: {}", spec.name, code));
            }
            _ = health_interval.tick() => {
                if let Some(url) = spec.health_url {
                    if !health::is_healthy(url).await {
                        tracing::warn!(service = spec.name, pid, "health check failed — killing");
                        db::log_event(db, spec.name, "unhealthy", Some(pid), None).await;
                        child.kill().await.ok();
                        return Err(anyhow::anyhow!("{} failed health check", spec.name));
                    }
                }
            }
        }
    }
}

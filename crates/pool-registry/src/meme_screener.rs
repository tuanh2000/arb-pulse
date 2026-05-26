use crate::config::MemeScreenerConfig;
use crate::db;
use sqlx::PgPool;
use std::time::Duration;

/// Returns true if the token's symbol or name contains any configured keyword
/// (case-insensitive substring match).
pub fn is_meme(symbol: Option<&str>, name: Option<&str>, keywords: &[String]) -> bool {
    let symbol = symbol.unwrap_or("").to_uppercase();
    let name = name.unwrap_or("").to_uppercase();
    keywords
        .iter()
        .any(|kw| symbol.contains(kw.to_uppercase().as_str()) || name.contains(kw.to_uppercase().as_str()))
}

/// Periodic worker. Fetches tokens that have metadata resolved but haven't been
/// meme-classified yet, runs keyword matching, writes results, then syncs the
/// denormalized `has_meme_token` flag on all affected pools.
pub async fn run(pool: PgPool, cfg: MemeScreenerConfig) {
    tracing::info!(
        keywords = cfg.keywords.len(),
        batch_size = cfg.batch_size,
        interval_secs = cfg.interval_secs,
        "Meme screener started"
    );

    loop {
        let tokens =
            match db::get_tokens_pending_meme_classification(&pool, cfg.batch_size).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!(error = %e, "Meme screener: failed to fetch candidates");
                    tokio::time::sleep(Duration::from_secs(cfg.interval_secs)).await;
                    continue;
                }
            };

        if tokens.is_empty() {
            tracing::info!(
                sleep_secs = cfg.interval_secs,
                "Meme screener: nothing to classify — sleeping"
            );
            tokio::time::sleep(Duration::from_secs(cfg.interval_secs)).await;
            continue;
        }

        let (mut flagged, mut clean, mut errors) = (0u64, 0u64, 0u64);
        for t in &tokens {
            let meme = is_meme(t.symbol.as_deref(), t.name.as_deref(), &cfg.keywords);
            match db::mark_token_meme(&pool, &t.token_address, meme).await {
                Ok(()) if meme => {
                    tracing::debug!(
                        token = %t.token_address,
                        symbol = ?t.symbol,
                        name   = ?t.name,
                        "Meme screener: flagged as meme"
                    );
                    flagged += 1;
                }
                Ok(()) => clean += 1,
                Err(e) => {
                    tracing::warn!(token = %t.token_address, error = %e, "Meme screener: mark failed");
                    errors += 1;
                }
            }
        }

        // Sync pool flags for tokens whose pool identity was set after their
        // initial classification (handles race between chain seeder and screener).
        match db::sync_pool_meme_flags(&pool).await {
            Ok(synced) if synced > 0 => {
                tracing::info!(synced, "Meme screener: synced stale pool flags");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Meme screener: pool flag sync failed"),
        }

        tracing::info!(
            batch = tokens.len(),
            flagged,
            clean,
            errors,
            "Meme screener batch complete"
        );
    }
}

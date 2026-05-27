pub mod api;
pub mod chain_seeder;
pub mod config;
pub mod db;
pub mod fot_screener;
pub mod price;
pub mod price_updater;
pub mod reserve_fetcher;
pub mod token_metadata;
pub mod tvl_worker;

pub mod startup {
    use crate::config::DatabaseConfig;
    use anyhow::Result;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;

    /// Connect to PostgreSQL, retrying up to 12 times (5-second gaps) so
    /// services survive a slow docker-compose startup.
    pub async fn connect_db(cfg: &DatabaseConfig) -> Result<PgPool> {
        const MAX_RETRIES: u32 = 12;
        const RETRY_DELAY_SECS: u64 = 5;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match PgPoolOptions::new()
                .max_connections(cfg.max_connections)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&cfg.url)
                .await
            {
                Ok(pool) => return Ok(pool),
                Err(e) if attempt < MAX_RETRIES => {
                    tracing::warn!(
                        attempt,
                        max = MAX_RETRIES,
                        retry_in_secs = RETRY_DELAY_SECS,
                        error = %e,
                        "PostgreSQL not ready, retrying…"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to connect to PostgreSQL after {} attempts: {}",
                        MAX_RETRIES,
                        e
                    ));
                }
            }
        }
    }

    /// Parse `--api-port <n>` / `--api-port=n` or `REGISTRY_API_PORT` env var.
    /// The CLI flag takes precedence.
    pub fn api_port_override() -> Result<Option<u16>> {
        let mut args = std::env::args().skip(1);
        let raw = loop {
            match args.next() {
                Some(a) if a == "--api-port" => break args.next(),
                Some(a) if a.starts_with("--api-port=") => {
                    break Some(a["--api-port=".len()..].to_string())
                }
                Some(_) => continue,
                None => break std::env::var("REGISTRY_API_PORT").ok(),
            }
        };
        match raw.as_deref().map(str::trim) {
            None | Some("") => Ok(None),
            Some(p) => p
                .parse::<u16>()
                .map(Some)
                .map_err(|_| anyhow::anyhow!("Invalid API port '{}': must be 1-65535", p)),
        }
    }
}

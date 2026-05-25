use anyhow::Result;
use sqlx::PgPool;

pub async fn log_event(
    db: &PgPool,
    component: &str,
    event_type: &str,
    pid: Option<u32>,
    detail: Option<&str>,
) {
    let result = sqlx::query(
        "INSERT INTO component_events (component, event_type, pid, detail) VALUES ($1, $2, $3, $4)",
    )
    .bind(component)
    .bind(event_type)
    .bind(pid.map(|p| p as i32))
    .bind(detail)
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::warn!(component, event_type, error = %e, "failed to log component event");
    }
}

/// Connect to PostgreSQL, retrying until ready.
pub async fn connect(url: &str) -> Result<PgPool> {
    use sqlx::postgres::PgPoolOptions;
    const MAX: u32 = 12;
    const DELAY: u64 = 5;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < MAX => {
                tracing::warn!(attempt, max = MAX, error = %e, "PostgreSQL not ready, retrying in {DELAY}s");
                tokio::time::sleep(std::time::Duration::from_secs(DELAY)).await;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to connect to PostgreSQL after {MAX} attempts: {e}\n\
                     Hint: run `docker-compose up -d`"
                ));
            }
        }
    }
}

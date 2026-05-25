use std::time::Duration;

/// Returns true if the URL responds 2xx within the timeout.
pub async fn is_healthy(url: &str) -> bool {
    let Ok(client) = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return false;
    };
    client.get(url).send().await.map(|r| r.status().is_success()).unwrap_or(false)
}

/// Polls `url` until it is healthy or `timeout` elapses.
/// Returns true if healthy, false if timed out.
pub async fn wait_healthy(url: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if is_healthy(url).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

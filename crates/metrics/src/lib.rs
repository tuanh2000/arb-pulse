//! Shared Prometheus instrumentation for every arb-pulse service.
//!
//! Each service calls [`init`] once at startup with its dedicated metrics port.
//! That installs a process-global `metrics` recorder and a Prometheus HTTP
//! exporter so `GET http://<host>:<port>/metrics` returns the text exposition
//! format Prometheus scrapes. After `init`, any code can emit metrics with the
//! `metrics::{counter,gauge,histogram}!` macros without threading a handle.
//!
//! Service identity (which process a series came from) is supplied by the
//! Prometheus scrape config's `job` label, so metric names here are unprefixed
//! infra signals; service-specific names are emitted at each call site.

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Conventional metrics-exporter ports, one per service. These are the scrape
/// targets listed in `monitoring/prometheus.yml`.
pub mod ports {
    pub const LISTENER: u16 = 9100;
    pub const POOL_REGISTRY: u16 = 9101;
    pub const OPPORTUNITY_FINDER: u16 = 9102;
    pub const BROADCASTER: u16 = 9103;
    pub const ORCHESTRATOR: u16 = 9104;
    // Split pool-registry services
    pub const POOL_REGISTRY_METADATA: u16 = 9105;
    pub const POOL_REGISTRY_PRICE: u16 = 9106;
    pub const POOL_REGISTRY_TVL: u16 = 9107;
}

/// Latency buckets (milliseconds). PulseChain blocks are ~10s apart, so a healthy
/// detect-latency lives in the hundreds of ms; the long tail catches pipeline
/// stalls. Applied to every `*_ms` histogram (block age, detect latency, send
/// latency). Configuring buckets makes histograms render as real Prometheus
/// histograms (`_bucket`/`_sum`/`_count`) so `histogram_quantile` and heatmaps
/// work — without this the exporter would emit summaries instead.
const LATENCY_MS_BUCKETS: &[f64] = &[
    25.0, 50.0, 100.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 3000.0, 5000.0,
    10000.0, 20000.0,
];

/// Gas-used buckets for `broadcaster_gas_used` (raw gas units).
const GAS_BUCKETS: &[f64] = &[
    100_000.0, 200_000.0, 300_000.0, 400_000.0, 500_000.0, 600_000.0, 700_000.0, 800_000.0,
];

/// Install the global Prometheus recorder and start the scrape HTTP listener on
/// `0.0.0.0:port`. Must be called from within a Tokio runtime (the listener is
/// spawned as a background task). Safe to call once per process; a second call
/// logs a warning and is a no-op. Never panics — if the exporter cannot be
/// configured or bound, the service keeps running uninstrumented.
pub fn init(port: u16) {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let builder = PrometheusBuilder::new().with_http_listener(addr);

    // `_ms` histograms get latency buckets; gas gets its own. Bucket config can
    // only fail on an invalid matcher, so any error here is a programming bug —
    // log and bail rather than panic.
    let builder = match builder
        .set_buckets_for_metric(Matcher::Suffix("_ms".to_string()), LATENCY_MS_BUCKETS)
        .and_then(|b| {
            b.set_buckets_for_metric(
                Matcher::Full("broadcaster_gas_used".to_string()),
                GAS_BUCKETS,
            )
        }) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "metrics bucket config failed — running uninstrumented");
            return;
        }
    };

    match builder.install() {
        Ok(()) => {
            metrics::gauge!("service_up").set(1.0);
            metrics::gauge!("service_start_time_seconds").set(now_secs());
            tracing::info!(%addr, "Prometheus metrics exporter listening on /metrics");
        }
        Err(e) => {
            tracing::warn!(error = %e, %addr, "failed to start metrics exporter — running uninstrumented");
        }
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

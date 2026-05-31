//! Request-latency histogram for the JSON-RPC middleware chain.
//!
//! [`MetricsService`](super::metrics::MetricsService) already measures each
//! request's wall-clock duration (`elapsed_ms`) but, prior to this module,
//! only logged it at `debug` level and threw the number away. This is the
//! aggregation layer: a lock-free fixed-bucket histogram that the `/metrics`
//! endpoint renders in Prometheus form, giving latency-distribution
//! observability (p50/p90/p99 via `histogram_quantile`) without a new
//! measurement path or any external dependency.
//!
//! A single process-wide instance is installed by [`MiddlewareChain`] at
//! startup (mirroring the global request-state registry) so both the writer
//! (`MetricsService`) and the reader (`/metrics`) reach the same counters.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// Upper bounds (inclusive, milliseconds) for the histogram buckets. An
/// observation falls into the first bucket whose bound is `>= value`; anything
/// larger is only counted in the implicit `+Inf` bucket (the total count).
const BUCKET_BOUNDS_MS: &[u64] = &[1, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000];

/// Lock-free latency histogram with fixed millisecond buckets.
///
/// Bucket counts are stored non-cumulatively (each observation increments
/// exactly one bucket); the cumulative form Prometheus expects is computed at
/// render time.
pub struct LatencyHistogram {
    /// One counter per entry in [`BUCKET_BOUNDS_MS`].
    buckets: Box<[AtomicU64]>,
    /// Sum of all observed durations, milliseconds.
    sum_ms: AtomicU64,
    /// Total number of observations (== the `+Inf` cumulative bucket).
    count: AtomicU64,
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self {
            buckets: (0..BUCKET_BOUNDS_MS.len())
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            sum_ms: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Record one request duration in milliseconds.
    pub fn observe(&self, ms: u64) {
        // First bound >= ms; if none, the value only lands in +Inf (count).
        if let Some(idx) = BUCKET_BOUNDS_MS.iter().position(|&b| ms <= b) {
            self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        }
        self.sum_ms.fetch_add(ms, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Render the Prometheus `histogram` lines for this metric family.
    ///
    /// Emits cumulative `_bucket{le="…"}` series (including `+Inf`), `_sum`,
    /// and `_count`, with the `# HELP`/`# TYPE` header.
    pub fn render_prometheus(&self, name: &str) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(&format!(
            "# HELP {name} JSON-RPC request wall-clock duration in milliseconds.\n"
        ));
        out.push_str(&format!("# TYPE {name} histogram\n"));

        let mut cumulative = 0u64;
        for (idx, &bound) in BUCKET_BOUNDS_MS.iter().enumerate() {
            cumulative += self.buckets[idx].load(Ordering::Relaxed);
            out.push_str(&format!("{name}_bucket{{le=\"{bound}\"}} {cumulative}\n"));
        }
        let count = self.count.load(Ordering::Relaxed);
        out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {count}\n"));
        out.push_str(&format!("{name}_sum {}\n", self.sum_ms.load(Ordering::Relaxed)));
        out.push_str(&format!("{name}_count {count}\n"));
        out
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_LATENCY: OnceLock<RwLock<Option<Arc<LatencyHistogram>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<LatencyHistogram>>> {
    GLOBAL_LATENCY.get_or_init(|| RwLock::new(None))
}

/// Fetch the process-wide latency histogram, initializing it on first call.
///
/// `MiddlewareChain::new` runs once per connection, so a plain "set" would
/// reset the global on every connect (the same quirk the request-state
/// registry has). Get-or-init instead returns the single shared instance, so
/// every connection's metrics layer observes into — and `/metrics` reads from
/// — the same aggregate.
pub fn global_latency_or_init() -> Arc<LatencyHistogram> {
    let mut guard = slot().write().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(|| Arc::new(LatencyHistogram::new()))
        .clone()
}

/// Fetch the process-wide latency histogram, or `None` before it is installed.
pub fn get_global_latency() -> Option<Arc<LatencyHistogram>> {
    let guard = slot().read().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_buckets_and_render_is_cumulative() {
        let h = LatencyHistogram::new();
        h.observe(3); // le=5
        h.observe(7); // le=10
        h.observe(7); // le=10
        h.observe(9001); // only +Inf

        let out = h.render_prometheus("aleph_gateway_request_duration_ms");
        // le=1 has nothing; le=5 has the 3ms; le=10 is cumulative (3ms + two 7ms).
        assert!(out.contains("aleph_gateway_request_duration_ms_bucket{le=\"5\"} 1"));
        assert!(out.contains("aleph_gateway_request_duration_ms_bucket{le=\"10\"} 3"));
        // +Inf == total count (4), the 9001ms observation only shows here.
        assert!(out.contains("aleph_gateway_request_duration_ms_bucket{le=\"+Inf\"} 4"));
        assert!(out.contains("aleph_gateway_request_duration_ms_count 4"));
        assert!(out.contains("aleph_gateway_request_duration_ms_sum 9018"));
    }

    #[test]
    fn empty_histogram_renders_zeroes() {
        let h = LatencyHistogram::new();
        let out = h.render_prometheus("m");
        assert!(out.contains("m_bucket{le=\"+Inf\"} 0"));
        assert!(out.contains("m_count 0"));
        assert!(out.contains("m_sum 0"));
        assert_eq!(out.matches("# TYPE m histogram").count(), 1);
    }
}

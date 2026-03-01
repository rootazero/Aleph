//! BaselineApi - Lazy calculation of baseline metrics with TTL caching

use crate::sync_primitives::{Arc, Mutex};
use std::collections::HashMap;
use chrono::{DateTime, Utc, Duration};
use crate::daemon::worldmodel::WorldModel;
use crate::daemon::events::DerivedEvent;

#[derive(Clone)]
struct CachedBaseline {
    value: f64,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct BaselineApi {
    metric: String,
    worldmodel: Arc<WorldModel>,
    cache: Arc<Mutex<HashMap<String, CachedBaseline>>>,
    ttl: Duration,
}

impl BaselineApi {
    pub fn new(metric: String, worldmodel: Arc<WorldModel>) -> Self {
        Self {
            metric,
            worldmodel,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::hours(1), // 1 hour TTL
        }
    }

    /// Calculate average value (with caching)
    pub fn avg(&self) -> f64 {
        let cache_key = format!("{}_avg", self.metric);

        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.get(&cache_key) {
                if cached.expires_at > Utc::now() {
                    log::debug!("Baseline cache hit: {}", cache_key);
                    return cached.value;
                }
            }
        }

        // Calculate new value
        log::debug!("Baseline cache miss, computing: {}", cache_key);
        let value = self.compute_baseline();

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(cache_key, CachedBaseline {
                value,
                expires_at: Utc::now() + self.ttl,
            });
        }

        value
    }

    fn compute_baseline(&self) -> f64 {
        // Fixed 7-day window for MVP
        let target_window = Duration::days(7);
        let now = Utc::now();

        // Create new runtime for sync call (MVP only, same pattern as HistoryApi)
        let worldmodel = self.worldmodel.clone();
        let metric = self.metric.clone();

        let events = std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("Failed to create tokio runtime: {}", e);
                    return Vec::new();
                }
            };
            rt.block_on(async {
                worldmodel.query_derived_events(now - target_window, now).await
            })
        })
        .join()
        .unwrap_or_else(|_| {
            log::error!("Failed to query baseline events");
            Vec::new()
        });

        if events.is_empty() {
            log::warn!("No historical data for baseline '{}'", metric);
            return 0.0;
        }

        // Calculate metric-specific baseline
        match metric.as_str() {
            "file_changes" => {
                // Count file change events per hour
                let count = events.iter()
                    .filter(|e| matches!(e, DerivedEvent::Aggregated { .. }))
                    .count();
                let hours = target_window.num_hours().max(1);
                count as f64 / hours as f64
            }
            "coding_time" => {
                // Sum coding duration per hour
                // TODO: Extract duration from events
                0.0
            }
            _ => {
                log::warn!("Unknown baseline metric: {}", metric);
                0.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::WorldModelConfig;
    use crate::daemon::event_bus::DaemonEventBus;

    #[tokio::test]
    async fn test_baseline_api_avg_caching() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let baseline = BaselineApi::new("file_changes".to_string(), worldmodel);

        // First call - cache miss
        let value1 = baseline.avg();

        // Second call - cache hit (should be same value)
        let value2 = baseline.avg();

        assert_eq!(value1, value2);
    }

    #[tokio::test]
    async fn test_baseline_api_graceful_degradation() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let baseline = BaselineApi::new("file_changes".to_string(), worldmodel);

        // No historical data - should return 0.0
        let value = baseline.avg();
        assert_eq!(value, 0.0);
    }
}

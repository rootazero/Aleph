//! HistoryApi - Exposes WorldModel history to Rhai scripts

use crate::daemon::worldmodel::WorldModel;
use crate::daemon::dispatcher::scripting::helpers::parse_duration;
use super::event_collection::EventCollection;
use super::baseline::BaselineApi;
use crate::sync_primitives::Arc;
use chrono::Utc;

#[derive(Clone)]
pub struct HistoryApi {
    worldmodel: Arc<WorldModel>,
}

impl HistoryApi {
    pub fn new(worldmodel: Arc<WorldModel>) -> Self {
        Self { worldmodel }
    }

    /// Get events from last N duration
    /// Example: history.last("2h") -> events from last 2 hours
    pub async fn last_async(&self, duration_str: &str) -> EventCollection {
        let duration = match parse_duration(duration_str) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("Invalid duration string '{}': {}", duration_str, e);
                return EventCollection::empty();
            }
        };

        let now = Utc::now();
        let since = now - duration;

        // Query events from WorldModel
        let events = self.worldmodel.query_derived_events(since, now).await;
        EventCollection::new(events)
    }

    /// Get events from last N duration (sync version for Rhai)
    /// Example: history.last("2h") -> events from last 2 hours
    ///
    /// Note: For MVP, this creates a new runtime. Phase 5.2 will refactor to async.
    pub fn last(&self, duration_str: &str) -> EventCollection {
        let duration_str = duration_str.to_string();
        let worldmodel = self.worldmodel.clone();

        // Create new runtime for sync call (MVP only)
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("Failed to create tokio runtime: {}", e);
                    return EventCollection::empty();
                }
            };
            let api = HistoryApi::new(worldmodel);
            rt.block_on(api.last_async(&duration_str))
        })
        .join()
        .unwrap_or_else(|_| {
            log::error!("Failed to query history events");
            EventCollection::empty()
        })
    }

    /// Get baseline calculator for a metric
    pub fn baseline(&self, metric: &str) -> BaselineApi {
        BaselineApi::new(metric.to_string(), self.worldmodel.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::WorldModelConfig;
    use crate::daemon::event_bus::DaemonEventBus;

    #[tokio::test]
    async fn test_history_api_last_returns_empty_from_stub() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus.clone()).await.unwrap());

        let api = HistoryApi::new(worldmodel);
        let events = api.last_async("2h").await;

        // Stub WorldModel always returns empty
        assert_eq!(events.count(), 0);
    }

    #[tokio::test]
    async fn test_history_api_last_respects_time_window() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let api = HistoryApi::new(worldmodel);

        // Query very small window - should be empty
        let events = api.last_async("1s").await;
        assert_eq!(events.count(), 0);
    }

    #[tokio::test]
    async fn test_history_api_baseline() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let api = HistoryApi::new(worldmodel);
        let baseline = api.baseline("file_changes");

        // Should return 0.0 for no data
        assert_eq!(baseline.avg(), 0.0);
    }
}

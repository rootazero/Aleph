//! HistoryApi - Exposes WorldModel history to Rhai scripts

use crate::daemon::worldmodel::WorldModel;
use std::sync::Arc;
use super::event_collection::EventCollection;

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
    pub fn last(&self, _duration_str: &str) -> EventCollection {
        // TODO: Implement
        EventCollection::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::WorldModelConfig;
    use crate::daemon::event_bus::DaemonEventBus;
    use tokio;

    #[tokio::test]
    async fn test_history_api_last() {
        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let api = HistoryApi::new(worldmodel);
        let events = api.last("2h");

        // Should return empty collection (no events yet)
        assert_eq!(events.count(), 0);
    }
}

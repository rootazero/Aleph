//! Agent Message Bus
//!
//! Pub/Sub event bus for horizontal agent communication.
//! Supports tiered event delivery with zero-blocking latency.

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info};

use super::events::{AgentEvent, EventTier};
use crate::error::Result;

/// Channel capacity for each tier
const CRITICAL_CHANNEL_CAPACITY: usize = 100;
const IMPORTANT_CHANNEL_CAPACITY: usize = 500;
const INFO_CHANNEL_CAPACITY: usize = 1000;

/// Agent Message Bus
///
/// Provides pub/sub messaging for agent-to-agent communication.
/// Events are organized into three tiers with different delivery guarantees.
///
/// # Example
///
/// ```rust,ignore
/// let bus = AgentMessageBus::new();
///
/// // Subscribe to critical events
/// let mut rx = bus.subscribe(EventTier::Critical).await?;
///
/// // Publish event
/// bus.publish(AgentEvent::Critical(CriticalEvent::GlobalFailure {
///     error: "System overload".into(),
///     timestamp: now(),
/// })).await?;
///
/// // Receive event
/// if let Ok(event) = rx.recv().await {
///     println!("Received: {:?}", event);
/// }
/// ```
pub struct AgentMessageBus {
    /// Broadcast channels for each tier
    channels: Arc<RwLock<BusChannels>>,
    /// Event statistics
    stats: Arc<RwLock<BusStatistics>>,
}

struct BusChannels {
    critical: broadcast::Sender<AgentEvent>,
    important: broadcast::Sender<AgentEvent>,
    info: broadcast::Sender<AgentEvent>,
}

/// Bus statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct BusStatistics {
    pub critical_published: u64,
    pub important_published: u64,
    pub info_published: u64,
    pub critical_dropped: u64,
    pub important_dropped: u64,
    pub info_dropped: u64,
}

impl AgentMessageBus {
    /// Create a new message bus
    pub fn new() -> Self {
        let (critical_tx, _) = broadcast::channel(CRITICAL_CHANNEL_CAPACITY);
        let (important_tx, _) = broadcast::channel(IMPORTANT_CHANNEL_CAPACITY);
        let (info_tx, _) = broadcast::channel(INFO_CHANNEL_CAPACITY);

        Self {
            channels: Arc::new(RwLock::new(BusChannels {
                critical: critical_tx,
                important: important_tx,
                info: info_tx,
            })),
            stats: Arc::new(RwLock::new(BusStatistics::default())),
        }
    }

    /// Publish an event to the bus
    ///
    /// This is a non-blocking operation. If no subscribers exist,
    /// the event is silently dropped.
    pub async fn publish(&self, event: AgentEvent) -> Result<()> {
        let channels = self.channels.read().await;
        let tier = event.tier();

        let result = match &event {
            AgentEvent::Critical(_) => channels.critical.send(event),
            AgentEvent::Important(_) => channels.important.send(event),
            AgentEvent::Info(_) => channels.info.send(event),
        };

        // Update statistics
        let mut stats = self.stats.write().await;
        match tier {
            EventTier::Critical => stats.critical_published += 1,
            EventTier::Important => stats.important_published += 1,
            EventTier::Info => stats.info_published += 1,
        }

        match result {
            Ok(subscriber_count) => {
                debug!(
                    "Published {} event to {} subscribers",
                    tier, subscriber_count
                );
                Ok(())
            }
            Err(broadcast::error::SendError(_)) => {
                // No subscribers - this is not an error
                debug!("Published {} event with no subscribers", tier);
                Ok(())
            }
        }
    }

    /// Subscribe to events of a specific tier
    ///
    /// Returns a receiver that will receive all future events of the specified tier.
    pub async fn subscribe(&self, tier: EventTier) -> Result<broadcast::Receiver<AgentEvent>> {
        let channels = self.channels.read().await;

        let rx = match tier {
            EventTier::Critical => channels.critical.subscribe(),
            EventTier::Important => channels.important.subscribe(),
            EventTier::Info => channels.info.subscribe(),
        };

        info!("New subscriber for {} events", tier);
        Ok(rx)
    }

    /// Subscribe to all event tiers
    ///
    /// Returns a tuple of (critical_rx, important_rx, info_rx)
    pub async fn subscribe_all(
        &self,
    ) -> Result<(
        broadcast::Receiver<AgentEvent>,
        broadcast::Receiver<AgentEvent>,
        broadcast::Receiver<AgentEvent>,
    )> {
        let channels = self.channels.read().await;

        Ok((
            channels.critical.subscribe(),
            channels.important.subscribe(),
            channels.info.subscribe(),
        ))
    }

    /// Get current bus statistics
    pub async fn statistics(&self) -> BusStatistics {
        self.stats.read().await.clone()
    }

    /// Reset statistics
    pub async fn reset_statistics(&self) {
        let mut stats = self.stats.write().await;
        *stats = BusStatistics::default();
    }

    /// Get subscriber count for a tier
    pub async fn subscriber_count(&self, tier: EventTier) -> usize {
        let channels = self.channels.read().await;

        match tier {
            EventTier::Critical => channels.critical.receiver_count(),
            EventTier::Important => channels.important.receiver_count(),
            EventTier::Info => channels.info.receiver_count(),
        }
    }
}

impl Default for AgentMessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::events::{CriticalEvent, ImportantEvent, InfoEvent, FileOperation};

    #[tokio::test]
    async fn test_bus_creation() {
        let bus = AgentMessageBus::new();
        let stats = bus.statistics().await;

        assert_eq!(stats.critical_published, 0);
        assert_eq!(stats.important_published, 0);
        assert_eq!(stats.info_published, 0);
    }

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = AgentMessageBus::new();

        // Subscribe before publishing
        let mut rx = bus.subscribe(EventTier::Critical).await.unwrap();

        // Publish event
        let event = AgentEvent::Critical(CriticalEvent::GlobalFailure {
            error: "test error".into(),
            timestamp: 12345,
        });

        bus.publish(event.clone()).await.unwrap();

        // Receive event
        let received = rx.recv().await.unwrap();
        assert_eq!(received.tier(), EventTier::Critical);
        assert_eq!(received.timestamp(), 12345);
    }

    #[tokio::test]
    async fn test_publish_without_subscribers() {
        let bus = AgentMessageBus::new();

        // Publish without subscribers - should not error
        let event = AgentEvent::Info(InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test".into(),
            operation: FileOperation::Read,
            timestamp: 0,
        });

        let result = bus.publish(event).await;
        assert!(result.is_ok());

        // Statistics should still be updated
        let stats = bus.statistics().await;
        assert_eq!(stats.info_published, 1);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = AgentMessageBus::new();

        // Create multiple subscribers
        let mut rx1 = bus.subscribe(EventTier::Important).await.unwrap();
        let mut rx2 = bus.subscribe(EventTier::Important).await.unwrap();

        assert_eq!(bus.subscriber_count(EventTier::Important).await, 2);

        // Publish event
        let event = AgentEvent::Important(ImportantEvent::Hotspot {
            area: "auth/".into(),
            agent_count: 3,
            activity: "analysis".into(),
            timestamp: 0,
        });

        bus.publish(event).await.unwrap();

        // Both subscribers should receive
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        assert_eq!(received1.tier(), EventTier::Important);
        assert_eq!(received2.tier(), EventTier::Important);
    }

    #[tokio::test]
    async fn test_subscribe_all() {
        let bus = AgentMessageBus::new();

        let (mut critical_rx, mut important_rx, mut info_rx) =
            bus.subscribe_all().await.unwrap();

        // Publish events to all tiers
        bus.publish(AgentEvent::Critical(CriticalEvent::GlobalFailure {
            error: "test".into(),
            timestamp: 1,
        }))
        .await
        .unwrap();

        bus.publish(AgentEvent::Important(ImportantEvent::SwarmStateSummary {
            summary: "test".into(),
            timestamp: 2,
        }))
        .await
        .unwrap();

        bus.publish(AgentEvent::Info(InfoEvent::ToolExecuted {
            agent_id: "agent_1".into(),
            tool: "grep".into(),
            path: None,
            timestamp: 3,
        }))
        .await
        .unwrap();

        // All receivers should get their respective events
        assert!(critical_rx.recv().await.is_ok());
        assert!(important_rx.recv().await.is_ok());
        assert!(info_rx.recv().await.is_ok());
    }

    #[tokio::test]
    async fn test_statistics() {
        let bus = AgentMessageBus::new();
        let _rx = bus.subscribe(EventTier::Critical).await.unwrap();

        // Publish multiple events
        for i in 0..5 {
            bus.publish(AgentEvent::Critical(CriticalEvent::GlobalFailure {
                error: format!("error {}", i),
                timestamp: i,
            }))
            .await
            .unwrap();
        }

        let stats = bus.statistics().await;
        assert_eq!(stats.critical_published, 5);
    }

    #[tokio::test]
    async fn test_tier_isolation() {
        let bus = AgentMessageBus::new();

        // Subscribe only to Critical
        let mut critical_rx = bus.subscribe(EventTier::Critical).await.unwrap();

        // Publish Important event
        bus.publish(AgentEvent::Important(ImportantEvent::Hotspot {
            area: "test".into(),
            agent_count: 1,
            activity: "test".into(),
            timestamp: 0,
        }))
        .await
        .unwrap();

        // Critical subscriber should not receive Important event
        tokio::select! {
            _ = critical_rx.recv() => panic!("Should not receive Important event"),
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                // Timeout is expected
            }
        }
    }
}

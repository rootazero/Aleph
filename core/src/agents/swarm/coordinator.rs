//! Swarm Coordinator
//!
//! Unified management of all swarm intelligence components.

use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use super::aggregator::{IntelligenceLayer, SemanticAggregator};
use super::bus::AgentMessageBus;
use super::collective_memory::CollectiveMemory;
use super::context_injector::ContextInjector;
use crate::error::Result;

/// Swarm Coordinator Configuration
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    /// Enable intelligence layer for LLM summarization
    pub enable_intelligence: bool,
    /// Intelligence layer summary interval
    pub summary_interval_secs: u64,
    /// Minimum events before summarizing
    pub min_events_for_summary: usize,
    /// Context window size (number of recent updates to keep)
    pub context_window_size: usize,
    /// Collective memory capacity (max events to store)
    pub memory_capacity: usize,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enable_intelligence: true,
            summary_interval_secs: 5,
            min_events_for_summary: 10,
            context_window_size: 5,
            memory_capacity: 10000,
        }
    }
}

/// Swarm Coordinator
///
/// Manages all swarm intelligence components and their lifecycle.
pub struct SwarmCoordinator {
    /// Event bus
    pub bus: Arc<AgentMessageBus>,
    /// Semantic aggregator
    pub aggregator: Arc<SemanticAggregator>,
    /// Context injector
    pub injector: Arc<ContextInjector>,
    /// Collective memory
    pub memory: Arc<CollectiveMemory>,
}

impl SwarmCoordinator {
    /// Initialize swarm coordinator with default configuration
    pub async fn new() -> Result<Self> {
        Self::with_config(SwarmConfig::default()).await
    }

    /// Initialize swarm coordinator with custom configuration
    pub async fn with_config(config: SwarmConfig) -> Result<Self> {
        info!("Initializing swarm coordinator");

        // Create event bus
        let bus = Arc::new(AgentMessageBus::new());

        // Create semantic aggregator
        let mut aggregator = SemanticAggregator::new(bus.clone());

        // Add intelligence layer if enabled
        if config.enable_intelligence {
            let intelligence = Arc::new(IntelligenceLayer::new(
                Duration::from_secs(config.summary_interval_secs),
                config.min_events_for_summary,
            ));
            aggregator = aggregator.with_intelligence_layer(intelligence);
        }
        let aggregator = Arc::new(aggregator);

        // Create context injector
        let injector = Arc::new(ContextInjector::with_window_size(
            bus.clone(),
            config.context_window_size,
        ));

        // Create collective memory
        let memory = Arc::new(CollectiveMemory::with_capacity(
            bus.clone(),
            config.memory_capacity,
        ));

        info!("Swarm coordinator initialized");

        Ok(Self {
            bus,
            aggregator,
            injector,
            memory,
        })
    }

    /// Start all background tasks
    pub async fn start(self: Arc<Self>) {
        info!("Starting swarm coordinator background tasks");

        // Start semantic aggregator
        let aggregator = self.aggregator.clone();
        tokio::spawn(async move {
            aggregator.run().await;
        });

        // Start context injector
        let injector = self.injector.clone();
        tokio::spawn(async move {
            injector.run().await;
        });

        // Start collective memory
        let memory = self.memory.clone();
        tokio::spawn(async move {
            memory.run().await;
        });

        info!("Swarm coordinator background tasks started");
    }

    /// Get statistics about the swarm
    pub async fn statistics(&self) -> SwarmStatistics {
        SwarmStatistics {
            bus_stats: self.bus.statistics().await,
            context_window_size: self.injector.window_size().await,
            memory_event_count: self.memory.event_count().await,
        }
    }

    /// Start background statistics logging
    ///
    /// Logs event statistics every 60 seconds for monitoring.
    pub fn start_statistics_logging(&self) {
        let bus = self.bus.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;

                let stats = bus.statistics().await;
                info!(
                    total_critical = stats.critical_published,
                    total_important = stats.important_published,
                    total_info = stats.info_published,
                    "Swarm event statistics"
                );
            }
        });
    }
}

/// Swarm statistics
#[derive(Debug, Clone)]
pub struct SwarmStatistics {
    pub bus_stats: super::bus::BusStatistics,
    pub context_window_size: usize,
    pub memory_event_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_coordinator_creation() {
        let coordinator = SwarmCoordinator::new().await.unwrap();

        let stats = coordinator.statistics().await;
        assert_eq!(stats.context_window_size, 0);
        assert_eq!(stats.memory_event_count, 0);
    }

    #[tokio::test]
    async fn test_coordinator_with_custom_config() {
        let config = SwarmConfig {
            enable_intelligence: false,
            summary_interval_secs: 10,
            min_events_for_summary: 20,
            context_window_size: 3,
            memory_capacity: 5000,
        };

        let coordinator = SwarmCoordinator::with_config(config).await.unwrap();

        let stats = coordinator.statistics().await;
        assert_eq!(stats.context_window_size, 0);
        assert_eq!(stats.memory_event_count, 0);
    }

    #[tokio::test]
    async fn test_coordinator_start() {
        let coordinator = Arc::new(SwarmCoordinator::new().await.unwrap());

        // Start background tasks
        coordinator.clone().start().await;

        // Give tasks time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Coordinator should still be accessible
        let stats = coordinator.statistics().await;
        assert_eq!(stats.context_window_size, 0);
    }
}

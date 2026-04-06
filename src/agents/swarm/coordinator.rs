//! Swarm Coordinator
//!
//! Unified management of all swarm intelligence components.

use crate::sync_primitives::Arc;
use std::time::Duration;
use tracing::info;

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

use super::aggregator::{IntelligenceLayer, SemanticAggregator};
use super::bus::AgentMessageBus;
use super::collective_memory::CollectiveMemory;
use super::context_injector::ContextInjector;
use super::events::*;
use super::tasks::CoordTaskStore;
use crate::error::Result;

/// Events from the agent loop that can be published to the swarm
#[derive(Debug, Clone)]
pub enum AgentLoopEvent {
    ActionInitiated {
        agent_id: String,
        action_type: String,
        target: String,
    },
    ActionCompleted {
        agent_id: String,
        action_type: String,
        result: String,
        duration_ms: u64,
    },
    DecisionMade {
        agent_id: String,
        decision: String,
        affected_files: Vec<String>,
    },
    InsightCaptured {
        agent_id: String,
        insight: String,
        severity: InsightSeverity,
    },
}

/// Severity level for captured insights
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightSeverity {
    Critical,
    Warning,
    Info,
}

/// Swarm Coordinator Configuration
#[derive(Clone)]
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
    /// Optional AI provider for intelligence layer LLM summarization.
    pub intelligence_provider: Option<Arc<dyn crate::providers::AiProvider>>,
}

impl std::fmt::Debug for SwarmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwarmConfig")
            .field("enable_intelligence", &self.enable_intelligence)
            .field("summary_interval_secs", &self.summary_interval_secs)
            .field("min_events_for_summary", &self.min_events_for_summary)
            .field("context_window_size", &self.context_window_size)
            .field("memory_capacity", &self.memory_capacity)
            .field(
                "intelligence_provider",
                &self.intelligence_provider.is_some(),
            )
            .finish()
    }
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enable_intelligence: true,
            summary_interval_secs: 5,
            min_events_for_summary: 10,
            context_window_size: 5,
            memory_capacity: 10000,
            intelligence_provider: None,
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
    /// Optional task coordination store
    task_store: Option<Arc<dyn CoordTaskStore>>,
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
            let mut layer = IntelligenceLayer::new(
                Duration::from_secs(config.summary_interval_secs),
                config.min_events_for_summary,
            );
            if let Some(provider) = config.intelligence_provider {
                layer = layer.with_provider(provider);
            }
            aggregator = aggregator.with_intelligence_layer(Arc::new(layer));
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
            task_store: None,
        })
    }

    /// Attach a task coordination store
    ///
    /// Rebuilds the context injector so it includes task context in prompts.
    /// Must be called before `start()` (i.e. before the Arc is shared).
    ///
    /// Returns `Err` if the injector Arc has already been cloned (e.g. after `start()`).
    pub fn with_task_store(mut self, store: Arc<dyn CoordTaskStore>) -> Result<Self> {
        let inner = Arc::try_unwrap(self.injector).map_err(|_| {
            crate::error::AlephError::config(
                "with_task_store must be called before start() — injector Arc already shared",
            )
        })?;
        self.injector = Arc::new(inner.with_task_store(store.clone()));
        self.task_store = Some(store);
        Ok(self)
    }

    /// Attach an inbox context provider for team message awareness.
    ///
    /// Must be called before `start()` (i.e. before the injector Arc is shared).
    pub fn with_inbox_provider(
        mut self,
        provider: Arc<dyn crate::teams::context::InboxContextProvider>,
    ) -> Result<Self> {
        let inner = Arc::try_unwrap(self.injector).map_err(|_| {
            crate::error::AlephError::config(
                "with_inbox_provider must be called before start() — injector Arc already shared",
            )
        })?;
        self.injector = Arc::new(inner.with_inbox_provider(provider));
        Ok(self)
    }

    /// Inject an AI provider into the intelligence layer for LLM-powered summarization.
    ///
    /// This supports deferred wiring: the coordinator can be constructed and even
    /// started before the AI provider is available. The intelligence layer will
    /// pick up the provider on its next summarization cycle.
    ///
    /// Returns `true` if the provider was successfully set.
    pub fn set_intelligence_provider(
        &self,
        provider: Arc<dyn crate::providers::AiProvider>,
    ) -> bool {
        self.aggregator.set_intelligence_provider(provider)
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

    /// Publish AgentLoop event (converts to internal event and classifies by tier)
    ///
    /// This method converts AgentLoopEvent to internal swarm events and publishes
    /// them to the message bus. Events are classified by tier for proper delivery:
    /// - Critical: Interrupt-driven delivery
    /// - Important: Passive injection before Think phase
    /// - Info: On-demand query via tools
    pub async fn publish_event(&self, event: AgentLoopEvent) {
        // Convert to internal event and classify by tier
        let swarm_event = match event {
            AgentLoopEvent::ActionInitiated {
                agent_id,
                action_type,
                target,
            } => AgentEvent::Info(InfoEvent::ActionStarted {
                agent_id,
                action_type,
                target: Some(target),
                timestamp: now_epoch(),
            }),
            AgentLoopEvent::ActionCompleted {
                agent_id,
                action_type,
                result,
                duration_ms,
            } => AgentEvent::Important(ImportantEvent::ToolExecuted {
                agent_id,
                tool_name: action_type,
                result: format!("{:?}", result),
                duration_ms,
                timestamp: now_epoch(),
            }),
            AgentLoopEvent::DecisionMade {
                agent_id,
                decision,
                affected_files,
            } => AgentEvent::Important(ImportantEvent::DecisionBroadcast {
                agent_id,
                decision,
                affected_files,
                timestamp: now_epoch(),
            }),
            AgentLoopEvent::InsightCaptured {
                agent_id,
                insight,
                severity,
            } => match severity {
                InsightSeverity::Critical => AgentEvent::Critical(CriticalEvent::ErrorDetected {
                    agent_id,
                    error_message: insight,
                    timestamp: now_epoch(),
                }),
                _ => AgentEvent::Info(InfoEvent::InsightCaptured {
                    agent_id,
                    insight,
                    timestamp: now_epoch(),
                }),
            },
        };

        let event_tier = match &swarm_event {
            AgentEvent::Critical(_) => "critical",
            AgentEvent::Important(_) => "important",
            AgentEvent::Info(_) => "info",
        };
        tracing::info!(
            subsystem = "swarm",
            event = "event_published",
            event_tier = event_tier,
            "swarm coordinator published event to bus"
        );

        // Publish to bus
        if let Err(e) = self.bus.publish(swarm_event).await {
            tracing::warn!("Failed to publish swarm event: {}", e);
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
            intelligence_provider: None,
        };

        let coordinator = SwarmCoordinator::with_config(config).await.unwrap();

        let stats = coordinator.statistics().await;
        assert_eq!(stats.context_window_size, 0);
        assert_eq!(stats.memory_event_count, 0);
    }

    #[tokio::test]
    async fn test_coordinator_with_inbox_provider() {
        use crate::teams::context::{InboxContext, InboxContextProvider};
        use async_trait::async_trait;

        struct MockInboxProvider;

        #[async_trait]
        impl InboxContextProvider for MockInboxProvider {
            async fn get_inbox_context(&self, _agent_id: &str) -> InboxContext {
                InboxContext::default()
            }
        }

        let coordinator = SwarmCoordinator::new().await.unwrap();
        let coordinator = coordinator
            .with_inbox_provider(Arc::new(MockInboxProvider))
            .unwrap();
        let stats = coordinator.statistics().await;
        assert_eq!(stats.context_window_size, 0);
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

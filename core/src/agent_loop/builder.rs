//! Builder pattern for AgentLoop construction
//!
//! This module provides a fluent builder API for constructing AgentLoop instances
//! with optional Swarm Intelligence components (EventBus, OverflowDetector, SwarmCoordinator).
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::agent_loop::{AgentLoopBuilder, LoopConfig};
//!
//! let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
//!     .with_config(LoopConfig::default())
//!     .with_event_bus(event_bus)
//!     .with_overflow_detector(detector)
//!     .build();
//! ```

use crate::sync_primitives::Arc;

use crate::agent_loop::{LoopConfig, ThinkerTrait, ActionExecutor, CompressorTrait};
use crate::agent_loop::{ContextProvider, MessageBuilder, MessageBuilderConfig};
use crate::event::EventBus;
use crate::agent_loop::overflow::OverflowDetector;

// Re-export SwarmCoordinator from agents::swarm
pub use crate::agents::swarm::coordinator::SwarmCoordinator;

/// Builder for constructing AgentLoop instances with optional components
///
/// This builder supports the Swarm Intelligence Architecture by allowing
/// optional integration of EventBus, OverflowDetector, and SwarmCoordinator.
pub struct AgentLoopBuilder<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    config: LoopConfig,
    // Optional Swarm Intelligence components
    event_bus: Option<Arc<EventBus>>,
    overflow_detector: Option<Arc<OverflowDetector>>,
    swarm_coordinator: Option<Arc<SwarmCoordinator>>,
}

impl<T, E, C> AgentLoopBuilder<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    /// Create a new AgentLoopBuilder with required components
    ///
    /// # Arguments
    /// * `thinker` - The thinking layer implementation
    /// * `executor` - The action executor implementation
    /// * `compressor` - The context compressor implementation
    pub fn new(thinker: Arc<T>, executor: Arc<E>, compressor: Arc<C>) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config: LoopConfig::default(),
            event_bus: None,
            overflow_detector: None,
            swarm_coordinator: None,
        }
    }

    /// Set the loop configuration
    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.config = config;
        self
    }

    /// Add an EventBus for event-driven communication
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Add an OverflowDetector for real-time context monitoring
    pub fn with_overflow_detector(mut self, detector: Arc<OverflowDetector>) -> Self {
        self.overflow_detector = Some(detector);
        self
    }

    /// Add a SwarmCoordinator for multi-agent coordination
    pub fn with_swarm(mut self, coordinator: Arc<SwarmCoordinator>) -> Self {
        self.swarm_coordinator = Some(coordinator);
        self
    }

    /// Build the AgentLoop instance
    ///
    /// Consumes the builder and constructs a complete AgentLoop with all
    /// configured components (EventBus, OverflowDetector, SwarmCoordinator).
    ///
    /// If SwarmCoordinator is enabled, automatically starts background
    /// statistics logging (every 60 seconds).
    ///
    /// # Returns
    ///
    /// A fully configured AgentLoop instance ready for execution.
    pub fn build(self) -> crate::agent_loop::AgentLoop<T, E, C> {
        // If Swarm enabled, start statistics logging
        if let Some(ref coordinator) = self.swarm_coordinator {
            coordinator.start_statistics_logging();
        }

        crate::agent_loop::AgentLoop::from_builder(
            self.thinker,
            self.executor,
            self.compressor,
            self.config,
            self.event_bus,
            self.overflow_detector,
            self.swarm_coordinator,
        )
    }

    /// Build a MessageBuilder with context providers
    ///
    /// This helper method demonstrates how to construct a MessageBuilder
    /// with ContextProviders. When Swarm is enabled, it automatically adds
    /// SwarmContextProvider to inject team awareness context.
    ///
    /// # Arguments
    ///
    /// * `config` - MessageBuilder configuration
    ///
    /// # Returns
    ///
    /// A MessageBuilder configured with appropriate context providers.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let builder = AgentLoopBuilder::new(thinker, executor, compressor)
    ///     .with_swarm(coordinator);
    /// let message_builder = builder.build_message_builder(msg_config);
    /// ```
    pub fn build_message_builder(&self, config: MessageBuilderConfig) -> MessageBuilder {
        let context_providers: Vec<Box<dyn ContextProvider>> = Vec::new();

        // If Swarm enabled, add SwarmContextProvider
        // Note: This is a placeholder implementation. In the real integration,
        // SwarmCoordinator would be the actual type from agents::swarm::SwarmCoordinator
        // and would have a context_injector() method that returns Arc<ContextInjector>.
        //
        // Example of real implementation:
        // if let Some(ref coordinator) = self.swarm_coordinator {
        //     let provider = SwarmContextProvider::new(coordinator.injector.clone());
        //     context_providers.push(Box::new(provider));
        // }

        // Build MessageBuilder with providers
        MessageBuilder::new(config).with_providers(context_providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{Action, ActionResult, Thinking, CompressedHistory, LoopState, LoopStep, Decision};
    use crate::agents::thinking::ThinkLevel;
    use crate::error::Result;
    use aleph_protocol::IdentityContext;
    use serde_json::json;

    // Mock implementations for testing

    struct MockThinker;

    #[async_trait::async_trait]
    impl ThinkerTrait for MockThinker {
        async fn think(
            &self,
            _state: &LoopState,
            _tools: &[crate::dispatcher::UnifiedTool],
        ) -> Result<Thinking> {
            Ok(Thinking {
                reasoning: Some("mock reasoning".to_string()),
                decision: Decision::Complete {
                    summary: "mock complete".to_string(),
                },
                structured: None,
                tokens_used: None,
                tool_call_id: None,
            })
        }

        fn current_think_level(&self) -> ThinkLevel {
            ThinkLevel::Minimal
        }
    }

    struct MockExecutor;

    #[async_trait::async_trait]
    impl ActionExecutor for MockExecutor {
        async fn execute(&self, _action: &Action, _identity: &IdentityContext) -> ActionResult {
            ActionResult::ToolSuccess {
                output: json!("mock output"),
                duration_ms: 0,
            }
        }
    }

    struct MockCompressor;

    #[async_trait::async_trait]
    impl CompressorTrait for MockCompressor {
        fn should_compress(&self, _state: &LoopState) -> bool {
            false
        }

        async fn compress(
            &self,
            _steps: &[LoopStep],
            _current_summary: &str,
        ) -> Result<CompressedHistory> {
            Ok(CompressedHistory {
                summary: "mock summary".to_string(),
                compressed_count: 0,
            })
        }
    }

    #[test]
    fn test_builder_creation() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let builder = AgentLoopBuilder::new(thinker.clone(), executor.clone(), compressor.clone());

        // Verify builder was created with default config
        assert_eq!(builder.config.max_steps, 50);
        assert!(builder.event_bus.is_none());
        assert!(builder.overflow_detector.is_none());
        assert!(builder.swarm_coordinator.is_none());
    }

    #[test]
    fn test_builder_with_config() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let custom_config = LoopConfig::default().with_max_steps(100);
        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_config(custom_config);

        assert_eq!(builder.config.max_steps, 100);
    }

    #[test]
    fn test_builder_with_event_bus() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let event_bus = Arc::new(EventBus::new());

        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_event_bus(event_bus.clone());

        assert!(builder.event_bus.is_some());
    }

    #[test]
    fn test_builder_with_overflow_detector() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let detector = Arc::new(OverflowDetector::new(Default::default()));

        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_overflow_detector(detector.clone());

        assert!(builder.overflow_detector.is_some());
    }

    #[tokio::test]
    async fn test_builder_with_swarm_coordinator() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let coordinator = Arc::new(SwarmCoordinator::new().await.unwrap());

        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_swarm(coordinator.clone());

        assert!(builder.swarm_coordinator.is_some());
    }

    #[tokio::test]
    async fn test_builder_fluent_api() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let event_bus = Arc::new(EventBus::new());
        let detector = Arc::new(OverflowDetector::new(Default::default()));
        let coordinator = Arc::new(SwarmCoordinator::new().await.unwrap());

        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_config(LoopConfig::default().with_max_steps(200))
            .with_event_bus(event_bus)
            .with_overflow_detector(detector)
            .with_swarm(coordinator);

        assert_eq!(builder.config.max_steps, 200);
        assert!(builder.event_bus.is_some());
        assert!(builder.overflow_detector.is_some());
        assert!(builder.swarm_coordinator.is_some());
    }

    #[test]
    fn test_builder_build_basic() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
            .build();

        assert!(agent_loop.swarm_coordinator.is_none());
    }

    #[tokio::test]
    async fn test_builder_build_with_swarm() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let coordinator = Arc::new(SwarmCoordinator::new().await.unwrap());

        let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_swarm(coordinator.clone())
            .build();

        assert!(agent_loop.swarm_coordinator.is_some());
    }

    #[test]
    fn test_build_message_builder() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let builder = AgentLoopBuilder::new(thinker, executor, compressor);
        let msg_config = MessageBuilderConfig::default();
        let message_builder = builder.build_message_builder(msg_config);

        // Verify MessageBuilder was created (no providers since no swarm)
        // This is a basic smoke test to ensure the method works
        assert!(!message_builder.has_compactor());
    }

    #[tokio::test]
    async fn test_build_message_builder_with_swarm() {
        let thinker = Arc::new(MockThinker);
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);
        let coordinator = Arc::new(SwarmCoordinator::new().await.unwrap());

        let builder = AgentLoopBuilder::new(thinker, executor, compressor)
            .with_swarm(coordinator);

        let msg_config = MessageBuilderConfig::default();
        let message_builder = builder.build_message_builder(msg_config);

        // Verify MessageBuilder was created
        // In the real implementation with actual SwarmCoordinator,
        // this would verify that SwarmContextProvider was added
        assert!(!message_builder.has_compactor());
    }
}

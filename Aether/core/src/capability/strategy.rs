//! Capability Strategy trait for pluggable capability execution.
//!
//! This module defines the `CapabilityStrategy` trait which allows different
//! capabilities (Memory, Search, MCP, Video) to be implemented as independent,
//! pluggable strategies.
//!
//! # Architecture
//!
//! The strategy pattern enables:
//! - Independent testing of each capability
//! - Runtime enable/disable of capabilities
//! - Easy addition of new capabilities
//! - Decoupled capability execution logic

use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use async_trait::async_trait;
use std::sync::Arc;

/// Strategy trait for capability execution
///
/// Each capability (Memory, Search, MCP, Video) implements this trait
/// to provide a pluggable execution strategy. Strategies are registered
/// with the `CompositeCapabilityExecutor` and executed in priority order.
///
/// # Example Implementation
///
/// ```ignore
/// pub struct MemoryStrategy {
///     memory_db: Option<Arc<VectorDatabase>>,
///     // ...
/// }
///
/// #[async_trait]
/// impl CapabilityStrategy for MemoryStrategy {
///     fn capability_type(&self) -> Capability {
///         Capability::Memory
///     }
///
///     fn priority(&self) -> u32 {
///         0 // Memory executes first
///     }
///
///     fn is_available(&self) -> bool {
///         self.memory_db.is_some()
///     }
///
///     async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload> {
///         // Execute memory retrieval
///         Ok(payload)
///     }
/// }
/// ```
#[async_trait]
pub trait CapabilityStrategy: Send + Sync {
    /// Get the capability type this strategy handles
    ///
    /// Used for matching against requested capabilities in the payload.
    fn capability_type(&self) -> Capability;

    /// Get the priority for execution ordering
    ///
    /// Lower values = higher priority (executed first).
    /// Default priorities:
    /// - Memory: 0
    /// - Search: 1
    /// - MCP: 2
    /// - Video: 3
    fn priority(&self) -> u32;

    /// Check if this strategy is available/configured
    ///
    /// Returns false if required dependencies are missing
    /// (e.g., no database for Memory, no API key for Search).
    fn is_available(&self) -> bool;

    /// Execute the capability, enriching the payload with context
    ///
    /// This is the main execution method. It should:
    /// 1. Check if execution is needed/possible
    /// 2. Perform the capability-specific operation
    /// 3. Populate the appropriate context field in the payload
    /// 4. Return the enriched payload
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload to enrich
    ///
    /// # Returns
    ///
    /// The enriched payload with capability-specific context added
    async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload>;

    /// Get strategy name for logging
    ///
    /// Defaults to the capability type name.
    fn name(&self) -> &str {
        match self.capability_type() {
            Capability::Memory => "memory",
            Capability::Search => "search",
            Capability::Mcp => "mcp",
            Capability::Video => "video",
            Capability::Skills => "skills",
        }
    }

    /// Check if this capability is enabled in the payload config
    ///
    /// Default implementation checks if the capability is in the payload's
    /// capability list.
    fn is_enabled_for(&self, payload: &AgentPayload) -> bool {
        payload.config.capabilities.contains(&self.capability_type())
    }
}

/// Composite executor that manages multiple capability strategies
///
/// This executor:
/// - Maintains a registry of capability strategies
/// - Sorts strategies by priority before execution
/// - Executes enabled and available strategies in order
/// - Gracefully handles missing or unavailable strategies
pub struct CompositeCapabilityExecutor {
    strategies: Vec<Arc<dyn CapabilityStrategy>>,
}

impl CompositeCapabilityExecutor {
    /// Create a new empty composite executor
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
        }
    }

    /// Register a capability strategy
    ///
    /// Strategies are automatically sorted by priority after registration.
    pub fn register(&mut self, strategy: Arc<dyn CapabilityStrategy>) {
        let name = strategy.name().to_string();
        let priority = strategy.priority();
        self.strategies.push(strategy);
        self.strategies.sort_by_key(|s| s.priority());
        tracing::debug!(
            strategy = %name,
            priority = priority,
            "Registered capability strategy"
        );
    }

    /// Builder method to register a strategy
    pub fn with_strategy(mut self, strategy: Arc<dyn CapabilityStrategy>) -> Self {
        self.register(strategy);
        self
    }

    /// Get a strategy by capability type
    pub fn get_strategy(&self, capability: &Capability) -> Option<&dyn CapabilityStrategy> {
        self.strategies
            .iter()
            .find(|s| &s.capability_type() == capability)
            .map(|s| s.as_ref())
    }

    /// Check if a capability strategy is registered
    pub fn has_strategy(&self, capability: &Capability) -> bool {
        self.strategies.iter().any(|s| &s.capability_type() == capability)
    }

    /// Get all registered strategies
    pub fn strategies(&self) -> &[Arc<dyn CapabilityStrategy>] {
        &self.strategies
    }

    /// Execute all enabled and available capabilities in priority order
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload to enrich
    ///
    /// # Returns
    ///
    /// The enriched payload with all capability contexts added
    pub async fn execute_all(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Get requested capabilities sorted by priority
        let requested = Capability::sort_by_priority(payload.config.capabilities.clone());

        tracing::info!(
            capabilities = ?requested,
            strategies_registered = self.strategies.len(),
            "Executing capabilities via strategy pattern"
        );

        for capability in requested {
            if let Some(strategy) = self.get_strategy(&capability) {
                if strategy.is_available() {
                    tracing::debug!(
                        strategy = %strategy.name(),
                        "Executing capability strategy"
                    );
                    payload = strategy.execute(payload).await?;
                } else {
                    tracing::warn!(
                        strategy = %strategy.name(),
                        "Strategy registered but not available (missing dependencies)"
                    );
                }
            } else {
                tracing::warn!(
                    capability = ?capability,
                    "No strategy registered for capability"
                );
            }
        }

        Ok(payload)
    }
}

impl Default for CompositeCapabilityExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    /// Mock strategy for testing
    struct MockStrategy {
        capability: Capability,
        priority: u32,
        available: bool,
        executed: std::sync::atomic::AtomicBool,
    }

    impl MockStrategy {
        fn new(capability: Capability, priority: u32, available: bool) -> Self {
            Self {
                capability,
                priority,
                available,
                executed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn was_executed(&self) -> bool {
            self.executed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CapabilityStrategy for MockStrategy {
        fn capability_type(&self) -> Capability {
            self.capability.clone()
        }

        fn priority(&self) -> u32 {
            self.priority
        }

        fn is_available(&self) -> bool {
            self.available
        }

        async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload> {
            self.executed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(payload)
        }
    }

    #[tokio::test]
    async fn test_composite_executor_empty() {
        let executor = CompositeCapabilityExecutor::new();
        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = executor.execute_all(payload).await.unwrap();
        assert_eq!(result.user_input, "Test");
    }

    #[tokio::test]
    async fn test_composite_executor_priority_ordering() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Search, 1, true)))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)));

        // Verify strategies are sorted by priority
        assert_eq!(executor.strategies()[0].capability_type(), Capability::Memory);
        assert_eq!(executor.strategies()[1].capability_type(), Capability::Search);
    }

    #[tokio::test]
    async fn test_composite_executor_skips_unavailable() {
        let memory_strategy = Arc::new(MockStrategy::new(Capability::Memory, 0, false));
        let search_strategy = Arc::new(MockStrategy::new(Capability::Search, 1, true));

        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(memory_strategy.clone())
            .with_strategy(search_strategy.clone());

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory, Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let _ = executor.execute_all(payload).await.unwrap();

        // Memory should be skipped (not available)
        assert!(!memory_strategy.was_executed());
        // Search should be executed
        assert!(search_strategy.was_executed());
    }

    #[tokio::test]
    async fn test_has_strategy() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)));

        assert!(executor.has_strategy(&Capability::Memory));
        assert!(!executor.has_strategy(&Capability::Search));
    }
}

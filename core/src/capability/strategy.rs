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
//! - Configuration validation and health checking
//!
//! # Lifecycle Methods
//!
//! Each strategy supports the following lifecycle:
//! 1. `validate_config()` - Called during initialization to verify configuration
//! 2. `is_available()` - Runtime check for dependency availability
//! 3. `health_check()` - Async health verification (e.g., API connectivity)
//! 4. `execute()` - Main capability execution

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
///     memory_db: Option<MemoryBackend>,
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
            Capability::Mcp => "mcp",
            Capability::Skills => "skills",
        }
    }

    /// Check if this capability is enabled in the payload config
    ///
    /// Default implementation checks if the capability is in the payload's
    /// capability list.
    fn is_enabled_for(&self, payload: &AgentPayload) -> bool {
        payload
            .config
            .capabilities
            .contains(&self.capability_type())
    }

    /// Validate the strategy configuration
    ///
    /// Called during initialization to verify that the strategy is properly
    /// configured. This is a synchronous check for configuration validity,
    /// NOT a runtime connectivity check.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if configuration is valid
    /// - `Err(...)` with details if configuration is invalid
    ///
    /// # Default Implementation
    ///
    /// Returns `Ok(())` by default. Override to add validation logic.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn validate_config(&self) -> Result<()> {
    ///     if self.api_key.is_empty() {
    ///         return Err(AlephError::Config("API key is required".into()));
    ///     }
    ///     Ok(())
    /// }
    /// ```
    fn validate_config(&self) -> Result<()> {
        Ok(())
    }

    /// Perform an async health check on the strategy
    ///
    /// This method verifies that the strategy can actually perform its function.
    /// Unlike `is_available()` which checks configuration, this method can
    /// perform actual connectivity tests (e.g., ping an API).
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if the strategy is healthy and operational
    /// - `Ok(false)` if the strategy is unhealthy but no error occurred
    /// - `Err(...)` if an error occurred during the health check
    ///
    /// # Default Implementation
    ///
    /// Returns `Ok(self.is_available())` by default. Override for deeper checks.
    ///
    /// # Example
    ///
    /// ```ignore
    /// async fn health_check(&self) -> Result<bool> {
    ///     // Try to make a simple API call
    ///     match self.client.ping().await {
    ///         Ok(_) => Ok(true),
    ///         Err(e) => {
    ///             tracing::warn!("Health check failed: {}", e);
    ///             Ok(false)
    ///         }
    ///     }
    /// }
    /// ```
    async fn health_check(&self) -> Result<bool> {
        Ok(self.is_available())
    }

    /// Get detailed status information for debugging
    ///
    /// Returns a map of status key-value pairs for diagnostics.
    ///
    /// # Default Implementation
    ///
    /// Returns basic status with availability and capability type.
    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert(
            "capability".to_string(),
            format!("{:?}", self.capability_type()),
        );
        info.insert("name".to_string(), self.name().to_string());
        info.insert("priority".to_string(), self.priority().to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info
    }
}

/// Health status for a single capability
#[derive(Debug, Clone)]
pub struct CapabilityHealth {
    /// The capability type
    pub capability: Capability,
    /// Strategy name
    pub name: String,
    /// Whether configuration is valid
    pub config_valid: bool,
    /// Configuration validation error (if any)
    pub config_error: Option<String>,
    /// Whether the strategy is available
    pub available: bool,
    /// Whether the strategy is healthy (from health_check)
    pub healthy: bool,
    /// Health check error (if any)
    pub health_error: Option<String>,
    /// Additional status info
    pub status_info: std::collections::HashMap<String, String>,
}

impl CapabilityHealth {
    /// Check if the capability is fully operational
    pub fn is_operational(&self) -> bool {
        self.config_valid && self.available && self.healthy
    }
}

/// Composite executor that manages multiple capability strategies
///
/// This executor:
/// - Maintains a registry of capability strategies
/// - Sorts strategies by priority before execution
/// - Executes enabled and available strategies in order
/// - Gracefully handles missing or unavailable strategies
/// - Provides validation and health checking for all strategies
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
        self.strategies
            .iter()
            .any(|s| &s.capability_type() == capability)
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

    /// Validate configuration for all registered strategies
    ///
    /// # Returns
    ///
    /// A list of (Capability, Result) pairs indicating validation status.
    /// Strategies with invalid config will have `Err(...)` results.
    pub fn validate_all(&self) -> Vec<(Capability, Result<()>)> {
        self.strategies
            .iter()
            .map(|s| (s.capability_type(), s.validate_config()))
            .collect()
    }

    /// Validate a specific capability's configuration
    ///
    /// # Returns
    ///
    /// - `Ok(())` if configuration is valid or capability not registered
    /// - `Err(...)` if configuration is invalid
    pub fn validate(&self, capability: &Capability) -> Result<()> {
        if let Some(strategy) = self.get_strategy(capability) {
            strategy.validate_config()
        } else {
            Ok(()) // No strategy = nothing to validate
        }
    }

    /// Perform health checks on all registered strategies
    ///
    /// # Returns
    ///
    /// A list of `CapabilityHealth` structs with full status information.
    pub async fn health_check_all(&self) -> Vec<CapabilityHealth> {
        let mut results = Vec::new();

        for strategy in &self.strategies {
            let capability = strategy.capability_type();
            let name = strategy.name().to_string();

            // Validate config
            let (config_valid, config_error) = match strategy.validate_config() {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            };

            // Check availability
            let available = strategy.is_available();

            // Health check (only if available)
            let (healthy, health_error) = if available {
                match strategy.health_check().await {
                    Ok(h) => (h, None),
                    Err(e) => (false, Some(e.to_string())),
                }
            } else {
                (false, None)
            };

            // Get status info
            let status_info = strategy.status_info();

            results.push(CapabilityHealth {
                capability,
                name,
                config_valid,
                config_error,
                available,
                healthy,
                health_error,
                status_info,
            });
        }

        results
    }

    /// Perform health check on a specific capability
    ///
    /// # Returns
    ///
    /// - `Some(CapabilityHealth)` with status if capability is registered
    /// - `None` if capability is not registered
    pub async fn health_check(&self, capability: &Capability) -> Option<CapabilityHealth> {
        let strategy = self.get_strategy(capability)?;

        let name = strategy.name().to_string();
        let cap = strategy.capability_type();

        // Validate config
        let (config_valid, config_error) = match strategy.validate_config() {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };

        // Check availability
        let available = strategy.is_available();

        // Health check (only if available)
        let (healthy, health_error) = if available {
            match strategy.health_check().await {
                Ok(h) => (h, None),
                Err(e) => (false, Some(e.to_string())),
            }
        } else {
            (false, None)
        };

        // Get status info
        let status_info = strategy.status_info();

        Some(CapabilityHealth {
            capability: cap,
            name,
            config_valid,
            config_error,
            available,
            healthy,
            health_error,
            status_info,
        })
    }

    /// Get the number of operational (fully working) capabilities
    pub async fn operational_count(&self) -> usize {
        let health = self.health_check_all().await;
        health.iter().filter(|h| h.is_operational()).count()
    }

    /// Check if all registered strategies are operational
    pub async fn all_operational(&self) -> bool {
        let health = self.health_check_all().await;
        health.iter().all(|h| h.is_operational())
    }

    /// Unregister a capability strategy
    ///
    /// # Returns
    ///
    /// `true` if a strategy was removed, `false` if not found
    pub fn unregister(&mut self, capability: &Capability) -> bool {
        let initial_len = self.strategies.len();
        self.strategies
            .retain(|s| &s.capability_type() != capability);
        let removed = self.strategies.len() < initial_len;
        if removed {
            tracing::debug!(
                capability = ?capability,
                "Unregistered capability strategy"
            );
        }
        removed
    }

    /// Clear all registered strategies
    pub fn clear(&mut self) {
        self.strategies.clear();
        tracing::debug!("Cleared all capability strategies");
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
    use crate::error::AlephError;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    /// Mock strategy for testing
    struct MockStrategy {
        capability: Capability,
        priority: u32,
        available: bool,
        config_valid: bool,
        healthy: bool,
        executed: std::sync::atomic::AtomicBool,
    }

    impl MockStrategy {
        fn new(capability: Capability, priority: u32, available: bool) -> Self {
            Self {
                capability,
                priority,
                available,
                config_valid: true,
                healthy: true,
                executed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn with_config_valid(mut self, valid: bool) -> Self {
            self.config_valid = valid;
            self
        }

        fn with_healthy(mut self, healthy: bool) -> Self {
            self.healthy = healthy;
            self
        }

        fn was_executed(&self) -> bool {
            self.executed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CapabilityStrategy for MockStrategy {
        fn capability_type(&self) -> Capability {
            self.capability
        }

        fn priority(&self) -> u32 {
            self.priority
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn validate_config(&self) -> Result<()> {
            if self.config_valid {
                Ok(())
            } else {
                Err(AlephError::config("Invalid config"))
            }
        }

        async fn health_check(&self) -> Result<bool> {
            Ok(self.healthy)
        }

        async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload> {
            self.executed
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
            .with_strategy(Arc::new(MockStrategy::new(Capability::Mcp, 1, true)))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)));

        // Verify strategies are sorted by priority
        assert_eq!(
            executor.strategies()[0].capability_type(),
            Capability::Memory
        );
        assert_eq!(executor.strategies()[1].capability_type(), Capability::Mcp);
    }

    #[tokio::test]
    async fn test_composite_executor_skips_unavailable() {
        let memory_strategy = Arc::new(MockStrategy::new(Capability::Memory, 0, false));
        let search_strategy = Arc::new(MockStrategy::new(Capability::Mcp, 1, true));

        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(memory_strategy.clone())
            .with_strategy(search_strategy.clone());

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory, Capability::Mcp],
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
        assert!(!executor.has_strategy(&Capability::Mcp));
    }

    #[tokio::test]
    async fn test_validate_config() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)))
            .with_strategy(Arc::new(
                MockStrategy::new(Capability::Mcp, 1, true).with_config_valid(false),
            ));

        let results = executor.validate_all();
        assert_eq!(results.len(), 2);

        // Memory should be valid
        let memory_result = results.iter().find(|(c, _)| c == &Capability::Memory);
        assert!(memory_result.unwrap().1.is_ok());

        // Search should be invalid
        let search_result = results.iter().find(|(c, _)| c == &Capability::Mcp);
        assert!(search_result.unwrap().1.is_err());
    }

    #[tokio::test]
    async fn test_validate_single() {
        let executor = CompositeCapabilityExecutor::new().with_strategy(Arc::new(
            MockStrategy::new(Capability::Memory, 0, true).with_config_valid(false),
        ));

        assert!(executor.validate(&Capability::Memory).is_err());
        assert!(executor.validate(&Capability::Mcp).is_ok()); // Not registered
    }

    #[tokio::test]
    async fn test_health_check_all() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)))
            .with_strategy(Arc::new(
                MockStrategy::new(Capability::Mcp, 1, true).with_healthy(false),
            ));

        let health = executor.health_check_all().await;
        assert_eq!(health.len(), 2);

        // Memory should be fully operational
        let memory_health = health
            .iter()
            .find(|h| h.capability == Capability::Memory)
            .unwrap();
        assert!(memory_health.config_valid);
        assert!(memory_health.available);
        assert!(memory_health.healthy);
        assert!(memory_health.is_operational());

        // Search should be unhealthy
        let search_health = health
            .iter()
            .find(|h| h.capability == Capability::Mcp)
            .unwrap();
        assert!(search_health.config_valid);
        assert!(search_health.available);
        assert!(!search_health.healthy);
        assert!(!search_health.is_operational());
    }

    #[tokio::test]
    async fn test_health_check_single() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)));

        let health = executor.health_check(&Capability::Memory).await;
        assert!(health.is_some());
        assert!(health.unwrap().is_operational());

        let health = executor.health_check(&Capability::Mcp).await;
        assert!(health.is_none());
    }

    #[tokio::test]
    async fn test_operational_count() {
        let executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)))
            .with_strategy(Arc::new(
                MockStrategy::new(Capability::Mcp, 1, false), // Not available
            ))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Skills, 2, true)));

        assert_eq!(executor.operational_count().await, 2);
        assert!(!executor.all_operational().await);
    }

    #[tokio::test]
    async fn test_unregister() {
        let mut executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Mcp, 1, true)));

        assert_eq!(executor.strategies().len(), 2);

        let removed = executor.unregister(&Capability::Memory);
        assert!(removed);
        assert_eq!(executor.strategies().len(), 1);
        assert!(!executor.has_strategy(&Capability::Memory));

        // Removing again should return false
        let removed = executor.unregister(&Capability::Memory);
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_clear() {
        let mut executor = CompositeCapabilityExecutor::new()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory, 0, true)))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Mcp, 1, true)));

        assert_eq!(executor.strategies().len(), 2);

        executor.clear();
        assert_eq!(executor.strategies().len(), 0);
    }

    #[test]
    fn test_status_info() {
        let strategy = MockStrategy::new(Capability::Memory, 0, true);
        let info = strategy.status_info();

        assert!(info.contains_key("capability"));
        assert!(info.contains_key("name"));
        assert!(info.contains_key("priority"));
        assert!(info.contains_key("available"));
        assert_eq!(info.get("name").unwrap(), "memory");
        assert_eq!(info.get("priority").unwrap(), "0");
        assert_eq!(info.get("available").unwrap(), "true");
    }

    #[tokio::test]
    async fn test_unavailable_strategy_not_health_checked() {
        let executor = CompositeCapabilityExecutor::new().with_strategy(Arc::new(
            MockStrategy::new(Capability::Memory, 0, false) // Not available
                .with_healthy(true),
        ));

        let health = executor.health_check(&Capability::Memory).await.unwrap();

        // Even though healthy is true, since not available, healthy should be false
        assert!(!health.available);
        assert!(!health.healthy);
        assert!(!health.is_operational());
    }
}

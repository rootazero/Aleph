//! Unified Capability System
//!
//! This module provides `CapabilitySystem`, a high-level coordinator for all
//! capability operations. It serves as the single entry point for:
//!
//! - Registering capability strategies
//! - Executing capabilities on payloads
//! - Validating configurations
//! - Health checking all capabilities
//! - Querying capability status
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              CapabilitySystem               │
//! │                                             │
//! │  ┌───────────────────────────────────────┐  │
//! │  │      CompositeCapabilityExecutor      │  │
//! │  │                                       │  │
//! │  │  ┌─────────────────────────────────┐  │  │
//! │  │  │  Memory  Search  MCP  Video ... │  │  │
//! │  │  │       (CapabilityStrategy)      │  │  │
//! │  │  └─────────────────────────────────┘  │  │
//! │  └───────────────────────────────────────┘  │
//! │                                             │
//! │  + Health Check                             │
//! │  + Validation                               │
//! │  + Diagnostics                              │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::capability::{CapabilitySystem, MemoryStrategy, SearchStrategy};
//!
//! let system = CapabilitySystem::builder()
//!     .with_strategy(Arc::new(MemoryStrategy::new(memory_db)))
//!     .with_strategy(Arc::new(SearchStrategy::new(search_registry)))
//!     .build();
//!
//! // Validate all configurations
//! system.validate_all()?;
//!
//! // Execute capabilities on a payload
//! let enriched_payload = system.execute(payload).await?;
//!
//! // Health check
//! let health = system.health_check_all().await;
//! ```

use super::strategy::{CapabilityHealth, CapabilityStrategy, CompositeCapabilityExecutor};
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Configuration for the CapabilitySystem
#[derive(Debug, Clone)]
pub struct CapabilitySystemConfig {
    /// Whether to validate configurations on startup
    pub validate_on_startup: bool,
    /// Whether to perform health checks on startup
    pub health_check_on_startup: bool,
    /// Continue execution even if some capabilities are unhealthy
    pub allow_degraded_mode: bool,
}

impl Default for CapabilitySystemConfig {
    fn default() -> Self {
        Self {
            validate_on_startup: true,
            health_check_on_startup: false, // Health checks may be slow
            allow_degraded_mode: true,
        }
    }
}

/// System status summary
#[derive(Debug, Clone)]
pub struct SystemStatus {
    /// Total number of registered capabilities
    pub total_capabilities: usize,
    /// Number of operational capabilities
    pub operational_count: usize,
    /// Number of unavailable capabilities
    pub unavailable_count: usize,
    /// Number of unhealthy capabilities
    pub unhealthy_count: usize,
    /// Whether the system is fully operational
    pub is_fully_operational: bool,
    /// Detailed health info for each capability
    pub capabilities: Vec<CapabilityHealth>,
}

impl SystemStatus {
    /// Get capabilities that are not operational
    pub fn failed_capabilities(&self) -> Vec<&CapabilityHealth> {
        self.capabilities
            .iter()
            .filter(|c| !c.is_operational())
            .collect()
    }

    /// Get operational capabilities
    pub fn operational_capabilities(&self) -> Vec<&CapabilityHealth> {
        self.capabilities
            .iter()
            .filter(|c| c.is_operational())
            .collect()
    }
}

/// Unified Capability System
///
/// High-level coordinator for all capability operations. This is the recommended
/// entry point for capability management in Aleph.
pub struct CapabilitySystem {
    /// The underlying composite executor
    executor: RwLock<CompositeCapabilityExecutor>,
    /// System configuration
    config: CapabilitySystemConfig,
}

impl CapabilitySystem {
    /// Create a new CapabilitySystem with default configuration
    pub fn new() -> Self {
        Self {
            executor: RwLock::new(CompositeCapabilityExecutor::new()),
            config: CapabilitySystemConfig::default(),
        }
    }

    /// Create with specific configuration
    pub fn with_config(config: CapabilitySystemConfig) -> Self {
        Self {
            executor: RwLock::new(CompositeCapabilityExecutor::new()),
            config,
        }
    }

    /// Create a builder for CapabilitySystem
    pub fn builder() -> CapabilitySystemBuilder {
        CapabilitySystemBuilder::new()
    }

    /// Get the current configuration
    pub fn config(&self) -> &CapabilitySystemConfig {
        &self.config
    }

    // =========================================================================
    // Strategy Management
    // =========================================================================

    /// Register a capability strategy
    ///
    /// Strategies are automatically sorted by priority after registration.
    pub async fn register(&self, strategy: Arc<dyn CapabilityStrategy>) {
        let name = strategy.name().to_string();
        let capability = strategy.capability_type();

        let mut executor = self.executor.write().await;
        executor.register(strategy);

        info!(
            capability = ?capability,
            name = %name,
            "Registered capability strategy"
        );
    }

    /// Unregister a capability strategy
    ///
    /// # Returns
    ///
    /// `true` if a strategy was removed, `false` if not found.
    pub async fn unregister(&self, capability: &Capability) -> bool {
        let mut executor = self.executor.write().await;
        let removed = executor.unregister(capability);
        if removed {
            info!(capability = ?capability, "Unregistered capability strategy");
        }
        removed
    }

    /// Check if a capability is registered
    pub async fn has_capability(&self, capability: &Capability) -> bool {
        let executor = self.executor.read().await;
        executor.has_strategy(capability)
    }

    /// Get list of registered capability types
    pub async fn registered_capabilities(&self) -> Vec<Capability> {
        let executor = self.executor.read().await;
        executor
            .strategies()
            .iter()
            .map(|s| s.capability_type())
            .collect()
    }

    /// Clear all registered strategies
    pub async fn clear(&self) {
        let mut executor = self.executor.write().await;
        executor.clear();
        info!("Cleared all capability strategies");
    }

    // =========================================================================
    // Execution
    // =========================================================================

    /// Execute all enabled and available capabilities on a payload
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload to enrich
    ///
    /// # Returns
    ///
    /// The enriched payload with capability contexts added.
    pub async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload> {
        let executor = self.executor.read().await;

        debug!(
            capabilities = ?payload.config.capabilities,
            "CapabilitySystem: Executing capabilities"
        );

        executor.execute_all(payload).await
    }

    // =========================================================================
    // Validation & Health
    // =========================================================================

    /// Validate configuration for all registered strategies
    ///
    /// # Returns
    ///
    /// `Ok(())` if all configurations are valid, `Err(...)` with the first error.
    pub async fn validate_all(&self) -> Result<()> {
        let executor = self.executor.read().await;
        let results = executor.validate_all();

        for (capability, result) in results {
            if let Err(e) = result {
                warn!(capability = ?capability, error = %e, "Configuration validation failed");
                return Err(e);
            }
        }

        info!("All capability configurations validated successfully");
        Ok(())
    }

    /// Validate a specific capability's configuration
    pub async fn validate(&self, capability: &Capability) -> Result<()> {
        let executor = self.executor.read().await;
        executor.validate(capability)
    }

    /// Perform health checks on all registered strategies
    ///
    /// # Returns
    ///
    /// A `SystemStatus` with detailed health information.
    pub async fn health_check_all(&self) -> SystemStatus {
        let executor = self.executor.read().await;
        let capabilities = executor.health_check_all().await;

        let total_capabilities = capabilities.len();
        let operational_count = capabilities.iter().filter(|c| c.is_operational()).count();
        let unavailable_count = capabilities.iter().filter(|c| !c.available).count();
        let unhealthy_count = capabilities.iter().filter(|c| !c.healthy).count();
        let is_fully_operational = operational_count == total_capabilities;

        SystemStatus {
            total_capabilities,
            operational_count,
            unavailable_count,
            unhealthy_count,
            is_fully_operational,
            capabilities,
        }
    }

    /// Perform health check on a specific capability
    pub async fn health_check(&self, capability: &Capability) -> Option<CapabilityHealth> {
        let executor = self.executor.read().await;
        executor.health_check(capability).await
    }

    /// Check if all capabilities are operational
    pub async fn is_operational(&self) -> bool {
        let executor = self.executor.read().await;
        executor.all_operational().await
    }

    /// Get count of operational capabilities
    pub async fn operational_count(&self) -> usize {
        let executor = self.executor.read().await;
        executor.operational_count().await
    }

    // =========================================================================
    // Diagnostics
    // =========================================================================

    /// Get detailed status information for all capabilities
    ///
    /// This is a convenience method that combines health check with
    /// strategy information.
    pub async fn diagnostics(&self) -> Vec<CapabilityDiagnostics> {
        let status = self.health_check_all().await;
        status
            .capabilities
            .into_iter()
            .map(|health| CapabilityDiagnostics {
                capability: health.capability,
                name: health.name.clone(),
                status: if health.is_operational() {
                    CapabilityStatus::Operational
                } else if !health.config_valid {
                    CapabilityStatus::ConfigError
                } else if !health.available {
                    CapabilityStatus::Unavailable
                } else {
                    CapabilityStatus::Unhealthy
                },
                config_error: health.config_error,
                health_error: health.health_error,
                details: health.status_info,
            })
            .collect()
    }

    /// Log system status summary
    pub async fn log_status(&self) {
        let status = self.health_check_all().await;
        info!(
            total = status.total_capabilities,
            operational = status.operational_count,
            unavailable = status.unavailable_count,
            unhealthy = status.unhealthy_count,
            fully_operational = status.is_fully_operational,
            "CapabilitySystem status"
        );

        for cap in status.failed_capabilities() {
            warn!(
                capability = ?cap.capability,
                name = %cap.name,
                config_valid = cap.config_valid,
                available = cap.available,
                healthy = cap.healthy,
                "Capability not operational"
            );
        }
    }
}

impl Default for CapabilitySystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Status of a capability
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// Fully operational
    Operational,
    /// Configuration error
    ConfigError,
    /// Dependencies not available
    Unavailable,
    /// Health check failed
    Unhealthy,
}

/// Diagnostics information for a capability
#[derive(Debug, Clone)]
pub struct CapabilityDiagnostics {
    /// The capability type
    pub capability: Capability,
    /// Strategy name
    pub name: String,
    /// Current status
    pub status: CapabilityStatus,
    /// Configuration error message (if any)
    pub config_error: Option<String>,
    /// Health check error message (if any)
    pub health_error: Option<String>,
    /// Additional details
    pub details: std::collections::HashMap<String, String>,
}

// ============================================================================
// Builder Pattern
// ============================================================================

/// Builder for CapabilitySystem
pub struct CapabilitySystemBuilder {
    strategies: Vec<Arc<dyn CapabilityStrategy>>,
    config: CapabilitySystemConfig,
}

impl CapabilitySystemBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
            config: CapabilitySystemConfig::default(),
        }
    }

    /// Add a capability strategy
    pub fn with_strategy(mut self, strategy: Arc<dyn CapabilityStrategy>) -> Self {
        self.strategies.push(strategy);
        self
    }

    /// Set the configuration
    pub fn with_config(mut self, config: CapabilitySystemConfig) -> Self {
        self.config = config;
        self
    }

    /// Enable/disable validation on startup
    pub fn validate_on_startup(mut self, enabled: bool) -> Self {
        self.config.validate_on_startup = enabled;
        self
    }

    /// Enable/disable health check on startup
    pub fn health_check_on_startup(mut self, enabled: bool) -> Self {
        self.config.health_check_on_startup = enabled;
        self
    }

    /// Enable/disable degraded mode
    pub fn allow_degraded_mode(mut self, enabled: bool) -> Self {
        self.config.allow_degraded_mode = enabled;
        self
    }

    /// Build the CapabilitySystem
    ///
    /// This will:
    /// 1. Register all strategies
    /// 2. Optionally validate configurations (if configured)
    ///
    /// Note: Health checks are NOT performed synchronously in build.
    /// Use `build_async()` for async initialization with health checks.
    pub fn build(self) -> Result<CapabilitySystem> {
        let mut executor = CompositeCapabilityExecutor::new();

        for strategy in self.strategies {
            executor.register(strategy);
        }

        // Synchronous validation if enabled
        if self.config.validate_on_startup {
            let results = executor.validate_all();
            for (capability, result) in results {
                if let Err(e) = result {
                    warn!(capability = ?capability, error = %e, "Configuration validation failed");
                    if !self.config.allow_degraded_mode {
                        return Err(e);
                    }
                }
            }
        }

        Ok(CapabilitySystem {
            executor: RwLock::new(executor),
            config: self.config,
        })
    }

    /// Build the CapabilitySystem with async initialization
    ///
    /// This will:
    /// 1. Register all strategies
    /// 2. Validate configurations (if configured)
    /// 3. Perform health checks (if configured)
    pub async fn build_async(self) -> Result<CapabilitySystem> {
        let system = self.build()?;

        // Async health check if enabled
        if system.config.health_check_on_startup {
            let status = system.health_check_all().await;

            if !status.is_fully_operational && !system.config.allow_degraded_mode {
                let failed: Vec<_> = status
                    .failed_capabilities()
                    .iter()
                    .map(|c| format!("{:?}", c.capability))
                    .collect();
                return Err(crate::error::AlephError::config(format!(
                    "Capabilities failed health check: {}",
                    failed.join(", ")
                )));
            }

            system.log_status().await;
        }

        Ok(system)
    }
}

impl Default for CapabilitySystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};
    use async_trait::async_trait;

    /// Mock strategy for testing
    struct MockStrategy {
        capability: Capability,
        priority: u32,
        available: bool,
        config_valid: bool,
        healthy: bool,
    }

    impl MockStrategy {
        fn new(capability: Capability) -> Self {
            Self {
                capability,
                priority: 0,
                available: true,
                config_valid: true,
                healthy: true,
            }
        }

        fn with_available(mut self, available: bool) -> Self {
            self.available = available;
            self
        }

        fn with_config_valid(mut self, valid: bool) -> Self {
            self.config_valid = valid;
            self
        }

        fn with_healthy(mut self, healthy: bool) -> Self {
            self.healthy = healthy;
            self
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
            Ok(payload)
        }
    }

    fn create_test_payload() -> AgentPayload {
        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn test_capability_system_new() {
        let system = CapabilitySystem::new();
        assert_eq!(system.registered_capabilities().await.len(), 0);
        assert!(system.is_operational().await);
    }

    #[tokio::test]
    async fn test_capability_system_register() {
        let system = CapabilitySystem::new();

        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;
        system
            .register(Arc::new(MockStrategy::new(Capability::Mcp)))
            .await;

        assert_eq!(system.registered_capabilities().await.len(), 2);
        assert!(system.has_capability(&Capability::Memory).await);
        assert!(system.has_capability(&Capability::Mcp).await);
        assert!(!system.has_capability(&Capability::Skills).await);
    }

    #[tokio::test]
    async fn test_capability_system_unregister() {
        let system = CapabilitySystem::new();

        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;
        assert!(system.has_capability(&Capability::Memory).await);

        let removed = system.unregister(&Capability::Memory).await;
        assert!(removed);
        assert!(!system.has_capability(&Capability::Memory).await);
    }

    #[tokio::test]
    async fn test_capability_system_execute() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;

        let payload = create_test_payload();
        let result = system.execute(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_capability_system_validate_all() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;

        let result = system.validate_all().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_capability_system_validate_fails() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(
                MockStrategy::new(Capability::Memory).with_config_valid(false),
            ))
            .await;

        let result = system.validate_all().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_capability_system_health_check() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;
        system
            .register(Arc::new(
                MockStrategy::new(Capability::Mcp).with_healthy(false),
            ))
            .await;

        let status = system.health_check_all().await;
        assert_eq!(status.total_capabilities, 2);
        assert_eq!(status.operational_count, 1);
        assert_eq!(status.unhealthy_count, 1);
        assert!(!status.is_fully_operational);
    }

    #[tokio::test]
    async fn test_capability_system_diagnostics() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;
        system
            .register(Arc::new(
                MockStrategy::new(Capability::Mcp).with_available(false),
            ))
            .await;

        let diagnostics = system.diagnostics().await;
        assert_eq!(diagnostics.len(), 2);

        let memory = diagnostics
            .iter()
            .find(|d| d.capability == Capability::Memory)
            .unwrap();
        assert_eq!(memory.status, CapabilityStatus::Operational);

        let search = diagnostics
            .iter()
            .find(|d| d.capability == Capability::Mcp)
            .unwrap();
        assert_eq!(search.status, CapabilityStatus::Unavailable);
    }

    #[tokio::test]
    async fn test_capability_system_builder() {
        let system = CapabilitySystem::builder()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory)))
            .with_strategy(Arc::new(MockStrategy::new(Capability::Mcp)))
            .validate_on_startup(false)
            .build()
            .unwrap();

        assert_eq!(system.registered_capabilities().await.len(), 2);
    }

    #[tokio::test]
    async fn test_capability_system_builder_with_validation() {
        let result = CapabilitySystem::builder()
            .with_strategy(Arc::new(
                MockStrategy::new(Capability::Memory).with_config_valid(false),
            ))
            .validate_on_startup(true)
            .allow_degraded_mode(false)
            .build();

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_capability_system_builder_async() {
        let system = CapabilitySystem::builder()
            .with_strategy(Arc::new(MockStrategy::new(Capability::Memory)))
            .health_check_on_startup(true)
            .build_async()
            .await
            .unwrap();

        assert!(system.is_operational().await);
    }

    #[tokio::test]
    async fn test_capability_system_clear() {
        let system = CapabilitySystem::new();
        system
            .register(Arc::new(MockStrategy::new(Capability::Memory)))
            .await;
        system
            .register(Arc::new(MockStrategy::new(Capability::Mcp)))
            .await;

        assert_eq!(system.registered_capabilities().await.len(), 2);

        system.clear().await;
        assert_eq!(system.registered_capabilities().await.len(), 0);
    }
}

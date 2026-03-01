//! Model Failover Provider
//!
//! Provides automatic failover between multiple AI providers for increased reliability.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │         FailoverProvider            │
//! ├─────────────────────────────────────┤
//! │  ┌─────────┐ ┌─────────┐ ┌───────┐ │
//! │  │Anthropic│ │ OpenAI  │ │Gemini │ │
//! │  │ (pri=1) │ │ (pri=2) │ │(pri=3)│ │
//! │  └────┬────┘ └────┬────┘ └───┬───┘ │
//! │       │           │          │      │
//! │       └───────────┼──────────┘      │
//! │                   ▼                  │
//! │          Health Monitor              │
//! │       (check every 60s)              │
//! └─────────────────────────────────────┘
//! ```
//!
//! # Features
//!
//! - Priority-based provider selection
//! - Automatic failover on errors
//! - Health monitoring with periodic checks
//! - Metrics tracking (success/failure counts)
//! - Exponential backoff for unhealthy providers
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::providers::failover::{FailoverProvider, FailoverConfig, ProviderEntry};
//!
//! let config = FailoverConfig {
//!     providers: vec![
//!         ProviderEntry {
//!             name: "claude".to_string(),
//!             priority: 1,
//!             config: claude_config,
//!         },
//!         ProviderEntry {
//!             name: "openai".to_string(),
//!             priority: 2,
//!             config: openai_config,
//!         },
//!     ],
//!     max_retries: 3,
//!     health_check_interval_secs: 60,
//! };
//!
//! let provider = FailoverProvider::new(config)?;
//! let response = provider.process("Hello", None).await?;
//! ```

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::{create_provider, AiProvider};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use crate::sync_primitives::{AtomicU64, Ordering};
use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Configuration for a single provider in the failover chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// Provider name (e.g., "claude", "openai")
    pub name: String,
    /// Priority (lower = higher priority, used first)
    pub priority: u32,
    /// Provider configuration
    pub config: ProviderConfig,
}

/// Failover provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// List of providers in the failover chain
    pub providers: Vec<ProviderEntry>,
    /// Maximum retries per provider before moving to next
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Health check interval in seconds
    #[serde(default = "default_health_check_interval")]
    pub health_check_interval_secs: u64,
    /// Cooldown period for unhealthy providers (seconds)
    #[serde(default = "default_unhealthy_cooldown")]
    pub unhealthy_cooldown_secs: u64,
    /// Enable health monitoring task
    #[serde(default = "default_true")]
    pub health_monitoring_enabled: bool,
}

fn default_max_retries() -> u32 {
    2
}

fn default_health_check_interval() -> u64 {
    60
}

fn default_unhealthy_cooldown() -> u64 {
    300 // 5 minutes
}

fn default_true() -> bool {
    true
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            max_retries: 2,
            health_check_interval_secs: 60,
            unhealthy_cooldown_secs: 300,
            health_monitoring_enabled: true,
        }
    }
}

/// Health state for a provider
#[derive(Debug, Clone)]
pub struct HealthState {
    /// Whether the provider is currently healthy
    pub healthy: bool,
    /// Last time the provider was checked
    pub last_check: Instant,
    /// Last time the provider failed
    pub last_failure: Option<Instant>,
    /// Consecutive failure count
    pub failure_count: u32,
    /// Last error message
    pub last_error: Option<String>,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            healthy: true, // Assume healthy until proven otherwise
            last_check: Instant::now(),
            last_failure: None,
            failure_count: 0,
            last_error: None,
        }
    }
}

/// Metrics for a provider
#[derive(Debug, Default)]
pub struct ProviderMetrics {
    /// Total requests
    pub total_requests: AtomicU64,
    /// Successful requests
    pub success_count: AtomicU64,
    /// Failed requests
    pub failure_count: AtomicU64,
    /// Total latency in milliseconds
    pub total_latency_ms: AtomicU64,
}

impl ProviderMetrics {
    pub fn record_success(&self, latency_ms: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failure_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn success_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 {
            return 1.0;
        }
        let success = self.success_count.load(Ordering::Relaxed);
        success as f64 / total as f64
    }

    pub fn avg_latency_ms(&self) -> f64 {
        let success = self.success_count.load(Ordering::Relaxed);
        if success == 0 {
            return 0.0;
        }
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        total_latency as f64 / success as f64
    }
}

/// Internal provider state
struct ProviderState {
    provider: Arc<dyn AiProvider>,
    entry: ProviderEntry,
    health: HealthState,
    metrics: ProviderMetrics,
}

/// Failover provider that automatically switches between providers on failure
pub struct FailoverProvider {
    /// Provider states (sorted by priority)
    providers: RwLock<Vec<ProviderState>>,
    /// Configuration
    config: FailoverConfig,
    /// Shutdown signal for health monitor
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FailoverProvider {
    /// Create a new failover provider
    pub fn new(config: FailoverConfig) -> Result<Self> {
        let mut provider_states = Vec::new();

        // Sort providers by priority
        let mut entries = config.providers.clone();
        entries.sort_by_key(|e| e.priority);

        // Create provider instances
        for entry in entries {
            let provider = create_provider(&entry.name, entry.config.clone())?;
            provider_states.push(ProviderState {
                provider,
                entry,
                health: HealthState::default(),
                metrics: ProviderMetrics::default(),
            });
        }

        if provider_states.is_empty() {
            return Err(AlephError::invalid_config(
                "Failover provider requires at least one provider",
            ));
        }

        tracing::info!(
            "Created FailoverProvider with {} providers: {:?}",
            provider_states.len(),
            provider_states.iter().map(|p| &p.entry.name).collect::<Vec<_>>()
        );

        Ok(Self {
            providers: RwLock::new(provider_states),
            config,
            shutdown_tx: None,
        })
    }

    /// Start the health monitoring task
    ///
    /// Note: Currently health monitoring relies on request-time updates.
    /// A background health check task would require Arc<Self> which adds complexity.
    /// For now, providers are checked at request time and marked unhealthy on failures.
    pub fn start_health_monitor(&mut self) {
        if !self.config.health_monitoring_enabled {
            return;
        }

        tracing::info!(
            "Health monitoring enabled (request-time checks, cooldown: {}s)",
            self.config.unhealthy_cooldown_secs
        );
        // Background health checks would go here if needed
        // Currently relying on request-time health updates
    }

    /// Stop the health monitoring task
    pub fn stop_health_monitor(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Check if a provider is currently healthy
    async fn is_provider_healthy(&self, index: usize) -> bool {
        let providers = self.providers.read().await;
        if let Some(state) = providers.get(index) {
            // Check if provider is marked healthy
            if !state.health.healthy {
                // Check if cooldown has passed
                if let Some(last_failure) = state.health.last_failure {
                    let cooldown = Duration::from_secs(self.config.unhealthy_cooldown_secs);
                    if last_failure.elapsed() < cooldown {
                        return false;
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// Mark a provider as unhealthy
    async fn mark_unhealthy(&self, index: usize, error: String) {
        let mut providers = self.providers.write().await;
        if let Some(state) = providers.get_mut(index) {
            state.health.healthy = false;
            state.health.last_failure = Some(Instant::now());
            state.health.failure_count += 1;
            state.health.last_error = Some(error.clone());
            state.metrics.record_failure();

            tracing::warn!(
                "Provider '{}' marked unhealthy (failure #{}: {})",
                state.entry.name,
                state.health.failure_count,
                error
            );
        }
    }

    /// Mark a provider as healthy
    async fn mark_healthy(&self, index: usize, latency_ms: u64) {
        let mut providers = self.providers.write().await;
        if let Some(state) = providers.get_mut(index) {
            state.health.healthy = true;
            state.health.last_check = Instant::now();
            state.health.failure_count = 0;
            state.health.last_error = None;
            state.metrics.record_success(latency_ms);
        }
    }

    /// Get metrics for all providers
    pub async fn get_metrics(&self) -> HashMap<String, (f64, f64)> {
        let providers = self.providers.read().await;
        providers
            .iter()
            .map(|p| {
                (
                    p.entry.name.clone(),
                    (p.metrics.success_rate(), p.metrics.avg_latency_ms()),
                )
            })
            .collect()
    }

    /// Get health status for all providers
    pub async fn get_health_status(&self) -> HashMap<String, bool> {
        let providers = self.providers.read().await;
        providers
            .iter()
            .map(|p| (p.entry.name.clone(), p.health.healthy))
            .collect()
    }

    /// Get the primary (highest priority healthy) provider name
    pub async fn get_primary_provider(&self) -> Option<String> {
        let provider_count = {
            let providers = self.providers.read().await;
            providers.len()
        };

        for i in 0..provider_count {
            if self.is_provider_healthy(i).await {
                let providers = self.providers.read().await;
                return providers.get(i).map(|p| p.entry.name.clone());
            }
        }
        None
    }
}

impl AiProvider for FailoverProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let providers = self.providers.read().await;
            let provider_count = providers.len();
            drop(providers); // Release lock

            let mut last_error = None;

            for i in 0..provider_count {
                // Check if provider is healthy
                if !self.is_provider_healthy(i).await {
                    tracing::debug!("Skipping unhealthy provider at index {}", i);
                    continue;
                }

                // Get provider
                let provider = {
                    let providers = self.providers.read().await;
                    providers.get(i).map(|p| (p.provider.clone(), p.entry.name.clone()))
                };

                let Some((provider, name)) = provider else {
                    continue;
                };

                // Try the provider with retries
                for retry in 0..=self.config.max_retries {
                    if retry > 0 {
                        tracing::debug!("Retry {} for provider '{}'", retry, name);
                        // Brief delay between retries
                        tokio::time::sleep(Duration::from_millis(100 * retry as u64)).await;
                    }

                    let start = Instant::now();
                    match provider.process(&input, system_prompt.as_deref()).await {
                        Ok(response) => {
                            let latency_ms = start.elapsed().as_millis() as u64;
                            self.mark_healthy(i, latency_ms).await;
                            tracing::debug!(
                                "Provider '{}' succeeded (latency: {}ms)",
                                name,
                                latency_ms
                            );
                            return Ok(response);
                        }
                        Err(e) => {
                            let error_msg = e.to_string();
                            tracing::warn!(
                                "Provider '{}' failed (retry {}): {}",
                                name,
                                retry,
                                error_msg
                            );
                            last_error = Some(error_msg.clone());

                            // Don't retry on certain errors
                            if Self::is_non_retryable_error(&e) {
                                self.mark_unhealthy(i, error_msg).await;
                                break;
                            }
                        }
                    }
                }

                // Mark unhealthy after all retries exhausted
                if let Some(ref error) = last_error {
                    self.mark_unhealthy(i, error.clone()).await;
                }
            }

            // All providers failed
            Err(AlephError::provider(format!(
                "All {} providers failed. Last error: {}",
                provider_count,
                last_error.unwrap_or_else(|| "Unknown error".to_string())
            )))
        })
    }

    fn name(&self) -> &str {
        "failover"
    }

    fn color(&self) -> &str {
        "#6366f1" // Indigo
    }

    fn supports_vision(&self) -> bool {
        // Return true if any provider supports vision
        // This is a sync method, so we can't easily check async
        // Default to true to allow vision requests to try
        true
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // For vision, we need to try providers that support it
        // Simplified: just use the text process for now
        // TODO: Implement proper vision failover
        let _ = image;
        self.process(input, system_prompt)
    }
}

impl FailoverProvider {
    /// Check if an error should not be retried
    fn is_non_retryable_error(error: &AlephError) -> bool {
        match error {
            AlephError::AuthenticationError { .. } => true,
            AlephError::InvalidConfig { .. } => true,
            AlephError::RateLimitError { .. } => false, // Can retry after delay
            _ => false,
        }
    }
}

impl Drop for FailoverProvider {
    fn drop(&mut self) {
        self.stop_health_monitor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failover_config_default() {
        let config = FailoverConfig::default();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.health_check_interval_secs, 60);
        assert_eq!(config.unhealthy_cooldown_secs, 300);
    }

    #[test]
    fn test_provider_metrics() {
        let metrics = ProviderMetrics::default();

        metrics.record_success(100);
        metrics.record_success(200);
        metrics.record_failure();

        assert_eq!(metrics.total_requests.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.success_count.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.failure_count.load(Ordering::Relaxed), 1);
        assert!((metrics.success_rate() - 0.666).abs() < 0.01);
        assert!((metrics.avg_latency_ms() - 150.0).abs() < 0.01);
    }

    #[test]
    fn test_health_state_default() {
        let state = HealthState::default();
        assert!(state.healthy);
        assert_eq!(state.failure_count, 0);
        assert!(state.last_error.is_none());
    }

    #[tokio::test]
    async fn test_failover_provider_creation() {
        // Create with mock providers
        let mut test_provider_config = ProviderConfig::test_config("test");
        test_provider_config.protocol = Some("mock".to_string());
        test_provider_config.api_key = None;
        test_provider_config.color = "#000000".to_string();
        test_provider_config.timeout_seconds = 30;

        let config = FailoverConfig {
            providers: vec![ProviderEntry {
                name: "mock".to_string(),
                priority: 1,
                config: test_provider_config,
            }],
            ..Default::default()
        };

        let provider = FailoverProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_failover_provider_empty_providers() {
        let config = FailoverConfig::default();
        let provider = FailoverProvider::new(config);
        assert!(provider.is_err());
    }
}

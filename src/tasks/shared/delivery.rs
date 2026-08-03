//! Shared delivery pipeline for task results.
//!
//! Supports pluggable delivery targets via the `DeliveryTarget` trait.
//! Built-in targets: Gateway (Telegram/Discord), Webhook, Memory.
//!
//! This module is task-type-agnostic: cron, heartbeat, or any future
//! task type can use it by constructing a `DeliveryPayload`.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Delivery type definitions ────────────────────────────────────────

/// Whether results were delivered to the user
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    NotDelivered,
    AlreadySentByAgent,
    NotRequested,
}

/// Configuration for delivering task results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryConfig {
    pub mode: DeliveryMode,
    pub targets: Vec<DeliveryTargetConfig>,
    #[serde(default)]
    pub fallback_target: Option<DeliveryTargetConfig>,
}

/// Delivery mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    None,
    Primary,
    Broadcast,
}

/// Delivery target configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum DeliveryTargetConfig {
    Gateway {
        channel: String,
        chat_id: String,
        #[serde(default)]
        format: Option<String>,
    },
    Memory {
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        importance: Option<f32>,
    },
    Webhook {
        url: String,
        #[serde(default)]
        method: Option<String>,
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
    },
}

/// Outcome of a delivery attempt
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryOutcome {
    pub target_kind: String,
    pub success: bool,
    pub message: Option<String>,
}

// ── DeliveryPayload ──────────────────────────────────────────────────

/// Generic payload for delivery — task-type-agnostic.
///
/// Callers (cron, heartbeat, etc.) construct this from their own domain
/// types and pass it to `DeliveryEngine::deliver`.
#[derive(Debug, Clone, Serialize)]
pub struct DeliveryPayload {
    /// The type of task that produced this result (e.g. "cron", "heartbeat")
    pub source_type: String,
    /// Human-readable task name
    pub task_name: String,
    /// Agent that executed the task
    pub agent_id: String,
    /// The output produced by the task
    pub output: String,
    /// Channel where the task was created (for result delivery)
    pub channel_id: Option<String>,
    /// Additional task-type-specific metadata
    pub metadata: serde_json::Value,
}

// ── Error type ───────────────────────────────────────────────────────

/// Error type for delivery operations
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("Invalid delivery config: {0}")]
    InvalidConfig(String),

    #[error("Delivery failed: {0}")]
    Failed(String),

    #[error("Target not registered: {0}")]
    TargetNotRegistered(String),
}

// ── DeliveryTarget trait ─────────────────────────────────────────────

/// Trait for delivery targets.
///
/// Each implementation handles delivering task results to a specific destination.
#[async_trait]
pub trait DeliveryTarget: Send + Sync {
    /// Identifier for this delivery target type
    fn kind(&self) -> &str;

    /// Deliver a task result to the target
    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError>;
}

// ── DeliveryEngine ───────────────────────────────────────────────────

/// Delivery engine that dispatches results to registered targets.
pub struct DeliveryEngine {
    targets: HashMap<String, Arc<dyn DeliveryTarget>>,
}

impl Default for DeliveryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DeliveryEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            targets: HashMap::new(),
        }
    }

    /// Register a delivery target
    pub fn register(&mut self, target: Arc<dyn DeliveryTarget>) {
        self.targets.insert(target.kind().to_string(), target);
    }

    /// Execute delivery for a task result according to its config
    pub async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryConfig,
    ) -> Vec<DeliveryOutcome> {
        let mut outcomes = Vec::new();

        match config.mode {
            DeliveryMode::None => {}
            DeliveryMode::Primary => {
                match config.targets.first() {
                    Some(target_cfg) => {
                        let outcome = self.deliver_to_target(payload, target_cfg).await;
                        let success = outcome.success;
                        outcomes.push(outcome);

                        // Fallback on failure
                        if !success {
                            if let Some(fallback) = &config.fallback_target {
                                outcomes.push(self.deliver_to_target(payload, fallback).await);
                            }
                        }
                    }
                    None => {
                        outcomes.push(DeliveryOutcome {
                            target_kind: "primary".to_string(),
                            success: false,
                            message: Some("no primary target configured".to_string()),
                        });
                    }
                }
            }
            DeliveryMode::Broadcast => {
                for target_cfg in &config.targets {
                    outcomes.push(self.deliver_to_target(payload, target_cfg).await);
                }
            }
        }

        outcomes
    }

    /// Deliver to a specific target configuration
    async fn deliver_to_target(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> DeliveryOutcome {
        let kind = match config {
            DeliveryTargetConfig::Gateway { .. } => "gateway",
            DeliveryTargetConfig::Memory { .. } => "memory",
            DeliveryTargetConfig::Webhook { .. } => "webhook",
        };

        match self.targets.get(kind) {
            Some(target) => match target.deliver(payload, config).await {
                Ok(outcome) => outcome,
                Err(e) => DeliveryOutcome {
                    target_kind: kind.to_string(),
                    success: false,
                    message: Some(format!("Delivery error: {e}")),
                },
            },
            None => DeliveryOutcome {
                target_kind: kind.to_string(),
                success: false,
                message: Some(format!("Target '{kind}' not registered")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicU32, Ordering};

    fn make_payload() -> DeliveryPayload {
        DeliveryPayload {
            source_type: "cron".to_string(),
            task_name: "Test".to_string(),
            agent_id: "main".to_string(),
            output: "done".to_string(),
            channel_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Test delivery target that records calls
    struct MockTarget {
        kind: String,
        call_count: AtomicU32,
        should_fail: bool,
    }

    impl MockTarget {
        fn new(kind: &str, should_fail: bool) -> Self {
            Self {
                kind: kind.to_string(),
                call_count: AtomicU32::new(0),
                should_fail,
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DeliveryTarget for MockTarget {
        fn kind(&self) -> &str {
            &self.kind
        }

        async fn deliver(
            &self,
            _payload: &DeliveryPayload,
            _config: &DeliveryTargetConfig,
        ) -> Result<DeliveryOutcome, DeliveryError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                Err(DeliveryError::Failed("mock failure".into()))
            } else {
                Ok(DeliveryOutcome {
                    target_kind: self.kind.clone(),
                    success: true,
                    message: None,
                })
            }
        }
    }

    #[tokio::test]
    async fn test_delivery_none_mode() {
        let engine = DeliveryEngine::new();
        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::None,
            targets: vec![],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&payload, &config).await;
        assert!(outcomes.is_empty());
    }

    #[tokio::test]
    async fn test_delivery_primary_mode() {
        let mut engine = DeliveryEngine::new();
        let mock = Arc::new(MockTarget::new("webhook", false));
        engine.register(mock.clone());

        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&payload, &config).await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_primary_with_fallback() {
        let mut engine = DeliveryEngine::new();
        let failing = Arc::new(MockTarget::new("gateway", true));
        let fallback = Arc::new(MockTarget::new("webhook", false));
        engine.register(failing.clone());
        engine.register(fallback.clone());

        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Gateway {
                channel: "telegram".into(),
                chat_id: "123".into(),
                format: None,
            }],
            fallback_target: Some(DeliveryTargetConfig::Webhook {
                url: "https://fallback.com".into(),
                method: None,
                headers: None,
            }),
        };

        let outcomes = engine.deliver(&payload, &config).await;
        assert_eq!(outcomes.len(), 2);
        assert!(!outcomes[0].success); // Primary failed
        assert!(outcomes[1].success); // Fallback succeeded
        assert_eq!(failing.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_broadcast_mode() {
        let mut engine = DeliveryEngine::new();
        let webhook = Arc::new(MockTarget::new("webhook", false));
        let memory = Arc::new(MockTarget::new("memory", false));
        engine.register(webhook.clone());
        engine.register(memory.clone());

        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::Broadcast,
            targets: vec![
                DeliveryTargetConfig::Webhook {
                    url: "https://example.com".into(),
                    method: None,
                    headers: None,
                },
                DeliveryTargetConfig::Memory {
                    tags: vec!["cron".into()],
                    importance: None,
                },
            ],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&payload, &config).await;
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.success));
        assert_eq!(webhook.calls(), 1);
        assert_eq!(memory.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_unregistered_target() {
        let engine = DeliveryEngine::new(); // No targets registered
        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&payload, &config).await;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0]
            .message
            .as_ref()
            .unwrap()
            .contains("not registered"));
    }

    #[tokio::test]
    async fn test_delivery_primary_empty_targets_returns_failure_outcome() {
        let engine = DeliveryEngine::new();
        let payload = make_payload();
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![],
            fallback_target: None,
        };
        let outcomes = engine.deliver(&payload, &config).await;
        assert_eq!(
            outcomes.len(),
            1,
            "empty targets must produce a single observable failure outcome"
        );
        assert!(!outcomes[0].success);
        assert_eq!(outcomes[0].target_kind, "primary");
        assert!(
            outcomes[0]
                .message
                .as_ref()
                .unwrap()
                .contains("no primary target configured"),
            "failure outcome must explain the configuration error"
        );
    }

}



//! Delivery pipeline for cron job results.
//!
//! Supports pluggable delivery targets via the `DeliveryTarget` trait.
//! Built-in targets: Gateway (Telegram/Discord), Webhook, Memory.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::cron::config::{
    CronJob, DeliveryConfig, DeliveryMode, DeliveryOutcome, DeliveryTargetConfig, JobRun,
};

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

/// Trait for delivery targets.
///
/// Each implementation handles delivering job results to a specific destination.
#[async_trait]
pub trait DeliveryTarget: Send + Sync {
    /// Identifier for this delivery target type
    fn kind(&self) -> &str;

    /// Deliver a job result to the target
    async fn deliver(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError>;
}

/// Delivery engine that dispatches results to registered targets.
pub struct DeliveryEngine {
    targets: HashMap<String, Arc<dyn DeliveryTarget>>,
}

impl DeliveryEngine {
    pub fn new() -> Self {
        Self {
            targets: HashMap::new(),
        }
    }

    /// Register a delivery target
    pub fn register(&mut self, target: Arc<dyn DeliveryTarget>) {
        self.targets.insert(target.kind().to_string(), target);
    }

    /// Execute delivery for a job result according to its config
    pub async fn deliver(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryConfig,
    ) -> Vec<DeliveryOutcome> {
        let mut outcomes = Vec::new();

        match config.mode {
            DeliveryMode::None => {}
            DeliveryMode::Primary => {
                if let Some(target_cfg) = config.targets.first() {
                    let outcome = self.deliver_to_target(job, run, target_cfg).await;
                    let success = outcome.success;
                    outcomes.push(outcome);

                    // Fallback on failure
                    if !success {
                        if let Some(fallback) = &config.fallback_target {
                            outcomes.push(self.deliver_to_target(job, run, fallback).await);
                        }
                    }
                }
            }
            DeliveryMode::Broadcast => {
                for target_cfg in &config.targets {
                    outcomes.push(self.deliver_to_target(job, run, target_cfg).await);
                }
            }
        }

        outcomes
    }

    /// Deliver to a specific target configuration
    async fn deliver_to_target(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryTargetConfig,
    ) -> DeliveryOutcome {
        let kind = match config {
            DeliveryTargetConfig::Gateway { .. } => "gateway",
            DeliveryTargetConfig::Memory { .. } => "memory",
            DeliveryTargetConfig::Webhook { .. } => "webhook",
        };

        match self.targets.get(kind) {
            Some(target) => match target.deliver(job, run, config).await {
                Ok(outcome) => outcome,
                Err(e) => DeliveryOutcome {
                    target_kind: kind.to_string(),
                    success: false,
                    message: Some(format!("Delivery error: {}", e)),
                },
            },
            None => DeliveryOutcome {
                target_kind: kind.to_string(),
                success: false,
                message: Some(format!("Target '{}' not registered", kind)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::*;
    use std::sync::atomic::{AtomicU32, Ordering};

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
            _job: &CronJob,
            _run: &JobRun,
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
        let job = CronJob::new("Test", "0 0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::None,
            targets: vec![],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert!(outcomes.is_empty());
    }

    #[tokio::test]
    async fn test_delivery_primary_mode() {
        let mut engine = DeliveryEngine::new();
        let mock = Arc::new(MockTarget::new("webhook", false));
        engine.register(mock.clone());

        let job = CronJob::new("Test", "0 0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
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

        let job = CronJob::new("Test", "0 0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
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

        let outcomes = engine.deliver(&job, &run, &config).await;
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

        let job = CronJob::new("Test", "0 0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
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

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.success));
        assert_eq!(webhook.calls(), 1);
        assert_eq!(memory.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_unregistered_target() {
        let engine = DeliveryEngine::new(); // No targets registered
        let job = CronJob::new("Test", "0 0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0].message.as_ref().unwrap().contains("not registered"));
    }
}

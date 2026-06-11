//! Concrete delivery targets for the shared delivery pipeline.
//!
//! - [`GatewayDeliveryTarget`] — pushes a result to a user-facing channel
//!   (Telegram / Discord / Slack / …) via the gateway `ChannelRegistry`.
//!   This is the primary "AI comes to you" path (R5).
//! - [`MemoryDeliveryTarget`] — records a result into raw memory so the
//!   agent retains it across sessions.
//!
//! The `webhook` target lives in `tasks::cron::webhook_target` and is
//! registered alongside these.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tracing::{info, warn};

use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::tasks::shared::delivery::{
    DeliveryError, DeliveryOutcome, DeliveryPayload, DeliveryTarget, DeliveryTargetConfig,
};

/// Deferred handle to the channel registry — populated once channels are up.
pub type ChannelRegistryCell = Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>;

// ── GatewayDeliveryTarget ────────────────────────────────────────────

/// Delivers task results to a user-facing channel via the gateway.
pub struct GatewayDeliveryTarget {
    channel_registry: ChannelRegistryCell,
}

impl GatewayDeliveryTarget {
    pub const fn new(channel_registry: ChannelRegistryCell) -> Self {
        Self { channel_registry }
    }
}

#[async_trait]
impl DeliveryTarget for GatewayDeliveryTarget {
    fn kind(&self) -> &str {
        "gateway"
    }

    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let (channel, chat_id) = match config {
            DeliveryTargetConfig::Gateway {
                channel, chat_id, ..
            } => (channel, chat_id),
            _ => {
                return Err(DeliveryError::InvalidConfig(
                    "expected Gateway config".into(),
                ))
            }
        };

        let registry = self
            .channel_registry
            .get()
            .ok_or_else(|| DeliveryError::Failed("ChannelRegistry not yet initialized".into()))?;

        let ch_id = ChannelId::new(channel.as_str());
        let message = OutboundMessage::text(chat_id.clone(), payload.output.clone());

        match registry.send(&ch_id, message).await {
            Ok(_) => {
                info!(
                    channel = %channel,
                    chat_id = %chat_id,
                    source = %payload.source_type,
                    "task result delivered via gateway"
                );
                Ok(DeliveryOutcome {
                    target_kind: "gateway".to_string(),
                    success: true,
                    message: None,
                })
            }
            Err(e) => Err(DeliveryError::Failed(format!("gateway send failed: {e}"))),
        }
    }
}

// ── MemoryDeliveryTarget ─────────────────────────────────────────────

/// Records task results into raw memory.
pub struct MemoryDeliveryTarget {
    store: Arc<dyn RawMemoryStore>,
}

impl MemoryDeliveryTarget {
    pub fn new(store: Arc<dyn RawMemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DeliveryTarget for MemoryDeliveryTarget {
    fn kind(&self) -> &str {
        "memory"
    }

    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        if !matches!(config, DeliveryTargetConfig::Memory { .. }) {
            return Err(DeliveryError::InvalidConfig(
                "expected Memory config".into(),
            ));
        }

        // The raw-memory layer carries no tags/importance — those are
        // assigned downstream by the compression pipeline. We fold the task
        // identity into the content so the result is self-describing.
        let content = format!(
            "[{} task '{}'] {}",
            payload.source_type, payload.task_name, payload.output
        );
        let raw = RawMemory::new(content, RawMemorySource::ToolOutput)
            .with_agent(payload.agent_id.clone());

        match self.store.insert_raw_memory(&raw).await {
            Ok(()) => Ok(DeliveryOutcome {
                target_kind: "memory".to_string(),
                success: true,
                message: None,
            }),
            Err(e) => {
                warn!(error = %e, "failed to record task result into memory");
                Err(DeliveryError::Failed(format!("memory insert failed: {e}")))
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> DeliveryPayload {
        DeliveryPayload {
            source_type: "heartbeat".to_string(),
            task_name: "Gmail check".to_string(),
            agent_id: "main".to_string(),
            output: "3 unread emails".to_string(),
            channel_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn gateway_rejects_non_gateway_config() {
        let target = GatewayDeliveryTarget::new(Arc::new(tokio::sync::OnceCell::new()));
        assert_eq!(target.kind(), "gateway");
        let err = target
            .deliver(
                &payload(),
                &DeliveryTargetConfig::Memory {
                    tags: vec![],
                    importance: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DeliveryError::InvalidConfig(_)));
    }

    #[tokio::test]
    async fn gateway_fails_soft_when_registry_uninitialized() {
        let target = GatewayDeliveryTarget::new(Arc::new(tokio::sync::OnceCell::new()));
        let err = target
            .deliver(
                &payload(),
                &DeliveryTargetConfig::Gateway {
                    channel: "telegram".into(),
                    chat_id: "123".into(),
                    format: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DeliveryError::Failed(_)));
    }

    #[tokio::test]
    async fn memory_records_raw_memory() {
        use crate::error::AlephError;
        use crate::sync_primitives::Mutex;

        struct FakeStore {
            inserted: Mutex<Vec<RawMemory>>,
        }
        #[async_trait]
        impl RawMemoryStore for FakeStore {
            async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
                self.inserted
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(raw.clone());
                Ok(())
            }
            async fn get_unprocessed_raw_memories(
                &self,
                _agent_id: &str,
                _limit: usize,
            ) -> Result<Vec<RawMemory>, AlephError> {
                Ok(Vec::new())
            }
            async fn mark_raw_as_processed(&self, _ids: &[String]) -> Result<usize, AlephError> {
                Ok(0)
            }
            async fn count_unprocessed(&self, _agent_id: &str) -> Result<usize, AlephError> {
                Ok(0)
            }
            async fn get_raw_by_path_prefix(
                &self,
                _path_prefix: &str,
                _agent_id: &str,
                _limit: usize,
            ) -> Result<Vec<RawMemory>, AlephError> {
                Ok(Vec::new())
            }
        }

        let store = Arc::new(FakeStore {
            inserted: Mutex::new(Vec::new()),
        });
        let target = MemoryDeliveryTarget::new(store.clone());
        assert_eq!(target.kind(), "memory");

        let outcome = target
            .deliver(
                &payload(),
                &DeliveryTargetConfig::Memory {
                    tags: vec!["mail".into()],
                    importance: None,
                },
            )
            .await
            .unwrap();
        assert!(outcome.success);

        let inserted = store.inserted.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(inserted.len(), 1);
        assert!(inserted[0].content.contains("3 unread emails"));
        assert_eq!(inserted[0].agent_id, "main");
    }
}

//! Cross-surface origin reply fan-out (goal #1, sub-gap (b)).
//!
//! When a run is initiated from one surface (e.g. the `WebChat` Panel) against a
//! session whose conversation *originated* on an external channel (Telegram,
//! Slack, ...), the agent's final reply should also reach that origin channel so
//! the two ends stay in sync ("AI comes to you" / one-core-many-channels, R5/R6).
//!
//! This decorator wraps the run's primary [`EventEmitter`] (the Panel's
//! `GatewayEventEmitter`) and, on `RunComplete`, delivers the final response as a
//! single message to the bound origin channel via the [`ChannelRegistry`].
//! Everything else passes straight through to the inner emitter — only the final
//! text is mirrored, so tool/reasoning chrome is never double-streamed to the
//! channel and the inner emitter's sequencing is untouched.
//!
//! Inbound channel runs never reach this path (they deliver via `ReplyEmitter`
//! in the inbound router), so there is no double delivery: the fan-out is only
//! wired on the gateway/Panel run path and only when the run's surface differs
//! from the session's recorded origin channel.

use crate::sync_primitives::RwLock;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

use super::types::{EventEmitError, StreamEvent};
use super::EventEmitter;
use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;

/// Global channel-registry handle, injected once at gateway boot, so the Panel
/// run path can fan a final reply back to an origin channel without threading
/// the registry through `AgentRunManager`'s constructor and its test sites.
/// Mirrors the `middleware::request_state` global-registry pattern.
static CHANNEL_REGISTRY: OnceLock<RwLock<Option<Arc<ChannelRegistry>>>> = OnceLock::new();

fn registry_slot() -> &'static RwLock<Option<Arc<ChannelRegistry>>> {
    CHANNEL_REGISTRY.get_or_init(|| RwLock::new(None))
}

/// Inject the gateway's channel registry. Called once during subsystem boot.
pub fn set_channel_registry(registry: Arc<ChannelRegistry>) {
    let mut guard = registry_slot().write().unwrap_or_else(|e| e.into_inner());
    *guard = Some(registry);
}

/// Fetch the injected channel registry, if boot wired one. `None` in contexts
/// that never built a gateway (unit tests, CLI subcommands) — fan-out is then
/// simply skipped.
#[must_use]
pub fn channel_registry() -> Option<Arc<ChannelRegistry>> {
    let guard = registry_slot().read().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

/// Decorator that mirrors a run's final reply to a bound origin channel while
/// delegating every event to the inner (primary) emitter.
pub struct OriginFanoutEmitter {
    inner: Arc<dyn EventEmitter + Send + Sync>,
    registry: Arc<ChannelRegistry>,
    origin_channel: ChannelId,
    origin_conversation: String,
}

impl OriginFanoutEmitter {
    /// Wrap `inner`, delivering the final reply to `(origin_channel,
    /// origin_conversation)` once `RunComplete` carries it.
    pub fn new(
        inner: Arc<dyn EventEmitter + Send + Sync>,
        registry: Arc<ChannelRegistry>,
        origin_channel: impl Into<String>,
        origin_conversation: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            registry,
            origin_channel: ChannelId::new(origin_channel),
            origin_conversation: origin_conversation.into(),
        }
    }

    /// Best-effort single-message delivery to the origin channel. A delivery
    /// failure (channel offline, etc.) must never abort the run, so the error
    /// is logged and swallowed.
    async fn deliver_final(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let msg = OutboundMessage::text(self.origin_conversation.clone(), text.to_string());
        if let Err(e) = self.registry.send(&self.origin_channel, msg).await {
            tracing::warn!(
                channel = %self.origin_channel.as_str(),
                "origin reply fan-out failed: {e}"
            );
        }
    }
}

#[async_trait]
impl EventEmitter for OriginFanoutEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        if let StreamEvent::RunComplete { ref summary, .. } = event {
            // Mirror the *deliverable* text — same single-source sanitizer the
            // inbound `ReplyEmitter` and the persisted-transcript path use — so
            // the origin channel never receives raw `<think>`/completion markers
            // (which `summary.final_response` still carries verbatim) and a
            // pure-thinking turn delivers nothing instead of leaking noise.
            if let Some(text) = summary
                .final_response
                .as_deref()
                .and_then(crate::gateway::reply_emitter::sanitize_final_response)
            {
                self.deliver_final(&text).await;
            }
        }
        // Always forward to the primary emitter (Panel sees the full stream).
        self.inner.emit(event).await
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::{CollectingEventEmitter, RunSummary};

    #[tokio::test]
    async fn forwards_all_events_to_inner_even_when_delivery_unavailable() {
        // No channel handle registered → registry.send errors → swallowed,
        // but the inner emitter must still receive the RunComplete (the Panel
        // stream is never blocked by a failed fan-out).
        let inner = Arc::new(CollectingEventEmitter::new());
        let fanout = OriginFanoutEmitter::new(
            inner.clone(),
            Arc::new(ChannelRegistry::new()),
            "telegram",
            "chat-123",
        );

        fanout
            .emit(StreamEvent::RunComplete {
                run_id: "r1".to_string(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("hello from panel".to_string()),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let events = inner.events().await;
        assert_eq!(events.len(), 1, "RunComplete must reach the inner emitter");
    }

    /// The mirrored origin reply is sanitized at the fan-out boundary — the
    /// inner Panel stream still sees the unaltered `RunComplete`, but the text
    /// handed to the origin channel goes through the same single-source
    /// sanitizer as the inbound `ReplyEmitter`, so raw `<think>` blocks never
    /// leak to Telegram/Slack. (Delivery itself is best-effort and untestable
    /// without a registered channel; this pins the inner-stream invariant and
    /// the sanitize wiring via the shared atom's own test coverage.)
    #[tokio::test]
    async fn forwards_run_complete_with_reasoning_tags_unaltered_to_inner() {
        let inner = Arc::new(CollectingEventEmitter::new());
        let fanout = OriginFanoutEmitter::new(
            inner.clone(),
            Arc::new(ChannelRegistry::new()),
            "telegram",
            "chat-9",
        );

        fanout
            .emit(StreamEvent::RunComplete {
                run_id: "r9".to_string(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("<think>plan</think>visible".to_string()),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        // Inner (Panel) stream is never rewritten by the decorator.
        match &inner.events().await[0] {
            StreamEvent::RunComplete { summary, .. } => assert_eq!(
                summary.final_response.as_deref(),
                Some("<think>plan</think>visible"),
                "inner stream must receive the unaltered summary"
            ),
            other => panic!("expected RunComplete, got {other:?}"),
        }
    }
}

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
use crate::gateway::channel::{outbound_chunk_len, ChannelId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::formatter::MessageFormatter;

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

    /// Best-effort delivery to the origin channel. A delivery failure (channel
    /// offline, etc.) must never abort the run, so the error is logged and
    /// swallowed.
    ///
    /// Chunked against the channel's own declared cap, through the same
    /// [`outbound_chunk_len`] / [`MessageFormatter::split`] pair the inbound
    /// `ReplyEmitter` uses. This path sent the entire final response as one
    /// unsplit frame — and it is the delivery path for cron ticks, goal/loop
    /// continuations and resumed runs, so on a channel that both caps below
    /// that length and does not split internally (Discord: 2000, no splitter)
    /// every autonomous-run result over the cap came back `SendFailed`, which
    /// the durable queue refuses. The whole answer vanished behind one `warn!`.
    async fn deliver_final(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        // A channel that has gone away answers `None`; `outbound_chunk_len`
        // then supplies the conservative fallback rather than the send being
        // skipped here — the registry's own error is the better report.
        let declared = self
            .registry
            .get_capabilities(&self.origin_channel)
            .await
            .map_or(0, |caps| caps.max_message_length);
        let chunks = MessageFormatter::split(text, outbound_chunk_len(declared));

        for chunk in chunks {
            let msg = OutboundMessage::text(self.origin_conversation.clone(), chunk);
            if let Err(e) = self.registry.send(&self.origin_channel, msg).await {
                let transient = crate::gateway::delivery_queue::should_enqueue(&e);
                tracing::warn!(
                    channel = %self.origin_channel.as_str(),
                    transient,
                    "origin reply fan-out failed: {e}"
                );
                // Same split as the ReplyEmitter's chokepoint, asked of the
                // queue's own predicate: a transient failure means each
                // remaining chunk is persisted for durable retry, so keep
                // offering them; a terminal one means the tail would only add
                // noise.
                if !transient {
                    break;
                }
            }
        }
    }
}

#[async_trait]
impl EventEmitter for OriginFanoutEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Mirror the *deliverable* text — same single-source sanitizer the
        // inbound `ReplyEmitter` and the persisted-transcript path use — so the
        // origin channel never receives raw `<think>`/completion markers (which
        // `summary.final_response` still carries verbatim) and a pure-thinking
        // turn delivers nothing instead of leaking noise.
        let mirror = match event {
            StreamEvent::RunComplete { ref summary, .. } => summary
                .final_response
                .as_deref()
                .and_then(crate::gateway::reply_emitter::sanitize_final_response),
            _ => None,
        };

        // Primary emitter FIRST (Panel sees the full stream). `emit` is
        // serialized per run, so awaiting a cross-surface channel send before
        // forwarding made one slow Telegram/Slack API call hold up the Panel's
        // terminal frame — and every event behind it — for the duration of the
        // remote round-trip. The mirror is best-effort by contract; the primary
        // stream is not, so it goes first.
        let forwarded = self.inner.emit(event).await;
        if let Some(text) = mirror {
            self.deliver_final(&text).await;
        }
        forwarded
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

    /// This path had no chunker at all — not even a wrong constant. It is the
    /// delivery path for cron ticks, goal/loop continuations and resumed runs,
    /// so on a channel that caps below the answer's length and does not split
    /// internally (Discord: 2000, and its `Channel::send` builds one
    /// `CreateMessage` with no split) every autonomous-run result over the cap
    /// came back `SendFailed` — which the durable queue refuses — and vanished
    /// behind a single `warn!`.
    #[tokio::test]
    async fn the_origin_fanout_chunks_against_the_channels_declared_cap() {
        use crate::gateway::channel::{
            Channel, ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState, ChannelStatus,
            MessageId, SendResult,
        };

        struct Capped {
            info: ChannelInfo,
            state: ChannelState,
            seen: Arc<tokio::sync::Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl Channel for Capped {
            fn info(&self) -> &ChannelInfo {
                &self.info
            }
            fn state(&self) -> &ChannelState {
                &self.state
            }
            async fn start(&mut self) -> ChannelResult<()> {
                Ok(())
            }
            async fn stop(&mut self) -> ChannelResult<()> {
                Ok(())
            }
            async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
                self.seen.lock().await.push(message.text.clone());
                Ok(SendResult {
                    message_id: MessageId::new("ok"),
                    timestamp: chrono::Utc::now(),
                })
            }
        }

        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let registry = ChannelRegistry::new();
        registry
            .register(Box::new(Capped {
                info: ChannelInfo {
                    id: ChannelId::new("discordish"),
                    name: "discordish".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities {
                        max_message_length: 2000,
                        ..ChannelCapabilities::default()
                    },
                },
                state: ChannelState::new(8),
                seen: seen.clone(),
            }))
            .await;

        let fanout = OriginFanoutEmitter::new(
            Arc::new(CollectingEventEmitter::new()),
            Arc::new(registry),
            "discordish",
            "chat-cap",
        );
        fanout
            .emit(StreamEvent::RunComplete {
                run_id: "r-cap".to_string(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("a".repeat(3000)),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let seen = seen.lock().await;
        assert!(
            seen.len() >= 2,
            "3000 chars must not reach a 2000-cap transport in one frame"
        );
        for (i, text) in seen.iter().enumerate() {
            assert!(
                text.len() <= 2000,
                "fan-out chunk {i} is {} bytes, over the channel's declared cap",
                text.len()
            );
        }
    }

    /// The four construction sites are still the four that cannot carry a side
    /// question.
    ///
    /// `btw::format_side_answer`'s doc answers each of them by name — why an
    /// announce, a resume, a goal/loop continuation and the Simulated-fallback
    /// `start_run` can never be a `/btw` turn, and therefore why this decorator
    /// applies no side-answer marker. That answer is a paragraph, and a
    /// paragraph does not notice a fifth site. This does.
    ///
    /// Source-level and by name, not a count: a bare number tells the next
    /// author that something moved, not what to go read. Comment lines are
    /// stripped first — a scanner judges code, and the doc that explains a
    /// name is the most likely thing to mention it.
    #[test]
    fn the_fan_out_construction_sites_are_still_the_four_that_cannot_carry_a_side_question() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        /// Every production site, each answered in `btw::format_side_answer`.
        const ANSWERED: &[&str] = &[
            "src/gateway/announce_delivery.rs",
            "src/gateway/busy_queue/durable.rs",
            "src/gateway/resume_coordinator.rs",
            "src/gateway/execution_engine/execute.rs",
            "src/gateway/handlers/agent.rs",
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");

        let mut found: Vec<String> = Vec::new();
        for file in files {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            // This file's own test module builds two; they are the guard's own
            // fixtures, not deliveries to a user.
            if rel == "src/gateway/event_emitter/origin_fanout.rs" {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if text
                .lines()
                .map(str::trim_start)
                .filter(|code| !code.starts_with("//") && !code.starts_with('*'))
                .any(|code| code.contains("OriginFanoutEmitter::new("))
            {
                found.push(rel);
            }
        }
        found.sort();

        let mut expected: Vec<String> = ANSWERED.iter().map(|s| (*s).to_string()).collect();
        expected.sort();

        assert_eq!(
            found, expected,
            "the set of files constructing an `OriginFanoutEmitter` changed. A \
             new site must answer the question the other four answer in \
             `gateway::btw::format_side_answer`'s doc — can a `/btw` run reach \
             it? If it can, it needs the marker; if it cannot, say why there \
             and add it here. A site that disappeared should be dropped from \
             that paragraph in the same edit."
        );
    }
}

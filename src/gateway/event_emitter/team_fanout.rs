//! Team-scoped run event fan-out.
//!
//! When a member agent run belongs to a team, this decorator republishes a
//! normalized, agent-attributed view of key stream events onto
//! `team.<team_id>.{message,activity}` topics so the Panel's team-chat view can
//! render attributed bubbles and roster status badges in real time.
//!
//! [`publish_team_event`] is the same wire format without a run behind it: the
//! broadcaster uses it for the fan-out tree's `fanout` lifecycle and for
//! in-chat `system` notices, and `CoordTaskStore` publishes `task.<verb>`.
//! Every `team.<id>.*` topic therefore shares one envelope shape, matched
//! Panel-side by `views::chat::team_events::parse_team_topic`.
//!
//! # Design
//!
//! Mirrors [`super::origin_fanout::OriginFanoutEmitter`] as the established
//! precedent for decorator-style fan-out. The inner emitter is optional:
//! member runs that previously used `NoOpEventEmitter` pass `None`, while runs
//! that also feed the Panel's primary stream pass `Some(inner)`.
//!
//! The fan-out is best-effort: bus publish failures are silently swallowed so
//! that a lagging subscriber never aborts a run.
//!
//! # Wire format
//!
//! Published JSON matches the `{topic, data}` envelope consumed by the Panel's
//! WS topic subscription handler:
//!
//! ```json
//! { "topic": "team.<team_id>.message", "data": { "agent_id": "...", "text": "...", ... } }
//! ```

use crate::sync_primitives::RwLock;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::types::{EventEmitError, StreamEvent};
use super::EventEmitter;
use crate::gateway::event_bus::GatewayEventBus;

/// Global event-bus handle, injected once at gateway boot, so the team
/// dispatcher's member-run path can fan run events onto `team.<id>.*` without
/// threading the bus through `GatewayContext`/dispatcher constructors and their
/// test sites. Mirrors `origin_fanout::CHANNEL_REGISTRY` / the
/// `middleware::request_state` global-registry pattern.
static TEAM_EVENT_BUS: OnceLock<RwLock<Option<Arc<GatewayEventBus>>>> = OnceLock::new();

fn team_event_bus_slot() -> &'static RwLock<Option<Arc<GatewayEventBus>>> {
    TEAM_EVENT_BUS.get_or_init(|| RwLock::new(None))
}

/// Inject the gateway's event bus. Called once during subsystem boot.
pub fn set_team_event_bus(bus: Arc<GatewayEventBus>) {
    let mut guard = team_event_bus_slot()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *guard = Some(bus);
}

/// Fetch the injected event bus, if boot wired one. `None` in contexts that
/// never built a gateway (unit tests, CLI subcommands) — team fan-out is then
/// simply skipped and the run stays silent (the prior NoOp behavior).
#[must_use]
pub fn team_event_bus() -> Option<Arc<GatewayEventBus>> {
    let guard = team_event_bus_slot()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

/// Publish one `{topic: "team.<team_id>.<suffix>", data}` envelope on the
/// injected bus. Single source for the team-topic wire format: both
/// [`TeamFanoutEmitter`] (per-run stream projection) and the broadcaster's
/// lifecycle/system notices go through here, so the Panel only ever has to
/// match one envelope shape.
///
/// Best-effort: no bus (pre-boot / unit tests) or a serialization failure is a
/// silent no-op — a lagging subscriber must never abort a run.
pub fn publish_team_event(team_id: &str, suffix: &str, data: Value) {
    let Some(bus) = team_event_bus() else { return };
    publish_team_event_on(&bus, team_id, suffix, data);
}

/// [`publish_team_event`] against an explicit bus (used by the emitter, which
/// holds its own handle, and by tests that build a local bus).
fn publish_team_event_on(bus: &GatewayEventBus, team_id: &str, suffix: &str, data: Value) {
    let topic = format!("team.{team_id}.{suffix}");
    let envelope = json!({ "topic": topic, "data": data });
    // Raw string-channel publish (not publish_frame): team.<id>.* is an ad-hoc
    // topic with no GatewayEventFrame variant, and this {topic,data} envelope is
    // byte-identical to publish_frame's non-streaming wire format that the Panel's
    // topic subscription already matches.
    if let Ok(json) = serde_json::to_string(&envelope) {
        bus.publish(json);
    }
}

/// Decorator that republishes a team member run's events onto
/// `team.<team_id>.*` topics, tagged with the producing `agent_id`.
///
/// Construct via [`TeamFanoutEmitter::new`] and pass as the emitter for any
/// run spawned on behalf of a team member.
pub struct TeamFanoutEmitter {
    /// Optional primary emitter to also forward all events to. `None` for
    /// silent member runs (Panel does not subscribe to their raw stream).
    inner: Option<Arc<dyn EventEmitter + Send + Sync>>,
    /// Shared event bus on which team topics are published.
    event_bus: Arc<GatewayEventBus>,
    /// Team identifier — becomes the middle segment of the topic.
    team_id: String,
    /// Agent identifier injected into every fan-out payload.
    agent_id: String,
    /// Local sequence counter, used when `inner` is `None`.
    seq: AtomicU64,
}

impl TeamFanoutEmitter {
    /// Create a new `TeamFanoutEmitter`.
    ///
    /// - `event_bus` — the gateway bus on which `team.<team_id>.*` are published.
    /// - `team_id` — team identifier (e.g. `"team-abc123"`).
    /// - `agent_id` — agent producing the events (e.g. `"agent-alice"`).
    /// - `inner` — optional primary emitter to forward all raw events to; pass
    ///   `None` for member runs that should not stream to the Panel directly.
    pub fn new(
        event_bus: Arc<GatewayEventBus>,
        team_id: impl Into<String>,
        agent_id: impl Into<String>,
        inner: Option<Arc<dyn EventEmitter + Send + Sync>>,
    ) -> Self {
        Self {
            inner,
            event_bus,
            team_id: team_id.into(),
            agent_id: agent_id.into(),
            seq: AtomicU64::new(0),
        }
    }

    /// Publish a `{topic, data}` envelope to the team channel.
    ///
    /// `agent_id` is injected into the data object before serializing.
    /// Serialization errors are swallowed — a bad payload must never abort a run.
    // `mut data` is owned so we can inject agent_id before serializing.
    fn publish(&self, suffix: &str, mut data: Value) {
        // Inject agent_id into the data payload so subscribers know the author.
        // All call sites pass json!({...}) objects, so agent_id injection always applies.
        if let Some(obj) = data.as_object_mut() {
            obj.insert("agent_id".to_string(), json!(self.agent_id));
        }
        publish_team_event_on(&self.event_bus, &self.team_id, suffix, data);
    }
}

#[async_trait]
impl EventEmitter for TeamFanoutEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Fan out normalized payloads to team topics.
        match &event {
            StreamEvent::RunComplete {
                run_id, summary, ..
            } => {
                // Publish the *deliverable* bubble text through the same
                // single-source sanitizer the persisted transcript uses
                // (`reply_emitter::sanitize_final_response`), so the live Panel
                // group-chat bubble can never leak raw `<think>`/completion
                // markers — nor drift from the sanitized transcript row the
                // broadcaster persists from the very same run.
                if let Some(text) = summary
                    .final_response
                    .as_deref()
                    .and_then(crate::gateway::reply_emitter::sanitize_final_response)
                {
                    self.publish(
                        "message",
                        json!({ "run_id": run_id, "text": text, "final": true }),
                    );
                }
                self.publish("activity", json!({ "run_id": run_id, "status": "done" }));
            }
            StreamEvent::RunError { run_id, error, .. } => {
                self.publish(
                    "activity",
                    json!({ "run_id": run_id, "status": "error", "error": error }),
                );
            }
            StreamEvent::ToolStart {
                run_id, tool_name, ..
            } => {
                self.publish(
                    "activity",
                    json!({ "run_id": run_id, "status": "working", "tool": tool_name }),
                );
            }
            // MVP: other events (streaming chunks, reasoning, etc.) are not
            // fanned out to avoid noise in the team chat timeline.
            _ => {}
        }

        // Forward to the primary emitter when one is wired (Panel stream).
        if let Some(inner) = &self.inner {
            inner.emit(event).await?;
        }
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        // Delegate to inner when present so its monotonic counter stays in sync.
        // Fall back to the local atomic counter for NoOp/standalone usage.
        if let Some(inner) = &self.inner {
            inner.next_seq()
        } else {
            self.seq.fetch_add(1, Ordering::Relaxed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;
    use std::sync::Arc;

    #[tokio::test]
    async fn run_less_publish_uses_the_same_envelope_as_the_emitter() {
        // The broadcaster's `fanout` / `system` notices have no StreamEvent
        // behind them, but the Panel matches ONE envelope shape for every
        // `team.<id>.*` topic — so they must go out byte-compatible with what
        // the emitter produces, not as a second ad-hoc format.
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();

        publish_team_event_on(
            &bus,
            "team-7",
            "fanout",
            json!({ "run_id": "r1", "status": "started" }),
        );

        let raw = rx.recv().await.expect("published");
        let v: Value = serde_json::from_str(&raw).expect("valid json envelope");
        assert_eq!(v["topic"], "team.team-7.fanout");
        assert_eq!(v["data"]["run_id"], "r1");
        assert_eq!(v["data"]["status"], "started");
    }

    #[tokio::test]
    async fn run_less_publish_does_not_inject_an_agent_id() {
        // A tree-level or system event belongs to nobody. Injecting an
        // `agent_id` here would make the Panel attribute it to a member.
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();

        publish_team_event_on(&bus, "team-7", "system", json!({ "text": "depth cap" }));

        let raw = rx.recv().await.expect("published");
        let v: Value = serde_json::from_str(&raw).expect("valid json envelope");
        assert_eq!(v["topic"], "team.team-7.system");
        assert!(v["data"].get("agent_id").is_none());
    }

    #[tokio::test]
    async fn test_fanout_publishes_message_topic_with_agent_id() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe(); // raw string channel
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-1", "agent-alice", None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-1".into(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("hello from alice".into()),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        // RunComplete fans out at least the message topic; find it among published frames.
        let mut saw_message = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if v.get("topic").and_then(|t| t.as_str()) == Some("team.team-1.message") {
                assert_eq!(
                    v.pointer("/data/agent_id").and_then(|a| a.as_str()),
                    Some("agent-alice")
                );
                assert_eq!(
                    v.pointer("/data/text").and_then(|a| a.as_str()),
                    Some("hello from alice")
                );
                saw_message = true;
            }
        }
        assert!(
            saw_message,
            "RunComplete should fan out a team.<id>.message with agent_id+text"
        );
    }

    #[tokio::test]
    async fn test_fanout_skips_message_when_final_response_none() {
        // A RunComplete with no final_response must NOT publish a message frame,
        // but should still publish the activity status=done frame.
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-5", "agent-erin", None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-5".into(),
                seq: 0,
                summary: RunSummary {
                    final_response: None,
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let mut saw_message = false;
        let mut saw_activity = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            match v.get("topic").and_then(|t| t.as_str()) {
                Some("team.team-5.message") => saw_message = true,
                Some("team.team-5.activity") => saw_activity = true,
                _ => {}
            }
        }
        assert!(
            !saw_message,
            "RunComplete with no final_response must not publish a message frame"
        );
        assert!(
            saw_activity,
            "RunComplete must still publish a team.<id>.activity status=done frame"
        );
    }

    #[tokio::test]
    async fn test_fanout_sanitizes_reasoning_tags_in_message_text() {
        // The live Panel bubble must carry the *deliverable* text — raw
        // `<think>` blocks the run produced are stripped at the fan-out
        // boundary so the bubble matches the sanitized persisted transcript.
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-9", "agent-zoe", None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-9".into(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("<think>scratch</think>final answer".into()),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let mut saw_clean_message = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if v.get("topic").and_then(|t| t.as_str()) == Some("team.team-9.message") {
                assert_eq!(
                    v.pointer("/data/text").and_then(|t| t.as_str()),
                    Some("final answer"),
                    "fan-out must publish sanitized text, not the raw <think> block"
                );
                saw_clean_message = true;
            }
        }
        assert!(
            saw_clean_message,
            "a message frame should have been published"
        );
    }

    #[tokio::test]
    async fn test_fanout_suppresses_message_for_pure_reasoning_turn() {
        // A turn whose final_response is *only* reasoning sanitizes to empty —
        // no bubble is published (avoids leaking a `<think>`-only noise message),
        // but the activity=done lifecycle frame is still emitted.
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-10", "agent-ivy", None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-10".into(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("<think>only thinking, no answer</think>".into()),
                    ..Default::default()
                },
                total_duration_ms: 0,
            })
            .await
            .unwrap();

        let mut saw_message = false;
        let mut saw_activity = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            match v.get("topic").and_then(|t| t.as_str()) {
                Some("team.team-10.message") => saw_message = true,
                Some("team.team-10.activity") => saw_activity = true,
                _ => {}
            }
        }
        assert!(
            !saw_message,
            "a pure-reasoning turn must not publish a message bubble"
        );
        assert!(saw_activity, "activity=done must still fire");
    }

    #[tokio::test]
    async fn test_fanout_publishes_activity_done_on_run_complete() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-2", "agent-bob", None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-2".into(),
                seq: 0,
                summary: RunSummary {
                    final_response: Some("done".into()),
                    ..Default::default()
                },
                total_duration_ms: 42,
            })
            .await
            .unwrap();

        let mut saw_activity = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if v.get("topic").and_then(|t| t.as_str()) == Some("team.team-2.activity") {
                assert_eq!(
                    v.pointer("/data/status").and_then(|s| s.as_str()),
                    Some("done")
                );
                assert_eq!(
                    v.pointer("/data/agent_id").and_then(|a| a.as_str()),
                    Some("agent-bob")
                );
                saw_activity = true;
            }
        }
        assert!(
            saw_activity,
            "RunComplete should also fan out a team.<id>.activity status=done"
        );
    }

    #[tokio::test]
    async fn test_fanout_tool_start_publishes_working_activity() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-3", "agent-carol", None);

        emitter
            .emit(StreamEvent::ToolStart {
                run_id: "run-3".into(),
                seq: 1,
                tool_name: "bash".into(),
                tool_id: "tool-1".into(),
                params: serde_json::json!({}),
            })
            .await
            .unwrap();

        let mut saw_working = false;
        while let Ok(raw) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if v.get("topic").and_then(|t| t.as_str()) == Some("team.team-3.activity")
                && v.pointer("/data/status").and_then(|s| s.as_str()) == Some("working")
            {
                assert_eq!(
                    v.pointer("/data/tool").and_then(|t| t.as_str()),
                    Some("bash")
                );
                saw_working = true;
            }
        }
        assert!(
            saw_working,
            "ToolStart should publish activity status=working with tool name"
        );
    }

    #[tokio::test]
    async fn test_next_seq_increments_without_inner() {
        let bus = Arc::new(GatewayEventBus::new());
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-4", "agent-dave", None);

        let s0 = emitter.next_seq();
        let s1 = emitter.next_seq();
        let s2 = emitter.next_seq();
        assert!(
            s1 > s0 && s2 > s1,
            "next_seq must be monotonically increasing"
        );
    }

    /// Verify the global boot-injection slot: after `set_team_event_bus`, the
    /// accessor returns `Some`. Does not assert `None` first because the
    /// `OnceLock`-backed global is process-wide and another test in the suite
    /// may have already set it; asserting `is_some()` after setting is stable
    /// regardless of test execution order.
    #[test]
    #[serial_test::serial(gateway_event_bus)]
    fn global_slot_returns_some_after_set() {
        let bus = Arc::new(GatewayEventBus::new());
        set_team_event_bus(bus);
        assert!(
            team_event_bus().is_some(),
            "team_event_bus() must return Some after set_team_event_bus()"
        );
    }
}

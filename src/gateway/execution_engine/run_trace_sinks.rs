//! `RunTraceSinks` — the one place that decides which trace sinks a gateway
//! run and the subagents it spawns write into. `run_loop/inner.rs` calls
//! [`RunTraceSinks::build`] once per run and hands the result to the run's
//! `SubagentTool`; nothing else composes these chains.
//!
//! The route is chosen by PRODUCER, not by event variant:
//!
//! | Producer | Chain |
//! |---|---|
//! | the run's own harness loop | `[UnattendedRedactingSink if unattended] → AgentTraceEmitSink → [ScratchpadProgressSink if armed] → persistence` |
//! | a spawned child's harness loop (sync and background alike) | `[UnattendedRedactingSink if unattended] → [ScratchpadProgressSink sharing the run's queue, if armed] → NoopTraceSink` |
//! | a spawned child's `MeteringProvider`s (the accounting exception) | the run's own chain, unchanged |
//!
//! A child's turns are not the run's steps. Before this split, both spawn
//! paths handed the child the run's own chain, so every child `TurnStarted` /
//! `TextEmitted` / `SessionCompleted` became a parent `agent_trace` frame with
//! the parent's `run_id` and a row under the parent's task, and no field on
//! the event could tell them apart. Background children still report
//! progress to the tracker through `ForwardingTraceSink`, which wraps the
//! child chain.
//!
//! The accounting exception exists because `teams.usage`, the `team_usage`
//! tool and the doctor's cache checks aggregate `provider_usage` /
//! `cache_health_degraded` rows across all tasks by agent id, and a child has
//! no task of its own to persist under (`task_traces.task_id` must name an
//! `agent_tasks` row). So a child's usage still reaches the live wire and is
//! still persisted under the parent's task. `is_step_event` publishes both
//! kinds on the live leg (they feed the cache indicators), so they do not
//! open steps by themselves: a reducer over the parent's replayed rows must
//! treat them as non-step, exactly as the live leg's reducers must.
//!
//! The one-site rule is enforced by type, not by review: a `SubagentTool`
//! receives sinks only through [`ChildTraceSinks`], whose fields are private
//! to this module, so no caller can hand a child the run's own chain as its
//! harness sink.

use tokio::sync::broadcast;

use crate::agents::subagent_tool::SubagentTool;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::harness::{NoopTraceSink, TraceSink};
use crate::orchestrator::dispatch::FlowStreamEvent;
use crate::sync_primitives::Arc;

use super::callback::{CallbackStateFlushHandle, StreamCallbackState};
use super::{
    AgentTraceEmitSink, GatewayTraceSink, ScratchpadProgressSink, UnattendedRedactingSink,
};

/// The user channel a run's scratchpad progress is mirrored to. Its fields are
/// private to `execution_engine`, so only this module tree can bind a run to a
/// channel; code elsewhere can only pass `None`.
pub(crate) struct ScratchpadTarget {
    pub(super) registry: Arc<ChannelRegistry>,
    pub(super) channel_id: String,
    pub(super) chat_id: String,
}

/// The two sinks a run's spawned children write into, split by producer: the
/// harness sink for their loops (both spawn twins) and the accounting sink for
/// their `MeteringProvider`s.
///
/// Only [`RunTraceSinks`] mints one in production: the fields are private to
/// this module. `SubagentTool::with_child_sinks` is the only way to give a
/// child any sink, so the run's own chain cannot be handed to a child's
/// harness loop from anywhere else.
pub(crate) struct ChildTraceSinks {
    harness: Arc<dyn TraceSink>,
    accounting: Arc<dyn TraceSink>,
}

impl ChildTraceSinks {
    /// `(harness, accounting)`.
    pub(crate) fn into_parts(self) -> (Arc<dyn TraceSink>, Arc<dyn TraceSink>) {
        (self.harness, self.accounting)
    }

    /// Arbitrary sinks for tests that observe a child's emissions directly.
    #[cfg(test)]
    pub(crate) fn for_test(harness: Arc<dyn TraceSink>, accounting: Arc<dyn TraceSink>) -> Self {
        Self {
            harness,
            accounting,
        }
    }
}

/// The persistence leaf of a run's chain: the run's trace rows, under the
/// task `state` was built for.
pub(super) fn persistence_leaf(state: Arc<StreamCallbackState>) -> Arc<dyn TraceSink> {
    Arc::new(GatewayTraceSink::new(Arc::new(
        CallbackStateFlushHandle::new(state),
    )))
}

/// A run's trace sinks, split by producer (see the module doc).
pub(crate) struct RunTraceSinks {
    /// The run's own chain; also the accounting sink handed to children.
    run: Arc<dyn TraceSink>,
    /// What a spawned child's harness loop emits into.
    child_harness: Arc<dyn TraceSink>,
}

impl RunTraceSinks {
    /// Compose both chains over the run's persistence leaf and flow channel.
    /// `unattended` wraps each chain in `UnattendedRedactingSink` outermost,
    /// so every event is masked before any sink beneath it sees it.
    pub(crate) fn build(
        persistence: Arc<dyn TraceSink>,
        scratchpad: Option<ScratchpadTarget>,
        event_tx: &broadcast::Sender<FlowStreamEvent>,
        unattended: bool,
    ) -> Self {
        let scratchpad = scratchpad.map(|target| {
            Arc::new(ScratchpadProgressSink::new(
                persistence.clone(),
                target.registry,
                target.channel_id,
                target.chat_id,
            ))
        });

        let below_emit: Arc<dyn TraceSink> = match &scratchpad {
            Some(sink) => sink.clone(),
            None => persistence,
        };
        let run = redact_if(
            unattended,
            Arc::new(AgentTraceEmitSink::new(below_emit, event_tx)),
        );

        // The child chain has no emit sink and no persistence: it ends at the
        // subagent boundary. It keeps the scratchpad push, sharing the run's
        // queue so parent and child lines reach the channel in one order.
        let child_end: Arc<dyn TraceSink> = Arc::new(NoopTraceSink);
        let child_harness = redact_if(
            unattended,
            match &scratchpad {
                Some(sink) => Arc::new(sink.sharing_queue(child_end)),
                None => child_end,
            },
        );

        Self { run, child_harness }
    }

    /// The run's own chain, handed to the run's harness.
    pub(crate) fn run_sink(&self) -> Arc<dyn TraceSink> {
        Arc::clone(&self.run)
    }

    /// The sinks a spawned child writes into: the child chain for its harness
    /// loop and the run's own chain for its `MeteringProvider`s.
    pub(crate) fn child_sinks(&self) -> ChildTraceSinks {
        ChildTraceSinks {
            harness: Arc::clone(&self.child_harness),
            accounting: Arc::clone(&self.run),
        }
    }

    /// Hand a run's `SubagentTool` the sinks its children write into (both
    /// spawn twins read them).
    #[must_use]
    pub(crate) fn hand_to_subagents(&self, tool: SubagentTool) -> SubagentTool {
        tool.with_child_sinks(self.child_sinks())
    }
}

fn redact_if(unattended: bool, sink: Arc<dyn TraceSink>) -> Arc<dyn TraceSink> {
    if unattended {
        Arc::new(UnattendedRedactingSink::new(sink))
    } else {
        sink
    }
}

/// Test probe over the real persistence leaf: the sink to hand
/// [`RunTraceSinks::build`], plus a way to await every pending row write.
#[cfg(test)]
pub(crate) struct PersistenceProbe {
    state: Arc<StreamCallbackState>,
}

#[cfg(test)]
impl PersistenceProbe {
    pub(crate) fn new(db: Arc<crate::resilience::StateDatabase>, task_id: &str) -> Self {
        Self {
            state: Arc::new(StreamCallbackState::new(Some(Arc::new(
                super::callback::TracePersistence::new(db, task_id.to_string()),
            )))),
        }
    }

    /// The same leaf `run_loop/inner.rs` builds, over this probe's task.
    pub(crate) fn sink(&self) -> Arc<dyn TraceSink> {
        persistence_leaf(Arc::clone(&self.state))
    }

    /// Await every row write recorded so far.
    pub(crate) async fn drain(&self) {
        self.state.flush_trace_persistence().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{
        Channel, ChannelId, ChannelInfo, ChannelResult, ChannelState, ChannelStatus, MessageId,
        OutboundMessage, SendResult,
    };
    use crate::harness::trace::{LoopTraceEvent, ToolCallEndEvent};
    use crate::tools::runtime::ToolResult;

    /// Records every message the registry hands it: what actually left for
    /// the user's chat.
    struct RecordingChannel {
        info: ChannelInfo,
        state: ChannelState,
        sent: Arc<tokio::sync::Mutex<Vec<OutboundMessage>>>,
    }

    #[async_trait::async_trait]
    impl Channel for RecordingChannel {
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
            self.sent.lock().await.push(message);
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    const PEM_BODY: &str = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQ";

    /// A child's scratchpad progress (the text the scratchpad sharer pushes to
    /// the user's bound channel) carrying a PEM private key.
    fn child_progress_with_a_key() -> LoopTraceEvent {
        let progress = format!(
            "[x] rotate the deploy key\n-----BEGIN PRIVATE KEY-----\n{PEM_BODY}...\n-----END PRIVATE KEY-----"
        );
        LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "t1".into(),
                tool_name: "scratchpad".into(),
                input: serde_json::json!({ "action": "complete_item" }),
                duration_ms: 5,
            },
            result: ToolResult::Success {
                output: serde_json::json!({ "success": true, "progress": progress }),
            },
        }
    }

    /// On an unattended run the CHILD chain is masked outermost, exactly like
    /// the run's own chain: a child's scratchpad progress reaches the bound
    /// channel through the queue it shares with the run, with a PEM key
    /// redacted. Red if the child chain loses its `UnattendedRedactingSink`
    /// wrap (the key body reaches the channel). Red the other way if the
    /// child's progress never reaches the channel at all (the sharer is
    /// dropped or pushes into a dead queue).
    #[tokio::test]
    async fn a_childs_text_reaches_the_scratchpad_sharer_masked_on_unattended_runs() {
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let registry = ChannelRegistry::new();
        registry
            .register(Box::new(RecordingChannel {
                info: ChannelInfo {
                    id: ChannelId::new("rec"),
                    name: "rec".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: Default::default(),
                },
                state: ChannelState::new(8),
                sent: sent.clone(),
            }))
            .await;
        let (tx, _rx) = crate::orchestrator::flow_event_channel();
        let sinks = RunTraceSinks::build(
            Arc::new(NoopTraceSink),
            Some(ScratchpadTarget {
                registry: Arc::new(registry),
                channel_id: "rec".to_string(),
                chat_id: "conv-1".to_string(),
            }),
            &tx,
            true,
        );
        let (child_harness, _accounting) = sinks.child_sinks().into_parts();

        child_harness.on_trace(&child_progress_with_a_key());

        let mut delivered = Vec::new();
        for _ in 0..100 {
            delivered = sent
                .lock()
                .await
                .iter()
                .map(|m| m.text.clone())
                .collect::<Vec<_>>();
            if !delivered.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            delivered.len(),
            1,
            "the child's progress line must reach the bound channel: {delivered:?}"
        );
        assert!(
            delivered[0].contains("rotate the deploy key"),
            "the pushed line is the child's progress: {delivered:?}"
        );
        assert!(
            !delivered[0].contains(PEM_BODY),
            "a child's PEM key reached the channel unmasked on an unattended run: {delivered:?}"
        );
        drop(tx);
    }
}

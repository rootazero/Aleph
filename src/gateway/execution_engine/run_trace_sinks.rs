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
//! still persisted under the parent's task. Readers that replay the parent's
//! rows must treat those two kinds as non-step, exactly as
//! `AgentTraceEmitSink::is_step_event`'s consumers do on the live leg.

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

/// The user channel a run's scratchpad progress is mirrored to.
pub(crate) struct ScratchpadTarget {
    pub(crate) registry: Arc<ChannelRegistry>,
    pub(crate) channel_id: String,
    pub(crate) chat_id: String,
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

    /// Hand a run's `SubagentTool` the sinks its children write into: the
    /// child chain for their harness loops (both spawn twins read it) and the
    /// run's own chain for their `MeteringProvider`s.
    #[must_use]
    pub(crate) fn hand_to_subagents(&self, tool: SubagentTool) -> SubagentTool {
        tool.with_trace_sink(Arc::clone(&self.child_harness))
            .with_accounting_sink(Arc::clone(&self.run))
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

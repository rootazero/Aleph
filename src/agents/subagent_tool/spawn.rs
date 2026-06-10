//! Background spawn + AgentRuntime construction for `SubagentTool`.
//!
//! All three call sites (foreground sync, sync batch, background) route
//! their per-call `CancellationToken` through `cancel_for_child_with` so a
//! cancelled harness propagates to its subagents.

use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::SubagentTool;
use crate::agents::background_tracker::CompletedOutcome;
use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig};
use crate::agents::AgentDef;

impl SubagentTool {
    /// A3 — a fresh child token derived from the parent run's token (cancelled
    /// when the parent is). Falls back to a standalone token for tests / direct
    /// callers with no parent token wired.
    pub(super) fn cancel_for_child(&self) -> CancellationToken {
        self.parent_cancel
            .as_ref()
            .map(|t| t.child_token())
            .unwrap_or_default()
    }

    /// Gap B follow-up — derive a subagent cancel token that ALSO honours the
    /// harness's per-call cancel signal. Returns a token that fires when EITHER:
    ///
    ///   1. The run-level `parent_cancel` fires (auto-cancel via `child_token`).
    ///   2. The per-call `harness` cancel fires (propagated via a watcher task).
    ///
    /// In production both descend from the same run cancel root, but the
    /// per-call token is more specific: the per-tool cancel RPC will only
    /// trigger `harness`, leaving the rest of the run untouched. The watcher
    /// task self-terminates as soon as `harness` cancels; if neither side ever
    /// fires it stays parked on `harness.cancelled()` until the process exits,
    /// which is acceptable for the rare-event subagent spawn path.
    pub(super) fn cancel_for_child_with(&self, harness: &CancellationToken) -> CancellationToken {
        let token = self.cancel_for_child();
        if harness.is_cancelled() {
            token.cancel();
            return token;
        }
        let token_clone = token.clone();
        let harness_clone = harness.clone();
        tokio::spawn(async move {
            harness_clone.cancelled().await;
            token_clone.cancel();
        });
        token
    }

    pub(super) fn spawn_background(
        &self,
        agent_def: AgentDef,
        task: String,
        context_summary: Option<String>,
        model: Option<String>,
        timeout_secs: u64,
        child_chain: crate::harness::chain_context::ChainContext,
        harness_cancel: &CancellationToken,
    ) -> String {
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel_token = self.cancel_for_child_with(harness_cancel);

        self.background_tracker
            .register(request_id.clone(), cancel_token.clone(), task.clone());

        let mut runtime = self.build_runtime(child_chain, cancel_token);
        if let Some(parent_sink) = self.trace_sink.clone() {
            let wrapper: std::sync::Arc<dyn crate::harness::TraceSink> = std::sync::Arc::new(
                crate::agents::forwarding_trace_sink::ForwardingTraceSink::new(
                    parent_sink,
                    self.background_tracker.clone(),
                    request_id.clone(),
                ),
            );
            runtime = runtime.with_trace_sink(wrapper);
        }

        // X1 C2: capture on_delegation inputs before the task/registry are
        // moved into the spawned future.
        let deleg_registry = self.capture_registry.clone();
        let deleg_parent_agent_id = self.parent_agent_id.clone();
        let deleg_parent_session_id = self.parent_session_id.clone();
        let deleg_task = task.clone();

        let tracker = self.background_tracker.clone();
        let rid = request_id.clone();
        tokio::spawn(async move {
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task,
                context_summary,
                model,
                timeout_secs,
            };
            let result = AssertUnwindSafe(runtime.run(runtime_config))
                .catch_unwind()
                .await;
            let outcome = match result {
                Ok(Ok(r)) => CompletedOutcome::Ok {
                    final_text: r.final_text.unwrap_or_else(|| "(no output)".to_string()),
                    iterations: r.iterations,
                    tool_calls_made: r.tool_calls_made,
                    total_tokens: r.total_tokens,
                },
                Ok(Err(e)) => CompletedOutcome::Err(e),
                Err(_panic) => CompletedOutcome::Err("Sub-agent panicked".to_string()),
            };
            // X1 C2: notify memory extensions that a delegated child finished.
            // Fire-and-forget; dispatch has its own per-hook timeout + warn.
            if let Some(reg) = deleg_registry {
                let result_summary = match &outcome {
                    CompletedOutcome::Ok { final_text, .. } => final_text.clone(),
                    CompletedOutcome::Err(e) => format!("(error) {e}"),
                };
                let ctx = crate::memory::extensions::types::DelegationCtx {
                    agent_id: deleg_parent_agent_id,
                    namespace: crate::memory::namespace::NamespaceScope::Owner,
                    parent_session_id: deleg_parent_session_id.unwrap_or_default(),
                    child_session_id: rid.clone(),
                    task: deleg_task,
                    result_summary,
                };
                reg.dispatch_on_delegation(&ctx).await;
            }
            tracker.mark_completed(&rid, outcome);
        });

        request_id
    }

    /// Build an `AgentRuntime` with every inheritable field this tool carries
    /// applied. Single construction point for the foreground, sync-batch, and
    /// background spawn paths so new wiring lands in one place.
    pub(super) fn build_runtime(
        &self,
        child_chain: crate::harness::chain_context::ChainContext,
        cancel: CancellationToken,
    ) -> AgentRuntime {
        let mut runtime = AgentRuntime::new(
            self.provider.clone(),
            child_chain,
            cancel,
            self.session.clone(),
            self.parent_tools.clone(),
            self.sandbox.clone(),
        )
        .with_parent_agent_id(self.parent_agent_id.clone());
        if let Some(w) = self.raw_memory_writer.clone() {
            runtime = runtime.with_raw_memory_writer(w);
        }
        if let Some(reg) = self.capture_registry.clone() {
            runtime = runtime.with_capture_registry(reg);
        }
        if let Some(sid) = self.parent_session_id.clone() {
            runtime = runtime.with_parent_session_id(sid);
        }
        runtime = runtime.with_subagent_semaphore(self.subagent_semaphore.clone());
        if let Some(reg) = self.plugin_registry.clone() {
            runtime = runtime.with_plugin_registry(reg);
        }
        if let Some(sink) = self.trace_sink.clone() {
            runtime = runtime.with_trace_sink(sink);
        }
        if let Some(sc) = self.stall_config.clone() {
            runtime = runtime.with_stall_config(sc);
        }
        if let Some(cap) = self.consecutive_failure_cap {
            runtime = runtime.with_consecutive_failure_cap(cap);
        }
        if let Some(tt) = self.turn_timeout {
            runtime = runtime.with_turn_timeout(tt);
        }
        if !self.provider_overrides.is_empty() {
            runtime = runtime.with_provider_overrides(self.provider_overrides.clone());
        }
        // Stage 5a (#9) — thread the parent harness's guardrail registry so
        // the child's SpawnerBase → HarnessDeps inheritance chain (already
        // wired downstream) actually receives a registry to inherit.
        if let Some(g) = self.guardrails.clone() {
            runtime = runtime.with_guardrails(g);
        }
        runtime
    }
}

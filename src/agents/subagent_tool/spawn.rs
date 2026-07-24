//! Background spawn + `AgentRuntime` construction for `SubagentTool`.
//!
//! All three call sites (foreground sync, sync batch, background) route
//! their per-call `CancellationToken` through `cancel_for_child_with` so a
//! cancelled harness propagates to its subagents.

use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::SubagentTool;
use crate::agents::background_tracker::{CompletedOutcome, SpawnMeta};
use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig};
use crate::agents::subagent_tree_events::{emit_tree_event, now_ms};
use crate::agents::AgentDef;
use aleph_protocol::subagent_tree::{NodeLifecycle, SubagentNode, SubagentTreeEvent};

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
    /// trigger `harness`, leaving the rest of the run untouched.
    ///
    /// The bridge watcher is lifetime-bounded: it exits as soon as EITHER the
    /// `harness` token fires (propagating the cancel onto the returned token)
    /// OR the returned token is itself cancelled — by run-level propagation
    /// from `parent_cancel`, or by the caller firing it once the child run has
    /// finished (every spawn site calls `.cancel()` on the returned token after
    /// the child run completes). Without that second arm the watcher would park
    /// on `harness.cancelled()` until process exit on the normal-completion
    /// path, leaking one task per subagent spawn (amplified by batch / MoA
    /// fan-out).
    pub(super) fn cancel_for_child_with(&self, harness: &CancellationToken) -> CancellationToken {
        let token = self.cancel_for_child();
        if harness.is_cancelled() {
            token.cancel();
            return token;
        }
        let token_clone = token.clone();
        let harness_clone = harness.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = harness_clone.cancelled() => token_clone.cancel(),
                _ = token_clone.cancelled() => {}
            }
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

        // Phase 0/1 — capture tree identity before `child_chain` is moved into
        // `build_runtime`. `root_session` is the owning top-level session (the
        // `parent_session_id` invariant down the chain); `parent_id` is `None`
        // because the production tool is shared per top-level run, so every
        // background subagent attaches under the session root (see design doc).
        let depth = child_chain.depth;
        let root_session = self.parent_session_id.clone().unwrap_or_default();
        let tree_agent_id = self.parent_agent_id.clone();

        self.background_tracker.register_with_meta(
            request_id.clone(),
            cancel_token.clone(),
            task.clone(),
            SpawnMeta {
                parent_id: None,
                depth,
                root_session: root_session.clone(),
                model: model.clone(),
            },
        );

        // Phase 1 — emit Spawned so the panel tree shows the node immediately.
        emit_tree_event(
            tree_agent_id.clone(),
            root_session.clone(),
            SubagentTreeEvent::Spawned {
                node: SubagentNode {
                    node_id: request_id.clone(),
                    parent_id: None,
                    depth,
                    root_session: root_session.clone(),
                    task: task.clone(),
                    model: model.clone(),
                    lifecycle: NodeLifecycle::Running,
                    started_at_ms: now_ms(),
                    elapsed_ms: 0,
                    tool_count: 0,
                    last_tool: None,
                    last_activity: None,
                },
            },
        );

        // Held past the `build_runtime` move so the spawned run task can fire
        // it on completion, terminating the `cancel_for_child_with` bridge
        // watcher (otherwise it parks until process exit).
        let bridge_cancel = cancel_token.clone();
        let mut runtime = self.build_runtime(child_chain, cancel_token);
        if let Some(parent_sink) = self.trace_sink.clone() {
            let wrapper: std::sync::Arc<dyn crate::harness::TraceSink> = std::sync::Arc::new(
                crate::agents::forwarding_trace_sink::ForwardingTraceSink::new(
                    parent_sink,
                    self.background_tracker.clone(),
                    request_id.clone(),
                    tree_agent_id.clone(),
                    root_session.clone(),
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

        // R5 announce inputs — the gateway subscriber needs the parent's
        // agent id and session key to proactively deliver the result.
        let announce_agent_id = self.parent_agent_id.clone();
        let announce_session_id = self.parent_session_id.clone();

        let tracker = self.background_tracker.clone();
        let rid = request_id.clone();
        // Phase 1 — Settled emit captures (moved into the run task).
        let root_session_for_done = root_session;
        let tree_agent_id_for_done = tree_agent_id;
        let settle_started = std::time::Instant::now();
        tokio::spawn(async move {
            let _cancel_guard = CancelGuard::new(bridge_cancel.clone());
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
                Ok(Ok(r)) => {
                    let mut final_text =
                        r.final_text.unwrap_or_else(|| "(no output)".to_string());
                    // R8 honesty: `CompletedOutcome` (background_tracker) has no
                    // structured hit_limit slot, so a capped run annotates its
                    // text — check_status / wait / list and the proactive
                    // announce all surface this field.
                    if r.hit_limit {
                        final_text.push_str(
                            "\n\n[note] Sub-agent hit its iteration limit before finishing; \
                             treat this as a partial result.",
                        );
                    }
                    CompletedOutcome::Ok {
                        final_text,
                        iterations: r.iterations,
                        tool_calls_made: r.tool_calls_made,
                        total_tokens: r.total_tokens,
                    }
                }
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

            // R5 announce: build the completion event before `outcome` moves
            // into the tracker. Broadcast AFTER `mark_completed` so an
            // announce-triggered parent turn already observes the completed
            // state through the subagent tool's `list`/`result` actions.
            // Skipped when no parent session is wired (CLI / direct callers)
            // — there is no session to announce into. Zero subscribers is
            // safe (library tests).
            let announce = announce_session_id.map(|sid| {
                let (success, summary, error) = match &outcome {
                    CompletedOutcome::Ok { final_text, .. } => (true, final_text.clone(), None),
                    CompletedOutcome::Err(e) => (false, String::new(), Some(e.clone())),
                };
                let result = crate::event::SubAgentCompletionEvent {
                    agent_id: announce_agent_id.clone(),
                    child_session_id: rid.clone(),
                    summary,
                    success,
                    error,
                    request_id: Some(rid.clone()),
                };
                (sid, result)
            });
            // Phase 1 — emit Settled (typed lifecycle + metrics) before the
            // outcome moves into the tracker. Reuses the tracker's single-source
            // classifier so live + cold-start lifecycles never diverge.
            let tree_lifecycle =
                crate::agents::background_tracker::lifecycle_from_outcome(&outcome);
            let (settled_iters, settled_tools, settled_tokens) = match &outcome {
                CompletedOutcome::Ok {
                    iterations,
                    tool_calls_made,
                    total_tokens,
                    ..
                } => (*iterations, *tool_calls_made, *total_tokens),
                CompletedOutcome::Err(_) => (0, 0, 0),
            };
            emit_tree_event(
                tree_agent_id_for_done.clone(),
                root_session_for_done.clone(),
                SubagentTreeEvent::Settled {
                    node_id: rid.clone(),
                    root_session: root_session_for_done.clone(),
                    lifecycle: tree_lifecycle,
                    duration_ms: settle_started
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                    iterations: settled_iters,
                    tool_calls_made: settled_tools,
                    total_tokens: settled_tokens,
                },
            );
            tracker.mark_completed(&rid, outcome);
            // Terminate the per-call cancel bridge watcher now the child run is
            // done (no-op if it already fired via cancel). Keeps background
            // spawns from leaking one parked task apiece.
            bridge_cancel.cancel();
            if let Some((session_id, result)) = announce {
                crate::event::GlobalBus::global()
                    .broadcast(
                        &announce_agent_id,
                        &session_id,
                        crate::event::AlephEvent::SubAgentCompleted(result),
                    )
                    .await;
            }
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
        if let Some(rs) = self.routing_store.clone() {
            runtime = runtime.with_routing_store(rs);
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
        if let Some(mi) = self.default_max_iterations {
            runtime = runtime.with_default_max_iterations(mi);
        }
        if let Some(cap) = self.parallel_tool_concurrency {
            runtime = runtime.with_parallel_tool_concurrency(cap);
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
        if let Some(s) = self.strategy.clone() {
            runtime = runtime.with_strategy(s);
        }
        if let Some(m) = self.session_mode {
            runtime = runtime.with_session_mode(m);
        }
        runtime
    }
}

/// RAII guard that cancels its [`CancellationToken`] on `Drop` — including
/// during a panic unwind. The bridge watcher spawned by
/// [`SubagentTool::cancel_for_child_with`] parks on
/// `harness.cancelled() | token.cancelled()`, so without this guard the
/// foreground / aggregator panic path (where the explicit `.cancel()` after
/// `runtime.run(...)` is never reached) leaks the parked watcher until the
/// token happens to be cancelled by something else.
///
/// `CancellationToken::drop` does NOT auto-cancel, so the explicit
/// `.cancel()` in the destructor is the only thing that wakes the bridge
/// watcher's `token_clone.cancelled()` arm on scope exit.
#[derive(Debug)]
pub(super) struct CancelGuard(CancellationToken);

impl CancelGuard {
    #[must_use]
    pub(super) fn new(token: CancellationToken) -> Self {
        Self(token)
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Normal scope exit must cancel the held token. The bridge watcher
    /// exits via its `token_clone.cancelled()` arm as a result.
    #[tokio::test]
    async fn cancel_guard_cancels_on_scope_exit() {
        let token = CancellationToken::new();
        let probe = token.clone();
        {
            let _g = CancelGuard::new(token);
        }
        assert!(
            probe.is_cancelled(),
            "Drop must cancel the held CancellationToken"
        );
    }

    /// Panic unwind must also trigger the Drop — the panic path is the
    /// primary motivation for the guard, because the explicit
    /// `.cancel()` after `runtime.run(...)` is unreachable on panic.
    #[tokio::test]
    async fn cancel_guard_cancels_on_panic_unwind() {
        let token = CancellationToken::new();
        let probe = token.clone();
        let outcome = futures::FutureExt::catch_unwind(
            std::panic::AssertUnwindSafe(async move {
                let _g = CancelGuard::new(token);
                panic!("simulated panic in scope");
            }),
        )
        .await;
        assert!(outcome.is_err(), "inner future must have panicked");
        assert!(
            probe.is_cancelled(),
            "Drop must fire during panic unwind and cancel the token"
        );
    }

    /// The guard's Drop must wake the bridge watcher pattern used by
    /// `cancel_for_child_with`. We mirror the watcher here (a clone of
    /// the same select! shape) and confirm it exits within the bounded
    /// window — without this, the watcher would park indefinitely.
    #[tokio::test]
    async fn cancel_guard_drop_wakes_bridge_watcher() {
        let token = CancellationToken::new();
        let harness = CancellationToken::new();
        let probe = Arc::new(AtomicBool::new(false));
        let token_for_watcher = token.clone();
        let harness_for_watcher = harness.clone();
        let watcher_flag = probe.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = harness_for_watcher.cancelled() => {}
                _ = token_for_watcher.cancelled() => {}
            }
            watcher_flag.store(true, Ordering::SeqCst);
        });
        {
            let _g = CancelGuard::new(token);
        }
        let joined = tokio::time::timeout(std::time::Duration::from_millis(200), watcher)
            .await
            .expect("watcher must exit well before the timeout");
            joined.expect("watcher task must not panic");
        assert!(
            probe.load(Ordering::SeqCst),
            "bridge watcher must exit when guard drops"
        );
    }
}

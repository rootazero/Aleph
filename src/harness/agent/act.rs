//! Act phase — tool-call execution with caching, guardrails, and an
//! opencode-parity parallel fast path.
//!
//! [`AgentHarness::act`] first **partitions** the batch into contiguous,
//! order-preserving groups that are each internally resource-disjoint (see
//! [`crate::tools::concurrency::partition_parallel_groups`]) and dispatches the
//! groups sequentially through [`AgentHarness::dispatch_group`]. This lets a
//! mixed batch — say three reads followed by one `bash` — parallelize the reads
//! and serialize only across the conflict boundary, a schedule the reference
//! agents (openclaw / hermes / Pi, all whole-batch deciders) cannot produce.
//! Every non-mixed shape (single call, parallelism disabled, within-batch
//! duplicate, or a batch with no parallel group) routes through one
//! `dispatch_group` over the whole batch, byte-identical to the prior behaviour.
//!
//! The default loop is serial in input order (every existing test relies on
//! that). When all of the following hold for a single dispatch group, the
//! harness routes through [`AgentHarness::act_parallel`] instead, dispatching
//! the actual `tools.execute(...)` futures concurrently via
//! [`futures::stream::FuturesOrdered`] while keeping every side effect — event
//! emit, trace, layer-3 budget, timeline push — strictly in input order:
//!
//! * [`HarnessDeps::parallel_tool_concurrency`](crate::harness::deps::HarnessDeps)
//!   is `Some(n)` with `n >= 2`.
//! * the batch has at least two calls.
//! * the resource-scope claims of every call admit parallel dispatch —
//!   [`batch_parallelizable`](crate::tools::concurrency::batch_parallelizable)
//!   over each call's [`ToolService::call_concurrency_claim`](crate::tools::service::ToolService::call_concurrency_claim).
//! * no two calls in the batch carry the same canonical `(name, args)` —
//!   parallel mode skips within-batch dedup, so duplicates fall back to the
//!   serial path where the memo correctly emits a cached result for the
//!   second occurrence.
//!
//! A wired guardrail registry no longer forces a serial fall-back. The
//! tool-call guardrail (Block / Sanitize / Pass) is applied sequentially in
//! `act_parallel`'s PASS 0 — the same prep/execute split Pi uses (sequential
//! validate + approval, then concurrent execute) — so guardrailed deployments
//! keep the parallel fast path. Block reuses the cross-batch-dedup
//! None-future plumbing (the call is skipped and the guardrail emits its own
//! `ToolError`); Sanitize rewrites the args the execute phase runs.
//!
//! Any failing precondition falls through to the existing serial loop with no
//! observable behavior change.
//!
//! ## Cooperative steer checkpoint
//!
//! Before dispatching each serial tool call (and before each parallel
//! group), Act re-checks `AgentHarness::has_unanswered_user_message`. If a
//! non-synthetic user message arrived after this turn's prompt boundary
//! (the `last_prompt_seq` watermark) — the user changed their mind
//! mid-batch — the remaining not-yet-started tools are skipped, each gets a
//! synthetic "deferred" `ToolResult` (so the `tool_use`↔`tool_result`
//! pairing the provider requires stays intact), and Act returns. The next
//! Think surfaces the new message + deferred results; the model decides to
//! pivot or re-issue (R7). In-flight tools are never killed (use `/stop` /
//! `Interrupt` mode for that). When gateway-side `mid_turn_steering` is off
//! no message is injected mid-turn, so the predicate never fires and
//! behaviour is unchanged.

use std::collections::HashMap;
use std::time::Instant;

use futures::future::BoxFuture;
use futures::stream;
use futures::StreamExt;

/// RAII guard that calls `end_turn` on drop so per-turn budget state is
/// always released even when `act()` exits early via `?`.
struct TurnBudgetGuard<'a> {
    budget: Option<&'a crate::tools::turn_budget::TurnResultBudget>,
    turn_id: &'a crate::tools::turn_budget::TurnId,
}

impl<'a> Drop for TurnBudgetGuard<'a> {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            budget.end_turn(self.turn_id);
        }
    }
}

use super::{AgentHarness, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnPhase};
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::SessionId;
use tokio_util::sync::CancellationToken;

/// Pick the effective wall-clock budget for a tool call. Per-tool
/// metadata wins over the harness-wide `turn_timeout` fallback. Both
/// unset → no timeout (legacy behaviour).
fn resolve_effective_budget(
    per_tool: Option<std::time::Duration>,
    harness_fallback: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    per_tool.or(harness_fallback)
}

/// Synthetic `ToolError::Execution` cause emitted when cross-batch dedup
/// refuses an identical repeat of a previously-failed `(name, args)` call.
/// Shared by the serial and parallel dispatch paths so the two can never drift.
const CROSS_BATCH_REFUSED_CAUSE: &str = "this exact call already failed earlier in the run; \
     change inputs or try a different tool";

/// Synthetic `ToolError` cause persisted for a call that overran the
/// harness-wide `turn_timeout` (a run-aborting stall, not a recoverable
/// per-tool budget). Emitted BEFORE `StalledTurn` bubbles so the turn's
/// `tool_use` blocks keep their result pairing: without it the prompt
/// builder drops the whole assistant turn as orphaned on the next build,
/// erasing exactly the context the Timeout grace turn needs to salvage.
/// Shared by the serial and parallel dispatch paths.
const STALLED_CALL_CAUSE: &str = "aborted: exceeded the run-level turn timeout \
     and the run is wrapping up — no result was produced";

/// Build the recoverable `ToolError` cause for a per-tool wall-clock budget
/// overrun. Shared by the serial and parallel dispatch paths so both surface
/// byte-identical guidance — the next Think turn reacts the same way regardless
/// of how the batch was scheduled.
fn budget_overrun_cause(seconds: f64) -> String {
    format!(
        "exceeded its {seconds:.1}s wall-clock budget (slow or unresponsive \
         source) — no result; retry, narrow the query, or switch source/tool"
    )
}

impl AgentHarness {
    /// Act phase: execute each `tool_call` sequentially, emitting a
    /// `ToolCallRequested` event before every call and either a `ToolResult`
    /// or `ToolError` event after.
    ///
    /// Tool failures are persisted as `SessionEvent::ToolError` and do NOT
    /// abort the batch — all tool calls in the batch are attempted. The next
    /// Think turn will see failures via `tool_result.is_error=true` (produced
    /// by the prompt assembler) and can decide whether to retry or give up.
    ///
    /// Returns the number of tool calls that succeeded (not errored).
    pub(crate) async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
        run_cancel: &CancellationToken,
    ) -> Result<usize, HarnessError> {
        // Layer 3 turn-budget boundary. `begin_turn` is idempotent — re-entering
        // the same `TurnId` is a no-op, so this is safe even if the caller
        // somehow loops on the same turn. `end_turn` is always called via the
        // RAII `TurnBudgetGuard` so per-turn state is released on every exit
        // (including a group's `?` early return). The guard spans ALL groups so
        // per-turn spill state is never reset between them.
        let budget_turn_id = crate::tools::turn_budget::TurnId::new(turn_id);
        if let Some(budget) = self.deps.turn_budget.as_ref() {
            budget.begin_turn(budget_turn_id);
        }
        let _budget_guard = TurnBudgetGuard {
            budget: self.deps.turn_budget.as_ref().map(|v| v.as_ref()),
            turn_id: &budget_turn_id,
        };

        // Fast path: a single call, or parallelism disabled, cannot partition —
        // run the whole batch through one dispatch group (identical to the
        // legacy path, including the shared within-batch dedup memo).
        let parallel_enabled = matches!(self.deps.parallel_tool_concurrency, Some(n) if n >= 2);
        if !parallel_enabled || tool_calls.len() < 2 {
            return self
                .dispatch_group(
                    session_id,
                    turn_id,
                    tool_calls,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await;
        }

        // Resource-scope-aware partition. Collect each call's concurrency claim
        // once, then split the batch into contiguous, order-preserving groups
        // (see `crate::tools::concurrency::partition_parallel_groups`). Unlike
        // the references (openclaw / hermes / Pi all make one whole-batch
        // parallel-or-serial decision), a batch of N reads followed by one
        // `bash` now parallelizes the reads and only serializes across the
        // conflict boundary. A within-batch duplicate collapses to a single
        // whole-batch group so the serial dedup memo retains ownership of that
        // semantics (mirrors `can_parallel_dispatch`, which rejects duplicates).
        let mut seen = std::collections::HashSet::new();
        let mut has_duplicate = false;
        let mut claims = Vec::with_capacity(tool_calls.len());
        for call in &tool_calls {
            let key = (
                call.name.clone(),
                super::canonical_json_string(&call.arguments),
            );
            if !seen.insert(key) {
                has_duplicate = true;
            }
            claims.push(
                self.deps
                    .tools
                    .call_concurrency_claim(&call.name, &call.arguments)
                    .await,
            );
        }
        let groups = if has_duplicate {
            vec![(0usize, tool_calls.len())]
        } else {
            crate::tools::concurrency::partition_parallel_groups(&claims)
        };

        // Only take the multi-group path when at least one group actually
        // parallelizes (>= 2 calls). Otherwise every group is a singleton and a
        // whole-batch serial dispatch is identical work with one memo and one
        // offered-tool snapshot — so route there instead (zero behaviour change).
        if !groups.iter().any(|&(s, e)| e - s >= 2) {
            return self
                .dispatch_group(
                    session_id,
                    turn_id,
                    tool_calls,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await;
        }

        // Multi-group: dispatch each contiguous slice in input order. Groups run
        // sequentially (a later group may observe an earlier group's side
        // effects); `dispatch_group` parallelizes within any group of >= 2 calls.
        let mut executed_count: usize = 0;
        let mut remaining = tool_calls.into_iter();
        for (start, end) in groups {
            // Cooperative steer checkpoint at the group boundary. Groups run
            // sequentially; a parallel group's `.buffered()` wave is not
            // interruptible mid-flight by design, so we stop *before*
            // launching the next group when a mid-turn user message arrived,
            // and defer everything still pending. R7/R10: mechanical.
            if self.has_unanswered_user_message(session_id).await {
                let deferred: Vec<NativeToolCall> = remaining.by_ref().collect();
                if !deferred.is_empty() {
                    self.emit_deferred_tool_results(session_id, turn_id, &deferred)
                        .await?;
                }
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                break;
            }
            let group: Vec<NativeToolCall> = remaining.by_ref().take(end - start).collect();
            match self
                .dispatch_group(
                    session_id,
                    turn_id,
                    group,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await
            {
                Ok(n) => executed_count = executed_count.saturating_add(n),
                Err(e) => {
                    // A stalled group aborts the run; close out the not-yet-
                    // started later groups so their tool_use blocks keep their
                    // result pairing (see the serial-path stall closure).
                    let pending_ids: Vec<String> = remaining.by_ref().map(|c| c.id).collect();
                    if !pending_ids.is_empty() {
                        self.close_unexecuted_tool_uses(
                            session_id,
                            turn_id,
                            &pending_ids,
                            "run stalled during an earlier tool group in this batch",
                        )
                        .await;
                    }
                    return Err(e);
                }
            }
        }
        Ok(executed_count)
    }

    /// Emit a synthetic "deferred" `ToolResult` for each tool call the
    /// cooperative steer checkpoint skipped. Every `tool_use` block in the
    /// turn's `AssistantMessage` must have a matching `tool_result` or the
    /// provider rejects the next request, so a skipped call still gets a
    /// result — a marker the model can re-issue from on its next turn.
    ///
    /// R10-safe: pure mechanical bookkeeping. Whether a deferred call is
    /// re-run is the model's decision next Think, not the harness's.
    pub(crate) async fn emit_deferred_tool_results(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        calls: &[NativeToolCall],
    ) -> Result<(), HarnessError> {
        for call in calls {
            let output = ToolOutput {
                value: serde_json::json!({
                    "deferred": true,
                    "reason": "superseded by a new user message that arrived mid-turn; \
                               re-issue this call if it is still needed",
                }),
                metadata: crate::session::events::ToolOutputMetadata::default(),
            };
            let event = SessionEvent::ToolResult {
                turn_id,
                call_id: call.id.clone(),
                output,
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, event).await?;
        }
        Ok(())
    }

    /// Dispatch a single group of tool calls: route through the opencode-parity
    /// parallel fast path when [`Self::can_parallel_dispatch`] admits the group,
    /// else fall to the serial loop. This is the former body of `act` minus the
    /// per-turn budget boundary, which `act` now owns so it spans all groups.
    ///
    /// Tool failures are persisted as `SessionEvent::ToolError` and do NOT
    /// abort the group — all calls are attempted. Returns the number of tool
    /// calls that succeeded (not errored).
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_group(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
        budget_turn_id: &crate::tools::turn_budget::TurnId,
        run_cancel: &CancellationToken,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        // Within-batch idempotency memo. Scoped to this single dispatch group:
        // duplicate calls inside one group are deduplicated, but a legitimate
        // cross-turn repeat (e.g. `read_file` after `write_file`, or any
        // time-varying tool such as `get_current_time`) always re-executes
        // against fresh state instead of replaying a stale result.
        let mut tool_call_cache: HashMap<(String, String), ToolOutput> = HashMap::new();

        // opencode-parity parallel fast path. Falls through to the serial loop
        // below when any precondition fails (see module docs).
        if self.can_parallel_dispatch(&tool_calls).await {
            return self
                .act_parallel(
                    session_id,
                    turn_id,
                    tool_calls,
                    callback,
                    iteration,
                    budget_turn_id,
                    run_cancel,
                )
                .await;
        }

        // Offered-tool snapshot for tool-name repair, taken once per batch (the
        // dispatchable set is stable across a single `act()` call). Used by the
        // unified resolver below; the parallel fast path naturally routes any
        // unrepaired/typo'd name back here (an unknown name yields a
        // conservative `Global` concurrency claim → non-parallelizable → serial).
        let offered_defs = self.deps.tools.dispatchable_list().await;
        let offered_names: Vec<&str> = offered_defs.iter().map(|d| d.name.as_str()).collect();

        let mut tool_iter = tool_calls.into_iter();
        while let Some(mut call) = tool_iter.next() {
            // Cooperative steer checkpoint. If a non-synthetic user message
            // arrived after this turn's prompt boundary (the user changed
            // their mind mid-batch), stop launching further tools and defer
            // the current call + everything still pending so the model sees
            // the message next Think and decides to pivot or resume.
            // R7/R10: mechanical — no intent judgement here.
            if self.has_unanswered_user_message(session_id).await {
                let mut deferred = Vec::with_capacity(1);
                deferred.push(call);
                deferred.extend(tool_iter.by_ref());
                self.emit_deferred_tool_results(session_id, turn_id, &deferred)
                    .await?;
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                break;
            }
            // G3 (opencode-inspired): mechanical tool-name auto-repair via the
            // unified resolver (`tools::name_repair`). Models emit names that
            // miss the offered set by case (`Read`→`read`), separator
            // (`web.search`↔`web_search`), or a single-edit typo (`web_serch`→
            // `web_search`); without repair the call bounces through ToolError
            // before the model self-corrects. The resolver is conservative —
            // exact match is a no-op, and every loose tier abstains on
            // ambiguity — so it never mis-routes between two similar tools.
            // R10-safe: mechanical identifier repair against a fixed offered
            // set, not intent inference; downstream guardrails/approval still
            // gate the resolved call.
            if let Some(repair) =
                crate::tools::name_repair::repair_tool_name(&call.name, &offered_names)
            {
                if repair.tier != crate::tools::name_repair::RepairTier::Exact
                    && repair.name != call.name
                {
                    tracing::debug!(
                        original = %call.name,
                        repaired = %repair.name,
                        tier = ?repair.tier,
                        "tool name auto-repaired",
                    );
                    call.name = repair.name;
                }
            }
            callback.on_tool_call(&call.name);
            // Structured tool-start event (id + name + args). The legacy
            // name-only `on_tool_call` above is kept for backward compat; this
            // is the preferred signal (see `HarnessCallback::on_tool_call_start`)
            // that lets the stream sink emit a real call id instead of "legacy".
            callback.on_tool_call_start(&call.id, &call.name, &call.arguments);
            let started = Instant::now();
            self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallStarted {
                iteration,
                call: crate::harness::trace::ToolCallStartEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                },
            });
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            // Stage 5b (#9): Tool-call guardrail (Block skips THIS call).
            if let Some(registry) = self.deps.guardrails.as_ref() {
                match self
                    .apply_tool_call_guardrail(
                        registry, session_id, turn_id, &call, started, iteration, callback,
                    )
                    .await?
                {
                    ToolCallGuardOutcome::Pass => {}
                    ToolCallGuardOutcome::Sanitize(args) => call.arguments = args,
                    ToolCallGuardOutcome::Block => {
                        if let Some(ref tracker) = self.stall_tracker {
                            tracker.record_activity().await;
                        }
                        continue;
                    }
                }
            }

            // Within-batch dedup: identical (tool_name, canonical_args) pairs
            // inside this single tool batch return the first result without
            // re-executing. The memo is per-`act()` call, so a cross-turn
            // repeat always re-runs against fresh state.
            let cache_key = (
                call.name.clone(),
                super::canonical_json_string(&call.arguments),
            );

            // Cross-batch dedup: refuse identical (tool, args) that already
            // failed earlier in this run. The synthesized ToolError nudges
            // the LLM to pivot instead of looping on the same deterministic
            // failure (sandbox-blocked URL, quota-exhausted API, etc.). The
            // failure set is cleared whenever any tool succeeds.
            if self.is_recent_failure(&call.name, &cache_key.1) {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "cross-batch dedup: refusing identical repeat of a previously-failed call",
                );
                let synthetic = crate::tools::service::ToolError::Execution {
                    name: call.name.clone(),
                    cause: CROSS_BATCH_REFUSED_CAUSE.to_string(),
                };
                self.emit_tool_error(
                    session_id, turn_id, &call, synthetic, started, iteration, callback,
                )
                .await;
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                continue;
            }

            if let Some(cached) = tool_call_cache.get(&cache_key) {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "duplicate tool call deduplicated within batch (no re-execution)",
                );
                executed_count = executed_count.saturating_add(1);
                let output = cached.clone();
                let output_value = output.value.clone();
                let result_event = SessionEvent::ToolResult {
                    turn_id,
                    call_id: call.id.clone(),
                    output,
                    at: now_ms(),
                };
                self.deps
                    .session
                    .emit_event(session_id, result_event)
                    .await?;
                // Live "done" event for the memo-hit (no re-execution but the
                // model observed a result). Keeps the broadcast stream's
                // ToolStart↔ToolEnd symmetry intact for deduplicated calls.
                // 0 ms — nothing was executed (mirrors the trace event below).
                callback.on_tool_call_done(&call.id, Some(&output_value), None, 0);
                self.emit(
                    || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: 0,
                        },
                        result: crate::tools::runtime::ToolResult::Success {
                            output: output_value,
                        },
                    },
                );
                // Memo-hit invocations cost 0 ms; record them anyway so the
                // tool count on the timeline matches what the agent actually
                // observed (the model saw a result come back).
                self.push_tool_invocation(call.id.clone(), call.name.clone(), 0, true, None);
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                continue;
            }

            // Fork a per-call cancel token from the run's parent so a
            // selective tool-level abort (or run-wide cancel) drops THIS
            // call's future without affecting earlier completed events.
            let call_cancel = run_cancel.child_token();
            // Gap B follow-up — surface this call's per-call token to the
            // gateway via the in-flight registry so `tools.cancel_call` can
            // fire it by `tool_call_id`. The guard removes the entry on drop
            // (normal completion, `?` early-return, or panic alike).
            let _in_flight_guard = self
                .deps
                .in_flight_tool_calls
                .as_ref()
                .map(|reg| reg.register(&call.id, &call.name, call_cancel.clone()));
            let exec_fut = self.deps.tools.execute_with_cancel(
                &call.name,
                call.arguments.clone(),
                call_cancel,
            );

            // Resolve effective wall-clock budget: per-tool metadata > global fallback.
            let per_tool_budget = self
                .deps
                .tools
                .describe(&call.name)
                .await
                .and_then(|d| d.metadata.max_duration_ms)
                .map(std::time::Duration::from_millis);
            let effective_budget =
                resolve_effective_budget(per_tool_budget, self.deps.turn_timeout);

            let exec_result: Result<
                Result<ToolOutput, crate::tools::service::ToolError>,
                HarnessError,
            > = match effective_budget {
                Some(budget) => {
                    let started_call = Instant::now();
                    match tokio::time::timeout(budget, exec_fut).await {
                        Ok(inner) => Ok(inner),
                        // A *per-tool* wall-clock budget overrun is RECOVERABLE:
                        // surface it as a tool error the next Think turn can react
                        // to (retry, narrow the query, switch source/tool) instead
                        // of aborting the whole run on one slow `search`/`web_fetch`.
                        // Only the harness-wide `turn_timeout` fallback
                        // (`per_tool_budget == None`) is a genuine run-level stall
                        // that must `StalledTurn`.
                        Err(_) if per_tool_budget.is_some() => {
                            Ok(Err(crate::tools::service::ToolError::Execution {
                                name: call.name.clone(),
                                cause: budget_overrun_cause(budget.as_secs_f64()),
                            }))
                        }
                        Err(_) => Err(HarnessError::StalledTurn {
                            phase: TurnPhase::Act {
                                tool_name: call.name.clone(),
                            },
                            elapsed: started_call.elapsed(),
                        }),
                    }
                }
                None => Ok(exec_fut.await),
            };
            let inner = match exec_result {
                Ok(r) => r,
                Err(stalled) => {
                    // Close the tool_use↔result pairing before bubbling the
                    // run-aborting stall. The timed-out call gets a synthetic
                    // ToolError (its ToolStart already fired, so this also
                    // closes the live stream's start/end symmetry); calls never
                    // started get a bare session-log closure. Without these the
                    // prompt builder drops the whole assistant turn as orphaned
                    // on the next build, erasing the context the Timeout grace
                    // turn needs to salvage a partial deliverable.
                    let synthetic = crate::tools::service::ToolError::Execution {
                        name: call.name.clone(),
                        cause: STALLED_CALL_CAUSE.to_string(),
                    };
                    self.emit_tool_error(
                        session_id, turn_id, &call, synthetic, started, iteration, callback,
                    )
                    .await;
                    let pending_ids: Vec<String> = tool_iter.by_ref().map(|c| c.id).collect();
                    if !pending_ids.is_empty() {
                        self.close_unexecuted_tool_uses(
                            session_id,
                            turn_id,
                            &pending_ids,
                            "run stalled during an earlier tool call in this batch",
                        )
                        .await;
                    }
                    return Err(stalled);
                }
            };
            match inner {
                Ok(mut output) => {
                    executed_count = executed_count.saturating_add(1);
                    self.apply_turn_budget(budget_turn_id, &call, &mut output);
                    tool_call_cache.insert(cache_key.clone(), output.clone());
                    // Cross-batch dedup: a single success clears the failure
                    // set — the LLM has demonstrably pivoted to a working
                    // strategy.
                    self.clear_failures();
                    self.emit_tool_success(
                        session_id, turn_id, &call, output, started, iteration, callback,
                    )
                    .await?;
                }
                Err(e) => {
                    // Do NOT abort — continue processing remaining tool calls.
                    // The error is persisted to session log; the next Think
                    // turn will see it as tool_result(is_error=true).
                    self.record_failure(call.name.clone(), cache_key.1.clone());
                    self.emit_tool_error(
                        session_id, turn_id, &call, e, started, iteration, callback,
                    )
                    .await;
                }
            }

            // Record activity after each tool execution completes so the stall
            // tracker is reset for each progress event.
            if let Some(ref tracker) = self.stall_tracker {
                tracker.record_activity().await;
            }
        }

        Ok(executed_count)
    }
}

// =============================================================================
// Parallel fast path
// =============================================================================

impl AgentHarness {
    /// Whether the current batch is eligible for the parallel fast path.
    ///
    /// Preconditions are checked in O(n) for batch size n; the
    /// `call_concurrency_claim` query is one trait await per call, followed by
    /// an O(n²) pairwise conflict scan over the (tiny) batch. Returns `false`
    /// cheaply when concurrency is disabled or when there are fewer than two
    /// calls. Admission is resource-scope-aware (see
    /// [`crate::tools::concurrency`]): disjoint-path mutations parallelize,
    /// same-path / whole-world mutations fall back to the serial loop.
    ///
    /// Guardrails no longer force a serial fall-back: the tool-call guardrail
    /// (Block / Sanitize / Pass) is applied sequentially in `act_parallel`'s
    /// PASS 0 — the same prep/execute split Pi uses (sequential validate +
    /// approval, then concurrent execute) — so guardrailed deployments keep
    /// the parallel fast path instead of paying a full-serial penalty.
    async fn can_parallel_dispatch(&self, tool_calls: &[NativeToolCall]) -> bool {
        let Some(par_n) = self.deps.parallel_tool_concurrency else {
            return false;
        };
        if par_n < 2 || tool_calls.len() < 2 {
            return false;
        }
        // Reject batches with within-batch duplicates so the serial dedup
        // memo continues to own that semantics. (Duplicates are rare; the
        // common LLM pattern is N distinct read-only calls.)
        let mut seen = std::collections::HashSet::new();
        for call in tool_calls {
            let key = (
                call.name.clone(),
                super::canonical_json_string(&call.arguments),
            );
            if !seen.insert(key) {
                return false;
            }
        }
        // Resource-scope-aware admission: collect each call's concurrency
        // claim (Shared / Exclusive{Global|Paths}) and admit the batch only
        // when no pair conflicts. This generalizes the old "every call must be
        // concurrent-safe" check — a batch of disjoint-path file mutations now
        // parallelizes, while same-path or whole-world mutations still fall
        // back to the serial loop. See `crate::tools::concurrency`.
        let mut claims = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            claims.push(
                self.deps
                    .tools
                    .call_concurrency_claim(&call.name, &call.arguments)
                    .await,
            );
        }
        crate::tools::concurrency::batch_parallelizable(&claims)
    }

    /// Parallel fast path. Pre-emits `ToolCallRequested` events in input
    /// order, dispatches all executes concurrently via `FuturesOrdered`, and
    /// then walks the completion results in input order to emit
    /// `ToolResult` / `ToolError`, record the Layer 3 turn budget, and push
    /// the timeline entry. Side-effect ordering matches the serial loop.
    ///
    /// Per-tool wall-clock budget is wrapped around each individual future
    /// (same as the serial path). A timeout in this path is bubbled up as
    /// `HarnessError::StalledTurn` AFTER all already-completed results have
    /// been emitted in input order — strictly more information than the
    /// serial path (which exits without emitting later calls' results).
    #[allow(clippy::too_many_arguments)]
    async fn act_parallel(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
        budget_turn_id: &crate::tools::turn_budget::TurnId,
        run_cancel: &CancellationToken,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        // Cross-batch dedup preflight: index calls whose (tool, args) already
        // failed earlier in this run. Skipped calls receive a synthetic
        // ToolError below alongside the normal request-event emission so the
        // session timeline stays linear. PASS 1 then dispatches ONLY the
        // surviving calls in parallel, preserving original input order.
        //
        // The `is_recent_failure` probe is deferred into PASS 0 (filled below)
        // so it runs AFTER the tool-call guardrail may have rewritten the args.
        // The serial path checks `is_recent_failure` against the *sanitised*
        // `cache_key`, and `record_failure` likewise records the sanitised
        // form, so the parallel path must dedup on the same post-sanitise
        // signature. Probing the pre-guardrail args here would diverge from the
        // serial path: a guardrail-rewritten call whose original matched a past
        // failure would be wrongly skipped, and one whose sanitised form
        // matches a past failure would be wrongly run.
        let mut canonical_args: Vec<String> = tool_calls
            .iter()
            .map(|c| super::canonical_json_string(&c.arguments))
            .collect();
        let mut skip: Vec<bool> = vec![false; tool_calls.len()];

        // Per-index guardrail outcome, populated in PASS 0 (mirrors the serial
        // path's Stage 5b). `blocked[idx]` => the tool-call guardrail returned
        // Block (which already emitted a `ToolError` + trace internally), so
        // PASS 1 builds no future and PASS 2 skips it — reusing the same
        // None-future plumbing as cross-batch dedup. `sanitized[idx]` carries
        // rewritten arguments the guardrail wants executed in place of the
        // model's original (the original args still appear in the already-
        // emitted `ToolCallRequested` event, matching serial semantics where
        // the request is logged before sanitisation mutates the call).
        let mut blocked: Vec<bool> = vec![false; tool_calls.len()];
        let mut sanitized: Vec<Option<serde_json::Value>> = vec![None; tool_calls.len()];

        // PASS 0 — serial: notify callback, emit ToolCallStarted trace,
        // emit ToolCallRequested SessionEvent. Resolve effective per-tool
        // wall-clock budget. Capture started Instant for duration metrics.
        // Skipped calls take the synthetic-error fast path here and are
        // omitted from PASS 1 dispatch.
        let mut started_at: Vec<Instant> = Vec::with_capacity(tool_calls.len());
        // Per-call effective budget plus whether it came from the *per-tool*
        // table (`true`) or the harness-wide `turn_timeout` fallback (`false`).
        // The flag decides, on timeout, between a recoverable tool error and a
        // run-aborting `StalledTurn` (see PASS 1 / PASS 2 below).
        let mut budgets: Vec<Option<(std::time::Duration, bool)>> =
            Vec::with_capacity(tool_calls.len());
        for (idx, call) in tool_calls.iter().enumerate() {
            callback.on_tool_call(&call.name);
            // Structured tool-start event (parallel path parity with serial).
            callback.on_tool_call_start(&call.id, &call.name, &call.arguments);
            started_at.push(Instant::now());
            self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallStarted {
                iteration,
                call: crate::harness::trace::ToolCallStartEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                },
            });
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            // Stage 5b (#9): tool-call guardrail — applied here in the serial
            // PASS 0 (Pi-style prep phase) so the parallel execute pass below
            // stays guardrail-safe. Order matches the serial path: after the
            // request is logged, before cross-batch dedup.
            if let Some(registry) = self.deps.guardrails.as_ref() {
                match self
                    .apply_tool_call_guardrail(
                        registry,
                        session_id,
                        turn_id,
                        call,
                        started_at[idx],
                        iteration,
                        callback,
                    )
                    .await?
                {
                    ToolCallGuardOutcome::Pass => {}
                    ToolCallGuardOutcome::Sanitize(args) => {
                        // Execute the rewritten args; keep the canonical
                        // signature in sync so a later failure records the
                        // sanitised form (serial parity).
                        canonical_args[idx] = super::canonical_json_string(&args);
                        sanitized[idx] = Some(args);
                    }
                    ToolCallGuardOutcome::Block => {
                        // The guardrail already emitted ToolError + trace.
                        // Treat exactly like a dedup-skipped call: no future,
                        // no PASS 2 emission.
                        blocked[idx] = true;
                        if let Some(ref tracker) = self.stall_tracker {
                            tracker.record_activity().await;
                        }
                        budgets.push(None);
                        continue;
                    }
                }
            }

            // Cross-batch dedup probe — evaluated here (post-guardrail) so it
            // sees the same canonical signature the serial path and
            // `record_failure` use: `canonical_args[idx]` reflects the
            // guardrail-sanitised args (updated above) when the call was
            // rewritten, the model's original otherwise. (A `Block` outcome
            // `continue`s above, so blocked indices keep `skip == false` and
            // are handled by the `blocked[idx]` plumbing instead.)
            skip[idx] = self.is_recent_failure(&call.name, &canonical_args[idx]);

            if skip[idx] {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "cross-batch dedup (parallel): refusing identical repeat of a previously-failed call",
                );
                let synthetic = crate::tools::service::ToolError::Execution {
                    name: call.name.clone(),
                    cause: CROSS_BATCH_REFUSED_CAUSE.to_string(),
                };
                self.emit_tool_error(
                    session_id,
                    turn_id,
                    call,
                    synthetic,
                    started_at[idx],
                    iteration,
                    callback,
                )
                .await;
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                budgets.push(None);
                continue;
            }

            let per_tool_budget = self
                .deps
                .tools
                .describe(&call.name)
                .await
                .and_then(|d| d.metadata.max_duration_ms)
                .map(std::time::Duration::from_millis);
            let per_tool = per_tool_budget.is_some();
            budgets.push(
                resolve_effective_budget(per_tool_budget, self.deps.turn_timeout)
                    .map(|d| (d, per_tool)),
            );
        }

        // PASS 1 — parallel: dispatch up to `parallelism` execute futures
        // concurrently via `stream::iter(...).buffered(n)`. `buffered` polls
        // at most `n` futures at a time AND yields completions in input
        // order — semantically identical to opencode's
        // `Effect.forEach({ concurrency: n })`. Per-call timeout is wrapped
        // INSIDE each future so the timeout is owned by the call, not the
        // batch.
        let parallelism = self.deps.parallel_tool_concurrency.unwrap_or(0).max(2);
        type ExecOutcome =
            Result<Result<ToolOutput, crate::tools::service::ToolError>, std::time::Duration>;
        // Build per-original-index futures, leaving None at skipped indices
        // (cross-batch dedup already emitted synthetic ToolError in PASS 0).
        // Run only the live futures through `.buffered()` and re-assemble
        // results in original input order via `live_indices`, so PASS 2 keeps
        // its existing `for (idx, exec_result) in results.iter().enumerate()`
        // contract against `tool_calls`.
        let mut boxed_futs_opt: Vec<Option<BoxFuture<'static, ExecOutcome>>> =
            Vec::with_capacity(tool_calls.len());
        // Gap B follow-up — keep one InFlightGuard per call alive for the
        // duration of the whole parallel dispatch. Each guard drops when this
        // Vec goes out of scope after PASS 2 finishes, which is strictly
        // later than every future resolves.
        let mut in_flight_guards: Vec<crate::tools::in_flight::InFlightGuard> = Vec::new();
        for (idx, call) in tool_calls.iter().enumerate() {
            if skip[idx] || blocked[idx] {
                boxed_futs_opt.push(None);
                continue;
            }
            let tools = self.deps.tools.clone();
            let name = call.name.clone();
            // Guardrail-sanitised args win over the model's original (the
            // `ToolCallRequested` event already logged the original).
            let args = sanitized[idx]
                .clone()
                .unwrap_or_else(|| call.arguments.clone());
            let budget = budgets[idx];
            let started = started_at[idx];
            // Each parallel call owns a fresh child token forked from the
            // run-level cancel. If the run is cancelled mid-batch, every
            // in-flight call short-circuits without waiting for the entire
            // batch to drain.
            let call_cancel = run_cancel.child_token();
            if let Some(reg) = self.deps.in_flight_tool_calls.as_ref() {
                in_flight_guards.push(reg.register(&call.id, &call.name, call_cancel.clone()));
            }
            boxed_futs_opt.push(Some(Box::pin(async move {
                let exec_fut = tools.execute_with_cancel(&name, args, call_cancel);
                match budget {
                    Some((b, per_tool)) => match tokio::time::timeout(b, exec_fut).await {
                        Ok(inner) => Ok(inner),
                        // Per-tool budget overrun → recoverable tool error (PASS 2
                        // emits it like any failure and continues); only a
                        // harness-wide `turn_timeout` overrun (`!per_tool`) bubbles
                        // up as a run-aborting stall. Mirrors the serial path.
                        Err(_) if per_tool => {
                            Ok(Err(crate::tools::service::ToolError::Execution {
                                name: name.clone(),
                                cause: budget_overrun_cause(b.as_secs_f64()),
                            }))
                        }
                        Err(_) => Err(started.elapsed()),
                    },
                    None => Ok(exec_fut.await),
                }
            })));
        }
        let live_indices: Vec<usize> = boxed_futs_opt
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.as_ref().map(|_| i))
            .collect();
        let live_futs: Vec<BoxFuture<'static, ExecOutcome>> =
            boxed_futs_opt.into_iter().flatten().collect();
        let live_results: Vec<ExecOutcome> = stream::iter(live_futs)
            .buffered(parallelism)
            .collect()
            .await;
        // Reassemble per-original-index results. `None` slots are skipped
        // calls — already emitted as synthetic errors in PASS 0; PASS 2 below
        // ignores them via the same `skip[idx]` flag.
        let mut results: Vec<Option<ExecOutcome>> = (0..tool_calls.len()).map(|_| None).collect();
        for (live_idx, exec) in live_results.into_iter().enumerate() {
            results[live_indices[live_idx]] = Some(exec);
        }
        // PASS 1 complete — every future has resolved, so the in-flight
        // registry entries are no longer cancellable in any meaningful sense.
        // Drop the guards now (vs end-of-function) so `tools.cancel_call`
        // doesn't quietly target a token attached to a future that's already
        // returned. PASS 2 below only emits events; it never touches the
        // tokens or the registry.
        drop(in_flight_guards);

        // PASS 2 — serial in input order: apply Layer 3 budget, emit
        // ToolResult/ToolError, trace, push timeline entry. The first
        // timeout encountered is remembered and bubbled up at the end so
        // later already-completed results still reach the session log.
        // Skipped indices (cross-batch dedup hits, already errored in PASS 0)
        // are passed through with no further action.
        let mut first_stall: Option<(String, std::time::Duration)> = None;
        for (idx, exec_slot) in results.into_iter().enumerate() {
            let Some(exec_result) = exec_slot else {
                continue; // PASS-0 dedup-rejected; already emitted synthetic error.
            };
            let call = &tool_calls[idx];
            let started = started_at[idx];
            match exec_result {
                Err(elapsed) => {
                    if first_stall.is_none() {
                        first_stall = Some((call.name.clone(), elapsed));
                    }
                    // Only a harness-wide `turn_timeout` overrun reaches this arm
                    // (per-tool budget overruns are recovered as `Ok(Err)` above)
                    // — a genuine run-level stall, bubbled up as `StalledTurn`
                    // below. Persist a synthetic ToolError first so the stalled
                    // call's tool_use keeps its result pairing (and its live
                    // ToolStart gets a matching ToolEnd); otherwise the prompt
                    // builder drops the whole assistant turn as orphaned on the
                    // next build, erasing the context the Timeout grace turn
                    // needs to salvage a partial deliverable.
                    let synthetic = crate::tools::service::ToolError::Execution {
                        name: call.name.clone(),
                        cause: STALLED_CALL_CAUSE.to_string(),
                    };
                    self.emit_tool_error(
                        session_id, turn_id, call, synthetic, started, iteration, callback,
                    )
                    .await;
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
                Ok(Ok(mut output)) => {
                    executed_count = executed_count.saturating_add(1);
                    self.apply_turn_budget(budget_turn_id, call, &mut output);
                    // Cross-batch dedup: any success clears the failure set —
                    // the LLM has demonstrably pivoted to a working strategy.
                    self.clear_failures();
                    self.emit_tool_success(
                        session_id, turn_id, call, output, started, iteration, callback,
                    )
                    .await?;
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
                Ok(Err(e)) => {
                    // Cross-batch dedup: record the (tool, args) signature so
                    // the next turn refuses an identical repeat.
                    self.record_failure(call.name.clone(), canonical_args[idx].clone());
                    self.emit_tool_error(
                        session_id, turn_id, call, e, started, iteration, callback,
                    )
                    .await;
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
            }
        }

        if let Some((tool_name, elapsed)) = first_stall {
            return Err(HarnessError::StalledTurn {
                phase: TurnPhase::Act { tool_name },
                elapsed,
            });
        }

        Ok(executed_count)
    }
}

// =============================================================================
// Shared helpers — used by both the serial path (`act`) and the parallel
// fast path (`act_parallel`). Extracted to eliminate ~240 LOC of dual-
// maintenance risk: fixes to budget-spill semantics or event emission only
// have to land in one place.
// =============================================================================

impl AgentHarness {
    /// Apply Layer-3 turn budget: record this result, persist any spills via
    /// the shared result store, and rewrite `output.value` to a marker string
    /// when THIS call's result was the one spilled (so the LLM's next Think
    /// sees the marker, not the full text). No-op when budget is unset.
    fn apply_turn_budget(
        &self,
        budget_turn_id: &crate::tools::turn_budget::TurnId,
        call: &NativeToolCall,
        output: &mut ToolOutput,
    ) {
        let Some(budget) = self.deps.turn_budget.as_ref() else {
            return;
        };
        let text = match &output.value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        // Layer-2 (`apply_result_budget`) may prepend an inline error digest
        // above the `[Full output persisted: …]` marker, so it is not always at
        // byte 0 — scan every line (mirrors `result_store::extract_persisted_ref`).
        // A byte-0-only test would mis-flag such results as un-persisted and let
        // the turn budget waste its spill slot re-offloading a marker.
        let already_persisted = text
            .lines()
            .any(|l| l.starts_with("[Full output persisted: "));
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(&text);
        let record = crate::tools::turn_budget::TurnResult {
            call_id: call.id.clone(),
            tool_name: call.name.clone(),
            tokens_in_context: tokens,
            in_context_text: text,
            already_persisted,
        };
        let spills = budget.record(budget_turn_id, record);
        if spills.is_empty() {
            return;
        }
        let Some(store) = self.deps.result_store.as_ref() else {
            return;
        };
        for spill in spills {
            if spill.call_id != call.id {
                // Earlier-iteration spill: its SessionEvent::ToolResult is
                // already persisted, so post-hoc rewrite from here isn't
                // possible. The marker file is still written for recovery;
                // cheap_passes surfaces it on the next preflight.
                let _ = store.persist_if_large(
                    &spill.call_id,
                    &spill.tool_name,
                    &spill.original_text,
                    0,
                );
                continue;
            }
            // Same-turn newest spill: rewrite output BEFORE the SessionEvent
            // is emitted so the LLM sees the marker instead of the full text.
            if let Some(marker) =
                store.persist_if_large(&spill.call_id, &spill.tool_name, &spill.original_text, 0)
            {
                output.value = serde_json::Value::String(marker);
                output.metadata.truncated = true;
            }
        }
    }

    /// Persist a successful tool call: emit `SessionEvent::ToolResult`,
    /// trace event, and timeline entry.
    async fn emit_tool_success(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        output: ToolOutput,
        started: Instant,
        iteration: usize,
        callback: &mut dyn HarnessCallback,
    ) -> Result<(), HarnessError> {
        let output_value = output.value.clone();
        let result_event = SessionEvent::ToolResult {
            turn_id,
            call_id: call.id.clone(),
            output,
            at: now_ms(),
        };
        self.deps
            .session
            .emit_event(session_id, result_event)
            .await?;
        let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        // Live "done" event — mirror of `on_tool_call_start`. Fired for every
        // tool that produces a `ToolCallCompleted` persistence trace so the
        // broadcast stream emits a `ToolCallDone` → `StreamEvent::ToolEnd`.
        // Without it the live stream shows tool starts with no ends.
        callback.on_tool_call_done(&call.id, Some(&output_value), None, dur_ms);
        self.emit(
            || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                iteration,
                call: crate::harness::trace::ToolCallEndEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                    duration_ms: dur_ms,
                },
                result: crate::tools::runtime::ToolResult::Success {
                    output: output_value,
                },
            },
        );
        self.push_tool_invocation(call.id.clone(), call.name.clone(), dur_ms, true, None);
        Ok(())
    }

    /// Persist a failed tool call: emit `SessionEvent::ToolError` (best
    /// effort — failure to persist logs at WARN, never aborts the batch),
    /// trace event, timeline entry.
    async fn emit_tool_error(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        e: crate::tools::service::ToolError,
        started: Instant,
        iteration: usize,
        callback: &mut dyn HarnessCallback,
    ) {
        let retryable = e.is_retryable();
        // Compose the LLM-facing error body: the upstream `to_string()`
        // followed by a single-line persistence hint that names the
        // error kind, recommends concrete alternative tools, and
        // reminds the model the ladder must be climbed before giving
        // up (the persistence doctrine in
        // `thinker::layers::provider_guidance`). The hint
        // travels through `SessionEvent::ToolError.error` and is
        // surfaced to the model as `tool_result(is_error=true)` in the
        // very next Think turn — same channel claude-code uses, no
        // new wiring required.
        let hint = crate::tools::fallback_registry::render_persistence_hint(&e, &call.name);
        // `NotFound` carries zero routing signal on its own (the fallback
        // registry deliberately stays silent for it — the call shape, not the
        // tool choice, is wrong). Name repair already rewrote unambiguous
        // drift before dispatch, so reaching here means the name was either
        // unknown or ambiguous; surface the near-matches the repair tier had
        // to abstain from, so the model self-corrects in one turn instead of
        // groping via `list_tools`. Advisory text only — never auto-dispatch
        // (R7: the model picks).
        let did_you_mean = if let crate::tools::service::ToolError::NotFound { name } = &e {
            let defs = self.deps.tools.metadata_schema();
            let offered: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
            let candidates = crate::tools::name_repair::suggest_candidates(name, &offered, 3);
            // Advertise no discovery tool: this said "call `list_tools`", which
            // production never registers, so the model looped on NotFound. Its
            // live tool array is already in context; the near-miss is the signal.
            if candidates.is_empty() {
                " No similarly-named tool is available.".to_string()
            } else {
                format!(" Did you mean: {}?", candidates.join(", "))
            }
        } else {
            String::new()
        };
        let error_msg = format!("{e}{hint}{did_you_mean}");
        let error_event = SessionEvent::ToolError {
            turn_id,
            call_id: call.id.clone(),
            error: error_msg.clone(),
            at: now_ms(),
        };
        if let Err(emit_err) = self.deps.session.emit_event(session_id, error_event).await {
            tracing::warn!(
                ?session_id,
                call_id = %call.id,
                ?emit_err,
                "failed to persist ToolError event",
            );
        }
        let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let error_for_timeline = error_msg.clone();
        // Live "done" event (error case) — paired with `on_tool_call_start`
        // so the broadcast stream emits `ToolCallDone` → `StreamEvent::ToolEnd`
        // with the error body. Fired before the persistence trace, mirroring
        // the success path.
        callback.on_tool_call_done(&call.id, None, Some(&error_msg), dur_ms);
        self.emit(
            || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                iteration,
                call: crate::harness::trace::ToolCallEndEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                    duration_ms: dur_ms,
                },
                result: crate::tools::runtime::ToolResult::Error {
                    error: error_msg,
                    retryable,
                },
            },
        );
        self.push_tool_invocation(
            call.id.clone(),
            call.name.clone(),
            dur_ms,
            false,
            Some(error_for_timeline),
        );
    }
}

#[cfg(test)]
mod per_tool_budget_tests {
    use super::*;

    #[test]
    fn resolve_effective_budget_prefers_per_tool_over_global() {
        let per_tool = Some(std::time::Duration::from_millis(50));
        let global = Some(std::time::Duration::from_secs(60));
        assert_eq!(
            resolve_effective_budget(per_tool, global),
            Some(std::time::Duration::from_millis(50)),
        );
    }

    #[test]
    fn resolve_effective_budget_falls_back_to_global() {
        let global = Some(std::time::Duration::from_secs(60));
        assert_eq!(
            resolve_effective_budget(None, global),
            Some(std::time::Duration::from_secs(60)),
        );
    }

    #[test]
    fn resolve_effective_budget_returns_none_when_both_unset() {
        assert_eq!(resolve_effective_budget(None, None), None);
    }
}

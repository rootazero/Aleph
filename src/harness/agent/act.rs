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
//! `buffer_unordered`. Live "done" callbacks fire in COMPLETION order (a fast
//! read is not head-of-line blocked behind a slow sibling, and its duration is
//! its real wall clock), while the transcript — `SessionEvent` emit, trace,
//! layer-3 budget, timeline push — stays strictly in input order (the
//! live/transcript split pi, openclaw and codex all share):
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
//! Before dispatching each serial group (and before each parallel
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

/// What one tool execute resolves to, before any event emission.
type ExecOutcome = Result<ToolOutput, crate::tools::service::ToolError>;

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
use crate::harness::trait_def::HarnessError;
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::SessionId;
use crate::thinker::nudges::{CROSS_BATCH_REFUSED_CAUSE, DEFERRED_TOOL_RESULT_REASON};
use tokio_util::sync::CancellationToken;

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
                    None,
                    None,
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
        // Canonical arg signatures and concurrency claims are computed once
        // per batch here and threaded down to `dispatch_group` /
        // `can_parallel_dispatch` / `act_parallel`, so the per-group paths do
        // not re-serialize the args or re-query the claim per call.
        let mut canonical: Vec<String> = Vec::with_capacity(tool_calls.len());
        let mut claims = Vec::with_capacity(tool_calls.len());
        for call in &tool_calls {
            let canon = super::canonical_json_string(&call.arguments);
            if !seen.insert((call.name.clone(), canon.clone())) {
                has_duplicate = true;
            }
            canonical.push(canon);
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
                    Some(&canonical),
                    Some(&claims),
                )
                .await;
        }

        // Multi-group: dispatch each contiguous slice in input order. Groups run
        // sequentially (a later group may observe an earlier group's side
        // effects); `dispatch_group` parallelizes within any group of >= 2 calls.
        let mut executed_count: usize = 0;
        let mut remaining = tool_calls.into_iter();
        for (start, end) in groups {
            // Cancel checkpoint at the group boundary. Without it `/stop` still
            // walks every remaining group: each logs a ToolCallRequested,
            // registers in-flight, dispatches (taking an instant cancellation
            // error) and logs a ToolError — phantom failures the model reads
            // next turn and `tool_summaries` counts as real. Closing the pending
            // blocks supplies the tool_use↔result pairing dispatch would have.
            // R7/R10: mechanical; an explicit user stop is not a completeness
            // judgement.
            if run_cancel.is_cancelled() {
                let pending: Vec<String> = remaining.by_ref().map(|c| c.id).collect();
                self.close_unexecuted_tool_uses(
                    session_id,
                    turn_id,
                    &pending,
                    "the run was cancelled before this tool group started",
                )
                .await;
                break;
            }
            // Cooperative steer checkpoint at the group boundary. Groups run
            // sequentially; a parallel group's `buffer_unordered` wave is not
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
                    Some(&canonical[start..end]),
                    Some(&claims[start..end]),
                )
                .await
            {
                Ok(n) => executed_count = executed_count.saturating_add(n),
                Err(e) => {
                    // A group that aborts the run (a session-store failure, a
                    // guardrail registry error) closes out the not-yet-started
                    // later groups, so their tool_use blocks keep their result
                    // pairing and the prompt builder does not drop the whole
                    // assistant turn as orphaned on the next build.
                    let pending_ids: Vec<String> = remaining.by_ref().map(|c| c.id).collect();
                    if !pending_ids.is_empty() {
                        self.close_unexecuted_tool_uses(
                            session_id,
                            turn_id,
                            &pending_ids,
                            "the run aborted during an earlier tool group in this batch",
                        )
                        .await;
                    }
                    return Err(e);
                }
            }
        }
        Ok(executed_count)
    }

    /// Emit a synthetic `ToolResult` for each tool call the cooperative steer
    /// checkpoint skipped. Every `tool_use` block in the turn's
    /// `AssistantMessage` must have a matching `tool_result` or the provider
    /// rejects the next request, so a skipped call still gets a result — one
    /// whose body says, in the only channel the model reads, that the call did
    /// not run: `{"deferred": true, "reason": …, "tool": …}`.
    ///
    /// ⚠️ This doc used to assert the opposite ("as `ToolError` … NOT as a
    /// `ToolResult`", citing H4 in review/harness-statics) while the code, the
    /// comment inside the loop, and three tests in `tests/act.rs`
    /// (`deferred_results_emit_one_tool_result_per_skipped_call` and the two
    /// steer-checkpoint counters) all pinned `ToolResult`. The H4 direction was
    /// never the shipped behaviour; the paragraph is kept here — as history,
    /// not as a contract — so the next reader does not "restore" it and flip a
    /// model-visible shape three tests are holding down. The reasoning for the
    /// shipped choice is in the loop body: neither "succeeded" nor "the tool
    /// failed" is true of a deferred call, so the marker states the third thing
    /// explicitly rather than picking the less-wrong lie.
    ///
    /// R10-safe: pure mechanical bookkeeping. Whether a deferred call is
    /// re-run is the model's decision next Think, not the harness's.
    pub(crate) async fn emit_deferred_tool_results(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        calls: &[NativeToolCall],
    ) -> Result<(), HarnessError> {
        // Emit a `ToolResult` (not `ToolError`) so the next Think sees
        // these calls as "ran with output `{"deferred": true}`" — that
        // matches the model-visible contract (`ToolError` would prompt the
        // model to think the tool itself failed, which is wrong: the
        // harness chose not to run it because a newer user message
        // arrived). The `deferred: true` marker is the audit signal; the
        // rest of the output shape is left to the model to inspect.
        for call in calls {
            let value = serde_json::json!({
                "deferred": true,
                "reason": DEFERRED_TOOL_RESULT_REASON,
                "tool": call.name,
            });
            let event = SessionEvent::ToolResult {
                turn_id,
                call_id: call.id.clone(),
                output: crate::session::events::ToolOutput {
                    value,
                    metadata: crate::session::events::ToolOutputMetadata::default(),
                },
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
    ///
    /// `canonical` / `claims` carry the per-call canonical arg signatures and
    /// concurrency claims pre-computed by `act()` (aligned by index with
    /// `tool_calls`) so admission does not recompute them. They are `None`
    /// only on `act()`'s single-call / parallelism-disabled fast path, which
    /// by construction cannot be admitted to the parallel route — so absence
    /// short-circuits straight to the serial loop.
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
        canonical: Option<&[String]>,
        claims: Option<&[crate::tools::concurrency::ConcurrencyClaim]>,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        // Within-batch idempotency memo. Scoped to this single dispatch group:
        // duplicate calls inside one group are deduplicated, but a legitimate
        // cross-turn repeat (e.g. `read_file` after `write_file`, or any
        // time-varying tool such as `get_current_time`) always re-executes
        // against fresh state instead of replaying a stale result.
        let mut tool_call_cache: HashMap<(String, String), ToolOutput> = HashMap::new();

        // opencode-parity parallel fast path. Falls through to the serial loop
        // below when any precondition fails (see module docs). Missing
        // pre-computed batch data is one such precondition, and not a special
        // case: it happens only on `act()`'s fast path, whose entry condition
        // (`!parallel_enabled || tool_calls.len() < 2`) is the same condition
        // `can_parallel_dispatch` rejects on. Recomputing it here would be a
        // branch with no caller.
        if let (Some(canonical), Some(claims)) = (canonical, claims) {
            if self.can_parallel_dispatch(&tool_calls, canonical, claims) {
                return self
                    .act_parallel(
                        session_id,
                        turn_id,
                        tool_calls,
                        callback,
                        iteration,
                        budget_turn_id,
                        run_cancel,
                        canonical,
                    )
                    .await;
            }
        }

        // Offered-tool snapshot for tool-name repair, taken once per batch (the
        // dispatchable set is stable across a single `act()` call). Used by the
        // unified resolver below; the parallel fast path naturally routes any
        // unrepaired/typo'd name back here (an unknown name yields a
        // conservative `Global` concurrency claim → non-parallelizable → serial).
        let offered_defs = self.deps.tools.dispatchable_list().await;
        let offered_names: Vec<&str> = offered_defs.iter().map(|d| d.name.as_str()).collect();

        // Cooperative steer checkpoint, hoisted to once per group (was per
        // tool call, which paid a seq-ranged session-store read for every
        // call in the batch). If a non-synthetic user message arrived after
        // this turn's prompt boundary (the user changed their mind
        // mid-batch), defer every call in this group so the model sees the
        // message next Think and decides to pivot or resume. A message
        // arriving mid-group is now caught at the next group boundary
        // (multi-group path) or by the run loop's Done-arm follow-up check.
        // R7/R10: mechanical — no intent judgement here.
        if self.has_unanswered_user_message(session_id).await {
            self.emit_deferred_tool_results(session_id, turn_id, &tool_calls)
                .await?;
            if let Some(ref tracker) = self.stall_tracker {
                tracker.record_activity().await;
            }
            return Ok(executed_count);
        }

        for mut call in tool_calls {
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
            // Structured tool-start event (id + name + args).
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
                let mut output = cached.clone();
                output.metadata.latency_ms = 0;
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
            // The Act-period wall clock is NOT here. It lives in the tool layer
            // (`ScopedToolService::execute_inner`), below the human approval gate
            // — charging an operator's reading time to the tool's budget could
            // kill a command they had just approved. An overrun arrives as
            // `ToolError::Timeout`, recoverable like any other tool failure; a
            // genuinely hung *run* is still caught by the stall tracker.
            // Ambient call identity: every approval gate / record built beneath
            // this dispatch reads the exact (turn, call.id) instead of scanning
            // the session log by tool name (see `crate::approval::tool_call`).
            // The scoped future is fully owned (clones, like the parallel
            // path) — a borrowed inner future inside a task-local scope trips
            // rustc's "Send is not general enough" HRTB limitation once this
            // future crosses a `tokio::spawn` chain.
            let inner = {
                let tools = self.deps.tools.clone();
                let name = call.name.clone();
                let args = call.arguments.clone();
                // Boxed: the task-local scope future is structural, and left
                // bare it trips rustc's "Send is not general enough" HRTB
                // limitation once this async fn crosses a `tokio::spawn`
                // chain (subagent runtimes). Erasing to a BoxFuture is the
                // standard workaround.
                let fut: BoxFuture<'static, ExecOutcome> =
                    Box::pin(crate::approval::with_call_identity(
                        Some(crate::approval::CallIdentity {
                            turn_id,
                            call_id: call.id.clone(),
                        }),
                        async move { tools.execute_with_cancel(&name, args, call_cancel).await },
                    ));
                fut.await
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
                    let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                    // Live "done" event — mirror of `on_tool_call_start`, fired
                    // before the transcript persists (same order as the
                    // parallel path's completion-time firing).
                    callback.on_tool_call_done(&call.id, Some(&output.value), None, dur_ms);
                    self.persist_tool_success(
                        session_id, turn_id, &call, output, dur_ms, iteration,
                    )
                    .await?;
                }
                Err(e) => {
                    // Do NOT abort — continue processing remaining tool calls.
                    // The error is persisted to session log; the next Think
                    // turn will see it as tool_result(is_error=true).
                    //
                    // Only a NON-retryable failure enters the cross-batch memo.
                    // A wall-clock `Timeout` (or a `Transport` blip) says nothing
                    // about the call, and banning it refused the very retry the
                    // error text invites on the very next batch.
                    if !e.is_retryable() {
                        self.record_failure(call.name.clone(), cache_key.1.clone());
                    }
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
    ///
    /// `canonical` / `claims` are the per-call canonical arg signatures and
    /// concurrency claims pre-computed by `act()`, aligned by index with
    /// `tool_calls`.
    fn can_parallel_dispatch(
        &self,
        tool_calls: &[NativeToolCall],
        canonical: &[String],
        claims: &[crate::tools::concurrency::ConcurrencyClaim],
    ) -> bool {
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
        for (call, canon) in tool_calls.iter().zip(canonical) {
            if !seen.insert((call.name.as_str(), canon.as_str())) {
                return false;
            }
        }
        // Resource-scope-aware admission: collect each call's concurrency
        // claim (Shared / Exclusive{Global|Paths}) and admit the batch only
        // when no pair conflicts. This generalizes the old "every call must be
        // concurrent-safe" check — a batch of disjoint-path file mutations now
        // parallelizes, while same-path or whole-world mutations still fall
        // back to the serial loop. See `crate::tools::concurrency`.
        crate::tools::concurrency::batch_parallelizable(claims)
    }

    /// Parallel fast path. Pre-emits `ToolCallRequested` events in input
    /// order, dispatches all executes concurrently via `FuturesOrdered`, and
    /// then walks the completion results in input order to emit
    /// `ToolResult` / `ToolError`, record the Layer 3 turn budget, and push
    /// the timeline entry. Side-effect ordering matches the serial loop.
    ///
    /// Like the serial path, it owns no wall clock: each call's budget is
    /// enforced by the tool layer, below the approval gate.
    ///
    /// `canonical` carries the per-call canonical arg signatures pre-computed
    /// by `act()` (aligned by index with `tool_calls`, pre-guardrail — PASS 0
    /// re-canonicalizes any guardrail-sanitised call).
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
        canonical: &[String],
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
        let mut canonical_args: Vec<String> = canonical.to_vec();
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
        // emit ToolCallRequested SessionEvent. The Instant captured here times
        // only the synthetic-error paths that resolve inside this pass; a
        // dispatched call is timed by its own future, which may not be polled
        // for a while yet. Skipped calls take the synthetic-error fast path
        // here and are omitted from PASS 1 dispatch.
        let mut started_at: Vec<Instant> = Vec::with_capacity(tool_calls.len());
        for (idx, call) in tool_calls.iter().enumerate() {
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
                continue;
            }
        }

        // PASS 1 — parallel: dispatch up to `parallelism` execute futures via
        // `stream::iter(...).buffer_unordered(n)`, which polls at most `n` at a
        // time and yields completions AS THEY RESOLVE. Each call's wall clock
        // is owned by the tool layer, below the approval gate — never by this
        // batch. A guardrail rewrite changes the path set the claim was derived
        // from, so the pre-admission disjointness proof is void: any rewrite
        // serializes the batch.
        let cap = self.deps.parallel_tool_concurrency.unwrap_or(0).max(2);
        let rewritten = sanitized.iter().any(Option::is_some);
        let parallelism = if rewritten { 1 } else { cap };
        // Build futures only for live indices — skipped indices (cross-batch
        // dedup / guardrail Block) already emitted their synthetic ToolError
        // in PASS 0. Each future carries its ORIGINAL index, so the
        // completion loop and PASS 2 address `tool_calls` directly with no
        // positional re-assembly.
        let mut live_futs: Vec<BoxFuture<'static, (usize, ExecOutcome, u64)>> =
            Vec::with_capacity(tool_calls.len());
        // Gap B follow-up — keep one InFlightGuard per call alive for the
        // duration of the whole parallel dispatch. Each guard drops when this
        // Vec goes out of scope after PASS 2 finishes, which is strictly
        // later than every future resolves.
        let mut in_flight_guards: Vec<crate::tools::in_flight::InFlightGuard> = Vec::new();
        for (idx, call) in tool_calls.iter().enumerate() {
            if skip[idx] || blocked[idx] {
                continue;
            }
            let tools = self.deps.tools.clone();
            let name = call.name.clone();
            // Guardrail-sanitised args win over the model's original (the
            // `ToolCallRequested` event already logged the original).
            let args = sanitized[idx]
                .clone()
                .unwrap_or_else(|| call.arguments.clone());
            // Each parallel call owns a fresh child token forked from the
            // run-level cancel. If the run is cancelled mid-batch, every
            // in-flight call short-circuits without waiting for the entire
            // batch to drain.
            let call_cancel = run_cancel.child_token();
            if let Some(reg) = self.deps.in_flight_tool_calls.as_ref() {
                in_flight_guards.push(reg.register(&call.id, &call.name, call_cancel.clone()));
            }
            // Ambient call identity (parallel parity with the serial path):
            // exact per-future scoping is what lets approval-gated calls share
            // a batch — each concurrent card stamps its own (turn, call.id).
            let identity = crate::approval::CallIdentity {
                turn_id,
                call_id: call.id.clone(),
            };
            live_futs.push(Box::pin(async move {
                // Clock starts on FIRST POLL, not at admission:
                // `buffer_unordered` runs at most `parallelism` at a time, so
                // anything past the cap queues first, and timing from PASS 0
                // bills a fast tool for the wait it spent not running.
                let started = Instant::now();
                let exec = crate::approval::with_call_identity(Some(identity), async move {
                    tools.execute_with_cancel(&name, args, call_cancel).await
                })
                .await;
                let dur_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                (idx, exec, dur_ms)
            }));
        }
        // Completion-order drive loop (pi/openclaw/codex parity): each live
        // "done" callback fires the moment ITS call resolves — a fast read is
        // no longer head-of-line blocked behind a slow sibling, and its
        // `duration_ms` is the tool's real wall clock instead of being
        // inflated by the wait for earlier-indexed calls. The live event
        // carries the raw output (the Layer-3 budget may later spill the
        // PERSISTED copy to a marker in PASS 2 — the transcript, not the live
        // stream, is the budgeted surface). Stall-tracker activity is also
        // recorded per completion, so a long mixed batch no longer looks
        // stalled until its slowest member returns.
        let mut settled: Vec<Option<(u64, Result<ToolOutput, (String, bool)>)>> =
            (0..tool_calls.len()).map(|_| None).collect();
        {
            let mut completions = stream::iter(live_futs).buffer_unordered(parallelism);
            while let Some((idx, exec, dur_ms)) = completions.next().await {
                let call = &tool_calls[idx];
                let outcome = match exec {
                    Ok(output) => {
                        callback.on_tool_call_done(&call.id, Some(&output.value), None, dur_ms);
                        Ok(output)
                    }
                    Err(e) => {
                        let retryable = e.is_retryable();
                        let msg = self.compose_tool_error_msg(call, &e);
                        callback.on_tool_call_done(&call.id, None, Some(&msg), dur_ms);
                        Err((msg, retryable))
                    }
                };
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                settled[idx] = Some((dur_ms, outcome));
            }
        }
        // PASS 1 complete — every future has resolved, so the in-flight
        // registry entries are no longer cancellable in any meaningful sense.
        // Drop the guards now (vs end-of-function) so `tools.cancel_call`
        // doesn't quietly target a token attached to a future that's already
        // returned. PASS 2 below only emits events; it never touches the
        // tokens or the registry.
        drop(in_flight_guards);

        // PASS 2 — serial in INPUT order (the transcript order): apply Layer 3
        // budget, emit ToolResult/ToolError session events, trace, push the
        // timeline entry. The live callback already fired per completion
        // above, so PASS 2 passes `live: None` — persistence and live
        // streaming are deliberately split (emission-order transcript,
        // completion-order live events). Skipped indices (cross-batch dedup
        // hits, already errored in PASS 0) pass through with no further action.
        for (idx, slot) in settled.into_iter().enumerate() {
            let Some((dur_ms, outcome)) = slot else {
                continue; // PASS-0 dedup-rejected; already emitted synthetic error.
            };
            let call = &tool_calls[idx];
            match outcome {
                Ok(mut output) => {
                    executed_count = executed_count.saturating_add(1);
                    self.apply_turn_budget(budget_turn_id, call, &mut output);
                    // Cross-batch dedup: any success clears the failure set —
                    // the LLM has demonstrably pivoted to a working strategy.
                    self.clear_failures();
                    self.persist_tool_success(session_id, turn_id, call, output, dur_ms, iteration)
                        .await?;
                }
                Err((error_msg, retryable)) => {
                    // Cross-batch dedup: record the (tool, args) signature so the
                    // next turn refuses an identical repeat — but only for a
                    // NON-retryable failure. A wall-clock `Timeout` says nothing
                    // about the call, and banning it refused the retry the error
                    // text itself invites (serial parity).
                    if !retryable {
                        self.record_failure(call.name.clone(), canonical_args[idx].clone());
                    }
                    self.persist_tool_error(
                        session_id, turn_id, call, error_msg, retryable, dur_ms, iteration,
                    )
                    .await;
                }
            }
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
        use crate::tools::result_processing::recovery_footer;
        let offload = |s: &crate::tools::turn_budget::SpillInstruction| {
            recovery_footer(Some(store), &s.call_id, &s.tool_name, &s.original_text, 0)
        };
        for spill in spills {
            if spill.call_id != call.id {
                // Earlier-iteration spill: its SessionEvent::ToolResult is
                // already persisted, so post-hoc rewrite from here isn't
                // possible. The marker file is still written for recovery;
                // cheap_passes surfaces it on the next preflight.
                let _ = offload(&spill);
                continue;
            }
            // Same-turn newest spill: rewrite output BEFORE the SessionEvent
            // is emitted so the LLM sees the marker instead of the full text.
            if let Some((footer, _)) = offload(&spill) {
                output.value = serde_json::Value::String(footer);
            }
        }
    }

    /// Persist a successful tool call to the transcript: emit
    /// `SessionEvent::ToolResult`, trace event, and timeline entry. The live
    /// "done" event is NOT fired here — both paths fire it before persisting
    /// (the serial loop right at the call site, the parallel path per
    /// completion in its drive loop), so live streaming and the transcript
    /// are cleanly split.
    async fn persist_tool_success(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        mut output: ToolOutput,
        dur_ms: u64,
        iteration: usize,
    ) -> Result<(), HarnessError> {
        output.metadata.latency_ms = dur_ms;
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

    /// Compose the LLM-facing error body for a failed tool call: the upstream
    /// `to_string()` followed by a single-line persistence hint that names the
    /// error kind, recommends concrete alternative tools, and reminds the
    /// model the ladder must be climbed before giving up (the persistence
    /// doctrine in `thinker::layers::provider_guidance`). The composed text
    /// travels through `SessionEvent::ToolError.error` and is surfaced to the
    /// model as `tool_result(is_error=true)` in the very next Think turn —
    /// same channel claude-code uses, no new wiring required.
    fn compose_tool_error_msg(
        &self,
        call: &NativeToolCall,
        e: &crate::tools::service::ToolError,
    ) -> String {
        let hint = crate::tools::fallback_registry::render_persistence_hint(e, &call.name);
        // `NotFound` carries zero routing signal on its own (the fallback
        // registry deliberately stays silent for it — the call shape, not the
        // tool choice, is wrong). Name repair already rewrote unambiguous
        // drift before dispatch, so reaching here means the name was either
        // unknown or ambiguous; surface the near-matches the repair tier had
        // to abstain from, so the model self-corrects in one turn instead of
        // groping via `list_tools`. Advisory text only — never auto-dispatch
        // (R7: the model picks).
        let did_you_mean = if let crate::tools::service::ToolError::NotFound { name } = e {
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
        format!("{e}{hint}{did_you_mean}")
    }

    /// Fire the live "done" event for a failed tool call, then persist it:
    /// the serial-path convenience over [`Self::compose_tool_error_msg`] +
    /// [`Self::persist_tool_error`]. Live-before-transcript matches the
    /// parallel path, where the live event fires per completion.
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
        let error_msg = self.compose_tool_error_msg(call, &e);
        let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        // Live "done" event (error case) — paired with `on_tool_call_start`
        // so the broadcast stream emits `ToolCallDone` → `StreamEvent::ToolEnd`
        // with the error body.
        callback.on_tool_call_done(&call.id, None, Some(&error_msg), dur_ms);
        self.persist_tool_error(
            session_id, turn_id, call, error_msg, retryable, dur_ms, iteration,
        )
        .await;
    }

    /// Persist a failed tool call to the transcript: emit
    /// `SessionEvent::ToolError` (best effort — failure to persist logs at
    /// WARN, never aborts the batch), trace event, timeline entry.
    /// `error_msg` is the pre-composed [`Self::compose_tool_error_msg`] body,
    /// so the live event (fired by the caller before this) and the transcript
    /// always carry the same text.
    #[allow(clippy::too_many_arguments)]
    async fn persist_tool_error(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        error_msg: String,
        retryable: bool,
        dur_ms: u64,
        iteration: usize,
    ) {
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
        let error_for_timeline = error_msg.clone();
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

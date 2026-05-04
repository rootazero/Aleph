//! AgentHarness — the concrete Think→Act implementation.
//!
//! Task 8 implemented the Think half of the loop. Task 9 added:
//!   * Act dispatch (executing tool_calls sequentially, emitting ToolResult /
//!     ToolError events).
//!   * Preservation of assistant `tool_use` intent inside `AssistantMessage`
//!     events so later Think cycles can reconstruct the conversation.
//!   * Full-history `build_prompt` that re-emits the preceding assistant
//!     tool_use turn and resolves real tool names for `ToolResult` messages.
//!
//! Task 10 (Phase 6b) additionally consumes the optional triad on
//! `HarnessDeps`:
//!   * `context_budget.before_turn(...)` — drives compaction / hit_limit.
//!   * `context_compactor.compact(...)` — fires when budget directs warning.
//!   * `stop_hooks` — consulted before an early `TurnState::Done` handoff;
//!     a blocking verdict forces one more `Continue` so the model reacts.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::budget::LoopDirective;
use crate::harness::callback::{HarnessCallback, NoopHarnessCallback};
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::providers::adapter::{NativeToolCall, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::{now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::SessionId;
use crate::tools::service::ToolError;
use crate::verification::stop_hooks::{execute_stop_hooks, StopHookContext, StopHookHandler};

pub struct AgentHarness {
    deps: HarnessDeps,
    /// Tracks agent activity for stall detection. `None` when stall detection
    /// is disabled (no `stall_config` in deps).
    stall_tracker: Option<crate::harness::stall::StallTracker>,
    /// Set when `context_budget.before_turn` returns `FinalReply`. Surfaced
    /// through [`AgentHarness::hit_limit`] so the orchestrator bridge can
    /// populate `FlowOutcome::hit_limit`.
    hit_limit: AtomicBool,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        let stall_tracker = deps.stall_config.as_ref().map(|config| {
            crate::harness::stall::StallTracker::new(
                config.clone(),
                tokio_util::sync::CancellationToken::new(),
            )
        });
        Self {
            deps,
            stall_tracker,
            hit_limit: AtomicBool::new(false),
        }
    }

    /// `true` if a budget directive forced an early exit during this run.
    /// Cleared by [`AgentHarness::reset_hit_limit`] before a fresh run.
    pub fn hit_limit(&self) -> bool {
        self.hit_limit.load(Ordering::Relaxed)
    }

    /// Reset the hit_limit flag. Called before a fresh session drive so a
    /// previous run's budget trip does not leak into the next outcome.
    pub fn reset_hit_limit(&self) {
        self.hit_limit.store(false, Ordering::Relaxed);
    }

    /// Convenience: wrap this harness as an `Arc<dyn SessionDriver>` so it
    /// can be stored in containers that don't depend on the concrete type.
    pub fn into_session_driver(self) -> std::sync::Arc<dyn crate::session::SessionDriver> {
        std::sync::Arc::new(self)
    }

    /// Max consecutive stop-hook vetos before the harness gives up and
    /// forces Done. Prevents infinite loops when a hook permanently blocks.
    const MAX_STOP_HOOK_VETOS: usize = 10;

    /// Internal turn execution with pre-computed counters to avoid O(n²)
    /// event-log scans in the outer loop.
    ///
    /// Returns `(TurnState, tool_calls_executed, is_stop_hook_veto)`.
    async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
    ) -> Result<(TurnState, usize, bool), HarnessError> {
        // Hold a sleep-inhibit assertion for the duration of this turn so a long
        // Think→Act cycle does not get cut off by macOS idle sleep. Drop happens
        // automatically when this scope exits, releasing the IOPMAssertion.
        let _sleep_guard = self.deps.power.as_ref().and_then(|power| {
            match power.inhibit_sleep("Aleph agent loop") {
                Ok(g) => Some(g),
                Err(e) => {
                    tracing::debug!(target: "power", "sleep inhibitor unavailable: {e}");
                    None
                }
            }
        });

        // Kick off a throttled skill prefetch scan before the LLM call. The
        // scan runs in a background task; its result surfaces on the next
        // turn rather than blocking this one.
        if let Some(prefetcher) = self.deps.skill_prefetcher.as_ref() {
            let _ = prefetcher.start_scan();
        }

        // 1. Fetch full event log and compute the tail boundary.
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let tail_start = tail_start_index(&events);

        // 2. Build the LLM request. `build_prompt` has access to the full log
        //    so it can reconstruct the preceding assistant tool_use turn and
        //    resolve tool names for tool_result messages.
        let mut messages = build_prompt(&events, tail_start);

        // 2a. Task-10 budget check: evaluate context pressure before issuing
        // the LLM call. `FinalReply` forces a terminal turn with no tools.
        let budget_directive = if let Some(budget) = self.deps.context_budget.as_ref() {
            let mut guard = budget.lock().await;
            Some(guard.before_turn(&messages, "", &[]))
        } else {
            None
        };

        // 2b. Compact when directive calls for it and a compactor is wired.
        if matches!(budget_directive, Some(LoopDirective::CompactAndContinue)) {
            if let Some(compactor) = self.deps.context_compactor.as_ref() {
                // `fresh_tail = 0` lets the compactor fall back to its own
                // config default (matches Task 6 spec).
                if let Err(e) = compactor.compact(&mut messages, 0, None).await {
                    tracing::warn!(
                        ?session_id,
                        ?e,
                        "context compactor failed; continuing with uncompacted messages",
                    );
                }
            }
        }

        // 2c. `FinalReply` directive — record hit_limit and short-circuit to
        // Done without calling the LLM or running tools. The last assistant
        // message already on the session log is the final text.
        if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
            self.hit_limit.store(true, Ordering::Relaxed);
            callback.on_complete_via_harness();
            return Ok((TurnState::Done, 0, false));
        }

        // 2d. Fetch tool definitions from the tool service and convert to
        // dispatcher format so the LLM sees available tools.
        let tool_defs = self.deps.tools.list().await;
        let dispatcher_tools: Vec<crate::dispatcher::ToolDefinition> = tool_defs
            .into_iter()
            .map(|def| crate::dispatcher::ToolDefinition {
                name: def.name,
                description: def.description,
                parameters: def.input_schema,
                requires_confirmation: false,
                category: crate::dispatcher::ToolCategory::Builtin,
                llm_context: None,
                strict: false,
            })
            .collect();
        let tools_ref: Option<&[crate::dispatcher::ToolDefinition]> = if dispatcher_tools.is_empty()
        {
            None
        } else {
            Some(&dispatcher_tools)
        };

        let payload = match self.deps.system_prompt.as_deref() {
            Some(sp) => RequestPayload::new(&messages)
                .with_system(Some(sp))
                .with_tools(tools_ref),
            None => RequestPayload::new(&messages).with_tools(tools_ref),
        };

        // 3. Call the LLM.
        let response = self.deps.llm.process(payload).await?;

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
        let turn_id = current_turn_id(&events);
        let text = response.text_content();
        if !text.is_empty() {
            // Non-streaming LLM layer emits one chunk per turn; the callback
            // shape permits finer chunking once `process_stream` is wired.
            callback.on_delta(&text);
        }
        let blocks = tool_use_blocks(&response.tool_calls);
        let assistant_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: text.clone(),
                blocks,
            },
            at: now_ms(),
        };
        self.deps
            .session
            .emit_event(session_id, assistant_event)
            .await?;

        // 5. If the LLM produced tool_calls, run the Act phase; otherwise
        //    evaluate stop hooks before declaring Done.
        if response.tool_calls.is_empty() {
            // Task-10: let stop hooks veto the stop. A blocking verdict
            // forces one more Continue turn so the model can react.
            let block = self
                .evaluate_stop_hooks(iterations, tool_calls_made, Some(text))
                .await;
            if let Some(reason) = block {
                tracing::info!(
                    ?session_id,
                    reason = %reason,
                    "stop hook vetoed; forcing continue",
                );
                // Inject the block reason as a user turn so the model sees it
                // and has a chance to act. Matches loop_core semantics.
                let new_turn = uuid::Uuid::new_v4();
                let block_event = SessionEvent::UserMessage {
                    turn_id: new_turn,
                    content: MessageContent {
                        text: format!("[stop-hook veto] {reason}"),
                        blocks: Vec::new(),
                    },
                    at: now_ms(),
                };
                self.deps
                    .session
                    .emit_event(session_id, block_event)
                    .await?;
                return Ok((TurnState::Continue, 0, true));
            }
            Ok((TurnState::Done, 0, false))
        } else {
            let executed = self
                .act(session_id, turn_id, response.tool_calls, callback)
                .await?;
            Ok((TurnState::Continue, executed, false))
        }
    }

    /// Act phase: execute each tool_call sequentially, emitting a
    /// `ToolCallRequested` event before every call and either a `ToolResult`
    /// or `ToolError` event after. A failure short-circuits the rest of the
    /// batch and surfaces as `HarnessError::Tool`.
    ///
    /// Returns the number of tool calls that were actually executed (not
    /// skipped due to a prior error).
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
    ) -> Result<usize, HarnessError> {
        let mut first_error: Option<ToolError> = None;
        let mut executed_count: usize = 0;

        for call in tool_calls {
            callback.on_tool_call(&call.name);
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            if let Some(ref prior_err) = first_error {
                let skip_event = SessionEvent::ToolError {
                    turn_id,
                    call_id: call.id.clone(),
                    error: format!("Skipped: {}", prior_err),
                    at: now_ms(),
                };
                if let Err(emit_err) = self.deps.session.emit_event(session_id, skip_event).await {
                    tracing::warn!(
                        ?session_id,
                        call_id = %call.id,
                        ?emit_err,
                        "failed to persist skipped-tool ToolError event",
                    );
                }
                continue;
            }

            match self.deps.tools.execute(&call.name, call.arguments).await {
                Ok(output) => {
                    executed_count = executed_count.saturating_add(1);
                    let result_event = SessionEvent::ToolResult {
                        turn_id,
                        call_id: call.id,
                        output,
                        at: now_ms(),
                    };
                    self.deps
                        .session
                        .emit_event(session_id, result_event)
                        .await?;
                }
                Err(e) => {
                    let error_event = SessionEvent::ToolError {
                        turn_id,
                        call_id: call.id.clone(),
                        error: e.to_string(),
                        at: now_ms(),
                    };
                    if let Err(emit_err) =
                        self.deps.session.emit_event(session_id, error_event).await
                    {
                        tracing::warn!(
                            ?session_id,
                            call_id = %call.id,
                            ?emit_err,
                            "failed to persist ToolError event",
                        );
                    }
                    first_error = Some(e);
                }
            }
        }

        if let Some(e) = first_error {
            return Err(HarnessError::Tool(e));
        }
        Ok(executed_count)
    }

    /// Evaluate stop hooks and return the blocking reason if any hook
    /// vetoes the stop. Returns `None` when `stop_hooks` is unset, empty,
    /// or when every hook allows the stop.
    async fn evaluate_stop_hooks(
        &self,
        iterations: usize,
        tool_calls_made: usize,
        final_text: Option<String>,
    ) -> Option<String> {
        let hooks = self.deps.stop_hooks.as_ref()?;
        if hooks.is_empty() {
            return None;
        }
        // `execute_stop_hooks` wants `&[Box<dyn StopHookHandler>]`; we hold
        // `Arc`s for shareability. Adapt by wrapping each Arc in a forwarding
        // Box — avoids cloning the hook implementations.
        struct ArcHook(std::sync::Arc<dyn StopHookHandler>);
        #[async_trait::async_trait]
        impl StopHookHandler for ArcHook {
            fn name(&self) -> &str {
                self.0.name()
            }
            async fn evaluate(
                &self,
                ctx: &StopHookContext,
                cancel: &CancellationToken,
            ) -> crate::verification::stop_hooks::StopHookVerdict {
                self.0.evaluate(ctx, cancel).await
            }
        }
        let boxed: Vec<Box<dyn StopHookHandler>> = hooks
            .iter()
            .map(|h| Box::new(ArcHook(h.clone())) as Box<dyn StopHookHandler>)
            .collect();
        let ctx = StopHookContext {
            final_text,
            iterations,
            tool_calls_made,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&boxed, &ctx, &cancel).await;
        result.blocking_reason().map(|s| s.to_string())
    }
}

#[async_trait]
impl crate::session::SessionDriver for AgentHarness {
    async fn drive(&self, session_id: &SessionId) -> crate::error::Result<()> {
        // Preserve `HarnessError` variant semantics when lifting into the
        // crate-wide `AlephError`:
        //   * `Cancelled` → `AlephError::Cancelled` (downstream UI branches
        //     on this to render "Operation cancelled." rather than a
        //     provider-failure banner).
        //   * `Llm(inner)` → unwrap: `inner` is already an `AlephError`, so
        //     re-wrapping would hide the structured `AuthenticationError`
        //     / `NetworkError` / etc. from callers that `match` on it.
        //   * `Tool` / `Session` → no better variant exists on `AlephError`;
        //     stringify through `provider` with a discriminating prefix.
        // Exhaustive match (no wildcard) so new `HarnessError` variants
        // force a review here.
        let mut cb = NoopHarnessCallback;
        // SessionDriver path has no external cancel source (legacy entry
        // point); construct a never-cancelled token so the Harness loop
        // behaves identically to pre-Task-5 runs.
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run(session_id, &mut cb, &cancel)
            .await
            .map_err(|e| match e {
                HarnessError::Cancelled => crate::error::AlephError::Cancelled,
                HarnessError::Llm(inner) => inner,
                HarnessError::Tool(tool_err) => {
                    crate::error::AlephError::provider(format!("harness tool error: {tool_err}"))
                }
                HarnessError::Session(sess_err) => {
                    crate::error::AlephError::provider(format!("harness session error: {sess_err}"))
                }
                HarnessError::Stalled { elapsed } => {
                    crate::error::AlephError::provider(format!("agent stalled after {:?}", elapsed))
                }
            })
    }
}

#[async_trait]
impl Harness for AgentHarness {
    /// Overrides the trait default to enforce `HarnessDeps.max_iterations`.
    /// When the cap is reached, sets `hit_limit=true`, fires `on_complete`,
    /// and returns `Ok(())` — the orchestrator bridge promotes `hit_limit`
    /// into `FlowOutcome::hit_limit`. `None` falls through to the unbounded
    /// default used by the Gateway path.
    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut stop_hook_veto_count: usize = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            if let Some(ref tracker) = self.stall_tracker {
                if tracker.is_stalled() {
                    let elapsed = tracker.elapsed().await;
                    return Err(HarnessError::Stalled { elapsed });
                }
            }
            match self
                .run_turn_internal(session_id, callback, iterations, tool_calls_made)
                .await?
            {
                (TurnState::Continue, executed, is_veto) => {
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    if is_veto {
                        stop_hook_veto_count = stop_hook_veto_count.saturating_add(1);
                        if stop_hook_veto_count >= Self::MAX_STOP_HOOK_VETOS {
                            tracing::warn!(
                                ?session_id,
                                max_vetos = Self::MAX_STOP_HOOK_VETOS,
                                "stop-hook veto limit reached; forcing Done to prevent infinite loop",
                            );
                            callback.on_complete();
                            return Ok(());
                        }
                    } else {
                        stop_hook_veto_count = 0;
                    }
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::Relaxed);
                            callback.on_complete();
                            return Ok(());
                        }
                    }
                }
                (TurnState::Done, _, _) => {
                    callback.on_complete();
                    return Ok(());
                }
            }
        }
    }

    /// Trait-required entry point. Computes counters from the event log so
    /// standalone callers (e.g. unit tests) don't need to pre-compute them.
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let iterations = count_assistant_messages(&events).saturating_add(1);
        let tool_calls_made = count_tool_calls(&events);
        self.run_turn_internal(session_id, callback, iterations, tool_calls_made)
            .await
            .map(|(state, _, _)| state)
    }
}

/// Extension trait on HarnessCallback so we can call `on_complete` even when
/// holding `&mut dyn HarnessCallback`. The direct fn call works on the
/// trait object; this is just a named shim to keep the call site readable.
trait HarnessCallbackExt {
    fn on_complete_via_harness(&mut self);
}

impl HarnessCallbackExt for dyn HarnessCallback + '_ {
    fn on_complete_via_harness(&mut self) {
        self.on_complete();
    }
}

/// Index at which events "since the last AssistantMessage" begin.
///
/// Returns `events.len()` when the log ends with an AssistantMessage and
/// `0` when there is none. The caller uses this both as the start of the
/// tail slice and as the boundary for reconstructing the prior assistant
/// turn.
fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

/// Count `AssistantMessage` events in the log — used as "iterations so far"
/// when composing stop-hook context.
fn count_assistant_messages(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count()
}

/// Count `ToolCallRequested` events — used as "tool_calls_made so far"
/// when composing stop-hook context.
fn count_tool_calls(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count()
}

/// Serialize each `NativeToolCall` as a JSON `tool_use` block so the full
/// assistant intent is preserved in the session log.
///
/// `pub(crate)` so round-trip tests can exercise the writer/reader pair.
pub(crate) fn tool_use_blocks(tool_calls: &[NativeToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|c| {
            json!({
                "type": "tool_use",
                "id": c.id,
                "name": c.name,
                "input": c.arguments,
            })
        })
        .collect()
}

/// Parse a previously persisted `tool_use` JSON block back into a
/// `ContentBlock::ToolCall`. Returns `None` for blocks that don't match
/// the shape written by `tool_use_blocks`.
///
/// `pub(crate)` so round-trip tests can exercise the writer/reader pair.
pub(crate) fn parse_tool_use_block(block: &Value) -> Option<ContentBlock> {
    let obj = block.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    let id = obj.get("id").and_then(Value::as_str)?.to_string();
    let name = obj.get("name").and_then(Value::as_str)?.to_string();
    let arguments = obj.get("input").cloned().unwrap_or(Value::Null);
    Some(ContentBlock::ToolCall {
        id,
        name,
        arguments,
    })
}

/// Build the prompt messages from the full event log.
///
/// Shape of the output:
///   1. Any `UserMessage` events that precede the last `AssistantMessage`
///      (current turn's boundary) are not replayed — only events at or after
///      the tail boundary are carried forward, EXCEPT for the immediately
///      preceding `AssistantMessage` itself which is reconstructed as an
///      `Assistant` turn so the model sees its own prior `tool_use` request.
///   2. The reconstructed assistant turn (if any) goes first.
///   3. Then the tail events (`UserMessage`, `ToolResult`) in log order.
///
/// Tool names for `ToolResult` entries are resolved by scanning backwards
/// through the full log for the matching `ToolCallRequested`. Falls back
/// to `"unknown"` only if no such event exists.
fn build_prompt(events: &[SessionEventRecord], tail_start: usize) -> Vec<UnifiedMessage> {
    let mut messages = Vec::new();

    // Reconstruct the preceding assistant turn (if any) so the model sees
    // its own tool_use request in context.
    if tail_start > 0 {
        if let SessionEvent::AssistantMessage { content, .. } = &events[tail_start - 1].event {
            let mut blocks: Vec<ContentBlock> = Vec::new();
            if !content.text.is_empty() {
                blocks.push(ContentBlock::Text {
                    text: content.text.clone(),
                    cache_control: None,
                });
            }
            for raw in &content.blocks {
                if let Some(tc) = parse_tool_use_block(raw) {
                    blocks.push(tc);
                }
            }
            if !blocks.is_empty() {
                messages.push(UnifiedMessage::Assistant { content: blocks });
            }
        }
    }

    // Walk the tail and emit UserMessage / ToolResult entries.
    for (offset, record) in events[tail_start..].iter().enumerate() {
        match &record.event {
            SessionEvent::UserMessage { content, .. } => {
                messages.push(UnifiedMessage::user(&content.text));
            }
            SessionEvent::ToolResult {
                call_id, output, ..
            } => {
                // Resolve the tool name by searching strictly BEFORE this
                // ToolResult, so matching `call_id`s in later turns cannot
                // win over the correct in-turn `ToolCallRequested`.
                let tool_result_idx = tail_start + offset;
                let tool_name =
                    resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                // Use ContentBlock::Json to preserve structure and avoid PII
                // false-positives on numeric values (e.g., stock prices,
                // index points) being mistaken for bank card numbers.
                messages.push(UnifiedMessage::tool_result_json(
                    call_id.clone(),
                    tool_name.to_string(),
                    output.value.clone(),
                    false,
                ));
            }
            SessionEvent::ToolError { call_id, error, .. } => {
                let tool_result_idx = tail_start + offset;
                let tool_name =
                    resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                messages.push(UnifiedMessage::ToolResult {
                    tool_call_id: call_id.clone(),
                    tool_name: tool_name.to_string(),
                    content: vec![ContentBlock::Text {
                        text: error.clone(),
                        cache_control: None,
                    }],
                    is_error: true,
                });
            }
            _ => {}
        }
    }

    messages
}

/// Find the `ToolCallRequested.name` whose `call_id` matches, searching
/// strictly BEFORE `before_idx` (i.e. within `events[..before_idx]`).
///
/// Scanning from the end of the log would return the most recent match
/// anywhere, which is wrong when two turns reuse the same `call_id`
/// (provider retries, deterministic ID schemes). Bounding the search to
/// the segment preceding the `ToolResult` guarantees we pick the matching
/// request from the same turn.
fn resolve_tool_name<'a>(
    events: &'a [SessionEventRecord],
    before_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    let upper = before_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        SessionEvent::ToolCallRequested {
            call_id: id, name, ..
        } if id == call_id => Some(name.as_str()),
        _ => None,
    })
}

/// Find the most recent `TurnStarted` id; generate a fresh one if none exists.
fn current_turn_id(events: &[SessionEventRecord]) -> TurnId {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}

#[cfg(test)]
mod tests {
    //! Inline tests for `AgentHarness` behaviours that assert on the exact
    //! `RequestPayload` handed to the provider. The broader Think/Act/Driver
    //! suites live in `harness::tests::{think,act,driver,task10_wiring}`.

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use crate::error::Result as AlephResult;
    use crate::harness::callback::NoopHarnessCallback;
    use crate::harness::deps::HarnessDeps;
    use crate::harness::trait_def::Harness;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    /// Provider that records the `system_prompt` it saw on each `process`
    /// call, then returns a text-only response so the harness loop finishes
    /// in one Think turn.
    struct RecordingProvider {
        captured: Arc<Mutex<Option<String>>>,
    }

    impl AiProvider for RecordingProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let captured = self.captured.clone();
            *captured.lock().unwrap() = payload.system_prompt.map(|s| s.to_string());
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
        }

        fn name(&self) -> &str {
            "recording"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Tool service with no registered tools. The harness never dispatches
    /// a tool in this test (the provider returns text-only) so `execute`
    /// returning `NotFound` is safe — it is simply never called.
    struct EmptyTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for EmptyTools {
        async fn execute(
            &self,
            name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Err(crate::tools::service::ToolError::NotFound {
                name: name.to_string(),
            })
        }

        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }
    }

    #[tokio::test]
    async fn system_prompt_flows_into_request_payload() {
        let captured = Arc::new(Mutex::new(None));
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingProvider {
            captured: captured.clone(),
        });

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn crate::session::service::SessionService> =
            Arc::new(InProcessActorSessionService::new(store));

        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(EmptyTools);
        // NoopSandbox never fires in this test — the provider returns
        // text-only, so no tool call → no sandbox dispatch.
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let sid = SessionKey::ephemeral("test-syspr");
        session.attach(sid.clone()).await.unwrap();
        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &sid,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &sid,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: MessageContent {
                        text: "hello".into(),
                        blocks: vec![],
                    },
                    at: now_ms(),
                },
            )
            .await
            .unwrap();

        let deps = HarnessDeps {
            session: session.clone(),
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: Some("ROLE: SPEC-BOT".into()),
            max_iterations: None,
            power: None,
            stall_config: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        harness.run_turn(&sid, &mut cb).await.expect("run_turn");

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.as_deref(), Some("ROLE: SPEC-BOT"));
    }

    /// Provider that always returns one tool_call, forcing `TurnState::Continue`
    /// forever. Used to verify the `max_iterations` cap cuts the loop off.
    struct LoopingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AiProvider for LoopingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![crate::providers::adapter::NativeToolCall {
                        id: format!("call-{n}"),
                        name: "noop".into(),
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: crate::providers::adapter::StopReason::ToolUse,
                    usage: None,
                })
            })
        }

        fn name(&self) -> &str {
            "looping"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Provider that returns a tool_call on calls 1..=tool_turns, then a final
    /// text-only response (stop_reason = EndTurn) so the harness reaches
    /// `TurnState::Done` naturally.
    struct CountingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        tool_turns: usize,
    }

    impl AiProvider for CountingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            let tool_turns = self.tool_turns;
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < tool_turns {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![crate::providers::adapter::NativeToolCall {
                            id: format!("call-{n}"),
                            name: "noop".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: crate::providers::adapter::StopReason::ToolUse,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".to_string()))
                }
            })
        }

        fn name(&self) -> &str {
            "counting"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Tool service whose `execute` always succeeds with an empty JSON payload.
    /// The real tool name is irrelevant — the harness only needs `execute` to
    /// return `Ok` so the tool_result is persisted and the next Think runs.
    struct AlwaysOkTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for AlwaysOkTools {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Ok(crate::session::events::ToolOutput {
                value: serde_json::json!({}),
                metadata: crate::session::events::ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }
    }

    /// Build a freshly-attached session with a single TurnStarted + UserMessage
    /// pair so `AgentHarness::run_turn` has work to do on the first call.
    async fn fresh_session(
        tag: &str,
    ) -> (
        Arc<dyn crate::session::service::SessionService>,
        crate::session::service::SessionId,
    ) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn crate::session::service::SessionService> =
            Arc::new(InProcessActorSessionService::new(store));

        let sid = SessionKey::ephemeral(tag);
        session.attach(sid.clone()).await.unwrap();
        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &sid,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &sid,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: MessageContent {
                        text: "go".into(),
                        blocks: vec![],
                    },
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        (session, sid)
    }

    #[tokio::test]
    async fn max_iterations_stops_runaway_loop() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(LoopingProvider {
            calls: calls.clone(),
        });

        let (session, sid) = fresh_session("test-cap").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();

        // Hard timeout: without the cap implemented, `run` would spin forever.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            harness.run(&sid, &mut cb, &cancel),
        )
        .await;

        outcome
            .expect("harness.run exceeded 2s timeout — max_iterations cap not enforced")
            .expect("harness.run returned an error");

        assert!(harness.hit_limit(), "expected hit_limit=true after cap");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "provider should be called exactly max_iterations (3) times",
        );
    }

    #[tokio::test]
    async fn max_iterations_none_keeps_unbounded() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            calls: calls.clone(),
            tool_turns: 4,
        });

        let (session, sid) = fresh_session("test-unbounded").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            max_iterations: None,
            power: None,
            stall_config: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            harness.run(&sid, &mut cb, &cancel),
        )
        .await
        .expect("harness.run exceeded 2s timeout")
        .expect("harness.run returned an error");

        assert!(
            !harness.hit_limit(),
            "hit_limit must be false for unbounded run"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "provider should run 4 tool turns + 1 final text turn = 5 calls",
        );
    }
}

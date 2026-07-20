# Long-Task Hardening Cycle (Hermes-Inspired)

**Date**: 2026-05-20
**Branch**: `feat/long-task-hardening` (worktree)
**Predecessor**: [Long-Task Wiring Cycle](../../../../.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_long_task_wiring_cycle.md) (closed 2026-05-19)

## Why

The prior wiring cycle closed all infrastructure-level gaps for long-running tasks (iteration cap, mid-run compaction, cron at-most-once, orphan reconciliation, token accumulator, stall tracker, tool-result 3-layer budget). A cross-codebase audit against `/Volumes/TBU4/Github/hermes-agent` (Python) found **three remaining surface defects** in Aleph's long-task path that are real bugs or correctness gaps, not "wired for wiring's sake":

| # | Defect | Type | Risk |
|---|--------|------|------|
| A | `execute_with_one_shot_backoff` blindly retries on `Timeout`/`Transport` for non-idempotent tools | Silent correctness bug | Double-send messages, double-charge payments |
| B | `FinalReply` short-circuits to Done even when the last assistant message is a `tool_use` (no terminal text reaches user) | User-visible bug | User sees a "hang" at iteration cap |
| C | ~6 zero-consumer compaction submodules under `src/context/{compact,budget}/` | Code rot | YAGNI tax on future readers |

All three are **non-destructive**: A adds an opt-in metadata field; B adds one conditional LLM call inside the existing `FinalReply` branch; C is pure deletion of confirmed orphan modules.

## R-Rules Compliance

- **R10 (Thin Harness, Dumb Loop)** — B adds one extra LLM call gated by an existing directive (`FinalReply`). No new "intelligence", no policy selection, no new state machine.
- **R7 (LLM Sovereignty)** — B does not replace LLM judgment with deterministic code; it gives the LLM **one more chance** to produce text when the loop is about to terminate.
- **R3 (Core Minimalism)** — C deletes ~600–1200 LoC of dead code (net negative cycle).

## A — Idempotency-guarded tool retries

### Current state

`src/tools/retry.rs:23-40` (`execute_with_one_shot_backoff`):

```rust
let first: Result<ToolOutput, ToolError> = op().await;
let Err(ref e) = first else { return first; };
if !e.is_retryable() { return first; }
tokio::time::sleep(RETRY_DELAY).await;
op().await   // <-- blind replay; no idempotency guard
```

`src/tools/service.rs:39-43`:

```rust
impl ToolError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Transport { .. })
    }
}
```

`Timeout` and `Transport` are exactly the error classes where a write may have already reached the server before the local error fired (network partition, slow upstream, dropped response mid-read). Re-running a `send_telegram_message` or HTTP POST under that condition can produce duplicate side effects.

Caller `src/tools/scoped.rs:499` is the only consumer; calls this for **every** tool, regardless of side-effect class.

`grep -rln 'idempotency'` in agent/tool paths → zero hits. The only `idempotency.rs` in the codebase (`src/gateway/idempotency.rs`) is for incoming gateway requests, not outgoing tool calls.

### Design

Borrow hermes-agent's discipline: idempotency is a **static, per-tool classification**, not an inferred runtime property. Hermes uses a whitelist set (`IDEMPOTENT_TOOL_NAMES`); Aleph already has a metadata struct on every `ToolDefinition`, so we extend that.

#### A.1 Schema change

`ToolDefinitionMetadata` gains one `bool` field (default `false` for safety):

```rust
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// True when re-running this tool with the same input is safe even if
    /// the previous attempt may have reached the server. Read-only / pure
    /// query tools (read_file, grep, web_search, memory_search) set this
    /// to true; side-effecting tools (write_file, bash, send_*) leave it
    /// false. Consumed by `execute_with_one_shot_backoff` to gate retries.
    #[serde(default)]
    pub idempotent: bool,
}
```

#### A.2 Retry signature change

```rust
pub async fn execute_with_one_shot_backoff<F, Fut>(
    op: F,
    idempotent: bool,
) -> Result<ToolOutput, ToolError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<ToolOutput, ToolError>>,
{
    let first = op().await;
    let Err(ref e) = first else { return first; };
    if !e.is_retryable() { return first; }
    if !idempotent { return first; }     // <-- new guard
    tokio::time::sleep(RETRY_DELAY).await;
    op().await
}
```

The conservative rule: **non-idempotent tools never auto-retry, even on retryable errors.** This sacrifices one retry per legitimate DNS failure on a POST to eliminate the double-send class entirely. The LLM can always observe the failed call and decide to retry itself at the next turn — which is the correct R7 placement of that decision.

#### A.3 Caller update

`src/tools/scoped.rs:499` reads `idempotent` from the tool's `ToolDefinitionMetadata` and passes it through.

#### A.4 Builtin annotations

Read-only builtins get `idempotent: true` in their `ToolDefinitionMetadata` constructor:

- `read_file`, `list_dir` / `ls`, `glob`, `grep`
- `web_search`, `web_fetch` (only when method=GET; constructor enforces this)
- `memory_search`, `memory_get`, `skill_search`
- `agent_status`, health/probe tools

Default (`false`) covers everything else, including all unknown user-installed tools — failing safe.

### Tests

1. `retries_once_when_idempotent_and_retryable` — existing behavior preserved for `idempotent=true`.
2. `does_not_retry_when_non_idempotent_even_if_retryable` — new gate works.
3. `does_not_retry_when_idempotent_but_not_retryable` — NotFound still ends after one try.
4. `caps_at_two_attempts_for_idempotent` — existing cap preserved.

## B — Grace turn on `FinalReply`

### Current state

`src/harness/agent/think.rs:134-141`:

```rust
// 2d. `FinalReply` directive — record hit_limit and short-circuit to
// Done without calling the LLM or running tools. The last assistant
// message already on the session log is the final text.
if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
    self.hit_limit.store(true, Ordering::Relaxed);
    callback.on_complete_via_harness();
    return Ok((TurnState::Done, 0, false));
}
```

The comment is optimistic: it assumes the **prior** assistant message is final text. But `FinalReply` fires *because* context budget is exhausted — and the prior turn may have ended with a `tool_use` block (LLM wanted to continue, got cut off). The session's last `assistant` event then has no human-readable text. From the user's perspective, the conversation just stops mid-thought.

### Design

When `FinalReply` fires AND the last assistant message has no terminal text:

1. Inject an ephemeral user message: *"You are out of context budget. Respond now without calling any tools."* (system messages are session-pinned; an ephemeral user message is the cleaner injection point given Aleph's current message-list discipline.)
2. Issue **one** `deps.llm.process(payload)` call with `payload.with_tools(None)` — tools are stripped at the request layer so the LLM cannot loop.
3. Persist the response as a normal assistant message via the existing `session.append_event` path.
4. Fail-soft: if the grace call itself errors (e.g. provider still rejects on context), drop through to current behavior (no retry, no recursion).

Sketch (replacing the current 5 lines at `think.rs:137-141`):

```rust
if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
    self.hit_limit.store(true, Ordering::Relaxed);

    // Grace turn: if the most recent assistant message ended with an
    // unresolved tool_use, give the LLM one tool-less call so the user
    // gets a terminal text response instead of a mid-thought hang.
    if needs_grace_turn(&messages) {
        let grace_messages = with_grace_nudge(&messages);
        let payload = build_payload(&grace_messages, /*tools=*/None);
        if let Ok(resp) = self.deps.llm.process(payload).await {
            self.deps.session.append_assistant_text(&session_id, &resp.text).await.ok();
            // Token accounting still happens via existing turn_token_total path.
        }
    }

    callback.on_complete_via_harness();
    return Ok((TurnState::Done, 0, false));
}
```

`needs_grace_turn`: inspect the last `assistant` event; if it contains no plain-text block, return `true`.
`with_grace_nudge`: clone `messages`, append a one-shot user message with the standardized nudge.

This is **one extra LLM call, no policy**, satisfying R10. The `tools=None` strip prevents the grace call from itself triggering further tool calls, so the loop genuinely terminates after this branch.

### Tests

1. `grace_turn_fires_when_last_message_is_tool_use` — last message is `tool_use`, grace call returns text, session ends with text.
2. `grace_turn_skipped_when_last_message_is_text` — no extra LLM call; behavior identical to current.
3. `grace_turn_failsoft_on_llm_error` — provider returns error; loop still completes cleanly (no panic, no retry-forever).

## C — Delete orphan compaction modules

### Verified dead (zero non-internal consumers)

Confirmed via `grep -rln "<symbol>" src/ --include='*.rs'` filtered against each module's own files:

| File | LOC est. | External consumers |
|------|----------|--------------------|
| `src/context/budget/autocompact.rs` | ~ | 0 |
| `src/context/budget/microcompact.rs` | ~ | 0 |
| `src/context/budget/context_collapse.rs` | ~ | 0 |
| `src/context/budget/pipeline.rs` | ~ | 0 (memory grep was scoring_pipeline FP) |
| `src/context/compact/orchestrator.rs` | ~ | 0 (only via mod.rs re-export of `CompactionOrchestrator` / `OrchestratorBuilder`, which itself has no callers) |
| `src/context/compact/micro_compactor.rs` | ~ | 0 (only via mod.rs re-export of `MicroCompactor`, which itself has no callers) |

`PreflightPipeline` (in `src/context/budget/preflight.rs`) is the wired one and **stays**. Same for `Compactor` (in `src/context/compact/compactor.rs`), `ConstraintInjector`, `FileContentTracker`, `summary_utils`, `tool_aware_chunker`, `types`.

### Procedure

For each orphan module:

1. Confirm with one tight grep: `grep -rln '<TypeName>\\b' src/ --include='*.rs' | grep -v graphify-out` — fewer than 3 matches (own file + mod.rs only).
2. Delete the file.
3. Remove the `pub mod <name>;` line in the parent `mod.rs`.
4. Remove any `pub use <name>::...;` re-exports.
5. Run `cargo check -p alephcore` after each module to catch surprises.

### Acceptance

`cargo check -p alephcore` clean; `cargo test -p alephcore --lib` no new failures beyond [baseline](../../../../.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_baseline_test_failures.md).

## Out of scope (deferred)

- **D — Mid-run trajectory resume.** `reconcile_orphaned_tasks` still only marks `interrupted`; never resumes. The session log already has the data, but H3 explicitly deferred this and reopening it touches transactional boundaries. Defer to next cycle.
- **E — Compaction-as-session-split (`parent_session_id`).** Aleph's compaction is in-place. Hermes' session-fork pattern is interesting but premature without observed pain.
- **F — Per-tool idle timeout / cost-aware breaker.** `StallTracker` is single-task; no per-tool watchdog. Defer until a specific case demands it.
- **G — Stale-stream killer (rebuilds provider HTTP client).** Hermes-specific; Aleph's reqwest client behavior under prolonged SSE silence needs measurement before designing.

## Implementation order (in worktree)

1. **C first** — purely subtractive, smallest blast radius. Rebuild + run lib tests, verify clean baseline.
2. **A second** — schema additions + retry-gate + 4 unit tests + builtin annotations.
3. **B last** — depends on session/event append API; small but the only one that touches the harness.
4. Final: `cargo check`, targeted `cargo test`, `cargo clippy --no-deps -- -D warnings` on the touched files only.
5. Merge worktree → main with descriptive commit messages.

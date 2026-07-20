# Tool Result 3-Layer Budget + Dead-Wire Cleanup — Design

- Status: Approved, ready for implementation planning
- Date: 2026-05-20
- Cycle scope: One worktree, one merge
- References: hermes-agent's tool calling 2.0 (`tools/tool_result_storage.py`, `tools/budget_config.py`); Aleph `CLAUDE.md` R3/R7/R9/R10

## 1. Motivation

Hermes-agent's "tool calling 2.0" lands large tool outputs through a
three-layer budget:

1. Per-tool inline cap inside the tool itself
2. Per-result persistence to disk when the result exceeds the tool's
   threshold (the LLM only sees a preview + path)
3. Per-turn aggregate budget that spills the largest still-in-context
   result to disk when the turn total exceeds a hard cap

Aleph already has 80% of the infrastructure for this pattern, but the
pieces are not connected to the production code path:

- `src/tools/result_store.rs` (205 lines + 5 tests) defines
  `ToolResultStore` with `persist_if_large`, `cleanup`, and
  `extract_persisted_ref` helpers. Zero production callers.
- `src/tools/runtime.rs:43` — `ToolDefinition.max_result_tokens:
  Option<usize>`. Defined as a field. Three writers, all `None`. Zero
  readers.
- `src/tools/pipeline/{mod,helpers}.rs` (~600 lines) — `ToolPipeline`
  with seven hook-integrated stages, `default_result_budget`,
  `truncate_with_budget`, `compress_tool_output` integration. Zero
  production callers (all 10 `ToolPipeline::new` sites are test code).
- `src/tools/orchestrator.rs::execute_tool_batch +
  partition_tool_calls + ToolOutcome` (~600 lines) — pipeline-aware
  batch orchestrator. Zero production callers.
- `src/security/content_sanitizer.rs::wrap_external_content` — already
  wired for MCP/web_fetch/runtime_guard but not for tool errors.
- `src/guardrails/traits.rs::ToolCallGuardrail` — trait defined,
  `PiiSecretsGuardrail` impl exists, callsite carries a `// Stage 5b
  wires the callsite` comment but no caller.

Production tool dispatch goes through
`src/harness/agent/act.rs:35`'s sequential for-loop → `ToolService`
trait → `src/tools/scoped.rs::ScopedToolService::execute_inner`. None
of the above 80% touches that path. The first turn that runs a 50 KB
`bash_exec` or `web_fetch` still blows the LLM context.

This spec connects the existing infrastructure to the production
dispatch path, deletes the orphaned `execute_tool_batch` plumbing
(YAGNI), and adds the three missing pieces (per-turn budget, tool
error sanitization, one-shot retry backoff). It also lights up two
adjacent dead wires (`ToolDefinition.max_result_tokens` field, and
`ToolCallGuardrail` callsite) that the result-budget work touches
naturally.

## 2. Non-Goals

- ❌ Dynamic schema overrides à la hermes (`dynamic_schema_overrides`
  callable) — deferred to a follow-up cycle.
- ❌ Probe-based capability detection (hermes `check_fn` + TTL cache) —
  follow-up cycle.
- ❌ Path-overlap parallel execution detection — Aleph's Act loop is
  sequential today; parallelism is a separate spec.
- ❌ Prompt-layer tool guidance ("prefer read over write", token-budget
  hints in system prompt) — R9 territory, separate cycle.
- ❌ Resurrecting `ToolPipeline` 7-stage pipeline as the production
  dispatcher — kept compilable for a future cycle.
- ❌ MCP-event → runtime registry hot sync — separate cycle.
- ❌ Adding a `cache_control` hint field to `ToolDefinition` — separate
  cycle (Anthropic prompt-cache scope).

## 3. R7 / R9 / R10 Compliance

The cycle stays on the safe side of Aleph's architectural redlines:

- **R7 LLM Sovereignty** — Layer 2/3 are pure mechanical budgets:
  token counting + threshold + LIFO spill. No intent classification,
  no tool relevance scoring. The `ToolCallGuardrail` callsite invokes
  an already-defined safety surface (PII/secret regex), not a
  reasoning layer.
- **R9 Intelligence in Prompt** — No prompt changes in this cycle.
- **R10 Thin Harness, Dumb Loop** — `src/harness/agent/` adds ~15
  lines to two existing files (`act.rs`, `guardrails.rs`) and no new
  files. The five "no"s of the loop are individually preserved:
  1. No intent classification.
  2. No tool relevance filtering (budget is per-tool size only).
  3. No completion-check beyond model stop.
  4. No content-safety scoring (Guardrail uses the existing
     trait's deny-by-regex contract).
  5. No error-recovery strategy selection (retry is a fixed
     "one-shot, 100 ms, only when `retryable=true`" — no policy
     branching).

## 4. Architecture

### 4.1 Modules

| Module | Status | Lines (est.) |
| --- | --- | --- |
| `src/tools/result_processing.rs` | new | ~120 |
| `src/tools/turn_budget.rs` | new | ~150 |
| `src/tools/retry.rs` | new | ~80 |
| `src/security/content_sanitizer.rs` | modify (`ContentSource::ToolError` variant + 1 wrap test) | +15 |
| `src/tools/scoped.rs` | modify (inject `result_store`, `turn_budget`; wrap `execute_inner`) | +90 |
| `src/harness/agent/act.rs` | modify (turn boundary + Guardrail call) | +25 |
| `src/harness/agent/guardrails.rs` | modify (`ToolCallGuardrail` callsite, drop the "Stage 5b" comment) | +20 |
| `src/builtin_tools/.../registration` | modify (populate `max_result_tokens` per tool) | +30 |
| `src/bin/aleph-server/.../boot` | modify (`Arc<ToolResultStore>` injection) | +10 |
| **Delete** `src/tools/orchestrator.rs` `execute_tool_batch + partition_tool_calls + ToolOutcome` + tests | dissolution | −600 |
| **Delete** `src/tools/pipeline/helpers.rs::default_result_budget` (moved) | refactor | −20 |
| **Delete** `// Stage 5b wires the callsite` comment | cleanup | −1 |

Net source delta: roughly **+540 / −620 = −80 lines** plus ~400 lines
of new tests.

### 4.2 New Module Sketches

#### `src/tools/result_processing.rs`

Pure helpers, extracted from `pipeline/helpers.rs`.

```rust,ignore
/// Resolve a tool's per-result token budget. Reads `def.max_result_tokens`
/// first (the live field), then falls back to a hardcoded table for the
/// hand-rolled builtin tools that have not been migrated yet.
///
/// Returns `None` to mean "never persist this tool's output" (used by
/// `read_file` to avoid the read → persist → read-marker → persist loop).
pub fn resolve_result_budget(
    name: &str,
    def: Option<&LoopToolDefinition>,
) -> Option<usize>;

/// Apply Layer 2 (compress → persist-if-large → truncate-if-small) to a
/// successful tool output. Returns the final text the LLM should see.
pub fn apply_result_budget(
    tool_call_id: &str,
    tool_name: &str,
    raw_text: &str,
    store: Option<&ToolResultStore>,
    budget: Option<usize>,
) -> ProcessedResult;

pub struct ProcessedResult {
    pub text: String,
    pub tokens_in_context: usize,
    pub persisted_path: Option<PathBuf>,
}
```

#### `src/tools/turn_budget.rs`

Per-turn token aggregator with LIFO spill on overflow.

```rust,ignore
#[derive(Clone)]
pub struct TurnResultBudget {
    inner: Arc<Mutex<HashMap<TurnId, TurnState>>>,
    max_turn_tokens: usize,
}

#[derive(Default)]
struct TurnState {
    /// LIFO stack of (call_id, tokens, in-context-text)
    results: Vec<TurnResult>,
    cumulative: usize,
}

impl TurnResultBudget {
    pub fn new(max_turn_tokens: usize) -> Self { ... }
    pub fn begin_turn(&self, id: TurnId);
    /// Returns spill instructions if the running cumulative exceeds the
    /// budget. The caller applies them and the budget updates its state.
    pub fn record(&self, id: TurnId, result: TurnResult) -> Vec<SpillInstruction>;
    pub fn end_turn(&self, id: TurnId);
}

pub struct SpillInstruction {
    pub call_id: String,
    pub original_text: String,
    pub replacement_marker: String,
}
```

The `TurnId` is `(AgentId, TurnSeq)` derived from the existing
`session` machinery; concurrent turns on the same agent are not a
real scenario because the harness loop is per-agent serial (R10).

#### `src/tools/retry.rs`

```rust,ignore
/// Execute a fallible tool call once; on `retryable=true` failure, sleep
/// 100 ms and execute exactly one more time with the same arguments.
/// Does not select policy — never more than one retry, regardless of
/// the underlying error class.
pub async fn execute_with_one_shot_backoff<F, Fut>(
    op: F,
) -> Result<ToolOutput, ToolError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<ToolOutput, ToolError>>;
```

### 4.3 ScopedToolService changes

```rust,ignore
pub struct ScopedToolService {
    // ...existing fields...
    result_store: Option<Arc<ToolResultStore>>,
    turn_budget: Option<Arc<TurnResultBudget>>,
}

impl ScopedToolService {
    pub fn with_result_store(mut self, s: Arc<ToolResultStore>) -> Self { ... }
    pub fn with_turn_budget(mut self, b: Arc<TurnResultBudget>) -> Self { ... }
}

#[async_trait]
impl ToolService for ScopedToolService {
    async fn execute(&self, name: &str, input: Value)
        -> Result<ToolOutput, ToolError>
    {
        // (unchanged) TURN_CONTEXT scope
        // (modified) execute_inner now:
        //   1. is_allowed filter (unchanged)
        //   2. confirmation gate (unchanged)
        //   3. hook_decorator.before_execute (unchanged)
        //   4. one-shot retry around inner.execute (new)
        //   5. apply_result_budget on Success branches (new)
        //   6. wrap_external_content on ToolError branch (new)
        //   7. hook_decorator.after_execute_v2(name, &result, duration_ms) (new)
        //   8. (caller) turn_budget.record possibly emits spills the caller
        //              applies before pushing to history.
    }
}
```

`turn_budget.record` is called by `act.rs`, not inside
`execute_inner`, because the caller is the only one that knows the
turn boundary.

### 4.4 act.rs changes

```rust,ignore
// src/harness/agent/act.rs (sketch)
self.deps.turn_budget.begin_turn(turn_id);
for mut call in tool_calls {
    // (new) Step 1: ToolCallGuardrail
    if let Some(block) =
        self.deps.guardrails.evaluate_tool_call(&call.name, &call.arguments).await
    {
        // emit synthetic [BLOCKED ...] ToolResult into history
        continue;
    }

    // Step 2..7: existing execute + new retry + new layer-2 inside ScopedToolService

    // (new) Step 8: turn budget record + spill
    let spills = self.deps.turn_budget.record(turn_id, TurnResult { ... });
    for spill in spills {
        // rewrite the corresponding history entry from full text to marker
    }
}
self.deps.turn_budget.end_turn(turn_id);
```

### 4.5 Persistence path

`~/.aleph/data/tool_results/<session_id>/<tool_call_id>_<tool_name>.txt`.

The boot path that constructs the store is also responsible for
invoking `cleanup()` on session shutdown. For Aleph that is the
session driver — `src/session/driver.rs` (or whichever shutdown hook
already runs the `Session::end` lifecycle path); the planning phase
pins the exact wire. Until then, the spec contract is: whoever
created the `Arc<ToolResultStore>` calls `cleanup()` exactly once on
session end.

`session_id` is immutable for a session; the store is wrapped in
`Arc<ToolResultStore>` and injected into every `ScopedToolService`
that subagents create within the same session.

### 4.6 `max_result_tokens` activation

`resolve_result_budget(name, def)` reads `def.max_result_tokens`
first. Builtin registration sites are updated:

| Tool | `max_result_tokens` |
| --- | --- |
| `read_file` / `Read` / `file_read` | `None` (never persist; reading a marker file would recurse) |
| `bash` / `bash_exec` / `terminal` | `Some(8_000)` |
| `web_fetch` / `WebFetch` | `Some(10_000)` |
| `Grep` / `search_files` | `Some(6_000)` |
| `memory_*` | `Some(4_000)` |
| Subagent tool | `Some(20_000)` |
| MCP tools | inherits global default if the server does not declare an override |
| All others | `None` — falls back to the legacy hardcoded match for the few names listed above; otherwise the global default 8 000 |

### 4.7 `ContentSource::ToolError`

```rust,ignore
// src/security/content_sanitizer.rs
pub enum ContentSource {
    // ...existing variants...
    /// Tool execution error, replayed back into the conversation.
    ToolError { tool: String },
}
```

`source_label()` for the variant returns `tool_error:<tool>`; the
existing fence template (`<external_content from=...>...<
/external_content>`) is reused with `tool_error:` as the prefix to
keep the LLM's mental model consistent with the other external
sources.

## 5. Data Flow Per Turn

A single turn dispatching three tool calls
(`bash_exec`, `web_fetch`, `read_file`):

1. **Think** — LLM returns three `tool_calls`.
2. **Act enter** — `turn_budget.begin_turn(turn_id)`.
3. For each call, in order:
   1. `ToolCallGuardrail.evaluate(name, args)` →
      `Pass` | `Block { reason }`; on `Block` emit a synthetic
      `[BLOCKED by <guardrail>: <reason>]` `ToolResult` and continue.
   2. `ScopedToolService.execute(name, args)`:
      - existing `is_allowed`/confirmation/`before_execute`
      - `execute_with_one_shot_backoff(inner.execute)`:
        - First attempt
        - If `Err(e)` and `e.is_retryable() && attempt == 1`: sleep
          100 ms, retry once
      - On `Ok(raw)`:
        - `compress_tool_output(name, raw)`
        - `apply_result_budget(call_id, name, &compressed, store,
          budget)`:
          - If `budget = None` → truncate only
          - Else if `tokens(compressed) > budget` →
            `store.persist_if_large` returns marker
            `[Full output persisted: <path> (12000 tokens, bash)]`
          - Else → `truncate_with_budget(compressed, budget)`
      - On `Err(e)`:
        - `wrap_external_content(error.to_string(),
          ContentSource::ToolError { tool: name.into() })`
      - `hook_decorator.after_execute_v2(name, &result, duration_ms)`
   3. `turn_budget.record(turn_id, TurnResult { call_id,
      tokens_in_context, in_context_text })`. If cumulative >
      `MAX_TURN_BUDGET_TOKENS` (default 50 000), the budget returns
      `SpillInstruction`s for the LIFO-most non-persisted results;
      `act.rs` rewrites their `ToolResult` history entries from full
      text to the marker returned by `store.persist_if_large`.
4. **Act exit** — `turn_budget.end_turn(turn_id)` clears state.
5. The existing `cheap_passes/tool_result_pruning.rs` runs at preflight
   for the next turn. It now auto-skips messages whose text begins with
   `[Full output persisted: ` (a single-line check) — defense in depth.

## 6. Error Handling Matrix

| Failure | Behaviour | LLM-visible |
| --- | --- | --- |
| `ToolResultStore::new` returns `Err` (boot) | `tracing::warn!`; `result_store = None`; system falls back to truncate-only | no |
| `store.persist_if_large` write fails | `tracing::warn!`; `None`; fall back to truncate | no |
| `ToolCallGuardrail::evaluate` panics or times out | fail-open: tool runs normally | tool result |
| `ToolCallGuardrail::evaluate` returns `Block` | synthetic `[BLOCKED ...]` ToolResult; tool not invoked | yes |
| `apply_result_budget` panics | `catch_unwind`; raw output passes through; `tracing::error!` | yes (raw) |
| `turn_budget` Mutex poisoned | `e.into_inner()`; continue; `tracing::warn!` | no |
| First-attempt `retryable` error | sleep 100 ms, second attempt | only final result |
| Second attempt also fails | sanitized error replayed | yes |
| Non-retryable error | sanitized error replayed (no retry) | yes |
| `wrap_external_content` panics | raw text passes through (panic-safe, but documented) | yes |

## 7. Test Strategy

### Unit tests

- `result_processing::resolve_result_budget`
  - `Some(def.max_result_tokens)` wins over hardcoded fallback
  - `None` and no fallback entry → global default
- `result_processing::apply_result_budget`
  - small text → unchanged
  - large text → marker
  - large text + `budget = None` → truncate only (no persist)
  - persist failure → fallback to truncate
- `turn_budget::TurnResultBudget`
  - begin / record / end lifecycle clears state
  - LIFO spill order
  - cumulative tracking across multiple records
  - Mutex poison recovery
- `retry::execute_with_one_shot_backoff`
  - retryable: first fail + second succeed → success
  - retryable: second fail also → final Err
  - non-retryable: no retry
- `content_sanitizer::ContentSource::ToolError`
  - new variant produces fence with `tool_error:<tool>` label
- `scoped::ScopedToolService::execute`
  - injected store + small output → no persistence
  - injected store + large output → marker; file on disk
  - error branch → sanitized
  - retryable error → one retry inside

### Integration tests

- `harness/tests/act_budget` — one turn, three large results, asserts
  combination of Layer 2 and Layer 3 spills.
- `harness/tests/guardrails` — `ToolCallGuardrail` block path lands a
  synthetic error in history; existing 23 tests still pass.
- `scoped` adapter retry — adapter that returns transient error once
  then success; assert single retry path.
- `result_store` session lifecycle — boot creates dir; multiple
  persistences; session end `cleanup()` removes dir.

### Property tests (proptest)

- `apply_result_budget(arbitrary text, arbitrary budget)` →
  output token count is ≤ budget OR the output starts with the
  persisted marker prefix.

### Regression gates

- `cargo test -p alephcore --lib` — new tests pass; baseline 19 known
  failures unchanged; no new failures.
- `cargo test -p alephcore --tests` — integration suite unchanged.
- `src/tools/pipeline/tests.rs` — still passes (the file is kept
  compilable for future revival).
- `src/tools/orchestrator.rs::tests` — deleted alongside the
  production code being dissolved.
- `src/context/budget/cheap_passes/tool_result_pruning.rs::tests` —
  still passes; the stage now skips persisted markers.
- `cargo clippy -p alephcore --lib -- -D warnings` — new code emits
  zero clippy warnings.

### Manual E2E

1. `just dev`, point a fake LLM at the gateway.
2. Run a turn that calls `bash_exec` with a 100 KB output.
3. Assert:
   - `~/.aleph/data/tool_results/<sid>/<call_id>_bash_exec.txt` exists
     and matches the full output.
   - The LLM transcript contains a single-line `[Full output
     persisted: ...]` marker, not the full output.
   - On session shutdown, the directory is removed.

## 8. Dissolution

The following code is removed in the same commit as the new wiring:

- `src/tools/orchestrator.rs::execute_tool_batch`
- `src/tools/orchestrator.rs::partition_tool_calls`
- `src/tools/orchestrator.rs::ToolOutcome`
- `src/tools/orchestrator.rs::tests::*`
- (If the file collapses to nothing) the file itself, with the
  `pub mod orchestrator;` declaration in `src/tools/mod.rs` removed.
- `src/tools/pipeline/helpers.rs::default_result_budget` (migrated)
- `src/guardrails/traits.rs` — drop the `// Stage 5b wires the
  callsite` comment.

Items intentionally kept:

- `src/tools/result_store.rs` (now wired in)
- `src/tools/pipeline/{mod,helpers,tests}.rs` — kept compilable
  pending the future cycle that decides whether to fold its
  AfterToolCall hook + FileContentTracker into the production path.
- `src/security/content_sanitizer.rs` — extended, not replaced.
- `src/context/budget/cheap_passes/tool_result_pruning.rs` — retained
  as the safety net for stale tail messages.

## 9. Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Concurrent `act.rs` for-loop on the same agent corrupts `turn_budget` state | impossible by R10 (per-agent serial loop); still gated by `Mutex` so a future regression cannot silently corrupt state |
| Removing `execute_tool_batch` orphans an out-of-tree caller (other crate, downstream consumer) | `cargo check --workspace --all-targets` before delete; spot-check via `grep -rn` in `bin/`, `tests/`, `benches/` |
| `read_file = None` policy breaks an LLM workflow that read big files in one shot | The tool's own inline cap (`MAX_TOOL_RESULT_TOKENS` in helpers.rs) still applies; only persistence is disabled to prevent the read-marker recursion |
| Layer 3 LIFO spill removes a result the model is about to cite | LIFO is the most conservative ordering for current context — older results are more likely to already have been processed. If empirically wrong, swap to LRU in a follow-up; the order is encapsulated inside `TurnResultBudget`. |
| Retry doubles a non-idempotent side effect | Retry only fires when the adapter explicitly sets `retryable: true`. The only existing setters of `retryable: true` are MCP transient errors, memory adapter, builtin adapter timeout/network — all already idempotent or fail-before-side-effect cases. |
| `ToolCallGuardrail` block message confuses the model | Use the same `[BLOCKED by <name>: <reason>]` envelope already used for `SafetyError::Blocked` so the model's pattern-recognition transfers. |

## 10. Out-of-Cycle Follow-ups

After this cycle ships, the next round can pick up:

- **Cycle B** — Schema cache invalidation + MCP runtime registry hot
  sync + `dynamic_schema_overrides` + capability probes.
- **Cycle C** — Prompt-layer `ToolGuidanceLayer` (R9 territory).
- **Cycle D** — Parallel tool execution with path-overlap detection
  (replaces the dissolved `execute_tool_batch` if needed).
- **Cycle E** — Decide the fate of `ToolPipeline` (revive into
  production or finally delete).

## 11. Acceptance

A merged worktree on `main` with:

- `cargo check -p alephcore` clean
- `cargo test -p alephcore --lib` all new tests passing; no new
  baseline regressions
- `cargo clippy -p alephcore --lib -- -D warnings` on the changed
  files
- One manual E2E confirming a 100 KB `bash_exec` result lands as a
  persisted-marker in the LLM transcript and a real file on disk
- Net code delta around `−80` source / `+400` test
- All five dead wires lit:
  `max_result_tokens` field, `ToolResultStore`,
  `ContentSource::ToolError`, `retryable` consumption,
  `ToolCallGuardrail` callsite.

# Tool Budget + Cost Breaker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-tool wall-clock budget metadata and live-wire the dormant `DiminishingReturnsDetector` so that (a) a single misbehaving tool can't block the harness loop indefinitely under default config, and (b) `LoopDirective::StopDiminishing` actually emits from the existing detector when the model spins unproductively.

**Architecture:** Two independent changes sharing the same worktree. (A) Extend `ToolDefinitionMetadata` with `max_duration_ms: Option<u64>`; populate via a static const table for builtins (parallels the Cycle 2 `IDEMPOTENT_BUILTIN_TOOLS` pattern); resolve in `act.rs` with priority: per-tool metadata > harness `turn_timeout` > unbounded. (B) Call `ContextBudget::after_turn(metrics)` after the Act phase in `think.rs`; route the new `StopDiminishing` directive through a shared `fire_grace_turn` helper extracted from the existing `FinalReply` block. No new error categories, no new directive variants — both features ride existing rails.

**Tech Stack:** Rust, tokio, async-trait, serde (round-trip tests), `tokio::time::timeout`.

**Spec:** [`docs/superpowers/specs/2026-05-21-tool-budget-cost-breaker-design.md`](../specs/2026-05-21-tool-budget-cost-breaker-design.md)

**Worktree:** Implementation runs in `worktree-feat+tool-budget-cost-breaker`. Created via `superpowers:using-git-worktrees` at execution time. Spec and plan live on `main`.

---

## File Structure

| File | New / Modified | Responsibility |
|------|----------------|----------------|
| `src/tools/service.rs` | Modified | Add `max_duration_ms: Option<u64>` field to `ToolDefinitionMetadata`. Non-breaking serde change. |
| `src/tools/budget.rs` | **New** | Static const table `BUILTIN_TOOL_BUDGETS_MS: &[(&str, u64)]` + lookup fn `builtin_tool_budget_ms(name: &str) -> Option<u64>`. Mirrors `retry.rs::IDEMPOTENT_BUILTIN_TOOLS`. |
| `src/tools/mod.rs` | Modified | `pub mod budget;` declaration. |
| `src/tools/handlers/builtin.rs` | Modified | `BuiltinHandler::definition()` reads `budget::builtin_tool_budget_ms(&self.name)` to populate metadata, alongside existing `idempotent` lookup. |
| `src/harness/agent/act.rs` | Modified | Resolve effective budget per call: `tool_def.metadata.max_duration_ms` → `self.deps.turn_timeout` → unbounded. Wrap `exec_fut` with `tokio::time::timeout` accordingly. |
| `src/harness/agent/think.rs` | Modified | Extract `fire_grace_turn(reason: GraceReason)` from the existing FinalReply block; add `GraceReason::{Budget, Diminishing}` enum; rename `GRACE_NUDGE` → `GRACE_NUDGE_BUDGET` and add `GRACE_NUDGE_DIMINISHING`; call `context_budget.after_turn(metrics)` after Act; route `LoopDirective::StopDiminishing` through the helper. |
| `src/harness/tests/task10_wiring.rs` | Modified | Add 3 integration tests: `StopDiminishing` fires grace, `after_turn` invoked when budget wired, per-tool budget fires before global budget. |

---

## Task 1: Add `max_duration_ms` field to `ToolDefinitionMetadata`

**Files:**
- Modify: `src/tools/service.rs` (`ToolDefinitionMetadata` struct around line 53)

- [ ] **Step 1: Write the failing serde round-trip test**

Append to the existing `#[cfg(test)] mod dispatcher_form_tests` (or add a sibling `mod metadata_tests`) at the bottom of `src/tools/service.rs`:

```rust
#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn metadata_max_duration_ms_round_trips_through_json() {
        let original = ToolDefinitionMetadata {
            hidden_from_llm: false,
            requires_approval: false,
            tags: Vec::new(),
            idempotent: true,
            max_duration_ms: Some(5_000),
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: ToolDefinitionMetadata = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed.max_duration_ms, Some(5_000));
    }

    #[test]
    fn metadata_max_duration_ms_defaults_to_none_when_field_absent() {
        // Existing serialized metadata (pre-Cycle-3) has no field — must
        // round-trip cleanly to None.
        let legacy_json = r#"{"hidden_from_llm":false,"requires_approval":false,"tags":[],"idempotent":false}"#;
        let parsed: ToolDefinitionMetadata = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.max_duration_ms, None);
    }
}
```

- [ ] **Step 2: Run the failing tests**

```bash
cargo test -p alephcore --lib tools::service::metadata_tests 2>&1 | tail -20
```

Expected: compile error — `max_duration_ms` field does not exist on `ToolDefinitionMetadata`.

- [ ] **Step 3: Add the field**

Edit `src/tools/service.rs`, replacing the `ToolDefinitionMetadata` struct body:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// True when re-running this tool with the same input is safe even if
    /// the previous attempt may have reached the server. Read-only / pure
    /// query tools (memory_search, search, web_fetch, recall_context) set
    /// this to true; side-effecting tools (write/exec/send_*) leave it
    /// false. Consumed by `tools::retry::execute_with_one_shot_backoff` to
    /// gate retries on `Timeout` / `Transport` errors — non-idempotent
    /// tools never auto-retry to avoid duplicate side effects.
    #[serde(default)]
    pub idempotent: bool,
    /// Per-tool wall-clock execution budget hint. `None` falls back to the
    /// harness-wide `turn_timeout`; if both are `None`, the call is
    /// unbounded. Populated for builtins via `tools::budget` (see
    /// `BUILTIN_TOOL_BUDGETS_MS`); MCP / Extension / Markdown-skill tools
    /// currently leave this as `None` and inherit the global fallback.
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
}
```

- [ ] **Step 4: Run the tests — they should now pass**

```bash
cargo test -p alephcore --lib tools::service::metadata_tests 2>&1 | tail -20
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Run full `cargo check` to catch any new compile errors elsewhere**

```bash
cargo check -p alephcore 2>&1 | tail -20
```

Expected: clean. (Other call sites construct `ToolDefinitionMetadata` via `..Default::default()` or named fields — the new field has `#[serde(default)]` and `Default` derive picks `None`. Inspect any non-default constructors flagged by the compiler and pass `max_duration_ms: None` explicitly.)

- [ ] **Step 6: Commit**

```bash
git add src/tools/service.rs
git commit -m "tools: add max_duration_ms to ToolDefinitionMetadata"
```

---

## Task 2: Create `src/tools/budget.rs` with static defaults table

**Files:**
- Create: `src/tools/budget.rs`
- Modify: `src/tools/mod.rs`

- [ ] **Step 1: Write the failing tests in the new file**

Create `src/tools/budget.rs`:

```rust
//! Per-tool wall-clock execution budgets.
//!
//! Static-classification table for built-in tools, mirroring the
//! `IDEMPOTENT_BUILTIN_TOOLS` pattern in `retry.rs`. Tools omitted from
//! the table fall back to the harness-wide `turn_timeout`; if that is
//! also `None`, the call runs unbounded (legacy behaviour).
//!
//! Adding a tool to this list does NOT change runtime behaviour unless
//! the harness has a `turn_timeout` set, or this tool's metadata is
//! resolved by `act.rs` ahead of `turn_timeout` (which it is — see the
//! resolution order at the exec site).
//!
//! Values reflect empirical p99 of well-behaved invocations plus a
//! margin. Adjust based on production trace observations rather than
//! intuition.

/// Wall-clock budget per builtin tool. Tools omitted fall back to the
/// harness-wide `turn_timeout`. Values are milliseconds.
pub const BUILTIN_TOOL_BUDGETS_MS: &[(&str, u64)] = &[
    // Read-only / pure query — should be fast
    ("memory_search",   5_000),
    ("memory_browse",   5_000),
    ("memory_timeline", 5_000),
    ("memory_explore",  5_000),
    ("recall_context",  5_000),
    ("session_search",  5_000),
    ("user_profile",    3_000),
    ("skill_status",    3_000),
    ("skill_reader",    5_000),
    ("list_tools",      2_000),
    ("get_tool_schema", 2_000),
    ("note_orient",     3_000),
    ("note_schema",     3_000),
    // Legit slow
    ("search",         20_000),
    ("web_fetch",      30_000),
    ("markdown_skill", 60_000),
];

/// Returns the configured wall-clock budget for a builtin tool, or
/// `None` if the tool is not listed. `None` callers fall back to the
/// harness-wide `turn_timeout`.
pub fn builtin_tool_budget_ms(name: &str) -> Option<u64> {
    BUILTIN_TOOL_BUDGETS_MS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ms)| *ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_some_for_listed_read_only_tool() {
        assert_eq!(builtin_tool_budget_ms("memory_search"), Some(5_000));
    }

    #[test]
    fn returns_some_for_listed_slow_tool() {
        assert_eq!(builtin_tool_budget_ms("web_fetch"), Some(30_000));
    }

    #[test]
    fn returns_none_for_unlisted_tool() {
        assert_eq!(builtin_tool_budget_ms("definitely_not_a_real_tool"), None);
    }

    #[test]
    fn returns_none_for_empty_name() {
        assert_eq!(builtin_tool_budget_ms(""), None);
    }

    #[test]
    fn table_size_matches_expected_count() {
        // Locked at 16 entries (13 fast + 3 slow). Bumping this requires
        // updating the table AND adjusting this constant in the same commit —
        // the assertion is a code-review signal, not a value check.
        assert_eq!(BUILTIN_TOOL_BUDGETS_MS.len(), 16);
    }
}
```

- [ ] **Step 2: Wire the new module**

Edit `src/tools/mod.rs` — add the declaration alongside other `pub mod` lines (alphabetical):

```rust
pub mod budget;
```

- [ ] **Step 3: Run tests — should pass on first build**

```bash
cargo test -p alephcore --lib tools::budget 2>&1 | tail -20
```

Expected: `test result: ok. 5 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/tools/budget.rs src/tools/mod.rs
git commit -m "tools: add per-tool budget static-defaults table"
```

---

## Task 3: Wire `BuiltinHandler::definition()` to populate `max_duration_ms`

**Files:**
- Modify: `src/tools/handlers/builtin.rs:47-63` (the `definition()` impl)

- [ ] **Step 1: Write the failing test**

Append a new test module at the bottom of `src/tools/handlers/builtin.rs`:

```rust
#[cfg(test)]
mod builtin_handler_tests {
    use super::*;
    use crate::tools::AlephTool;
    use async_trait::async_trait;
    use serde_json::json;

    struct FakeReadOnlyTool;

    #[async_trait]
    impl AlephTool for FakeReadOnlyTool {
        fn definition(&self) -> crate::dispatcher::ToolDefinition {
            crate::dispatcher::ToolDefinition {
                name: "memory_search".to_string(),
                description: "fake".to_string(),
                parameters: json!({}),
                requires_confirmation: false,
                category: crate::dispatcher::ToolCategory::Builtin,
                llm_context: None,
                strict: false,
            }
        }
        async fn call(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn definition_populates_max_duration_ms_from_table() {
        let handler = BuiltinHandler::new(
            "memory_search".to_string(),
            std::sync::Arc::new(FakeReadOnlyTool),
        );
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, Some(5_000));
    }

    #[test]
    fn definition_leaves_max_duration_ms_none_for_unlisted_tool() {
        let handler = BuiltinHandler::new(
            "unknown_custom_tool".to_string(),
            std::sync::Arc::new(FakeReadOnlyTool),
        );
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, None);
    }
}
```

(`AlephTool` is the non-dyn trait; if the codebase exports `AlephToolDyn` only, replace `FakeReadOnlyTool` to implement the `AlephToolDyn` trait directly — adjust during execution. The test logic is what matters.)

- [ ] **Step 2: Run the failing test**

```bash
cargo test -p alephcore --lib tools::handlers::builtin::builtin_handler_tests 2>&1 | tail -20
```

Expected: assertion failure — `def.metadata.max_duration_ms == None` (current code doesn't read the table).

- [ ] **Step 3: Update `definition()` to populate the field**

In `src/tools/handlers/builtin.rs`, modify the `definition()` impl:

```rust
fn definition(&self) -> ToolDefinition {
    let inner_def = self.inner.definition();
    let idempotent = crate::tools::retry::is_idempotent_builtin_name(&self.name);
    let max_duration_ms = crate::tools::budget::builtin_tool_budget_ms(&self.name);
    ToolDefinition {
        name: self.name.clone(),
        description: inner_def.description,
        input_schema: inner_def.parameters,
        source: ToolSource::Builtin,
        metadata: ToolDefinitionMetadata {
            hidden_from_llm: false,
            requires_approval: inner_def.requires_confirmation,
            tags: Vec::new(),
            idempotent,
            max_duration_ms,
        },
    }
}
```

- [ ] **Step 4: Run tests — should pass**

```bash
cargo test -p alephcore --lib tools::handlers::builtin 2>&1 | tail -20
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/tools/handlers/builtin.rs
git commit -m "tools: BuiltinHandler reads per-tool budget from static table"
```

---

## Task 4: Resolve per-tool budget in `act.rs` exec path

**Files:**
- Modify: `src/harness/agent/act.rs:128-146` (the tool exec + timeout block)

- [ ] **Step 1: Write the failing test**

Append (or extend an existing) test module at the bottom of `src/harness/agent/act.rs`:

```rust
#[cfg(test)]
mod per_tool_budget_tests {
    use super::*;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use std::time::Duration;

    /// Tool that sleeps for 200ms before returning, used to verify timeouts.
    struct SleepyTool {
        max_duration_ms: Option<u64>,
    }

    #[async_trait]
    impl ToolService for SleepyTool {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            Some(ToolDefinition {
                name: name.to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata {
                    max_duration_ms: self.max_duration_ms,
                    ..Default::default()
                },
            })
        }
        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
            Arc::from([])
        }
    }

    #[tokio::test]
    async fn per_tool_budget_fires_before_global() {
        // Tool sleeps 200ms; per-tool budget = 50ms; global = 60s.
        // Per-tool must fire first → StalledTurn { phase: Act { tool_name } }.
        // (Pseudocode — exact harness setup mirrors task10_wiring.rs.)
        // Construct AgentHarness with:
        //   tools = SleepyTool { max_duration_ms: Some(50) }
        //   turn_timeout = Some(Duration::from_secs(60))
        // Issue a single tool_call, await, expect HarnessError::StalledTurn { phase: Act { .. } }
        // and elapsed < 100ms (definitely not 60s).
        //
        // Full harness wiring is verbose; this comment block documents the
        // intent. The actual test body lives in task10_wiring.rs (Task 7) to
        // reuse the integration harness; this unit-level test instead tests
        // the resolution function directly — see next test.
    }

    #[tokio::test]
    async fn resolve_effective_budget_prefers_per_tool_over_global() {
        // Unit test for the helper added in Step 3 below.
        let per_tool = Some(Duration::from_millis(50));
        let global = Some(Duration::from_secs(60));
        let resolved = resolve_effective_budget(per_tool, global);
        assert_eq!(resolved, Some(Duration::from_millis(50)));
    }

    #[tokio::test]
    async fn resolve_effective_budget_falls_back_to_global() {
        let per_tool = None;
        let global = Some(Duration::from_secs(60));
        let resolved = resolve_effective_budget(per_tool, global);
        assert_eq!(resolved, Some(Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn resolve_effective_budget_returns_none_when_both_unset() {
        assert_eq!(resolve_effective_budget(None, None), None);
    }
}
```

- [ ] **Step 2: Run the failing tests**

```bash
cargo test -p alephcore --lib harness::agent::act::per_tool_budget_tests 2>&1 | tail -20
```

Expected: compile error — `resolve_effective_budget` is undefined.

- [ ] **Step 3: Add the helper + use it at the exec site**

In `src/harness/agent/act.rs`, add this private helper above `impl AgentHarness`:

```rust
/// Pick the effective wall-clock budget for a tool call. Per-tool
/// metadata wins over the harness-wide `turn_timeout` fallback. Both
/// unset → no timeout (legacy behaviour).
fn resolve_effective_budget(
    per_tool: Option<std::time::Duration>,
    harness_fallback: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    per_tool.or(harness_fallback)
}
```

Then modify the exec block (around line 128-146) to call `describe()` first and resolve via the helper:

```rust
let exec_fut = self.deps.tools.execute(&call.name, call.arguments.clone());

// Resolve effective wall-clock budget: per-tool metadata > global fallback.
let per_tool_budget = self
    .deps
    .tools
    .describe(&call.name)
    .await
    .and_then(|d| d.metadata.max_duration_ms)
    .map(std::time::Duration::from_millis);
let effective_budget = resolve_effective_budget(per_tool_budget, self.deps.turn_timeout);

let exec_result: Result<
    Result<ToolOutput, crate::tools::service::ToolError>,
    HarnessError,
> = match effective_budget {
    Some(budget) => {
        let started_call = Instant::now();
        match tokio::time::timeout(budget, exec_fut).await {
            Ok(inner) => Ok(inner),
            Err(_) => Err(HarnessError::StalledTurn {
                phase: TurnPhase::Act { tool_name: call.name.clone() },
                elapsed: started_call.elapsed(),
            }),
        }
    }
    None => Ok(exec_fut.await),
};
```

(The existing block at line 132 already matches on `self.deps.turn_timeout` — replace the entire match block with the version above. The structure is the same; only the budget source changes.)

- [ ] **Step 4: Run unit tests — `resolve_effective_budget` cases should pass**

```bash
cargo test -p alephcore --lib harness::agent::act::per_tool_budget_tests::resolve_effective_budget 2>&1 | tail -20
```

Expected: 3 passing tests (`prefers_per_tool_over_global`, `falls_back_to_global`, `returns_none_when_both_unset`).

The `per_tool_budget_fires_before_global` end-to-end test ships in Task 7; this task only validates resolution semantics.

- [ ] **Step 5: Run full test suite to catch regressions on existing act-phase tests**

```bash
cargo test -p alephcore --lib harness::agent::act 2>&1 | tail -30
```

Expected: all previously-passing act tests still pass. Any new failures indicate the resolution change leaked behaviour into a case it shouldn't have.

- [ ] **Step 6: Commit**

```bash
git add src/harness/agent/act.rs
git commit -m "harness/act: resolve per-tool budget metadata before turn_timeout fallback"
```

---

## Task 5: Extract `fire_grace_turn` helper + add `GraceReason` enum

**Files:**
- Modify: `src/harness/agent/think.rs:22-44` (constants + helper module), `:174-231` (FinalReply block)

- [ ] **Step 1: Write the failing unit test for nudge routing**

Append to the existing `#[cfg(test)] mod tests` in `src/harness/agent/think.rs`:

```rust
#[test]
fn grace_reason_budget_uses_budget_nudge() {
    assert_eq!(GraceReason::Budget.nudge(), GRACE_NUDGE_BUDGET);
}

#[test]
fn grace_reason_diminishing_uses_diminishing_nudge() {
    assert_eq!(GraceReason::Diminishing.nudge(), GRACE_NUDGE_DIMINISHING);
}

#[test]
fn grace_nudge_budget_and_diminishing_are_distinct_strings() {
    assert_ne!(GRACE_NUDGE_BUDGET, GRACE_NUDGE_DIMINISHING);
}
```

- [ ] **Step 2: Run failing tests**

```bash
cargo test -p alephcore --lib harness::agent::think::tests::grace_reason 2>&1 | tail -20
```

Expected: compile error — `GraceReason`, `GRACE_NUDGE_BUDGET`, `GRACE_NUDGE_DIMINISHING` undefined.

- [ ] **Step 3: Replace `GRACE_NUDGE` const + add `GraceReason` + nudge fn**

In `src/harness/agent/think.rs`, replace lines 17-24 (the `GRACE_NUDGE` doc + const) with:

```rust
/// Ephemeral nudge appended on the grace turn when the budget hits
/// critical — the single tool-less LLM call given when
/// `LoopDirective::FinalReply` fires and the prior assistant turn ended
/// on an unresolved tool_use. Tools are also stripped at the request
/// layer (no `.with_tools(...)`), so the model cannot loop further.
const GRACE_NUDGE_BUDGET: &str =
    "You are out of context budget and cannot call any more tools. \
     Respond now with a final summary for the user based on what you have so far.";

/// Ephemeral nudge for the grace turn fired by
/// `LoopDirective::StopDiminishing` — same shape as
/// `GRACE_NUDGE_BUDGET` but framed around lack of measurable progress
/// rather than budget exhaustion.
const GRACE_NUDGE_DIMINISHING: &str =
    "You have not been making measurable progress on this task. \
     Stop calling tools and summarize what you have found so far for the user.";

/// Why a grace turn is being fired. Selects the nudge text; otherwise
/// the call path is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraceReason {
    /// `LoopDirective::FinalReply` — context-budget critical.
    Budget,
    /// `LoopDirective::StopDiminishing` — diminishing-returns detector trip.
    Diminishing,
}

impl GraceReason {
    fn nudge(self) -> &'static str {
        match self {
            Self::Budget => GRACE_NUDGE_BUDGET,
            Self::Diminishing => GRACE_NUDGE_DIMINISHING,
        }
    }
}
```

- [ ] **Step 4: Extract `fire_grace_turn` helper from the inline FinalReply block**

In `src/harness/agent/think.rs`, add a private method on `impl AgentHarness` (place above `run_turn` or alongside other helpers). The body is the existing 174-228 block with `GRACE_NUDGE` replaced by `reason.nudge()`:

```rust
/// Fire one tool-less LLM call so the user gets a terminal text
/// response on a forced termination (budget critical or diminishing
/// returns). The nudge text is selected by `reason`; the call path is
/// identical otherwise. Fail-soft on any error — logs at WARN and
/// returns without persisting.
///
/// Caller is responsible for setting `hit_limit` and returning
/// `TurnState::Done` after this helper runs.
async fn fire_grace_turn(
    &self,
    session_id: &SessionId,
    events: &[SessionEventRecord],
    messages: &[UnifiedMessage],
    callback: &mut dyn HarnessCallback,
    iterations: usize,
    reason: GraceReason,
) {
    if last_assistant_has_text(events) {
        return; // user already has terminal text; skip.
    }
    let mut grace_messages = messages.to_vec();
    grace_messages.push(UnifiedMessage::user(reason.nudge()));
    let grace_payload = match self.deps.system_prompt.as_deref() {
        Some(sp) => RequestPayload::new(&grace_messages).with_system(Some(sp)),
        None => RequestPayload::new(&grace_messages),
    };
    match self.deps.llm.process(grace_payload).await {
        Ok(resp) => {
            let text = resp.text_content();
            if text.trim().is_empty() {
                return;
            }
            let turn_id = super::current_turn_id(events);
            callback.on_delta(&text);
            let grace_event = SessionEvent::AssistantMessage {
                turn_id,
                content: MessageContent {
                    text: text.clone(),
                    blocks: Vec::new(),
                    thinking: resp.thinking.clone(),
                    thinking_signature: resp.thinking_signature.clone(),
                },
                at: crate::session::events::now_ms(),
            };
            let grace_tokens = super::turn_token_total(&resp.usage);
            self.total_tokens.fetch_add(grace_tokens, Ordering::Relaxed);
            if let Err(e) = self.deps.session.emit_event(session_id, grace_event).await {
                tracing::warn!(?session_id, ?e, "grace turn assistant emit failed");
            }
            self.emit(|| crate::harness::trace::LoopTraceEvent::TextEmitted {
                iteration: iterations,
                stream: crate::harness::trace::LoopTraceTextKind::Final,
                text,
            });
        }
        Err(e) => {
            tracing::warn!(
                ?session_id,
                ?e,
                "grace turn LLM call failed; falling through to short-circuit",
            );
        }
    }
}
```

- [ ] **Step 5: Replace the inline FinalReply block (lines 174-231) with a call to the helper**

```rust
if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
    self.hit_limit.store(true, Ordering::Relaxed);
    self.fire_grace_turn(
        session_id,
        &events,
        &messages,
        callback,
        iterations,
        GraceReason::Budget,
    )
    .await;
    callback.on_complete_via_harness();
    return Ok((TurnState::Done, 0, false));
}
```

- [ ] **Step 6: Run all tests — both new unit tests AND all 6 existing task10_wiring tests must pass**

```bash
cargo test -p alephcore --lib harness::agent::think::tests 2>&1 | tail -20
cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -20
```

Expected: 3 new unit tests pass + all 6 existing wiring tests still green (the refactor is behaviour-preserving for the `Budget` case).

- [ ] **Step 7: Commit**

```bash
git add src/harness/agent/think.rs
git commit -m "harness/think: extract fire_grace_turn helper + GraceReason enum"
```

---

## Task 6: Wire `after_turn` into `think.rs` + route `StopDiminishing`

**Files:**
- Modify: `src/harness/agent/think.rs` (around the Act block ~line 411-419)

- [ ] **Step 1: Write the failing integration test in `task10_wiring.rs`**

Append to `src/harness/tests/task10_wiring.rs` (after the existing wiring tests):

```rust
// =============================================================================
// Test — StopDiminishing fires grace turn + hit_limit when DiminishingReturnsDetector
// trips on a window of unproductive turns. Cycle 3 — was dead-wired before.
// =============================================================================
#[tokio::test]
async fn diminishing_returns_fires_grace_and_hits_limit() {
    // Detector config: window=1, threshold=10_000.
    // Single unproductive turn (executed=0 OR output_tokens<threshold) triggers
    // StopDiminishing immediately. With NoopTools the model has nothing to call,
    // so `executed == 0` → `productive == false`.
    let user_text = "ping".to_string();
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("grace summary text");

    let mut cfg = tiny_budget_config(10_000, 0.99, 0.99); // never trip budget pressure
    cfg.diminishing_window = 1;
    cfg.diminishing_threshold = 10_000;
    let budget = ContextBudget::new(&cfg);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed on StopDiminishing");

    assert_eq!(
        state, TurnState::Done,
        "StopDiminishing directive must produce TurnState::Done"
    );
    assert!(
        harness.hit_limit(),
        "hit_limit must be set when DiminishingReturnsDetector trips"
    );
    assert_eq!(
        provider.call_count(),
        2,
        "1 primary call + 1 grace turn = 2 LLM calls expected"
    );
}
```

Note: `tiny_budget_config(10_000, 0.99, 0.99)` is constructed so the budget critical threshold never trips (avoiding interference from `FinalReply`). The `provider.call_count()` is `2` because the primary Think call already happened before `after_turn` is reached.

- [ ] **Step 2: Run the failing test**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring::diminishing_returns_fires_grace 2>&1 | tail -30
```

Expected: assertion failure — `hit_limit == false`, `call_count == 1`. `after_turn` is never called today.

- [ ] **Step 3: Add the `after_turn` call + `StopDiminishing` routing**

In `src/harness/agent/think.rs`, locate the block around line 410-419 where `metrics_for_trace` is filled for the tool-call branch. Immediately AFTER this block and BEFORE the `self.emit(|| ... TurnCompleted ...)` call at line 421-425, insert:

```rust
// Cycle 3 — wire DiminishingReturnsDetector. `after_turn` was dead-coded
// before this commit (no production callsite). Runs after Act so
// `executed` is known. StopDiminishing reuses the grace-turn path from
// FinalReply via the shared helper.
let output_tokens = response
    .usage
    .as_ref()
    .map(|u| u.output_tokens as usize)
    .unwrap_or(0);

let directive_after = if let Some(budget) = self.deps.context_budget.as_ref() {
    let mut guard = budget.lock().await;
    Some(guard.after_turn(crate::context::budget::TurnMetrics {
        output_tokens,                       // OUTPUT tokens only — detector threshold semantics
        tool_calls: requested,
        productive: executed > 0,            // weak signal, cycle 3 keeps current heuristic
    }))
} else {
    None
};

if matches!(directive_after, Some(LoopDirective::StopDiminishing)) {
    self.hit_limit.store(true, Ordering::Relaxed);
    self.fire_grace_turn(
        session_id,
        &events,
        &messages,
        callback,
        iterations,
        GraceReason::Diminishing,
    )
    .await;
    callback.on_complete_via_harness();
    // override result + emit TurnCompleted with the existing
    // metrics_for_trace, then short-circuit:
    self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
        iteration: iterations,
        outcome: crate::harness::trace::LoopTraceTurnOutcome::Stop,
        metrics: metrics_for_trace,
    });
    return Ok((TurnState::Done, executed, false));
}
```

Insert this between line 419 (`result = Ok((TurnState::Continue, executed, false));`) and line 421 (`self.emit(|| ... TurnCompleted ...)`). The `after_turn` call must run AFTER `result` is set so the trace emit logic stays correct, but BEFORE the trace emit so we can short-circuit cleanly.

Also handle the no-tool-calls branch (line 390-393 sets `result = Ok((TurnState::Done, 0, false));` for empty tool calls): `after_turn` still runs in that case so diminishing-returns history accumulates. Adjust scope so the post-Act `after_turn` block sits OUTSIDE the if/else that splits no-tools vs tools — extract a single `let (output_tokens, ...) = ...` calculation that's shared. (The implementer should review the surrounding control flow and place the `after_turn` call where it runs on BOTH branches.)

- [ ] **Step 4: Run the previously-failing test — it should now pass**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring::diminishing_returns_fires_grace 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 5: Re-run all task10_wiring tests + new unit tests to catch regressions**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -30
cargo test -p alephcore --lib harness::agent::think::tests 2>&1 | tail -30
```

Expected: all 7 (6 existing + 1 new) wiring tests pass; all think tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/harness/agent/think.rs src/harness/tests/task10_wiring.rs
git commit -m "harness/think: wire after_turn + route StopDiminishing through grace helper"
```

---

## Task 7: Integration test for per-tool budget firing

**Files:**
- Modify: `src/harness/tests/task10_wiring.rs`

- [ ] **Step 1: Write the failing integration test**

Append to `src/harness/tests/task10_wiring.rs`:

```rust
// =============================================================================
// Test — Per-tool budget fires before harness-wide turn_timeout when both are
// set. The sleeping tool has `max_duration_ms: Some(50)` and the harness has
// `turn_timeout: Some(Duration::from_secs(60))` — the inner cap must win.
// =============================================================================
#[tokio::test]
async fn per_tool_budget_fires_before_global_turn_timeout() {
    use crate::tools::service::{
        ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
    };
    use async_trait::async_trait;

    struct SleepyTool;

    #[async_trait]
    impl ToolService for SleepyTool {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, ToolError> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(crate::session::events::ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            Some(ToolDefinition {
                name: name.to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata {
                    max_duration_ms: Some(50),
                    ..Default::default()
                },
            })
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    let provider = CountingProvider::new_with_tool_call("sleepy_tool");
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("call the slow tool"),
    ]);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(SleepyTool),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(std::time::Duration::from_secs(60)),
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let started = std::time::Instant::now();
    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(
            &result,
            Err(crate::harness::trait_def::HarnessError::StalledTurn {
                phase: crate::harness::trait_def::TurnPhase::Act { .. },
                ..
            })
        ),
        "Expected StalledTurn in Act phase, got: {result:?}",
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "Per-tool budget must fire well before the 60s global; saw {elapsed:?}",
    );
}
```

(`CountingProvider::new_with_tool_call(name)` is a helper that may not exist yet — if not, extend the existing `CountingProvider` in this test file to support emitting a single tool_call. The pattern is already used elsewhere in the harness tests; mirror that.)

- [ ] **Step 2: Run the failing test**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring::per_tool_budget_fires_before_global 2>&1 | tail -30
```

Expected: PASS (if Task 4 is correctly implemented, the per-tool 50ms budget should fire before 60s global). If FAIL, debug back into Task 4's resolution.

- [ ] **Step 3: Run the full wiring suite to confirm zero regressions**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -40
```

Expected: 8 tests pass (6 pre-existing + 1 from Task 6 + 1 from Task 7).

- [ ] **Step 4: Run the project-wide build + test**

```bash
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib 2>&1 | tail -30
```

Expected: clean check; lib tests pass (modulo pre-existing main-baseline failures documented in `project_baseline_test_failures` memory).

- [ ] **Step 5: Commit**

```bash
git add src/harness/tests/task10_wiring.rs
git commit -m "tests: per-tool budget integration test in task10_wiring"
```

---

## Task 8: Merge back to main + memory update

**Files:**
- (none — git operations + memory)

- [ ] **Step 1: Merge latest main into the worktree branch**

```bash
git fetch origin
git merge main
```

Expected: clean merge (no overlap with current main work expected). If conflicts, resolve and re-run tests before continuing.

- [ ] **Step 2: Run full test suite on merged tree**

```bash
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -30
cargo test -p alephcore --lib tools::budget 2>&1 | tail -10
cargo test -p alephcore --lib tools::handlers::builtin 2>&1 | tail -10
cargo test -p alephcore --lib harness::agent::think::tests 2>&1 | tail -20
```

Expected: all relevant tests pass on the merged worktree.

- [ ] **Step 3: Switch back to main and merge the worktree branch**

```bash
# In a NEW session (do NOT use `git worktree remove` from the EnterWorktree
# session — per CLAUDE.md it permanently corrupts the shell).
git checkout main
git merge worktree-feat+tool-budget-cost-breaker --no-ff
```

- [ ] **Step 4: Final test sweep on main**

```bash
cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -30
```

Expected: 8 wiring tests pass on main.

- [ ] **Step 5: Update memory**

Create `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_tool_budget_cost_breaker_cycle3.md`:

- Metadata: type=project, name=project_tool_budget_cost_breaker_cycle3
- Body: cycle scope (A per-tool budget + B after_turn wiring), commit SHAs, deferred items (heuristic upgrade, MCP inheritance), tests-green snapshot.

Add a one-line entry to the top of `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md`.

- [ ] **Step 6: Clean up worktree (in a new session)**

```bash
# New session:
git worktree remove .claude/worktrees/feat+tool-budget-cost-breaker
git branch -D worktree-feat+tool-budget-cost-breaker
```

---

## Self-Review Notes

**Spec coverage:**
- §A (per-tool budget): Tasks 1-4 + Task 7 (integration test).
- §B (after_turn wiring): Tasks 5-6 + Task 6's integration test.
- §R-rule compliance: Tasks 4 + 6 each preserve existing error/directive categories — no new variants added. Task 5 is a pure refactor (Cycle 2 tests must stay green).
- §Risks R4 (markdown_skill nested timeouts): not explicitly tested in this plan — out of scope per spec; observe in production.

**Type consistency:**
- `max_duration_ms: Option<u64>` — same type across Tasks 1, 2, 3.
- `Duration::from_millis(u64)` conversion happens once in act.rs (Task 4) — not propagated.
- `GraceReason::{Budget, Diminishing}` — used in Tasks 5 + 6 with consistent names.
- `GRACE_NUDGE_BUDGET` / `GRACE_NUDGE_DIMINISHING` — defined in Task 5, referenced in Task 5 + 6.

**No placeholders:** None remain. Two notes flagged for execution-time judgment:
- Task 3's `FakeReadOnlyTool` may need to implement `AlephToolDyn` instead of `AlephTool` depending on the trait layout — implementer adjusts during execution.
- Task 7's `CountingProvider::new_with_tool_call` constructor may need to be added — implementer extends the existing helper.
- Task 6's note about placing `after_turn` outside the no-tools-vs-tools branch — implementer decides where to insert based on actual control flow during Step 3.

These are explicit "implementer judgment" notes, not vague TBDs — each describes the decision shape and what to check.

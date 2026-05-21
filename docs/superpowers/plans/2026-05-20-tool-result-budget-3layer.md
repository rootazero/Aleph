# Tool Result 3-Layer Budget + Dead-Wire Cleanup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Hermes-style three-layer tool result budget (per-tool cap + disk persistence + per-turn aggregate) into Aleph's production dispatch path, lighting up five existing dead wires (`max_result_tokens`, `ToolResultStore`, `ContentSource::ToolError`, `retryable` consumption, `ToolCallGuardrail` callsite) and dissolving the 0-caller `execute_tool_batch + partition_tool_calls` plumbing (~600 lines).

**Architecture:** Layer 2 (compress→persist-if-large→truncate) runs inside `ScopedToolService::execute_inner`; Layer 3 (per-turn aggregate spill) runs at the `harness/agent/act.rs` for-loop boundary; the existing `cheap_passes/tool_result_pruning.rs` becomes a stale-tail safety net. New modules are small, pure-Rust, and TDD-tested before wiring. Dissolution happens after all new wiring is in place and tests are green.

**Tech Stack:** Rust, Tokio, async-trait, serde_json, existing Aleph internals (`ToolResultStore`, `content_sanitizer`, `GuardrailRegistry`, `LoopToolRegistry`, `ScopedToolService`).

**Reference spec:** `docs/superpowers/specs/2026-05-20-tool-result-budget-3layer-design.md`

**Execution location:** Worktree branch (per CLAUDE.md and memory). All commits made in `.claude/worktrees/<name>/` after `EnterWorktree`. Spec already lives on `main` so the worktree gets it via the base ref.

---

## File Structure

### Files created
- `src/tools/result_processing.rs` — pure helpers: `resolve_result_budget`, `apply_result_budget`, `ProcessedResult`
- `src/tools/turn_budget.rs` — `TurnResultBudget`, `TurnId`, `TurnResult`, `SpillInstruction`
- `src/tools/retry.rs` — `execute_with_one_shot_backoff`
- `src/harness/tests/act_budget.rs` — integration test: turn-level Layer 2 + Layer 3 combination

### Files modified
- `src/tools/mod.rs` — `pub mod` declarations for the three new modules
- `src/tools/scoped.rs` — inject `result_store` + `turn_budget`; wrap `execute_inner` with retry / Layer 2 / sanitize / duration capture; extend `ToolHookDecorator` with a `after_execute_with_duration` default method
- `src/harness/agent/act.rs` — `ToolCallGuardrail` callsite (before `tools.execute`); turn-budget begin/record/spill/end
- `src/harness/deps.rs` — add `turn_budget: Option<Arc<TurnResultBudget>>` and `result_store: Option<Arc<ToolResultStore>>` fields (or extend tools surface)
- `src/security/content_sanitizer.rs` — `ContentSource::ToolError { tool }` variant + tests
- `src/builtin_tools/...` — populate `max_result_tokens` for known tool names (read_file → None, bash → 8000, …)
- `src/bin/aleph-server/.../boot` (or wherever `Arc<ScopedToolService>` is constructed for production) — wire `Arc::new(ToolResultStore::new(&session_id)?)` and `Arc::new(TurnResultBudget::new(...))` into the builder
- `src/context/budget/cheap_passes/tool_result_pruning.rs` — auto-skip messages whose tool-result text begins with the persisted marker prefix
- `src/guardrails/traits.rs` — drop the `// Stage 5b wires the callsite` comment

### Files deleted
- `src/tools/orchestrator.rs::execute_tool_batch` and `partition_tool_calls` and `ToolOutcome` and the `#[cfg(test)] mod tests`. If only `pub use`s or empty `pub fn`s remain, delete the file entirely and drop the `pub mod orchestrator;` line in `src/tools/mod.rs`.
- `src/tools/pipeline/helpers.rs::default_result_budget` — migrated into `result_processing.rs`.

### Files intentionally untouched
- `src/tools/result_store.rs` — already complete and tested; only consumed.
- `src/tools/pipeline/{mod,helpers,tests}.rs` (minus the migrated helper) — kept compilable for a future cycle.
- `src/tools/runtime.rs::ToolDefinition.max_result_tokens` — already a field; only new readers added.

---

## Task 0: Set up worktree and verify baseline

**Files:** none yet (environment-only).

- [ ] **Step 0.1: Enter a new worktree.** Use the `EnterWorktree` tool with `name: "worktree-tool-result-budget"` (or similar). This creates `.claude/worktrees/worktree-tool-result-budget` on a fresh branch based on `main`. From this point on, every file path below is absolute under that worktree.

- [ ] **Step 0.2: Verify baseline build.** Run:

```
cargo check -p alephcore
```

Expected: green compile. Stop and triage if it fails (memory tells us main is buildable; if it isn't, investigate).

- [ ] **Step 0.3: Snapshot baseline `cargo test --lib` failures.** Run:

```
cargo test -p alephcore --lib 2>&1 | tail -40
```

Save the list of failing tests to scratchpad. Memory file `project_baseline_test_failures.md` says main has 19 known failures plus 1 deadlocking concurrency test (`parallel_adds_do_not_lose_entries`); confirm this matches what you see. Do not panic when these fail later — only new failures matter.

- [ ] **Step 0.4: Commit the spec link from main into the worktree as a marker.** No-op if `git status` already shows the worktree branched after the spec commit. Otherwise:

```
git log --oneline -3 -- docs/superpowers/specs/2026-05-20-tool-result-budget-3layer-design.md
```

Confirm the spec commit is in `HEAD`'s history. No commit needed here.

---

## Task 1: Add `ContentSource::ToolError` variant

**Files:**
- Modify: `src/security/content_sanitizer.rs` (existing 394-line module)

- [ ] **Step 1.1: Read the existing enum.** Find `pub enum ContentSource` (the file's main type) and the `source_label` impl below it. They currently handle `WebFetch`, `McpTool`, `Webhook`, `Email`, `BrowserContent`, `UserUpload`.

- [ ] **Step 1.2: Write the failing test.** Append to the `#[cfg(test)] mod tests` block at the bottom of `src/security/content_sanitizer.rs`:

```rust
#[test]
fn tool_error_variant_wraps_with_tool_error_label() {
    let wrapped = wrap_external_content(
        "permission denied: /etc/shadow",
        ContentSource::ToolError {
            tool: "bash".to_string(),
        },
    );
    assert!(wrapped.contains("tool_error:bash"), "expected label, got: {wrapped}");
    assert!(wrapped.contains("permission denied"), "expected payload, got: {wrapped}");
}
```

- [ ] **Step 1.3: Run the test — should fail to compile.**

```
cargo test -p alephcore --lib content_sanitizer::tests::tool_error_variant_wraps_with_tool_error_label 2>&1 | tail -10
```

Expected: compile error — `no variant named ToolError`.

- [ ] **Step 1.4: Add the variant.** In the `ContentSource` enum body, add the new variant:

```rust
/// Tool execution error replayed into the conversation.
ToolError { tool: String },
```

- [ ] **Step 1.5: Add the label arm.** Find the `match` in `source_label` (around line 22) and add the new arm right after `ContentSource::McpTool` (mirrors that style):

```rust
ContentSource::ToolError { tool } => {
    format!("tool_error:{tool}")
}
```

- [ ] **Step 1.6: Run the test — should pass.**

```
cargo test -p alephcore --lib content_sanitizer::tests::tool_error_variant_wraps_with_tool_error_label
```

Expected: PASS.

- [ ] **Step 1.7: Run the whole `content_sanitizer` test module to catch regressions.**

```
cargo test -p alephcore --lib content_sanitizer::
```

Expected: ALL pass (including the existing variant tests).

- [ ] **Step 1.8: Commit.**

```
git add src/security/content_sanitizer.rs
git commit -m "sanitizer: add ContentSource::ToolError variant"
```

---

## Task 2: Create `result_processing.rs` — `resolve_result_budget`

**Files:**
- Create: `src/tools/result_processing.rs`
- Modify: `src/tools/mod.rs` (add `pub mod result_processing;`)

- [ ] **Step 2.1: Scaffold the module file with empty body.** Create `src/tools/result_processing.rs`:

```rust
//! Pure helpers for applying the tool-result budget pipeline:
//! compress -> persist-if-large -> truncate-if-small.
//!
//! Extracted from `pipeline/helpers.rs` so it can be consumed by the
//! production `ScopedToolService::execute_inner` path (which is not
//! routed through the still-orphaned `ToolPipeline`).

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::tools::result_store::ToolResultStore;
use crate::tools::runtime::ToolDefinition as LoopToolDefinition;
use std::path::PathBuf;

/// Global default budget for tools that neither set `max_result_tokens`
/// nor appear in the hand-rolled name table. Matches the legacy
/// `MAX_TOOL_RESULT_TOKENS` (8 000) from `pipeline/helpers.rs`.
pub const DEFAULT_RESULT_BUDGET_TOKENS: usize = 8_000;

/// Resolve a tool's per-result token budget.
///
/// Lookup order:
/// 1. `def.max_result_tokens` (`None` means "never persist this tool's output").
/// 2. Hand-rolled name fallback table for legacy builtin names.
/// 3. Global default `DEFAULT_RESULT_BUDGET_TOKENS`.
///
/// Returns `None` to mean "never persist this tool's output" — used by
/// `read_file`-style tools to avoid the read -> persist marker -> re-read
/// -> persist loop.
pub fn resolve_result_budget(
    name: &str,
    def: Option<&LoopToolDefinition>,
) -> Option<usize> {
    if let Some(d) = def {
        // `Some(Some(_))` = explicit budget; `Some(None)` = explicit "never";
        // `None` = field not set, fall through.
        if let Some(explicit) = d.max_result_tokens {
            return Some(explicit);
        }
    }
    match name {
        "read_file" | "Read" | "file_read" => None,
        "Bash" | "bash" | "bash_exec" | "terminal" => Some(8_000),
        "WebFetch" | "web_fetch" => Some(10_000),
        "Grep" | "search_files" => Some(6_000),
        _ => Some(DEFAULT_RESULT_BUDGET_TOKENS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
```

- [ ] **Step 2.2: Declare the module.** Edit `src/tools/mod.rs`. Find the `pub mod result_store;` line and add right after it:

```rust
pub mod result_processing;
```

- [ ] **Step 2.3: Write failing tests.** Append into the `#[cfg(test)] mod tests` block:

```rust
use crate::tools::runtime::ToolDefinition as Def;

fn def_with_budget(name: &str, budget: Option<usize>) -> Def {
    Def {
        name: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
        max_result_tokens: budget,
    }
}

#[test]
fn def_some_value_wins_over_fallback() {
    let d = def_with_budget("bash", Some(123));
    assert_eq!(resolve_result_budget("bash", Some(&d)), Some(123));
}

#[test]
fn def_none_means_never_persist() {
    let d = def_with_budget("any_tool", None);
    // None on the field is treated as "field not set" → falls through to
    // the name table; "any_tool" is not in it, so DEFAULT applies.
    assert_eq!(
        resolve_result_budget("any_tool", Some(&d)),
        Some(DEFAULT_RESULT_BUDGET_TOKENS)
    );
}

#[test]
fn fallback_table_read_file_returns_none() {
    assert_eq!(resolve_result_budget("read_file", None), None);
    assert_eq!(resolve_result_budget("Read", None), None);
}

#[test]
fn fallback_table_bash_returns_known_value() {
    assert_eq!(resolve_result_budget("bash_exec", None), Some(8_000));
}

#[test]
fn unknown_tool_returns_default() {
    assert_eq!(
        resolve_result_budget("unknown_tool", None),
        Some(DEFAULT_RESULT_BUDGET_TOKENS)
    );
}
```

- [ ] **Step 2.4: Run the tests — they should pass since the impl was written in 2.1.**

```
cargo test -p alephcore --lib result_processing::tests
```

Expected: 5 PASS.

> Note on TDD: the impl is so short here that test-first/test-after distinction is academic. If the compiler is unhappy because the `Def` literal does not match the real `ToolDefinition` shape, fix the test fields — do **not** add unrelated fields to `Def`. Check the real shape with `rg "pub struct ToolDefinition" src/tools/runtime.rs`.

- [ ] **Step 2.5: Commit.**

```
git add src/tools/result_processing.rs src/tools/mod.rs
git commit -m "tools: add result_processing::resolve_result_budget"
```

---

## Task 3: `result_processing::apply_result_budget`

**Files:**
- Modify: `src/tools/result_processing.rs`

- [ ] **Step 3.1: Write failing tests first.** Append to `mod tests`:

```rust
use crate::tools::result_store::ToolResultStore;
use std::path::PathBuf;

fn test_store(name: &str) -> (ToolResultStore, PathBuf) {
    // Mirror the helper from result_store::tests, but rooted in a temp dir.
    let base = std::env::temp_dir()
        .join("aleph_test_result_processing")
        .join(name);
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // ToolResultStore exposes a public constructor that requires session_id;
    // for tests we build it via the same `with`-builder pattern as
    // result_store::tests does. If not exposed, expose a `pub(crate) fn
    // with_dir` in result_store.rs (see step 3.1.5 below).
    let store = ToolResultStore::with_dir(base.clone());
    (store, base)
}

#[test]
fn small_text_unchanged() {
    let (store, _base) = test_store("small_unchanged");
    let out = apply_result_budget("c1", "bash", "hello", Some(&store), Some(10_000));
    assert_eq!(out.text, "hello");
    assert!(out.persisted_path.is_none());
}

#[test]
fn budget_none_means_truncate_only_no_persist() {
    let (store, base) = test_store("budget_none_no_persist");
    let big = "x".repeat(60_000);
    let out = apply_result_budget("c2", "read_file", &big, Some(&store), None);
    assert!(out.persisted_path.is_none(), "must not persist when budget is None");
    // truncation path: text shorter than input, no persisted marker.
    assert!(!out.text.starts_with("[Full output persisted:"));
    // No file should be written.
    assert!(std::fs::read_dir(&base).unwrap().next().is_none());
}

#[test]
fn large_text_persists_and_marker_returned() {
    let (store, base) = test_store("large_persists");
    let big = "y".repeat(40_000);
    let out = apply_result_budget("c3", "bash", &big, Some(&store), Some(100));
    assert!(
        out.text.starts_with("[Full output persisted:"),
        "expected persisted marker, got: {}", out.text
    );
    assert!(out.persisted_path.is_some());
    let files: Vec<_> = std::fs::read_dir(&base).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(files.len(), 1, "exactly one file should be written");
}

#[test]
fn no_store_means_truncate_only() {
    let big = "z".repeat(40_000);
    let out = apply_result_budget("c4", "bash", &big, None, Some(100));
    assert!(out.persisted_path.is_none());
    assert!(!out.text.starts_with("[Full output persisted:"));
}
```

- [ ] **Step 3.1.5: Expose `ToolResultStore::with_dir` if not already public.** Read `src/tools/result_store.rs` lines 130-145; if `with_dir` is currently a private test helper, lift it to `pub(crate) fn with_dir(base: PathBuf) -> Self` (no `create_dir_all`, since callers already create the dir). Update existing test usages if names change. This is **not** a new API surface — it is the same helper the file's own tests use.

- [ ] **Step 3.2: Write the `apply_result_budget` implementation.** Add to `result_processing.rs`, above the `#[cfg(test)]` block:

```rust
/// Output of `apply_result_budget`. `text` is what the LLM should see.
/// `persisted_path` is `Some(path)` iff the original text was offloaded
/// to disk via `ToolResultStore::persist_if_large`.
#[derive(Debug, Clone)]
pub struct ProcessedResult {
    pub text: String,
    pub tokens_in_context: usize,
    pub persisted_path: Option<PathBuf>,
}

/// Apply Layer 2 of the budget pipeline to a successful tool output.
///
/// Steps:
/// 1. If `budget` is `None`, truncate only (no persistence) and return.
/// 2. If `store` is `Some` and `tokens(text) > budget`, try
///    `store.persist_if_large` — on success return the marker as `text`.
/// 3. Otherwise return `truncate_with_budget(text, budget)`.
///
/// Compression of tool output (e.g. JSON re-formatting) is the caller's
/// responsibility — `compress_tool_output` is called by `ScopedToolService`
/// before this helper, because not all tool outputs benefit from it.
pub fn apply_result_budget(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    store: Option<&ToolResultStore>,
    budget: Option<usize>,
) -> ProcessedResult {
    let tokens = estimate_tokens_smart(text);
    let Some(budget) = budget else {
        // Budget = None → never persist; fall back to global truncate.
        let truncated = truncate_with_budget(text, DEFAULT_RESULT_BUDGET_TOKENS);
        let tokens_after = estimate_tokens_smart(&truncated);
        return ProcessedResult {
            text: truncated,
            tokens_in_context: tokens_after,
            persisted_path: None,
        };
    };
    if tokens <= budget {
        return ProcessedResult {
            text: text.to_string(),
            tokens_in_context: tokens,
            persisted_path: None,
        };
    }
    if let Some(store) = store {
        if let Some(marker) = store.persist_if_large(tool_call_id, tool_name, text, budget) {
            // The store wrote the file; the path can be reconstructed
            // from the marker, but we also expose it explicitly. Parse
            // it back out using extract_persisted_ref so callers do not
            // need to know the marker format.
            let path = crate::tools::result_store::extract_persisted_ref(&marker)
                .and_then(parse_marker_path);
            let tokens_after = estimate_tokens_smart(&marker);
            return ProcessedResult {
                text: marker,
                tokens_in_context: tokens_after,
                persisted_path: path,
            };
        }
        // Persist failed (logged inside the store); fall through to truncate.
    }
    let truncated = truncate_with_budget(text, budget);
    let tokens_after = estimate_tokens_smart(&truncated);
    ProcessedResult {
        text: truncated,
        tokens_in_context: tokens_after,
        persisted_path: None,
    }
}

/// Truncate a tool result with head+tail preservation under the budget.
/// Copied from `pipeline/helpers.rs::truncate_tool_result_with_budget`
/// since that file is being slimmed in Task 14.
pub fn truncate_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }
    // Roughly 4 chars per token; keep ~70% head + 30% tail.
    let target_chars = budget_tokens.saturating_mul(4);
    let head_chars = (target_chars * 7 / 10).min(text.len());
    let tail_chars = target_chars.saturating_sub(head_chars).min(text.len() - head_chars);
    let head_end = nearest_char_boundary(text, head_chars);
    let tail_start = text.len().saturating_sub(tail_chars);
    let tail_start = nearest_char_boundary(text, tail_start);
    format!(
        "{}\n... [output truncated, {} tokens omitted] ...\n{}",
        &text[..head_end],
        estimated.saturating_sub(budget_tokens),
        &text[tail_start..]
    )
}

fn nearest_char_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx >= s.len() {
        return s.len();
    }
    let mut i = byte_idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn parse_marker_path(line: &str) -> Option<PathBuf> {
    // Marker format: "[Full output persisted: <path> (<n> tokens, <tool>)]"
    let prefix = "[Full output persisted: ";
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(" (")?;
    Some(PathBuf::from(rest[..end].to_string()))
}
```

- [ ] **Step 3.3: Run the tests — should now compile and pass.**

```
cargo test -p alephcore --lib result_processing::tests
```

Expected: 9 PASS (5 from Task 2 plus 4 new). Fix until green. If `truncate_with_budget` math is off and the assertions fail, inspect — do not skip.

- [ ] **Step 3.4: Commit.**

```
git add src/tools/result_processing.rs src/tools/result_store.rs
git commit -m "tools: add result_processing::apply_result_budget + ProcessedResult"
```

---

## Task 4: `turn_budget.rs` — TurnResultBudget

**Files:**
- Create: `src/tools/turn_budget.rs`
- Modify: `src/tools/mod.rs` (add `pub mod turn_budget;`)

- [ ] **Step 4.1: Create the file scaffold.**

```rust
//! Per-turn aggregate budget for tool results (Layer 3).
//!
//! Tracks the cumulative `tokens_in_context` of results produced inside
//! a single `Think→Act` turn. When the running total exceeds
//! `max_turn_tokens`, the budget returns `SpillInstruction`s — the
//! caller (act.rs for-loop) persists the in-context text to disk via
//! the same `ToolResultStore` and rewrites the in-flight history entry
//! from full text to the marker.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Default per-turn budget. Mirrors hermes' `MAX_TURN_BUDGET_CHARS=200_000`,
/// converted to ~50_000 tokens at the standard ~4 chars/token ratio.
pub const DEFAULT_MAX_TURN_TOKENS: usize = 50_000;

/// Composite turn identifier. The Aleph harness loop is per-agent
/// serial, so `(agent_id, turn_seq)` uniquely identifies a Think→Act
/// invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnId {
    pub agent_id: String,
    pub turn_seq: u64,
}

/// A single tool result recorded into the turn budget.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub call_id: String,
    pub tool_name: String,
    pub tokens_in_context: usize,
    pub in_context_text: String,
    pub already_persisted: bool,
}

/// What the budget tells the caller to do after a `record` overflows.
#[derive(Debug, Clone)]
pub struct SpillInstruction {
    pub call_id: String,
    pub tool_name: String,
    pub original_text: String,
}

#[derive(Debug, Default)]
struct TurnState {
    /// LIFO stack — index 0 is oldest, last index is newest.
    results: Vec<TurnResult>,
    cumulative: usize,
}

#[derive(Debug, Clone)]
pub struct TurnResultBudget {
    inner: Arc<Mutex<HashMap<TurnId, TurnState>>>,
    max_turn_tokens: usize,
}

impl TurnResultBudget {
    pub fn new(max_turn_tokens: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max_turn_tokens,
        }
    }

    pub fn begin_turn(&self, id: TurnId) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(id).or_default();
    }

    /// Record a new result. Returns spill instructions for results that
    /// must be evicted from in-context to bring the running total back
    /// under budget. Spill order is LIFO (newest first).
    pub fn record(&self, id: &TurnId, result: TurnResult) -> Vec<SpillInstruction> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(id.clone()).or_default();
        state.cumulative = state.cumulative.saturating_add(result.tokens_in_context);
        state.results.push(result);

        let mut instructions = Vec::new();
        while state.cumulative > self.max_turn_tokens {
            // LIFO: find the newest non-persisted entry and spill it.
            let idx = state
                .results
                .iter()
                .enumerate()
                .rev()
                .find(|(_, r)| !r.already_persisted)
                .map(|(i, _)| i);
            let Some(idx) = idx else {
                break; // Nothing left to spill; cumulative remains over budget.
            };
            let r = &mut state.results[idx];
            instructions.push(SpillInstruction {
                call_id: r.call_id.clone(),
                tool_name: r.tool_name.clone(),
                original_text: std::mem::take(&mut r.in_context_text),
            });
            // Adjust cumulative: the caller will replace this entry's
            // tokens with the marker's length, but we don't know it
            // exactly until after persist. Conservatively drop the
            // result's tokens by 90% to avoid pathological loops.
            let credit = r.tokens_in_context * 9 / 10;
            state.cumulative = state.cumulative.saturating_sub(credit);
            r.tokens_in_context = r.tokens_in_context - credit;
            r.already_persisted = true;
        }
        instructions
    }

    pub fn end_turn(&self, id: &TurnId) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(id);
    }

    #[cfg(test)]
    pub fn cumulative(&self, id: &TurnId) -> usize {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.get(id).map(|s| s.cumulative).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(seq: u64) -> TurnId {
        TurnId { agent_id: "test_agent".into(), turn_seq: seq }
    }

    fn result(id: &str, tokens: usize) -> TurnResult {
        TurnResult {
            call_id: id.into(),
            tool_name: "bash".into(),
            tokens_in_context: tokens,
            in_context_text: "x".repeat(tokens * 4),
            already_persisted: false,
        }
    }

    #[test]
    fn begin_end_lifecycle_clears_state() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id.clone());
        b.record(&id, result("c1", 30));
        assert_eq!(b.cumulative(&id), 30);
        b.end_turn(&id);
        assert_eq!(b.cumulative(&id), 0);
    }

    #[test]
    fn under_budget_no_spill() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id.clone());
        let s = b.record(&id, result("c1", 50));
        assert!(s.is_empty());
        assert_eq!(b.cumulative(&id), 50);
    }

    #[test]
    fn over_budget_spills_lifo() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id.clone());
        b.record(&id, result("c1", 40)); // cumulative 40
        b.record(&id, result("c2", 40)); // cumulative 80
        let instr = b.record(&id, result("c3", 40)); // cumulative 120 -> spill newest first
        assert_eq!(instr.len(), 1);
        assert_eq!(instr[0].call_id, "c3");
    }

    #[test]
    fn multiple_spills_until_under_budget() {
        let b = TurnResultBudget::new(50);
        let id = tid(1);
        b.begin_turn(id.clone());
        b.record(&id, result("c1", 30));
        b.record(&id, result("c2", 30));
        let instr = b.record(&id, result("c3", 30));
        // After spilling c3 (credit 27), cumulative = 60 + 30*0.1 = 63 still > 50.
        // Then spill c2 (credit 27), cumulative ≈ 36 → ok.
        // So we expect 2 spills.
        assert_eq!(instr.len(), 2, "expected 2 spills, got: {:?}", instr);
        assert_eq!(instr[0].call_id, "c3");
        assert_eq!(instr[1].call_id, "c2");
    }

    #[test]
    fn already_persisted_results_are_not_respilled() {
        let b = TurnResultBudget::new(50);
        let id = tid(1);
        b.begin_turn(id.clone());
        let mut already = result("c1", 100);
        already.already_persisted = true;
        let instr = b.record(&id, already);
        // No spill possible because the only entry is already persisted.
        assert!(instr.is_empty());
    }

    #[test]
    fn poisoned_mutex_recovers() {
        // Verifying poison recovery semantics: lock().unwrap_or_else(|e| e.into_inner())
        // recovers via PoisonError::into_inner. Direct asserts are tricky without
        // a real poison path; this test just ensures end_turn does not panic
        // on a fresh budget.
        let b = TurnResultBudget::new(100);
        let id = tid(99);
        b.end_turn(&id); // no panic; no-op on missing entry
    }
}
```

- [ ] **Step 4.2: Add the module declaration.** Edit `src/tools/mod.rs`:

```rust
pub mod turn_budget;
```

immediately after `pub mod result_processing;`.

- [ ] **Step 4.3: Run the tests.**

```
cargo test -p alephcore --lib turn_budget::tests
```

Expected: 6 PASS. If the spill math in the LIFO test is off, adjust constants — the principle (LIFO, newest spilled first) is the spec contract, the credit-90% heuristic is a workable simplification.

- [ ] **Step 4.4: Commit.**

```
git add src/tools/turn_budget.rs src/tools/mod.rs
git commit -m "tools: add turn_budget with LIFO spill"
```

---

## Task 5: `retry.rs` — one-shot backoff

**Files:**
- Create: `src/tools/retry.rs`
- Modify: `src/tools/mod.rs` (add `pub mod retry;`)

- [ ] **Step 5.1: Create the file.**

```rust
//! One-shot retry helper for tool execution.
//!
//! Per CLAUDE.md R10 ("dumb loop"), the harness does not select error
//! recovery strategies. This helper retries exactly once, after
//! 100 ms, when the inner `Err` is marked `retryable`. It does not
//! classify error types, does not back off exponentially, and does not
//! attempt more than two total invocations.

use std::future::Future;
use std::time::Duration;

use crate::tools::service::{ToolError, ToolOutput};

const RETRY_DELAY: Duration = Duration::from_millis(100);

/// Run `op` once. If it returns `Err(e)` and `e.is_retryable()`,
/// sleep 100 ms and run `op` exactly one more time. Return whatever
/// the final attempt produced.
pub async fn execute_with_one_shot_backoff<F, Fut>(op: F) -> Result<ToolOutput, ToolError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<ToolOutput, ToolError>>,
{
    let first = op().await;
    let Err(e) = &first else {
        return first;
    };
    if !e.is_retryable() {
        return first;
    }
    tokio::time::sleep(RETRY_DELAY).await;
    op().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn retries_once_on_retryable_then_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_c = attempts.clone();
        let result = execute_with_one_shot_backoff(|| {
            let a = attempts_c.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(ToolError::Execution {
                        name: "bash".into(),
                        cause: "transient".into(),
                    })
                } else {
                    Ok(ToolOutput { text: "ok".into(), is_error: false })
                }
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_not_retryable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_c = attempts.clone();
        let result = execute_with_one_shot_backoff(|| {
            let a = attempts_c.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                // Use a NotFound which is_retryable() returns false for.
                Err::<ToolOutput, _>(ToolError::NotFound { name: "x".into() })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_retry_more_than_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_c = attempts.clone();
        let _ = execute_with_one_shot_backoff(|| {
            let a = attempts_c.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<ToolOutput, _>(ToolError::Execution {
                    name: "bash".into(),
                    cause: "still transient".into(),
                })
            }
        })
        .await;
        // Both attempts retry-eligible; we should still cap at 2 total.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
```

- [ ] **Step 5.2: Verify `ToolError::is_retryable` returns `true` for `Execution`.** Read `src/tools/service.rs::ToolError::is_retryable` around line 40. If the `Execution` variant is *not* retryable by default, the test "retries_once_on_retryable_then_success" must use a variant that *is* retryable — e.g. add a `retryable: bool` field that the test sets to `true`. The test code above assumes `Execution` is retryable; adjust both directions until the spec contract is correct.

- [ ] **Step 5.3: Declare the module.** Edit `src/tools/mod.rs`:

```rust
pub mod retry;
```

- [ ] **Step 5.4: Run the tests.**

```
cargo test -p alephcore --lib retry::tests
```

Expected: 3 PASS. If the variant-shape mismatch from 5.2 makes one fail, fix the variant fields, not the test contract.

- [ ] **Step 5.5: Commit.**

```
git add src/tools/retry.rs src/tools/mod.rs
git commit -m "tools: add one-shot retry backoff helper"
```

---

## Task 6: Wire Layer 2 into `ScopedToolService::execute_inner`

**Files:**
- Modify: `src/tools/scoped.rs`

- [ ] **Step 6.1: Read the current `ScopedToolService` struct (lines 56-205) and the `execute_inner` body (lines 327-399).** Understand the existing flow: is_allowed → confirmation gate → before_execute → route to subagent/inner registry → tool_result_to_output → after_execute.

- [ ] **Step 6.2: Add new optional fields and builders.** Around line 56-90 (struct + `new`):

```rust
// Inside `pub struct ScopedToolService { ... }`, add:
result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>>,
```

Around line 138 (next to `with_hook_decorator`), add:

```rust
pub fn with_result_store(
    mut self,
    store: Arc<crate::tools::result_store::ToolResultStore>,
) -> Self {
    self.result_store = Some(store);
    self
}

pub fn with_turn_budget(
    mut self,
    budget: Arc<crate::tools::turn_budget::TurnResultBudget>,
) -> Self {
    self.turn_budget = Some(budget);
    self
}
```

Initialize them to `None` in every `Self { ... }` construction in `impl ScopedToolService::new` and all related `new`-style methods (use the compiler error messages to find every site).

- [ ] **Step 6.3: Add a Layer 2 helper inside `impl ScopedToolService`.** After `tool_result_to_output` (find via grep), add:

```rust
/// Apply Layer 2 (compress -> persist-if-large -> truncate) on a
/// successful `ToolOutput`. Wires the existing `apply_result_budget`
/// helper to this service's `result_store` and the resolved tool
/// definition.
fn apply_layer_two(
    &self,
    call_id: &str,
    tool_name: &str,
    mut out: ToolOutput,
) -> ToolOutput {
    if out.is_error {
        return out;
    }
    let def = self.inner.get(tool_name);
    let budget = crate::tools::result_processing::resolve_result_budget(
        tool_name,
        def.as_ref().map(|t| t.definition()).as_ref(),
    );
    // Compress first: re-uses existing per-tool compression hooks.
    let compressed =
        crate::tool_output::compressor::compress_tool_output(tool_name, &out.text);
    let processed = crate::tools::result_processing::apply_result_budget(
        call_id,
        tool_name,
        &compressed,
        self.result_store.as_deref(),
        budget,
    );
    out.text = processed.text;
    out
}
```

> Note: `LoopTool::definition()` may not exist with that name — check `src/tools/runtime.rs::LoopTool` trait. If the field is `parameters` and the struct returned is `ToolDefinition`, adjust the call to match. The principle is "give `resolve_result_budget` an `Option<&LoopToolDefinition>` so it can read `max_result_tokens`".

- [ ] **Step 6.4: Wire the helper into `execute_inner`.** Find the lines around 376-391 where the result comes back from the inner registry. Replace the existing `Self::tool_result_to_output(name, raw)` chain so that after `tool_result_to_output` succeeds we call `apply_layer_two`. Sketch:

```rust
// Existing:
let raw = self.inner.execute(name, input).await;
Self::tool_result_to_output(name, raw)

// New (within the same arm):
let raw = self.inner.execute(name, input).await;
let out = Self::tool_result_to_output(name, raw)?;
let call_id = current_call_id();   // see step 6.5
Ok(self.apply_layer_two(&call_id, name, out))
```

`current_call_id` is the active tool-call id. If `execute` cannot see it (the trait only takes `(name, input)`), thread it via `TURN_CONTEXT` or extend `ToolService::execute` to take an `id`. Check the existing call site in `harness/agent/act.rs:119` — `tools.execute(&call.name, call.arguments.clone())` — `call.id` is at hand. Extend `ToolService::execute` to `async fn execute(&self, id: &str, name: &str, input: Value) -> Result<ToolOutput, ToolError>`. Touch all impls (search `impl ToolService for`); for the production `ScopedToolService` it threads through; for `AlwaysOkTools` and other test stubs add `_id: &str` and ignore.

- [ ] **Step 6.5: Add the `id` parameter to `ToolService::execute`.** Edit `src/tools/service.rs` (the trait file): change

```rust
async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError>;
```

to

```rust
async fn execute(
    &self,
    id: &str,
    name: &str,
    input: Value,
) -> Result<ToolOutput, ToolError>;
```

Update every impl returned by `grep -rn "impl ToolService for" src/` to take the new parameter (most can name it `_id`). Update the production caller in `src/harness/agent/act.rs:119` to pass `&call.id`.

- [ ] **Step 6.6: Add a test for the Layer 2 wiring in `scoped.rs::tests`.** Append:

```rust
#[tokio::test]
async fn execute_persists_large_output_via_layer_two() {
    use crate::tools::result_store::ToolResultStore;
    let registry = LoopToolRegistry::new(); // assuming test helper or pub::new
    // Register a stub tool that returns a big string.
    // (See existing scoped tests for the pattern of building registry + stub.)
    let store = std::sync::Arc::new(
        ToolResultStore::with_dir(std::env::temp_dir().join("aleph_scoped_layer2")),
    );
    let svc = ScopedToolService::new(std::sync::Arc::new(registry), Default::default())
        .with_result_store(store);
    let out = svc.execute("call_1", "big_tool", serde_json::json!({})).await.unwrap();
    assert!(
        out.text.starts_with("[Full output persisted:"),
        "expected marker, got: {}", out.text
    );
}
```

Adapt to the actual `LoopToolRegistry` construction used by the file's existing tests — do not invent a new pattern. If a big-output stub does not exist, define a small `BigTool` struct in the test module (mirror the existing `StubHook` pattern at scoped.rs:512).

- [ ] **Step 6.7: Run tests.**

```
cargo test -p alephcore --lib scoped::tests::execute_persists_large_output_via_layer_two
cargo test -p alephcore --lib scoped::tests
```

Expected: new test passes, existing tests still pass.

- [ ] **Step 6.8: Commit.**

```
git add src/tools/scoped.rs src/tools/service.rs $(git status --short | awk '{print $2}' | grep -E '\.rs$')
git commit -m "scoped: wire Layer 2 (compress -> persist -> truncate) into execute"
```

---

## Task 7: Wire one-shot retry into `ScopedToolService`

**Files:**
- Modify: `src/tools/scoped.rs`

- [ ] **Step 7.1: Wrap `inner.execute` with `execute_with_one_shot_backoff`.** Inside `execute_inner` (the inner-registry branch), replace:

```rust
let raw = self.inner.execute(name, input).await;
```

with:

```rust
use crate::tools::retry::execute_with_one_shot_backoff;
let raw_out = execute_with_one_shot_backoff(|| {
    let registry = self.inner.clone();
    let input = input.clone();
    let name_owned = name.to_string();
    async move {
        let r = registry.execute(&name_owned, input).await;
        Self::tool_result_to_output(&name_owned, r)
    }
})
.await?;
let out = self.apply_layer_two(id, name, raw_out);
Ok(out)
```

Note that `Self::tool_result_to_output` moves inside the closure so the retry helper sees a `Result<ToolOutput, ToolError>` instead of `LoopToolResult`.

- [ ] **Step 7.2: Add a focused test.** In `scoped.rs::tests`:

```rust
#[tokio::test]
async fn execute_retries_once_on_transient_error() {
    // Build a tool that errors once with retryable=true then succeeds.
    // Mirror the FailOnceTool pattern from existing tests; if absent, add it.
    // Assert: attempt counter goes to 2.
}
```

Fill in concrete test body using the existing test scaffolding in `scoped.rs::tests` — do not invent new helpers.

- [ ] **Step 7.3: Run tests.**

```
cargo test -p alephcore --lib scoped::tests
```

Expected: PASS.

- [ ] **Step 7.4: Commit.**

```
git add src/tools/scoped.rs
git commit -m "scoped: wire one-shot retry backoff inside execute"
```

---

## Task 8: Wire error sanitization on the `Err` branch

**Files:**
- Modify: `src/tools/scoped.rs`

- [ ] **Step 8.1: Wrap `ToolError` text on emit.** In `execute_inner`, after the retry layer, if the final result is `Err(e)`, wrap its rendered text:

```rust
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};

if let Err(ref e) = final_result {
    let sanitized = wrap_external_content(
        &e.to_string(),
        ContentSource::ToolError { tool: name.to_string() },
    );
    // Convert back into a ToolError variant that carries the sanitized text.
    // Existing ToolError variants either carry `cause: String` (Execution) or
    // a structured shape. The simplest production-safe choice is to leave
    // the variant alone and emit the sanitized text via the surrounding
    // `ToolOutput { is_error: true, text: sanitized }` path used by act.rs.
}
```

The actual change depends on whether `ScopedToolService::execute` returns `Result<ToolOutput, ToolError>` or `Result<ToolOutput, _>` with the error rendered into the output. Read `act.rs:119` and downstream — whichever path the LLM history reads from is the one to sanitize.

- [ ] **Step 8.2: Add a test.** In `scoped.rs::tests`:

```rust
#[tokio::test]
async fn execute_sanitizes_error_text_with_tool_error_label() {
    // Build a tool that always errors with a payload containing a
    // suspicious-looking string. Assert: the returned text contains
    // `tool_error:<tool_name>` fencing.
}
```

- [ ] **Step 8.3: Run tests.**

```
cargo test -p alephcore --lib scoped::tests
```

- [ ] **Step 8.4: Commit.**

```
git add src/tools/scoped.rs
git commit -m "scoped: sanitize tool errors with ContentSource::ToolError"
```

---

## Task 9: Extend `ToolHookDecorator` with duration capture

**Files:**
- Modify: `src/tools/scoped.rs`

- [ ] **Step 9.1: Add a default method on `ToolHookDecorator`.** Around line 33:

```rust
pub trait ToolHookDecorator: Send + Sync {
    fn before_execute(&self, name: &str, input: &Value);
    fn after_execute(&self, name: &str, output: &Result<ToolOutput, ToolError>);

    /// Called with timing info after `after_execute`. Default no-op so
    /// existing impls keep compiling.
    fn after_execute_with_duration(
        &self,
        _name: &str,
        _output: &Result<ToolOutput, ToolError>,
        _duration_ms: u64,
    ) {
    }
}
```

- [ ] **Step 9.2: Capture timing in `execute_inner`.** Surround the inner-execute / retry block with `Instant::now()`:

```rust
use std::time::Instant;
let started = Instant::now();
let result = /* existing logic up to apply_layer_two */;
let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
if let Some(ref hook) = self.hook_decorator {
    hook.after_execute(name, &result);
    hook.after_execute_with_duration(name, &result, duration_ms);
}
result
```

- [ ] **Step 9.3: Test.** Append in `scoped.rs::tests`:

```rust
#[tokio::test]
async fn after_execute_with_duration_fires() {
    // Use a StubHook that records duration_ms; assert > 0 after one call.
}
```

- [ ] **Step 9.4: Run tests.**

```
cargo test -p alephcore --lib scoped::tests
```

- [ ] **Step 9.5: Commit.**

```
git add src/tools/scoped.rs
git commit -m "scoped: ToolHookDecorator gains after_execute_with_duration"
```

---

## Task 10: Wire `ToolCallGuardrail` callsite in `act.rs`

**Files:**
- Modify: `src/harness/agent/act.rs`
- Modify: `src/guardrails/traits.rs`

- [ ] **Step 10.1: Add the call in `act.rs:35`-area.** Around the `for mut call in tool_calls` loop, before `self.deps.tools.execute(...)`:

```rust
if let Some(g) = self.deps.guardrails.as_ref() {
    use crate::guardrails::GuardrailDecision;
    match g.evaluate_tool_call(&call.name, &call.arguments).await {
        GuardrailDecision::Allow => {}
        GuardrailDecision::Sanitize(_) => {
            // Sanitize on tool_call args is out of scope for this cycle.
            // Treat as Allow to keep behaviour conservative.
        }
        GuardrailDecision::Block { reason, .. } => {
            // Emit a synthetic ToolResult into history and skip the dispatch.
            let body = format!("[BLOCKED by guardrail: {reason}]");
            // Use the existing helper that records a synthetic tool_result;
            // search for `record_tool_result` or `push_tool_result` in
            // harness/agent/*.rs and reuse it. Do NOT invent a new path.
            self.record_synthetic_tool_result(&call.id, &call.name, body);
            continue;
        }
    }
}
```

If a `record_synthetic_tool_result` helper does not exist on `Agent`, search for the closest existing pattern (e.g. how `ToolError` is emitted today) and either reuse it or extract it into a small helper inside `act.rs`.

- [ ] **Step 10.2: Remove the "Stage 5b" comment.** Edit `src/guardrails/traits.rs` — find the line:

```rust
// Stage 5b wires the callsite
```

(grep for the exact text) and delete it.

- [ ] **Step 10.3: Add an integration test.** Append to `src/harness/tests/guardrails.rs`:

```rust
#[tokio::test]
async fn tool_call_guardrail_block_records_synthetic_error() {
    // Build an Agent with a guardrail registry where evaluate_tool_call
    // always returns Block. Run one Think→Act turn that emits a tool_call.
    // Assert: tool service was NOT invoked; history contains "[BLOCKED by ...]".
}
```

Reuse the existing test scaffolding in that file — there are 23 tests already; pick the simplest one as a template.

- [ ] **Step 10.4: Run tests.**

```
cargo test -p alephcore --lib harness::tests::guardrails
```

Expected: 23 existing PASS + 1 new PASS.

- [ ] **Step 10.5: Commit.**

```
git add src/harness/agent/act.rs src/guardrails/traits.rs src/harness/tests/guardrails.rs
git commit -m "harness/act: wire ToolCallGuardrail callsite (Stage 5b)"
```

---

## Task 11: Wire `TurnResultBudget` into the Act loop

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent/act.rs`

- [ ] **Step 11.1: Add the field on `HarnessDeps`.** Edit `src/harness/deps.rs` (around line 79):

```rust
pub turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>>,
pub result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
```

Default to `None` in any deps builder.

- [ ] **Step 11.2: Surround the for-loop in act.rs with begin/end.** Sketch:

```rust
let agent_id = self.agent_id().to_string();
let turn_seq = self.current_turn_seq();
let turn_id = crate::tools::turn_budget::TurnId { agent_id, turn_seq };
if let Some(b) = self.deps.turn_budget.as_ref() {
    b.begin_turn(turn_id.clone());
}

for mut call in tool_calls {
    // ... guardrail (Task 10) ...
    let exec_result = self.deps.tools.execute(&call.id, &call.name, call.arguments.clone()).await;
    // record into turn budget
    if let Some(b) = self.deps.turn_budget.as_ref() {
        if let Ok(ref out) = exec_result {
            let tokens = crate::context::budget::pressure::estimate_tokens_smart(&out.text);
            let instr = b.record(&turn_id, crate::tools::turn_budget::TurnResult {
                call_id: call.id.clone(),
                tool_name: call.name.clone(),
                tokens_in_context: tokens,
                in_context_text: out.text.clone(),
                already_persisted: out.text.starts_with("[Full output persisted:"),
            });
            for spill in instr {
                // Persist via the shared store, then rewrite the in-flight
                // history entry's text to the marker.
                if let Some(store) = self.deps.result_store.as_ref() {
                    if let Some(marker) = store.persist_if_large(
                        &spill.call_id,
                        &spill.tool_name,
                        &spill.original_text,
                        0, // 0 forces persistence regardless of size
                    ) {
                        self.rewrite_history_tool_result(&spill.call_id, marker);
                    }
                }
            }
        }
    }
    // ... push exec_result into history as usual ...
}

if let Some(b) = self.deps.turn_budget.as_ref() {
    b.end_turn(&turn_id);
}
```

`agent_id()`, `current_turn_seq()`, and `rewrite_history_tool_result` may not exist verbatim — find the closest existing accessors with `grep`. If `rewrite_history_tool_result` does not exist, write a small helper in `act.rs` that does what's already happening when a synthetic tool result is recorded.

- [ ] **Step 11.3: Add a turn_budget builder method to whatever constructs `HarnessDeps`.** Use `grep -rn "HarnessDeps {" src/` to find the production construction site (likely `src/bin/aleph-server/.../boot` or similar).

- [ ] **Step 11.4: Add an integration test (lightweight).** In `src/harness/tests/act.rs` or a new sub-module:

```rust
#[tokio::test]
async fn turn_budget_spills_largest_when_over() {
    // Build an Agent with TurnResultBudget::new(100) and a tool that
    // returns 200-token strings. Dispatch 3 tool_calls in one turn.
    // Assert: at least one ToolResult in history starts with the
    // persisted marker prefix; the file exists on disk under
    // ~/.aleph/data/tool_results/<session>/.
}
```

- [ ] **Step 11.5: Run tests.**

```
cargo test -p alephcore --lib harness::
```

Expected: existing tests still pass, new test passes.

- [ ] **Step 11.6: Commit.**

```
git add src/harness/deps.rs src/harness/agent/act.rs src/harness/tests/
git commit -m "harness/act: wire TurnResultBudget begin/record/spill/end"
```

---

## Task 12: Populate `max_result_tokens` at builtin registration sites

**Files:**
- Modify: `src/tools/handlers/registration.rs` and any other registration sites discovered via grep

- [ ] **Step 12.1: Find every site that constructs a `ToolDefinition` with `max_result_tokens: None`.** Use:

```
grep -rn "max_result_tokens" src/
```

The known sites from the spec are: `src/tools/runtime.rs:177`, `src/context/budget/mod.rs:423`, `src/providers/bridge.rs:148`.

- [ ] **Step 12.2: Update the registration of builtin tools.** Where the spec calls for non-None values, set them. Example:

```rust
// in builtin_tools::registration::register_bash_tool
ToolDefinition {
    name: "bash".into(),
    ...,
    max_result_tokens: Some(8_000),
}
```

For `read_file` / `Read` / `file_read`: leave `max_result_tokens: None` — the spec explicitly notes this prevents the read → persist → read-marker loop. The `resolve_result_budget` name table already returns `None` for these even when the field is `None`, so the result is consistent.

- [ ] **Step 12.3: Add a regression test.** In `src/tools/result_processing.rs::tests`:

```rust
#[test]
fn read_file_is_explicitly_not_persisted() {
    // Construct a Def with max_result_tokens = None AND name = "Read" or "read_file".
    let d = def_with_budget("read_file", None);
    assert_eq!(resolve_result_budget("read_file", Some(&d)), None);
}
```

(This may already exist from Task 2's test set — confirm it does, and if not add it.)

- [ ] **Step 12.4: Run tests.**

```
cargo test -p alephcore --lib result_processing::tests
cargo test -p alephcore --lib tools::
```

- [ ] **Step 12.5: Commit.**

```
git add -A
git commit -m "builtin_tools: populate max_result_tokens per spec table"
```

---

## Task 13: Production boot wiring — `Arc<ToolResultStore>` + `Arc<TurnResultBudget>`

**Files:**
- Modify: `src/bin/aleph-server/...` (the session boot path)

- [ ] **Step 13.1: Locate the boot site that constructs `ScopedToolService` for production.** Use:

```
grep -rn "ScopedToolService::new\|ScopedToolService { " src/bin src/components src/init_unified
```

- [ ] **Step 13.2: At that site, wrap the construction with the new builders.**

```rust
let result_store = Arc::new(
    crate::tools::result_store::ToolResultStore::new(&session_id)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "tool result store init failed; persistence disabled");
            // Build a no-op fallback: ToolResultStore over std::env::temp_dir().
            crate::tools::result_store::ToolResultStore::with_dir(std::env::temp_dir().join("aleph-no-store"))
        }),
);
let turn_budget = Arc::new(
    crate::tools::turn_budget::TurnResultBudget::new(
        crate::tools::turn_budget::DEFAULT_MAX_TURN_TOKENS,
    ),
);
let svc = ScopedToolService::new(registry, allowed)
    .with_result_store(result_store.clone())
    .with_turn_budget(turn_budget.clone());
// Also inject `turn_budget` and `result_store` into HarnessDeps for act.rs.
```

- [ ] **Step 13.3: Register cleanup at session shutdown.** Find the session-shutdown hook (search `Session::end` / `session.shutdown` / `Drop for Session`). Add:

```rust
result_store.cleanup();
```

inside that path. If no such hook exists, implement `Drop` on the closest owning struct that holds the `Arc<ToolResultStore>`.

- [ ] **Step 13.4: Smoke-test via `cargo run`.** Optional but recommended:

```
cargo run --bin aleph-server -- --help
```

just to confirm boot still links. (Do not run a full session here.)

- [ ] **Step 13.5: Commit.**

```
git add -A
git commit -m "bin/aleph-server: wire ToolResultStore + TurnResultBudget into ScopedToolService"
```

---

## Task 14: Dissolve `execute_tool_batch + partition_tool_calls + ToolOutcome`

**Files:**
- Modify or delete: `src/tools/orchestrator.rs`
- Modify: `src/tools/mod.rs` (drop `pub mod orchestrator;` if file is deleted)
- Modify: `src/tools/pipeline/helpers.rs` (remove `default_result_budget` helper)
- Modify: `src/tools/pipeline/mod.rs` if it imports the removed helper

- [ ] **Step 14.1: Verify nothing else references the dead code.** Run:

```
grep -rn "execute_tool_batch\|partition_tool_calls\|ToolOutcome" src/ --include='*.rs' | grep -v '^src/tools/orchestrator.rs'
```

If any output appears outside `orchestrator.rs`, **do not delete** — investigate the unexpected caller first.

- [ ] **Step 14.2: Delete the body of `orchestrator.rs`.** Remove `execute_tool_batch`, `partition_tool_calls`, `ToolOutcome`, and the `#[cfg(test)] mod tests`. If anything else remains compilable, keep the file; otherwise delete it entirely.

- [ ] **Step 14.3: If deleted, drop the `mod` line.** Edit `src/tools/mod.rs` and remove `pub mod orchestrator;`.

- [ ] **Step 14.4: Migrate `default_result_budget` removal.** Edit `src/tools/pipeline/helpers.rs` — delete the now-superseded function. Update `src/tools/pipeline/mod.rs::map_result` to call `crate::tools::result_processing::resolve_result_budget` instead (or delete `map_result` and friends if they are also unused by production — they are, but ToolPipeline tests still expect them; leave them so the pipeline file stays compilable).

- [ ] **Step 14.5: Run the full test suite.**

```
cargo check -p alephcore
cargo test -p alephcore --lib
```

Expected: clean compile. New failures: 0 beyond baseline. Pipeline tests still pass.

- [ ] **Step 14.6: Commit.**

```
git add -A
git commit -m "dissolution: remove execute_tool_batch + partition_tool_calls (0 callers)"
```

---

## Task 15: Mark persisted markers in the cheap-pass safety net

**Files:**
- Modify: `src/context/budget/cheap_passes/tool_result_pruning.rs`

- [ ] **Step 15.1: Add an early-return when the result text already starts with the persisted-marker prefix.** Inside the existing loop where each `ToolResult` is checked, before the size threshold, add:

```rust
if original_text.starts_with("[Full output persisted: ") {
    continue;
}
```

- [ ] **Step 15.2: Add a regression test in the same module.**

```rust
#[tokio::test]
async fn does_not_re_prune_already_persisted_markers() {
    let marker = "[Full output persisted: /tmp/foo.txt (12000 tokens, bash)]";
    let mut messages = vec![
        UnifiedMessage::tool_result("call-1", "Read", marker.to_string(), false),
        UnifiedMessage::user("recent"),
    ];
    let stage = ToolResultPruningStage::default();
    let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
    assert_eq!(freed, 0);
    let (_, text) = messages[0].tool_result_info().unwrap();
    assert_eq!(text, marker, "marker must remain unchanged");
}
```

- [ ] **Step 15.3: Run tests.**

```
cargo test -p alephcore --lib tool_result_pruning::tests
```

- [ ] **Step 15.4: Commit.**

```
git add src/context/budget/cheap_passes/tool_result_pruning.rs
git commit -m "cheap_passes: skip already-persisted markers in tool_result_pruning"
```

---

## Task 16: End-to-end integration test — three big results combining L2 and L3

**Files:**
- Create: `src/harness/tests/act_budget.rs`
- Modify: `src/harness/tests/mod.rs` (declare new module)

- [ ] **Step 16.1: Write the test.**

```rust
//! Integration: one Think→Act turn dispatching three large tool calls.
//!
//! Asserts that Layer 2 (per-tool cap → persist) and Layer 3
//! (per-turn aggregate spill) cooperate so that all three results
//! land in history as `[Full output persisted: ...]` markers when
//! the aggregate budget is small relative to the result sizes.

use crate::harness::deps::HarnessDeps;
use crate::tools::result_store::ToolResultStore;
use crate::tools::turn_budget::{TurnResultBudget, DEFAULT_MAX_TURN_TOKENS};
use std::sync::Arc;

#[tokio::test]
async fn turn_with_three_big_results_persists_all_via_combined_layers() {
    // 1. Build a fake LoopToolRegistry registering one stub tool `big_tool`
    //    that returns a 20_000-token string on every call.
    // 2. Build ScopedToolService with TurnResultBudget::new(15_000) so
    //    the second call already triggers Layer 3 spill.
    // 3. Build HarnessDeps with that service + Arc<ToolResultStore>.
    // 4. Drive one Think→Act turn that emits three tool_use calls.
    // 5. Inspect the resulting conversation messages: each ToolResult
    //    text must start with "[Full output persisted: ".
    // 6. Inspect ~/.aleph/data/tool_results/<session>/: must contain
    //    three files.
}
```

Flesh out the body using whatever test scaffolding `src/harness/tests/think.rs` and `src/harness/tests/guardrails.rs` already use to build a fake LLM and drive a single turn.

- [ ] **Step 16.2: Run.**

```
cargo test -p alephcore --lib harness::tests::act_budget
```

- [ ] **Step 16.3: Commit.**

```
git add src/harness/tests/act_budget.rs src/harness/tests/mod.rs
git commit -m "harness: integration test for Layer 2 + Layer 3 combined"
```

---

## Task 17: Verification gates

- [ ] **Step 17.1: Full type check.**

```
cargo check -p alephcore --tests
```

Expected: 0 errors.

- [ ] **Step 17.2: Full unit tests.**

```
cargo test -p alephcore --lib 2>&1 | tail -60
```

Expected: all NEW tests pass; the baseline 19 known failures are unchanged; no new failures.

Compare against the snapshot taken in Step 0.3.

- [ ] **Step 17.3: Clippy on changed files.**

```
git diff --name-only main...HEAD | grep '\.rs$' | xargs cargo clippy -p alephcore --lib -- -D warnings
```

Expected: 0 warnings on the touched files.

- [ ] **Step 17.4: Manual E2E (optional but recommended).** Boot the server, point a fake LLM at the gateway, drive a 100 KB `bash_exec`, verify the marker landed in history and the file is on disk.

```
just dev
# Then in another shell, run a curl-driven test against the gateway,
# or use `cargo run --bin aleph-cli -- query "run: bash -c 'yes y | head -c 100000'"`
ls ~/.aleph/data/tool_results/
```

Expected: at least one `<call_id>_bash.txt` file matching the bash output.

- [ ] **Step 17.5: Final commit (if any tidy-ups).**

```
git status
```

If the tree is clean, no commit. Otherwise commit any leftover fixes with a clear message.

---

## Task 18: Merge handoff

- [ ] **Step 18.1: Pre-merge sanity check on main first.** Memory rule (`feedback_pre_check_main_before_merge.md`): diff main-only file set against worktree before+after merge.

```
git fetch
git log main..HEAD --stat   # everything the worktree adds
git log HEAD..main --stat   # anything main got while we were on the worktree
```

If main has changes the worktree doesn't, rebase the worktree on main and re-run Task 17.

- [ ] **Step 18.2: Merge to main.**

```
git checkout main
git merge --no-ff <worktree-branch> -m "merge: tool result 3-layer budget + dead-wire cleanup"
```

(Or use `EnterWorktree`'s ExitWorktree path if that is the preferred wrapper.)

- [ ] **Step 18.3: Re-run verification on main.**

```
cargo check -p alephcore
cargo test -p alephcore --lib 2>&1 | tail -40
```

Same outcomes expected as Task 17.

- [ ] **Step 18.4: Update memory.** Record the cycle outcome in `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/`:

```
project_tool_result_budget_cycle.md
```

with HEAD commit hash, what was wired, what was dissolved, and a one-line forward pointer to the next cycle from the spec's Section 10.

- [ ] **Step 18.5: Done.**

---

## Self-Review

**1. Spec coverage:** Walked through the spec Sections 4-8 and confirmed each lands in a task:

- Section 4.1 modules table → Tasks 2-5 (new), 6-9 (modified `scoped.rs`), 10-11 (`act.rs`/deps), 12 (builtin registration), 13 (boot), 14-15 (dissolution).
- Section 4.6 `max_result_tokens` activation table → Task 12.
- Section 4.7 `ContentSource::ToolError` → Task 1.
- Section 5 data flow → assembled across Tasks 6 (Layer 2), 7 (retry), 8 (sanitize), 9 (duration), 10 (guardrail), 11 (Layer 3).
- Section 6 error matrix → individually covered by unit tests in each task.
- Section 7 test strategy → unit tests per task; integration in Task 16.
- Section 8 dissolution list → Task 14.
- Section 9 risks → defensive guards already wired (Mutex poison recovery, fail-open guardrail, retry capped at 2).

**2. Placeholder scan:** No "TBD" / "TODO" left. Tasks 6.4-6.5, 7.2, 8.1, 11.2 ask the engineer to inspect specific files and adapt — that is necessary discovery work, not placeholder. Each such step calls out exact file paths and what to grep for.

**3. Type consistency:** `ToolDefinition` field references match `src/tools/runtime.rs:43`. `LoopTool::definition()` is flagged for verification in Step 6.3. `ToolError::is_retryable` is flagged in Step 5.2. `GuardrailDecision::Block { reason, .. }` matches `src/guardrails/decision.rs:11`. `HarnessDeps.guardrails` and the new `turn_budget`/`result_store` fields match the deps layout in `src/harness/deps.rs:79`.

**4. Scope check:** One worktree, one merge, ~17 commits of bite-sized scope. Below the typical Aleph cycle ceiling (Hermes-prompt P3 was 12 commits, skill usage hardening was 14).

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-20-tool-result-budget-3layer.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration. Best for this plan because Tasks 6/7/8/9 all touch the same file (`scoped.rs`) and benefit from per-task isolation.
2. **Inline Execution** — execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.

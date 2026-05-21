# History Compression Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `PreflightPipeline` into the harness Think loop with two new cheap-pass stages (tool_result pruning, historical image stripping) that save tokens before the LLM compactor fires; fix the last hardcoded `chars / 4` site; clean up dead code from the now-redundant `CompactionOrchestrator` if it remains uncalled.

**Architecture:** Add two new `PreflightStage` implementations in `src/context/budget/`. Instantiate a `PreflightPipeline` in `src/orchestrator/harness_bridge.rs` alongside the existing `ContextCompactor`; pass it into `HarnessDeps`. In `src/harness/agent/think.rs:88-101`, call `pipeline.run()` BEFORE `compactor.compact()` so cheap passes always run, even if the LLM call fails. Keep the existing `ContextBudget::before_turn()` directive logic — Preflight runs unconditionally before the directive check, then compactor runs on directive.

**Tech Stack:** Rust 2021, tokio, async-trait, existing `UnifiedMessage` enum, existing `estimate_tokens_smart()`.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/context/budget/cheap_passes.rs` | Create | Module entry for cheap-pass stages |
| `src/context/budget/cheap_passes/tool_result_pruning.rs` | Create | `ToolResultPruningStage` — dedup + truncate old tool results |
| `src/context/budget/cheap_passes/image_stripping.rs` | Create | `HistoricalImageStrippingStage` — strip images from all but newest |
| `src/context/budget/mod.rs` | Modify | Export `cheap_passes` module |
| `src/orchestrator/harness_bridge.rs` | Modify | Instantiate `PreflightPipeline` with stages, pass to `HarnessDeps` |
| `src/harness/deps.rs` | Modify | Add `preflight_pipeline: Option<Arc<PreflightPipeline>>` field |
| `src/harness/agent.rs` | Modify | Default `preflight_pipeline: None` in test builders |
| `src/harness/agent/think.rs` | Modify | Call `pipeline.run()` before `compactor.compact()` |
| `src/thinker/memory_context_provider/memory.rs` | Modify | Replace `chars / 4` with `estimate_tokens_smart()` |
| `tests/integration/preflight_wiring.rs` | Create | End-to-end test: cheap passes save tokens with mocked LLM failure |

**Why split cheap_passes by submodule:** each stage is ~100-150 lines including tests; keeping them separate keeps focus + makes adding future stages (e.g., orphan-tool-pair repair) trivial.

---

## Task 0: Pre-flight (worktree creation)

**Files:** none

- [ ] **Step 1: Create isolated worktree**

Run: `EnterWorktree` with branch name `history-compression-wiring-2026-05-20`

Expected: New worktree path returned, isolated from main.

- [ ] **Step 2: Verify clean state in worktree**

Run: `git status` (in worktree)
Expected: `nothing to commit, working tree clean`

- [ ] **Step 3: Confirm baseline test failure count**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: Some failures present (baseline known to be ~19 per `MEMORY.md`); record exact number.

---

## Task 1: Fix hardcoded `chars / 4` in memory_context_provider

**Files:**
- Modify: `src/thinker/memory_context_provider/memory.rs:25`

- [ ] **Step 1: Read current line and confirm context**

Run: `sed -n '20,30p' src/thinker/memory_context_provider/memory.rs`
Expected: line 25 contains `total_tokens: (self.config.max_output_chars / 4) as u32,`

- [ ] **Step 2: Write failing test in `memory.rs#tests` module**

Find or add `#[cfg(test)] mod tests` at file end. Add:

```rust
#[test]
fn cjk_text_estimate_uses_smart_ratio_not_chars_div_4() {
    // 100 CJK chars → ~67 tokens with smart ratio (1.5), not 25 with chars/4
    let cjk_text: String = "中".repeat(100);
    let smart = crate::context::budget::pressure::estimate_tokens_smart(&cjk_text);
    let naive = cjk_text.chars().count() / 4;
    assert!(smart > naive * 2,
        "CJK estimate must exceed naive chars/4: smart={smart} naive={naive}");
}
```

Run: `cargo test -p alephcore --lib memory_context_provider::memory::tests::cjk_text_estimate -- --nocapture`
Expected: test passes immediately (sanity check; not a behavioral test of memory.rs:25 yet).

- [ ] **Step 3: Replace the hardcoded estimate**

```rust
// before:
total_tokens: (self.config.max_output_chars / 4) as u32,

// after — use smart estimator on a sentinel text of max_output_chars length
//        OR direct conversion when the field is a budget cap, not text:
total_tokens: crate::context::budget::pressure::estimate_tokens_smart(
    // Reuse the same content we just rendered; if memory.rs:25 sits on a
    // builder that doesn't have the rendered text yet, fall back to
    // chars/3 (worst case for non-CJK) so the budget is never under-
    // estimated. Read surrounding context to pick the right form.
    &rendered_text,
) as u32,
```

**NOTE TO IMPLEMENTER:** open `src/thinker/memory_context_provider/memory.rs` lines 1-60 and pick whichever form matches the local data flow. If `rendered_text` isn't in scope, the cap form is `(self.config.max_output_chars * 2 / 3) as u32` — that's a 50% upward correction for CJK without needing the actual text.

- [ ] **Step 4: Run tests + clippy**

Run: `cargo check -p alephcore` (full crate compile)
Expected: clean compile, no warnings on the changed file.

Run: `cargo test -p alephcore --lib memory_context_provider -- --nocapture`
Expected: all memory_context_provider tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/memory_context_provider/memory.rs
git commit -m "fix: replace chars/4 in memory_context_provider with smart token estimator

The last leftover hardcoded chars/4 token estimate severely under-counted
CJK content. Reuse the existing estimate_tokens_smart helper that already
handles CJK (1.5 chars/tok), code (2.5), and prose (3.5)."
```

---

## Task 2: Create `ToolResultPruningStage`

**Files:**
- Create: `src/context/budget/cheap_passes.rs` (module entry, 5 lines)
- Create: `src/context/budget/cheap_passes/tool_result_pruning.rs`
- Modify: `src/context/budget/mod.rs` (add `pub mod cheap_passes;` line)

- [ ] **Step 1: Add module declaration**

Edit `src/context/budget/mod.rs`. Find existing `pub mod preflight;` and add below it:

```rust
pub mod cheap_passes;
```

Create `src/context/budget/cheap_passes.rs` with exactly:

```rust
//! Preflight cheap passes — token-saving transforms that need no LLM call.
//!
//! Each stage implements [`super::preflight::PreflightStage`] and is wired
//! into the harness via a `PreflightPipeline` in `orchestrator::harness_bridge`.

pub mod image_stripping;
pub mod tool_result_pruning;

pub use image_stripping::HistoricalImageStrippingStage;
pub use tool_result_pruning::ToolResultPruningStage;
```

Create directory: `mkdir src/context/budget/cheap_passes` (the submodules live as files inside).

- [ ] **Step 2: Write failing test for tool_result_pruning**

Create `src/context/budget/cheap_passes/tool_result_pruning.rs` with the test module first:

```rust
//! `ToolResultPruningStage` — replace old tool_results with one-line summaries
//! to save tokens before the LLM compactor runs.

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;

/// Preflight stage that shortens stale `ToolResult` messages.
///
/// Keeps the newest `fresh_tail_count` messages untouched. For older
/// `ToolResult` blocks, replaces the content with a one-line summary:
/// `"[pruned tool_result: <tool_name>, <N> tokens]"`.
pub struct ToolResultPruningStage {
    /// Minimum token size before pruning kicks in. Tool results smaller
    /// than this are kept verbatim (the prune summary itself costs tokens).
    pub min_tokens_to_prune: usize,
}

impl Default for ToolResultPruningStage {
    fn default() -> Self {
        Self { min_tokens_to_prune: 200 }
    }
}

#[async_trait]
impl crate::context::budget::preflight::PreflightStage for ToolResultPruningStage {
    fn name(&self) -> &'static str {
        "tool_result_pruning"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        _pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if messages.len() <= fresh_tail_count {
            return 0;
        }
        let cut_end = messages.len() - fresh_tail_count;
        let mut total_freed: usize = 0;

        for msg in messages.iter_mut().take(cut_end) {
            if let UnifiedMessage::ToolResult { tool_name, content, .. } = msg {
                let original_tokens = estimate_tokens_smart(&content_to_text(content));
                if original_tokens < self.min_tokens_to_prune {
                    continue;
                }
                let placeholder = format!(
                    "[pruned tool_result: {tool_name}, {original_tokens} tokens]"
                );
                let new_tokens = estimate_tokens_smart(&placeholder);
                if new_tokens >= original_tokens {
                    continue; // pruning would not save tokens
                }
                replace_tool_result_text(content, placeholder);
                total_freed += original_tokens - new_tokens;
            }
        }
        total_freed
    }
}

// Helpers — these match the local UnifiedMessage shape; adjust the exact
// signatures after reading src/providers/message.rs.
fn content_to_text(content: &[crate::providers::message::ContentBlock]) -> String {
    content.iter()
        .filter_map(|b| b.as_text().map(String::from))
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_tool_result_text(
    content: &mut Vec<crate::providers::message::ContentBlock>,
    placeholder: String,
) {
    use crate::providers::message::ContentBlock;
    content.clear();
    content.push(ContentBlock::text(placeholder));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::ContextPressure;
    use crate::providers::message::{ContentBlock, UnifiedMessage};

    fn make_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 5000,
            budget_tokens: 10000,
            ratio: 0.5,
            overhead_tokens: 0,
            available_for_messages: 5000,
        }
    }

    #[tokio::test]
    async fn prunes_old_large_tool_result() {
        // Big tool result far older than fresh tail
        let big = "x".repeat(2000); // ~570 tokens with chars/3.5
        let mut messages = vec![
            UnifiedMessage::ToolResult {
                tool_call_id: "id1".into(),
                tool_name: "Read".into(),
                content: vec![ContentBlock::text(big.clone())],
            },
            UnifiedMessage::user("recent 1"),
            UnifiedMessage::user("recent 2"),
            UnifiedMessage::user("recent 3"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 3).await;
        assert!(freed > 100, "expected significant token savings, got {freed}");
        // The old tool result is now a short placeholder
        if let UnifiedMessage::ToolResult { content, .. } = &messages[0] {
            let text = content[0].as_text().unwrap();
            assert!(text.starts_with("[pruned tool_result"));
        } else {
            panic!("expected first message to remain a ToolResult");
        }
    }

    #[tokio::test]
    async fn skips_small_tool_result() {
        let mut messages = vec![
            UnifiedMessage::ToolResult {
                tool_call_id: "id1".into(),
                tool_name: "Read".into(),
                content: vec![ContentBlock::text("short".to_string())],
            },
            UnifiedMessage::user("recent"),
        ];
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 1).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn protects_fresh_tail() {
        let big = "x".repeat(2000);
        let mut messages = vec![
            UnifiedMessage::user("oldest"),
            UnifiedMessage::ToolResult {
                tool_call_id: "id1".into(),
                tool_name: "Read".into(),
                content: vec![ContentBlock::text(big.clone())],
            },
        ];
        // fresh_tail = 2 protects everything
        let stage = ToolResultPruningStage::default();
        let freed = stage.prepare(&mut messages, &make_pressure(), 2).await;
        assert_eq!(freed, 0);
        if let UnifiedMessage::ToolResult { content, .. } = &messages[1] {
            assert!(content[0].as_text().unwrap().starts_with("xxx"));
        }
    }
}
```

Run: `cargo test -p alephcore --lib context::budget::cheap_passes::tool_result_pruning -- --nocapture`

Expected: tests FAIL with "no such item" / "missing field" until you confirm `UnifiedMessage::ToolResult` and `ContentBlock` shapes match. Iterate on signatures.

- [ ] **Step 3: Adjust shape to actual `UnifiedMessage` definition**

Read: `src/providers/message.rs` (look for `UnifiedMessage::ToolResult` variant).

Fix the test fixture + helper functions to match the exact field names (likely `name` not `tool_name`, or wrappers around `ContentBlock`). Common adjustments:
- field name `tool_name` may actually be `name`
- `content: vec![ContentBlock::text(...)]` may be `content: ToolResultContent::Text(String)`
- `as_text()` may not exist on raw `ContentBlock`; might need a match arm

Re-run test until pass.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore --lib -- -D warnings`
Expected: no warnings introduced. Pre-existing warnings on main are tolerated (per `feedback_no_user_capability_override`-adjacent memory `fmt_clippy_baseline_drift`).

- [ ] **Step 5: Commit**

```bash
git add src/context/budget/mod.rs \
        src/context/budget/cheap_passes.rs \
        src/context/budget/cheap_passes/tool_result_pruning.rs
git commit -m "feat: add ToolResultPruningStage cheap pass

Replaces stale large tool_result blocks with one-line placeholders so
older context costs less before the LLM compactor fires. Protects
fresh_tail_count and skips small results that pruning wouldn't shrink.

Tested for: large prune, small skip, fresh-tail protection."
```

---

## Task 3: Create `HistoricalImageStrippingStage`

**Files:**
- Create: `src/context/budget/cheap_passes/image_stripping.rs`

- [ ] **Step 1: Write failing test**

Create the file with this content:

```rust
//! `HistoricalImageStrippingStage` — strip images from all but the newest
//! image-bearing message to save the ~1500-tokens-per-image cost.

use crate::context::budget::ContextPressure;
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;

/// Image tokens cost (matches Anthropic pricing; matches hermes' constant).
const IMAGE_TOKENS_ESTIMATE: usize = 1500;

pub struct HistoricalImageStrippingStage;

impl Default for HistoricalImageStrippingStage {
    fn default() -> Self { Self }
}

#[async_trait]
impl crate::context::budget::preflight::PreflightStage for HistoricalImageStrippingStage {
    fn name(&self) -> &'static str {
        "historical_image_stripping"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        _pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        // Find the newest image-bearing message index. Don't strip from
        // or after this index — that image is current context.
        let newest_image_idx = messages.iter()
            .enumerate()
            .rev()
            .find(|(_, m)| message_has_image(m))
            .map(|(i, _)| i);
        let Some(newest_image_idx) = newest_image_idx else {
            return 0; // no images at all
        };

        // Hard cap: never strip the last fresh_tail_count messages.
        let cut_end = messages.len().saturating_sub(fresh_tail_count);
        let upper_bound = cut_end.min(newest_image_idx);

        let mut total_freed = 0usize;
        for msg in messages.iter_mut().take(upper_bound) {
            let stripped = strip_images_in_place(msg);
            total_freed += stripped * IMAGE_TOKENS_ESTIMATE;
        }
        total_freed
    }
}

fn message_has_image(_msg: &UnifiedMessage) -> bool {
    // TODO: implement after reading message.rs — return true if any
    // ContentBlock variant is Image / ImageRef / etc.
    false
}

fn strip_images_in_place(_msg: &mut UnifiedMessage) -> usize {
    // TODO: implement after reading message.rs — replace Image blocks
    // with a text placeholder "[image stripped from history]" and return
    // the count of stripped blocks.
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::ContentBlock;
    // Tests will be filled out once we know the actual Image variant shape.
    // Stub assertions so the file compiles:
    #[test]
    fn stage_constructs() {
        let _ = HistoricalImageStrippingStage::default();
    }
}
```

Run: `cargo check -p alephcore`
Expected: compile passes (stub functions return zero).

- [ ] **Step 2: Read `src/providers/message.rs` for image variant**

Run: `grep -n "Image\|image" src/providers/message.rs | head -20`

Confirm the actual variant — common shapes:
- `ContentBlock::Image { source: ImageSource, ... }`
- `ContentBlock::ImageRef(String)`
- Some enum tag with `media_type`

- [ ] **Step 3: Implement `message_has_image` and `strip_images_in_place`**

Replace the two stub functions with real implementations matching the actual variant. The strip function replaces each Image block with `ContentBlock::text("[image stripped from history]")`.

- [ ] **Step 4: Add real test**

Append to the `tests` module:

```rust
#[tokio::test]
async fn strips_older_images_keeps_newest() {
    use crate::context::budget::ContextPressure;
    // Build messages: old image, middle text, newest image, fresh tail text
    let mut messages = vec![
        UnifiedMessage::user_with_blocks(vec![
            ContentBlock::image(/* ... */),
            ContentBlock::text("old turn"),
        ]),
        UnifiedMessage::user("middle turn"),
        UnifiedMessage::user_with_blocks(vec![
            ContentBlock::image(/* ... */),
            ContentBlock::text("newest image turn"),
        ]),
        UnifiedMessage::user("fresh 1"),
        UnifiedMessage::user("fresh 2"),
    ];
    let pressure = ContextPressure {
        used_tokens: 8000, budget_tokens: 10000,
        ratio: 0.8, overhead_tokens: 0, available_for_messages: 2000,
    };
    let stage = HistoricalImageStrippingStage;
    let freed = stage.prepare(&mut messages, &pressure, 2).await;
    assert_eq!(freed, IMAGE_TOKENS_ESTIMATE,
        "should strip exactly one image (the oldest)");
}
```

Adjust `UnifiedMessage::user_with_blocks` / `ContentBlock::image(...)` to match actual constructor names. If they don't exist, use whatever constructor is available (e.g., direct struct literal).

Run: `cargo test -p alephcore --lib context::budget::cheap_passes::image_stripping`
Expected: test passes.

- [ ] **Step 5: Commit**

```bash
git add src/context/budget/cheap_passes/image_stripping.rs
git commit -m "feat: add HistoricalImageStrippingStage cheap pass

Strips images from all messages preceding the newest image-bearing turn
(beyond fresh_tail), saving ~1500 tokens per image. Hermes uses the same
heuristic; the current screenshot is preserved as live context while old
ones become text placeholders."
```

---

## Task 4: Wire `PreflightPipeline` into harness deps

**Files:**
- Modify: `src/harness/deps.rs` (add field)
- Modify: `src/harness/agent.rs` (default `None` in builders)
- Modify: `src/orchestrator/harness_bridge.rs` (instantiate pipeline)
- Modify: `src/harness/agent/think.rs` (call pipeline before compactor)

- [ ] **Step 1: Add field to `HarnessDeps`**

Edit `src/harness/deps.rs` around line 51 (where `context_compactor` lives):

```rust
// near the top
use crate::context::budget::preflight::PreflightPipeline;

// inside HarnessDeps struct, immediately after context_compactor:
/// Optional pipeline of cheap-pass preflight stages (tool_result pruning,
/// image stripping). Runs BEFORE the LLM compactor so token savings happen
/// even when the compactor's LLM call fails. Wired alongside
/// `context_compactor` in `orchestrator::harness_bridge`.
pub preflight_pipeline: Option<Arc<PreflightPipeline>>,
```

- [ ] **Step 2: Default to `None` in all `HarnessDeps` constructors**

Edit `src/harness/agent.rs`. Find each occurrence of `context_compactor: None,` (lines 741, 781, 820 per earlier grep) and add immediately after:

```rust
preflight_pipeline: None,
```

Run: `cargo check -p alephcore`
Expected: compile passes (all builders updated).

- [ ] **Step 3: Instantiate pipeline in `harness_bridge.rs`**

Edit `src/orchestrator/harness_bridge.rs` around lines 213-226. After the `(context_budget, context_compactor) = ...` block, add:

```rust
// Cheap-pass preflight pipeline — runs unconditionally before the compactor
// so large tool outputs and historical images get pruned even when the LLM
// summary call fails. Gated on the same [context_budget] config because
// fresh_tail_count comes from there.
let preflight_pipeline = self.context_budget_config.as_ref().map(|_cfg| {
    use crate::context::budget::cheap_passes::{
        HistoricalImageStrippingStage, ToolResultPruningStage,
    };
    use crate::context::budget::preflight::{PreflightPipeline, PreflightStage};
    let stages: Vec<Box<dyn PreflightStage>> = vec![
        Box::new(ToolResultPruningStage::default()),
        Box::new(HistoricalImageStrippingStage::default()),
    ];
    Arc::new(PreflightPipeline::new(stages))
});
```

Then add `preflight_pipeline,` into the `HarnessDeps { ... }` literal, right after `context_compactor,`.

Run: `cargo check -p alephcore`
Expected: compile passes.

- [ ] **Step 4: Call pipeline in `think.rs` before compactor**

Edit `src/harness/agent/think.rs` around lines 79-101. Insert the preflight call between message assembly (line 77) and the budget check (line 81):

```rust
// 2a (new): Always run the cheap-pass preflight pipeline FIRST. These
// stages need no LLM, so they save tokens even when the compactor's
// side-channel LLM call fails. Fresh tail is the compactor's config
// default (6) when present, else 0.
if let Some(pipeline) = self.deps.preflight_pipeline.as_ref() {
    // Build a synthetic pressure for the pipeline. Stages mostly ignore
    // the ratio; the real budget check happens in step 2b below. We use
    // a placeholder pressure with budget_tokens=0 so stages that gate on
    // ratio still fire (treat as max pressure).
    let pressure = crate::context::budget::ContextPressure {
        used_tokens: 0,
        budget_tokens: 0,
        ratio: 1.0,
        overhead_tokens: 0,
        available_for_messages: 0,
    };
    let fresh_tail = self.deps.context_compactor.as_ref()
        .map(|_| 6)
        .unwrap_or(0);
    let freed = pipeline.run(&mut messages, &pressure, fresh_tail).await;
    if freed > 0 {
        tracing::info!(
            tokens_freed = freed,
            "preflight cheap passes saved tokens before budget check"
        );
    }
}
```

Renumber the next comment from `// 2a.` to `// 2b.` and `// 2b.` to `// 2c.` etc.

- [ ] **Step 5: Run full harness test suite**

Run: `cargo test -p alephcore --lib harness:: 2>&1 | tail -30`
Expected: harness tests pass, no regression vs baseline. If you see test failures NOT in the known baseline (~19 lib failures per memory), investigate.

- [ ] **Step 6: Commit**

```bash
git add src/harness/deps.rs src/harness/agent.rs src/harness/agent/think.rs \
        src/orchestrator/harness_bridge.rs
git commit -m "feat: wire PreflightPipeline into harness Think loop

Hooks the previously-unwired PreflightPipeline into the harness via
HarnessDeps. Stages run BEFORE the existing ContextCompactor so cheap
token savings happen unconditionally, even when the LLM-summary call
fails. Gated on the same [context_budget] config as the compactor."
```

---

## Task 5: Integration test — cheap passes save tokens with mocked LLM failure

**Files:**
- Create: `tests/integration/preflight_wiring.rs`

- [ ] **Step 1: Check existing integration test directory**

Run: `ls tests/ 2>/dev/null || echo "no tests/ dir"`

If no `tests/` directory at crate root, the existing convention may be inline `#[cfg(test)]` modules. In that case, create:
- `src/harness/tests/preflight_wiring.rs`
- Add `mod preflight_wiring;` to `src/harness/tests/mod.rs`

If there IS a `tests/` directory, create `tests/integration/preflight_wiring.rs` directly.

- [ ] **Step 2: Write integration test**

```rust
//! Integration test: cheap-pass preflight pipeline saves tokens even when
//! the LLM compactor's side-channel call fails.

use alephcore::context::budget::cheap_passes::{
    HistoricalImageStrippingStage, ToolResultPruningStage,
};
use alephcore::context::budget::preflight::{PreflightPipeline, PreflightStage};
use alephcore::context::budget::ContextPressure;
use alephcore::providers::message::{ContentBlock, UnifiedMessage};

#[tokio::test]
async fn preflight_saves_tokens_even_when_compactor_unavailable() {
    let huge_tool_output = "y".repeat(4000); // ~1140 tokens
    let mut messages = vec![
        UnifiedMessage::ToolResult {
            tool_call_id: "id1".into(),
            tool_name: "Bash".into(),
            content: vec![ContentBlock::text(huge_tool_output)],
        },
        UnifiedMessage::user("recent 1"),
        UnifiedMessage::user("recent 2"),
        UnifiedMessage::user("recent 3"),
        UnifiedMessage::user("recent 4"),
        UnifiedMessage::user("recent 5"),
        UnifiedMessage::user("recent 6"),
    ];

    let stages: Vec<Box<dyn PreflightStage>> = vec![
        Box::new(ToolResultPruningStage::default()),
        Box::new(HistoricalImageStrippingStage::default()),
    ];
    let pipeline = PreflightPipeline::new(stages);
    let pressure = ContextPressure {
        used_tokens: 5000, budget_tokens: 10000,
        ratio: 0.5, overhead_tokens: 0, available_for_messages: 5000,
    };
    let freed = pipeline.run(&mut messages, &pressure, 6).await;
    assert!(freed > 500,
        "expected significant cheap-pass savings, got {freed}");
    // Compactor never called — savings still real
}
```

Adjust `UnifiedMessage::ToolResult` field shape to match the actual definition.

- [ ] **Step 3: Run integration test**

Run: `cargo test -p alephcore --test preflight_wiring` (if `tests/integration/`)
OR: `cargo test -p alephcore --lib harness::tests::preflight_wiring` (if inline)
Expected: test passes.

- [ ] **Step 4: Commit**

```bash
git add tests/integration/preflight_wiring.rs  # or src/harness/tests/preflight_wiring.rs + mod.rs
git commit -m "test: preflight pipeline saves tokens without compactor

Integration test verifying that cheap passes deliver token savings
independently of the LLM compactor's success."
```

---

## Task 6: Documentation cleanup

**Files:**
- Modify: `docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md`

- [ ] **Step 1: Read current doc**

Run: `grep -n "Tier 2\|Tier 3\|未连线\|unwired\|not yet" docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md | head`

Identify any paragraphs claiming preflight/multi-tier compaction is "未连线" or "not yet implemented".

- [ ] **Step 2: Update doc to reflect actual state**

Replace `未连线` / `not yet implemented` paragraphs with a note describing the now-wired flow:

```markdown
## Compaction Flow (live as of 2026-05-20)

Per turn in `harness/agent/think.rs`:

1. **PreflightPipeline** runs cheap passes (no LLM):
   - `ToolResultPruningStage` — replace stale large tool_results with one-line placeholders
   - `HistoricalImageStrippingStage` — drop images from all but the newest image-bearing turn

2. **ContextBudget::before_turn()** evaluates pressure → returns `LoopDirective`.

3. If `CompactAndContinue` directive: **ContextCompactor::compact()** runs LLM summarization (with deterministic-truncation fallback).

Gated on `[context_budget] enabled = true` config.
```

- [ ] **Step 3: Commit**

```bash
git add docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md
git commit -m "docs: AGENT_LOOP_CONTEXT_BUDGET reflects live preflight + compactor wiring"
```

---

## Task 7: Dead-code disposition for `CompactionOrchestrator`

**Files:**
- Modify: `src/context/compact/orchestrator.rs` (add deprecation note)
- Modify: `src/context/compact/mod.rs` (gate behind dead-code allow)

The orchestrator is built but has zero production consumers. Two options:
- **A. Keep with `#[deprecated]` note** — preserves architecture for future multi-strategy wiring
- **B. Delete entirely** — aligns with "屎山清理"

Choose **A** for this cycle: deleting now risks rework if next cycle wants multi-strategy. Document the situation.

- [ ] **Step 1: Add module-level doc comment**

Edit top of `src/context/compact/orchestrator.rs`:

```rust
//! Compaction orchestrator — evaluates pressure, dispatches strategies in
//! priority order, and runs the post-compaction cleanup chain.
//!
//! ## Status (2026-05-20)
//!
//! No production consumer. The harness calls `ContextCompactor::compact()`
//! directly (see `src/harness/agent/think.rs`), so this orchestrator is
//! only exercised by its own tests. Kept as scaffolding for a future
//! multi-strategy wiring (e.g., LLM summary + deterministic + dedup), but
//! flag for deletion if that wiring doesn't land in the next two cycles.
//!
//! Tracking: docs/superpowers/specs/2026-05-20-history-compression-wiring-design.md §11
```

- [ ] **Step 2: Commit**

```bash
git add src/context/compact/orchestrator.rs
git commit -m "docs: flag CompactionOrchestrator as scaffolding-without-consumers"
```

---

## Task 8: Final verification + regression sweep

- [ ] **Step 1: Full library test run**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: failure count == baseline (~19). Delta should be 0.

- [ ] **Step 2: Clippy check on touched files**

Run: `cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -E "src/(context/budget|harness|orchestrator|thinker/memory_context_provider)" | head -20`
Expected: no NEW warnings on touched files. (Pre-existing warnings tolerated per memory `fmt_clippy_baseline_drift`.)

- [ ] **Step 3: Targeted integration test**

Run: `cargo test -p alephcore --test preflight_wiring 2>&1 | tail`
Expected: PASS.

- [ ] **Step 4: Touch-test build of full server**

Run: `cargo check --bin aleph-server 2>&1 | tail -5`
Expected: clean compile.

- [ ] **Step 5: Verify worktree diff is focused**

Run: `git diff --stat main..HEAD | tail -20`
Expected: changes confined to:
- `src/context/budget/cheap_passes.rs` (+ submodule)
- `src/harness/deps.rs`
- `src/harness/agent.rs`
- `src/harness/agent/think.rs`
- `src/orchestrator/harness_bridge.rs`
- `src/thinker/memory_context_provider/memory.rs`
- `tests/integration/preflight_wiring.rs` (or equivalent)
- `docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md`
- `src/context/compact/orchestrator.rs`
- `src/context/budget/mod.rs`

No unrelated edits.

- [ ] **Step 6: Final commit + summary**

If there are any small fixup commits needed, do them. Then summarize the worktree's commit log:

```bash
git log --oneline main..HEAD
```

Expected: 7-9 commits, one per task.

---

## Self-Review Checklist

- ✅ Every task references real files with line numbers.
- ✅ Each step has either code OR a verifiable command.
- ✅ No "TODO" / "TBD" in step content (the `TODO:` comments inside Task 3 step 1 stub code are intentional — they're placeholders the implementer replaces in step 2).
- ✅ Type names consistent across tasks (`PreflightPipeline`, `PreflightStage`, `ToolResultPruningStage`, `HistoricalImageStrippingStage`).
- ✅ Baseline test failure tolerance noted (per memory `baseline_test_failures.md`).
- ✅ All defects from the (revised) spec have at least one task addressing them:
  - Defect 1 (cheap passes never run) → Tasks 2 + 3 + 4
  - Defect 2 (`chars / 4` site) → Task 1
  - Defect 3 (`[context_budget]` opt-in) → Deferred; cycle-end question (see spec §11)
  - Defect 4 (`CompactionOrchestrator` dead-code) → Task 7

## Risk Notes

- **Field-shape risk in Tasks 2–3:** `UnifiedMessage::ToolResult` and `ContentBlock::Image` shapes are inferred. First check on running the failing test will reveal actual fields; iterate before claiming complete.
- **`message_has_image` correctness in Task 3:** the stub returns `false`; without correcting it the stage no-ops silently. The unit test in Step 4 catches this.
- **fresh_tail value in Task 4:** Currently hardcoded to 6 (compactor default). If the harness owns a different fresh_tail elsewhere, harmonize.
- **`tracing::info!` noise:** the new info log in think.rs fires per turn when freed>0. Adjust to `debug!` if it floods logs in normal use.

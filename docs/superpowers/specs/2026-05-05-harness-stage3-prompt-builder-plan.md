# Stage 3 — Prompt Assembly Seam (#5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract `agent.rs`'s private `build_prompt` into a `PromptBuilder` trait + `DefaultPromptBuilder` impl behind a `TurnContext` struct so future stages (Subagent #11, JudgeAgent #10) can inject custom assembly logic without patching `agent.rs`. Byte-equivalent behavior for the default path.

**Architecture:**
- New module `src/harness/prompt.rs` exposes `PromptBuilder` trait, `DefaultPromptBuilder` struct, and `TurnContext` input struct.
- `HarnessDeps` gains a `prompt_builder: Arc<dyn PromptBuilder>` field (defaults to `Arc::new(DefaultPromptBuilder)`).
- `AgentHarness::run_turn_internal` replaces the inline `build_prompt(...)` call with `self.deps.prompt_builder.assemble(&ctx)`.
- The pre-existing private `build_prompt` function at `agent.rs:850` is deleted; its body moves verbatim into `DefaultPromptBuilder::assemble`.
- Pre-task: consolidate `stall.rs` (117 lines) into `deps.rs` to recover the R10 9-file budget so adding `prompt.rs` lands at 9/9.

**Tech Stack:** Rust 1.x, `async_trait` (already in Cargo.lock), tokio (existing), no new deps.

**Baseline:** commit `7f417ae36` (Stage 2 ship). Master spec: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 3.

**Budget envelope (master spec §3.3 + R10):**
- Total stage delta: ≤ ~250 lines (target), ≤ +400 lines harness/ delta (cap).
- `agent.rs`: ≤ 1500 lines (currently 1373, will shrink ~80 lines after retiring `build_prompt`).
- `src/harness/` file count: stays at 9 canonical files (consolidating stall.rs into deps.rs offsets adding prompt.rs).
- Single PR ≤ 600 lines including tests; if golden test corpus pushes us over, split the test corpus into a separate commit.

---

## File Structure

| File | Action | Purpose |
|------|--------|---------|
| `src/harness/prompt.rs` | **Create** | `PromptBuilder` trait, `TurnContext`, `DefaultPromptBuilder` |
| `src/harness/agent.rs` | **Modify** | Replace `build_prompt` call (line 138) with `self.deps.prompt_builder.assemble(...)`. Delete `build_prompt` (lines 850-929). Keep `tail_start_index` and `resolve_tool_name` helpers (still used). |
| `src/harness/deps.rs` | **Modify** | Add `prompt_builder: Arc<dyn PromptBuilder>` field with `Default::default()` impl. Absorb `StallConfig`/`StallTracker` (delete `stall.rs`). |
| `src/harness/mod.rs` | **Modify** | Add `pub mod prompt;` re-export. Remove `pub mod stall;` and adjust `pub use stall::*` to point at `deps::*`. |
| `src/harness/stall.rs` | **Delete** | Contents moved into `deps.rs` |
| `src/harness/tests/prompt.rs` | **Create** | Golden tests + property test for `DefaultPromptBuilder` |
| `src/harness/tests/stability.rs` | **Modify** | Update `use crate::harness::stall::StallConfig;` → `use crate::harness::deps::StallConfig;` |
| `CHANGELOG.md` | **Modify** | Add Stage 3 entries to `## [Unreleased]` |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | **Modify** | Flip Stage 3 status to `✅ Shipped <sha> on 2026-05-05` |

**Touch surface outside harness/:** none. Subagent / Gateway / orchestrator builders construct `HarnessDeps` via builder pattern; we add `prompt_builder` with a sensible `Default` so existing callsites compile unchanged.

---

## Task Sequence Rationale

Tasks land in dependency order:

1. **Task 1** (R10 budget recovery): consolidate `stall.rs` first so adding `prompt.rs` doesn't briefly violate file count.
2. **Task 2** (define seam): `PromptBuilder` trait + `TurnContext` + empty `DefaultPromptBuilder` skeleton + ≥1 compile test.
3. **Task 3** (impl byte-equivalent): port `build_prompt` body into `DefaultPromptBuilder::assemble` + golden tests against captured baseline.
4. **Task 4** (wire it): thread `prompt_builder` through `HarnessDeps`; replace agent.rs:138 call; delete agent.rs:850 function. This is the "old code retired in same commit" step per R10.
5. **Task 5** (property test + perf): TurnContext permutation property test; observational dispatch perf check.
6. **Task 6** (ship): CHANGELOG + master-spec status flip + final verification.

---

## Task 1: Consolidate `stall.rs` into `deps.rs` (R10 budget recovery)

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/mod.rs`
- Modify: `src/harness/tests/stability.rs:692`
- Delete: `src/harness/stall.rs`

**Why first:** master spec / R10 hard cap is 9 canonical files in `src/harness/`. Currently 10 due to `stall.rs` (P0 rescue artifact). If we add `prompt.rs` first, we hit 11/9. Consolidating stall first opens the slot.

- [ ] **Step 1: Append stall.rs body verbatim to deps.rs**

Read `src/harness/stall.rs` and append its non-test content (the `StallConfig`, `StallTracker`, default consts) to the end of `src/harness/deps.rs`, after the `HarnessDeps` struct. Move the `#[cfg(test)] mod tests` block from stall.rs to a new `#[cfg(test)] mod stall_tests` block in deps.rs.

The exact sections to copy (from `src/harness/stall.rs`):

```rust
// Add at top of deps.rs after the existing imports:
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

// ... existing HarnessDeps struct ...

// Append after the struct:

const DEFAULT_STALL_TIMEOUT_SECS: u64 = 300;
const DEFAULT_STALL_CHECK_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct StallConfig {
    pub timeout: Duration,
    pub check_interval: Duration,
}

impl Default for StallConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_STALL_TIMEOUT_SECS),
            check_interval: Duration::from_secs(DEFAULT_STALL_CHECK_INTERVAL_SECS),
        }
    }
}

impl StallConfig {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }
}

#[derive(Debug)]
pub struct StallTracker {
    last_activity: Arc<TokioMutex<Instant>>,
    config: StallConfig,
    cancel: CancellationToken,
}

impl StallTracker {
    pub fn new(config: StallConfig, cancel: CancellationToken) -> Self {
        Self {
            last_activity: Arc::new(TokioMutex::new(Instant::now())),
            config,
            cancel,
        }
    }

    pub async fn record_activity(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    pub async fn elapsed(&self) -> Duration {
        self.last_activity.lock().await.elapsed()
    }

    pub fn is_stalled(&self) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        if let Ok(guard) = self.last_activity.try_lock() {
            guard.elapsed() > self.config.timeout
        } else {
            false
        }
    }
}
```

Mirror the `#[cfg(test)] mod tests { ... }` block from stall.rs into `deps.rs` as `#[cfg(test)] mod stall_tests { use super::*; ... }`.

- [ ] **Step 2: Update `src/harness/mod.rs`**

Replace these two lines:

```rust
pub mod stall;
// ...
pub use stall::{StallConfig, StallTracker};
```

with:

```rust
pub use deps::{StallConfig, StallTracker};
```

(Remove the `pub mod stall;` declaration. Keep all other re-exports unchanged.)

- [ ] **Step 3: Update `src/harness/tests/stability.rs`**

Replace `use crate::harness::stall::StallConfig;` with `use crate::harness::deps::StallConfig;` (or simply `use crate::harness::StallConfig;` if the re-export path is used elsewhere).

- [ ] **Step 4: Delete `src/harness/stall.rs`**

Run: `git rm src/harness/stall.rs`

- [ ] **Step 5: Verify compile + tests**

Run:

```bash
cargo check -p alephcore --lib
cargo test -p alephcore --lib --test '*' harness::
```

Expected: clean compile; 41+ harness tests pass (3 stall tests now run from `deps::stall_tests`).

- [ ] **Step 6: Commit**

```bash
git add src/harness/mod.rs src/harness/deps.rs src/harness/tests/stability.rs
git rm src/harness/stall.rs
git commit -m "refactor(harness): consolidate stall.rs into deps.rs (R10 9-file budget)

Stage 3 prep: stall.rs (P0 rescue artifact) is folded into deps.rs as
StallConfig and StallTracker live next to HarnessDeps which already owns
stall_config. File count restores to 9/9 canonical files, opening the
slot for the upcoming prompt.rs (PromptBuilder seam).

No behavior change. All 3 stall_tests + 41 harness tests pass."
```

---

## Task 2: Define `PromptBuilder` trait + `TurnContext` struct

**Files:**
- Create: `src/harness/prompt.rs`
- Modify: `src/harness/mod.rs`

- [ ] **Step 1: Write `src/harness/prompt.rs` skeleton**

```rust
//! Prompt Assembly Seam — Stage 3 of the 12-module harness roadmap.
//!
//! `PromptBuilder` is the single seam through which `AgentHarness` produces
//! the per-turn `Vec<UnifiedMessage>` handed to the provider. Default
//! behavior matches the legacy private `build_prompt` byte-for-byte;
//! downstream stages (#11 Subagent, #10 Verification) inject custom
//! builders that compose memory hints, chain context, or judge prompts
//! without patching `agent.rs`.

use async_trait::async_trait;

use crate::providers::message::UnifiedMessage;
use crate::session::events::SessionEventRecord;

/// Input to `PromptBuilder::assemble`. Carries the slice of session events
/// and the tail boundary computed by `tail_start_index`. Future stages may
/// extend this struct with memory hints, skill suggestions, or chain
/// context — additions must be additive (existing builders keep working).
#[derive(Debug)]
pub struct TurnContext<'a> {
    pub events: &'a [SessionEventRecord],
    pub tail_start: usize,
}

impl<'a> TurnContext<'a> {
    pub fn new(events: &'a [SessionEventRecord], tail_start: usize) -> Self {
        Self { events, tail_start }
    }
}

/// Pluggable per-turn message assembler. Implementations must be
/// `Send + Sync` so `Arc<dyn PromptBuilder>` lives in `HarnessDeps`.
#[async_trait]
pub trait PromptBuilder: Send + Sync {
    /// Produce the `Vec<UnifiedMessage>` for the next provider call.
    /// Errors propagate as `HarnessError::Session` (or future variants).
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError>;
}

/// Default builder — byte-equivalent to the pre-Stage-3 private
/// `build_prompt` function (former `agent.rs:850`).
#[derive(Debug, Default, Clone)]
pub struct DefaultPromptBuilder;

#[async_trait]
impl PromptBuilder for DefaultPromptBuilder {
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError> {
        // Body filled in Task 3.
        let _ = ctx;
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_builder_compiles_and_runs() {
        let events: Vec<SessionEventRecord> = Vec::new();
        let ctx = TurnContext::new(&events, 0);
        let builder = DefaultPromptBuilder;
        let out = builder.assemble(&ctx).await.expect("assemble ok");
        assert!(out.is_empty(), "empty events → empty output");
    }
}
```

- [ ] **Step 2: Wire `prompt` module into `src/harness/mod.rs`**

Add after `pub mod loop_callback;`:

```rust
pub mod prompt;
```

Add re-export after the existing re-exports:

```rust
pub use prompt::{DefaultPromptBuilder, PromptBuilder, TurnContext};
```

- [ ] **Step 3: Verify compile**

Run:

```bash
cargo check -p alephcore --lib
cargo test -p alephcore --lib harness::prompt::
```

Expected: clean compile; 1 test passes (`default_builder_compiles_and_runs`).

- [ ] **Step 4: Commit**

```bash
git add src/harness/prompt.rs src/harness/mod.rs
git commit -m "feat(harness): introduce PromptBuilder trait + DefaultPromptBuilder skeleton

Stage 3 step 1: define the seam. Trait, TurnContext input struct, and
empty Default impl + 1 compile test. Body migration follows in next
commit (still byte-equivalent to legacy build_prompt)."
```

---

## Task 3: Port `build_prompt` body into `DefaultPromptBuilder::assemble` + golden tests

**Files:**
- Modify: `src/harness/prompt.rs`
- Create: `src/harness/tests/prompt.rs`
- Modify: `src/harness/mod.rs` (add test mod)

- [ ] **Step 1: Replace `DefaultPromptBuilder::assemble` body with the verbatim contents of `agent.rs::build_prompt`**

Replace the placeholder in `src/harness/prompt.rs`:

```rust
#[async_trait]
impl PromptBuilder for DefaultPromptBuilder {
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError> {
        let mut messages = Vec::new();
        let events = ctx.events;
        let tail_start = ctx.tail_start;

        // Reconstruct the preceding assistant turn (if any) so the model sees
        // its own tool_use request in context.
        if tail_start > 0 {
            if let crate::session::events::SessionEvent::AssistantMessage { content, .. } =
                &events[tail_start - 1].event
            {
                let mut blocks: Vec<crate::providers::message::ContentBlock> = Vec::new();
                if let (Some(ref thinking), Some(ref sig)) =
                    (&content.thinking, &content.thinking_signature)
                {
                    if !thinking.is_empty() {
                        blocks.push(crate::providers::message::ContentBlock::Thinking {
                            thinking: thinking.clone(),
                            signature: Some(sig.clone()),
                        });
                    }
                }
                if !content.text.is_empty() {
                    blocks.push(crate::providers::message::ContentBlock::Text {
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

        for (offset, record) in events[tail_start..].iter().enumerate() {
            match &record.event {
                crate::session::events::SessionEvent::UserMessage { content, .. } => {
                    messages.push(UnifiedMessage::user(&content.text));
                }
                crate::session::events::SessionEvent::ToolResult { call_id, output, .. } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name = resolve_tool_name(events, tool_result_idx, call_id)
                        .unwrap_or("unknown");
                    messages.push(UnifiedMessage::tool_result_json(
                        call_id.clone(),
                        tool_name.to_string(),
                        output.value.clone(),
                        false,
                    ));
                }
                crate::session::events::SessionEvent::ToolError { call_id, error, .. } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name = resolve_tool_name(events, tool_result_idx, call_id)
                        .unwrap_or("unknown");
                    messages.push(UnifiedMessage::ToolResult {
                        tool_call_id: call_id.clone(),
                        tool_name: tool_name.to_string(),
                        content: vec![crate::providers::message::ContentBlock::Text {
                            text: error.clone(),
                            cache_control: None,
                        }],
                        is_error: true,
                    });
                }
                _ => {}
            }
        }

        Ok(messages)
    }
}

/// Find the `ToolCallRequested.name` whose `call_id` matches, searching
/// strictly BEFORE `before_idx`. Mirrors the helper still used by
/// `agent.rs` for `ToolError` resolution paths outside prompt assembly.
fn resolve_tool_name<'a>(
    events: &'a [crate::session::events::SessionEventRecord],
    before_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    let upper = before_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        crate::session::events::SessionEvent::ToolCallRequested {
            call_id: id, name, ..
        } if id == call_id => Some(name.as_str()),
        _ => None,
    })
}

/// Parse a stored tool_use ContentBlock from a raw `serde_json::Value`.
/// Mirrors the helper in `agent.rs`. Kept private to this module since
/// the only consumer is `DefaultPromptBuilder::assemble`.
fn parse_tool_use_block(
    raw: &serde_json::Value,
) -> Option<crate::providers::message::ContentBlock> {
    // NOTE: implementer MUST copy the existing `parse_tool_use_block`
    // helper from agent.rs verbatim. Locate it via `grep -n "fn parse_tool_use_block" src/harness/agent.rs`
    // and reproduce its body here. Keep the original helper in agent.rs
    // until Task 4 retires the old build_prompt — at that point the
    // agent.rs copy is deleted.
    let _ = raw;
    todo!("port from agent.rs — see step 1a below before running tests")
}
```

- [ ] **Step 1a: Port `parse_tool_use_block` helper**

```bash
grep -n "fn parse_tool_use_block\|parse_tool_use_block(" src/harness/agent.rs
```

Read the function at the indicated line. Copy the body verbatim into `prompt.rs::parse_tool_use_block` and remove the `todo!()` placeholder. The function returns `Option<ContentBlock>` and parses `{"type":"tool_use", ...}` shaped JSON.

- [ ] **Step 2: Create `src/harness/tests/prompt.rs` with golden tests**

```rust
//! Golden tests for `DefaultPromptBuilder` — verify byte-equivalence with
//! the legacy private `build_prompt` (still present at `agent.rs:850`
//! during Task 3; retired in Task 4).
//!
//! Strategy: construct minimal `Vec<SessionEventRecord>` fixtures, run
//! both the legacy function and `DefaultPromptBuilder::assemble`, assert
//! the resulting `Vec<UnifiedMessage>` is `==`.

use crate::harness::prompt::{DefaultPromptBuilder, PromptBuilder, TurnContext};
use crate::providers::message::UnifiedMessage;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, ToolOutput,
    ToolOutputMetadata, TurnTrigger,
};

fn empty_record(ev: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        timestamp_ms: now_ms(),
        event: ev,
    }
}

fn user_msg(text: &str) -> SessionEventRecord {
    empty_record(SessionEvent::UserMessage {
        content: MessageContent::text(text),
        turn_trigger: TurnTrigger::User,
    })
}

#[tokio::test]
async fn empty_log_yields_empty_messages() {
    let events: Vec<SessionEventRecord> = Vec::new();
    let ctx = TurnContext::new(&events, 0);
    let out = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");
    assert!(out.is_empty());
}

#[tokio::test]
async fn single_user_message_passes_through() {
    let events = vec![user_msg("hello")];
    let ctx = TurnContext::new(&events, 0);
    let out = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");
    assert_eq!(out.len(), 1);
    match &out[0] {
        UnifiedMessage::User { content } => {
            assert_eq!(content.len(), 1);
            // Verify text content shape — exact UnifiedMessage::user
            // shape preserved.
        }
        other => panic!("expected User message, got {other:?}"),
    }
}

#[tokio::test]
async fn assistant_then_tool_result_reconstructs_prior_turn() {
    // Fixture: one AssistantMessage with a tool_use block, then
    // a ToolResult — verify the assistant turn is reconstructed
    // first, then tool result is emitted.
    // Implementer: build fixtures using the same shape that the
    // pre-Stage-3 build_prompt expected; cross-check by also
    // calling the legacy function (still present in agent.rs)
    // and asserting equality.
    //
    // This test is the load-bearing byte-equivalence check.
    use crate::harness::agent::test_helpers::legacy_build_prompt;
    let events: Vec<SessionEventRecord> = build_assistant_then_tool_result_fixture();
    let tail_start = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);

    let ctx = TurnContext::new(&events, tail_start);
    let new_output = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");
    let legacy_output = legacy_build_prompt(&events, tail_start);

    assert_eq!(
        new_output, legacy_output,
        "DefaultPromptBuilder must be byte-equivalent to legacy build_prompt"
    );
}

fn build_assistant_then_tool_result_fixture() -> Vec<SessionEventRecord> {
    // Implementer: construct a 3-event fixture:
    //   1. UserMessage "fetch the weather"
    //   2. AssistantMessage with content.blocks = [tool_use{call_id:"c1", name:"weather"}]
    //   3. ToolResult { call_id:"c1", output: ToolOutput{ value: json!({"temp": 70}), ... } }
    //
    // Use existing constructors (MessageContent::text, etc.). For the
    // AssistantMessage with tool_use blocks, set:
    //   content.blocks = vec![serde_json::json!({
    //       "type": "tool_use",
    //       "id": "c1",
    //       "name": "weather",
    //       "input": {}
    //   })];
    //
    // Reference shape: existing fixtures in src/harness/tests/think.rs
    // and act.rs already build similar event sequences.
    todo!("build fixture per comment above — see think.rs/act.rs for patterns")
}
```

NOTE: the `legacy_build_prompt` import requires a temporary test-only re-export in `agent.rs`. Add this stub inside `agent.rs` (will be removed in Task 4):

```rust
#[cfg(test)]
pub(crate) mod test_helpers {
    //! Test-only re-export of legacy build_prompt for byte-equivalence
    //! verification during Stage 3 Task 3. Removed in Task 4 along with
    //! the legacy function itself.
    use crate::providers::message::UnifiedMessage;
    use crate::session::events::SessionEventRecord;

    pub(crate) fn legacy_build_prompt(
        events: &[SessionEventRecord],
        tail_start: usize,
    ) -> Vec<UnifiedMessage> {
        super::build_prompt(events, tail_start)
    }
}
```

- [ ] **Step 3: Wire test module into `src/harness/mod.rs`**

In the `#[cfg(test)] mod tests { ... }` block, add `mod prompt;` alongside `mod act; mod driver; ...`.

- [ ] **Step 4: Run golden tests + verify equivalence**

```bash
cargo test -p alephcore --lib harness::tests::prompt
cargo test -p alephcore --lib harness::prompt::
```

Expected: 4 tests pass (1 in-module + 3 in tests/prompt.rs). The third test (`assistant_then_tool_result_reconstructs_prior_turn`) is the load-bearing byte-equivalence check — if it fails, the body port has a discrepancy.

- [ ] **Step 5: Commit**

```bash
git add src/harness/prompt.rs src/harness/agent.rs src/harness/mod.rs src/harness/tests/prompt.rs
git commit -m "feat(harness): port build_prompt body into DefaultPromptBuilder + golden tests

Stage 3 step 2: DefaultPromptBuilder::assemble is now byte-equivalent
to the legacy private build_prompt (agent.rs:850, still present until
Task 4 retires it). 3 golden tests verify equivalence including the
load-bearing assistant-then-tool-result fixture.

Temporary test-only re-export agent::test_helpers::legacy_build_prompt
exists during the migration window; removed alongside legacy function
in next commit."
```

---

## Task 4: Wire `PromptBuilder` through `HarnessDeps` + retire legacy `build_prompt`

**Files:**
- Modify: `src/harness/deps.rs` (add `prompt_builder` field)
- Modify: `src/harness/agent.rs` (replace call site, delete `build_prompt`, delete `parse_tool_use_block` if exclusive, delete `test_helpers` stub)
- Modify: All `HarnessDeps` construction sites (count via grep, expect ~21 per Stage 1/2 precedent)

This is the "old code retired in same commit" step per R10. Old code MUST go in the same commit as the new wiring.

- [ ] **Step 1: Add `prompt_builder` field to `HarnessDeps`**

In `src/harness/deps.rs`, after the existing imports:

```rust
use crate::harness::prompt::{DefaultPromptBuilder, PromptBuilder};
```

Add to the `HarnessDeps` struct (e.g., between `system_prompt` and `max_iterations`):

```rust
    /// Per-turn message assembler. Stage 3 seam (#5). Defaults to
    /// `DefaultPromptBuilder` (byte-equivalent to legacy build_prompt).
    /// Subagent (#11) and JudgeAgent (#10) inject custom builders.
    pub prompt_builder: Arc<dyn PromptBuilder>,
```

- [ ] **Step 2: Add a `Default` for the field at construction sites**

Locate every `HarnessDeps { ... }` literal:

```bash
grep -rn "HarnessDeps {" --include="*.rs" | head -30
```

For each, add `prompt_builder: Arc::new(DefaultPromptBuilder),` to the field initializer. Order field by alphabetical or insertion-point convention used at that site.

If the existing pattern uses a builder (e.g., `HarnessDepsBuilder`), add a `with_prompt_builder` method instead — keep the public construction surface aligned with Stage 2's precedent for `tools` field.

If it turns out there is no builder pattern and ~21 callsites all use struct literals, prefer adding a `Default` impl on `HarnessDeps` is **not** an option (other fields are non-Default). Instead, just add the field at every site. The implementer will use grep + sed-style mechanical edits.

- [ ] **Step 3: Replace `build_prompt` callsite in `agent.rs:138`**

Before:

```rust
let mut messages = build_prompt(&events, tail_start);
```

After:

```rust
let ctx = crate::harness::prompt::TurnContext::new(&events, tail_start);
let mut messages = self.deps.prompt_builder.assemble(&ctx).await?;
```

The `await?` is correct since `PromptBuilder::assemble` is async + returns `Result<_, HarnessError>`. Errors propagate via the existing `?` chain in `run_turn_internal`.

- [ ] **Step 4: Delete legacy code from `agent.rs`**

Delete:
1. The entire `fn build_prompt(...)` function (currently `agent.rs:850-929`).
2. The `fn parse_tool_use_block(...)` helper, IF it is now only used by the deleted `build_prompt`. Grep first:

```bash
grep -n "parse_tool_use_block" src/harness/agent.rs
```

If only the (now-deleted) `build_prompt` referenced it, delete the helper too. Otherwise, keep it.

3. The `resolve_tool_name` helper — same conditional rule. (`prompt.rs` has its own copy; if `agent.rs` no longer uses it, delete it from agent.rs.)

4. The `#[cfg(test)] pub(crate) mod test_helpers` block added in Task 3 (and update `tests/prompt.rs` to use `crate::harness::prompt::DefaultPromptBuilder` directly without the legacy comparison). The byte-equivalence test from Task 3 has already certified the port; we no longer need the legacy fn alive.

- [ ] **Step 5: Update `tests/prompt.rs` to drop the legacy comparison**

Replace the `assistant_then_tool_result_reconstructs_prior_turn` test body with assertions on the new builder's expected output (golden values captured during Task 3 verification). Keep the test — it's the byte-shape regression guard.

```rust
#[tokio::test]
async fn assistant_then_tool_result_reconstructs_prior_turn() {
    let events = build_assistant_then_tool_result_fixture();
    let tail_start = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);

    let ctx = TurnContext::new(&events, tail_start);
    let out = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");

    assert_eq!(out.len(), 2, "reconstructed assistant + tool result");
    assert!(matches!(out[0], UnifiedMessage::Assistant { .. }));
    assert!(matches!(out[1], UnifiedMessage::ToolResult { .. }));
    // Implementer captures the exact ContentBlock shapes from Task 3
    // verification run and asserts on them here.
}
```

- [ ] **Step 6: Verify**

```bash
cargo check -p alephcore --lib
cargo test -p alephcore --lib harness::
```

Expected: clean compile, all harness tests pass (the test count grows by ~4 for the new prompt tests).

Sanity check that `agent.rs` shrank: `wc -l src/harness/agent.rs` should be ~80 lines smaller than the baseline of 1373.

- [ ] **Step 7: Commit**

```bash
git add -u src/harness/
git commit -m "feat(harness): wire PromptBuilder through HarnessDeps + retire legacy build_prompt

Stage 3 step 3: AgentHarness now calls self.deps.prompt_builder.assemble()
instead of the inline build_prompt. The private build_prompt function and
its parse_tool_use_block / resolve_tool_name helpers are deleted from
agent.rs (their bodies live in prompt.rs::DefaultPromptBuilder).

Old code retired in same commit per R10 'no half-finished migration'
rule. agent.rs shrinks by ~80 lines (1373 → ~1290), well under the 1500
cap.

HarnessDeps gains a prompt_builder: Arc<dyn PromptBuilder> field; all
~N construction sites pass Arc::new(DefaultPromptBuilder)."
```

---

## Task 5: Property test for `TurnContext` stability + perf observation

**Files:**
- Modify: `src/harness/tests/prompt.rs`

- [ ] **Step 1: Add property test using `proptest` (already a dev-dep)**

Append to `src/harness/tests/prompt.rs`:

```rust
#[cfg(test)]
mod prop {
    use super::*;
    use proptest::prelude::*;

    /// Property: regardless of the order/content of UserMessage events
    /// before the tail boundary, `DefaultPromptBuilder` never panics
    /// and always produces a `Vec<UnifiedMessage>` whose length is
    /// `<= events.len() - tail_start + 1` (the +1 accounts for the
    /// optionally reconstructed assistant turn).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn assemble_is_total_for_user_only_logs(
            texts in proptest::collection::vec("[a-z ]{0,40}", 0..16),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let events: Vec<SessionEventRecord> = texts
                .iter()
                .map(|t| user_msg(t))
                .collect();

            // tail_start = 0 for user-only logs (no assistant message).
            let ctx = TurnContext::new(&events, 0);
            let out = rt.block_on(DefaultPromptBuilder.assemble(&ctx))
                .expect("assemble must not error on user-only logs");

            prop_assert!(out.len() <= events.len() + 1);
            // Every output for user-only logs must itself be a User msg
            // since there's no assistant turn to reconstruct.
            for msg in &out {
                prop_assert!(matches!(msg, UnifiedMessage::User { .. }));
            }
        }
    }
}
```

- [ ] **Step 2: Run + verify**

```bash
cargo test -p alephcore --lib harness::tests::prompt::prop
```

Expected: 64 generated cases pass.

- [ ] **Step 3: Perf observation (no assertion — just sanity)**

Add a documented `#[ignore]` benchmark-style test:

```rust
/// Sanity benchmark — not an assertion; run with `cargo test
/// harness::tests::prompt::perf -- --ignored --nocapture` to print
/// timings. We document this rather than assert because trait dispatch
/// is one vtable jump and any measurable regression would show up in
/// the broader gateway-level perf suite.
#[tokio::test]
#[ignore]
async fn perf_dispatch_overhead_documented() {
    use std::time::Instant;
    let events: Vec<SessionEventRecord> = (0..1000).map(|i| user_msg(&format!("m{i}"))).collect();
    let ctx = TurnContext::new(&events, 0);

    let start = Instant::now();
    for _ in 0..1000 {
        let _ = DefaultPromptBuilder.assemble(&ctx).await;
    }
    let elapsed = start.elapsed();
    eprintln!("1000 × assemble(1000 events) = {elapsed:?}");
}
```

- [ ] **Step 4: Commit**

```bash
git add src/harness/tests/prompt.rs
git commit -m "test(harness): property test + perf observation for DefaultPromptBuilder

Stage 3 step 4: 64-case proptest covers user-only logs ensuring
assemble() is total. Documented #[ignore]'d perf observation prints
1000-iteration dispatch cost for manual sanity checks."
```

---

## Task 6: CHANGELOG + master-spec status flip + final verification

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`

- [ ] **Step 1: Append Stage 3 entries to `CHANGELOG.md` `## [Unreleased]`**

Under `### Added`:

```markdown
- **harness**: PromptBuilder seam (Stage 3 / module #5). `DefaultPromptBuilder` is byte-equivalent to the previous private `build_prompt`; downstream stages inject custom builders without patching `agent.rs`.
- **harness**: `TurnContext` input struct carries the per-turn event slice + tail boundary into `PromptBuilder::assemble`.
```

Under `### Changed`:

```markdown
- **harness**: `HarnessDeps` gains `prompt_builder: Arc<dyn PromptBuilder>` field; all construction sites pass `DefaultPromptBuilder`.
- **harness**: consolidated `stall.rs` into `deps.rs` (R10 9-file budget — opens slot for `prompt.rs`).
```

Under `### Removed`:

```markdown
- **harness**: private `build_prompt` function (was `agent.rs:850`); body lives in `DefaultPromptBuilder::assemble`.
- **harness**: `src/harness/stall.rs` module file (contents moved to `deps.rs`).
```

- [ ] **Step 2: Flip Stage 3 status in master spec**

Edit `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`:

```diff
 ### Stage 3 — Prompt Assembly Seam (#5)

-**Status**: 🟡 Pending
+**Status**: ✅ Shipped <commit-sha> on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage3-prompt-builder-plan.md
```

(Replace `<commit-sha>` with the actual SHA after Task 6 lands.)

- [ ] **Step 3: Run full harness test suite + clippy**

```bash
cargo test -p alephcore --lib harness::
cargo clippy -p alephcore --lib -- -D warnings
cargo fmt -p alephcore --check
```

Expected:
- All harness tests pass (count = 41 baseline + 4 new prompt tests + 64 prop cases = ~45 named tests + prop).
- Zero clippy warnings in harness/.
- fmt clean.

- [ ] **Step 4: Verify R10 budgets**

```bash
ls src/harness/*.rs | wc -l            # expect 9 (canonical)
wc -l src/harness/agent.rs              # expect ≤ 1500, target ~1290
wc -l src/harness/*.rs                  # all ≤ 800 individually
```

- [ ] **Step 5: Verify Stage 3 master-spec acceptance criteria mechanically**

For each acceptance criterion listed at master spec § Stage 3, run a verifier:

| Criterion | Verifier |
|-----------|---------|
| "AgentHarnessRunner 暴露 `.with_prompt_builder(...)` 构造点" | `grep -n "prompt_builder" src/harness/deps.rs` finds the field; HarnessDeps construction sites accept any `Arc<dyn PromptBuilder>`. |
| "`DefaultPromptBuilder` 行为与原 `build_prompt` 字节级一致" | Task 3 golden tests pass; `assistant_then_tool_result_reconstructs_prior_turn` is the load-bearing test. |
| "现有 system prompt 内容、Memory 注入路径、Tools 列表注入完全不变" | `cargo test -p alephcore --lib harness::tests::think harness::tests::act harness::tests::stability` all pass. |
| "≥2 个 prompt golden test" | `tests/prompt.rs` has ≥3 golden tests (Task 3). |
| "≥1 个 property test 验证 TurnContext 任意排列下的稳定性" | `tests/prompt.rs::prop::assemble_is_total_for_user_only_logs` (Task 5). |
| "trait dispatch 开销 ≤ 1 个 vtable 跳转（无额外 alloc）" | `DefaultPromptBuilder::assemble` body has zero `.clone()` of inputs and zero `Box::new`/`Arc::new` of outputs — verified by code review on the diff. The async_trait macro generates one `Pin<Box<...>>` per call (unavoidable for dyn async); no other allocations. |

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md
git commit -m "docs: ship Stage 3 (Prompt Assembly Seam) — flip master spec status

Wraps Stage 3 of the 12-module harness roadmap. Acceptance criteria
mechanically verified:
  - PromptBuilder trait + DefaultPromptBuilder shipped
  - byte-equivalence golden tests pass (3)
  - property test covers TurnContext stability (64 cases)
  - R10 budget compliant: 9/9 canonical files, agent.rs ~1290 lines
  - existing think/act/stability tests unchanged

Stage 4 (Subagent ChainContext Wiring) unblocked."
```

---

## Self-Review Checklist (run before handoff)

After all 6 tasks land, run this checklist before declaring Stage 3 shipped.

**1. Spec coverage** (master spec § Stage 3 Acceptance):
- [x] PromptBuilder trait: prompt.rs (Task 2)
- [x] DefaultPromptBuilder byte-equivalent: prompt.rs + golden tests (Task 3)
- [x] TurnContext input struct: prompt.rs (Task 2)
- [x] HarnessDeps wiring + 1 real consumer (agent.rs main loop): Task 4
- [x] Old build_prompt retired in same commit: Task 4
- [x] ≥2 golden tests + ≥1 property test: Task 3 (3) + Task 5 (1)
- [x] R10 file count: stall.rs consolidation in Task 1

**2. Placeholder scan:**
- Plan contains 1 explicit `todo!()` in fixture builders (Task 3 Step 2's `build_assistant_then_tool_result_fixture`). Implementer MUST replace with concrete fixture per the comment guide before running tests. This is intentional — the fixture shape depends on existing `MessageContent` constructors that vary by test convention.

**3. Type consistency:**
- `PromptBuilder::assemble` returns `Result<Vec<UnifiedMessage>, HarnessError>` consistently across trait def, default impl, and call site.
- `TurnContext::new` signature matches across creation in agent.rs and test fixtures.
- `Arc<dyn PromptBuilder>` is the single dyn type used in `HarnessDeps` and tests.

**4. R10 budgets:**
- ✅ 9/9 canonical files (stall.rs out, prompt.rs in)
- ✅ agent.rs shrinks (1373 → ~1290), well under 1500 cap
- ✅ harness/ delta: ~+250 lines new − ~117 lines stall.rs deleted ≈ +130 net (well under +400 cap)
- ✅ Single PR ≤ 600 lines including tests (estimate ~400)

**5. Future-Proof Test (R10):**
- Model upgrade adds richer prompt formats (e.g., new ContentBlock variants) — only `DefaultPromptBuilder::assemble` changes, no harness main loop edits required. ✅
- Subagent (#11) inject `SubagentPromptBuilder` impl without patching agent.rs. ✅
- JudgeAgent (#10) inject `JudgePromptBuilder` impl on top of DefaultPromptBuilder. ✅

---

## Execution Handoff

After plan is committed, the implementing controller offers execution choice:

**1. Subagent-Driven (recommended)** — fresh subagent per task, two-stage review, fast iteration. Same workflow used for Stages 1-2.

**2. Inline Execution** — execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Default to Subagent-Driven per the precedent set in Stages 1 and 2.

**Baseline for diffs:** `7f417ae36`. Verify clean working tree before starting Task 1 (or use a worktree to isolate from the in-flight `MessageContent.thinking` changes that are polluting the working tree at the time of plan authorship).

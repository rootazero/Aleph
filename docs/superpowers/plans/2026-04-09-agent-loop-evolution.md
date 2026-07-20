# Agent Loop Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Aleph's agent loop with multi-layer context management, refined tool execution, and robust recovery mechanisms — learning from Claude Code while preserving Aleph's Rust-native architecture.

**Architecture:** Three-phase context management (inline budget → pre-flight pipeline → emergency compaction), per-tool execution contexts with cascading abort, and escalating recovery for truncation and 413 errors.

**Tech Stack:** Rust, async-trait, tokio (mpsc, CancellationToken), serde_json, xxhash (via xxhash-rust crate)

**Spec:** `docs/superpowers/specs/2026-04-08-agent-loop-evolution-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `src/agent_loop/context_budget/preflight.rs` | `PreflightStage` async trait + `PreflightPipeline` orchestrator |
| `src/agent_loop/context_budget/microcompact.rs` | Content-addressed tool result dedup |
| `src/agent_loop/context_budget/context_collapse.rs` | Semantic message group folding |
| `src/agent_loop/context_budget/autocompact.rs` | LLM-based conversation summarization |
| `src/agent_loop/tool_execution_context.rs` | `ToolExecutionContext`, `CascadePolicy`, `ToolProgress` |

### Modified files

| File | Changes |
|------|---------|
| `src/agent_loop/context_budget/mod.rs` | Add `pub mod preflight/microcompact/context_collapse/autocompact` |
| `src/agent_loop/mod.rs` | Add `pub mod tool_execution_context` |
| `src/agent_loop/tool.rs` | Add `max_result_tokens: Option<usize>` to `ToolDefinition` |
| `src/agent_loop/tool_pipeline.rs` | Per-tool budget with head+tail truncation |
| `src/agent_loop/streaming_bridge.rs` | Integrate `ToolExecutionContext`, batch cancel, progress channel |
| `src/agent_loop/loop_core.rs` | Call preflight in `prepare_turn()`, enhance 413 recovery |
| `src/agent_loop/truncation_recovery.rs` | Add escalation loop with doubling strategy |
| `src/providers/message.rs` | Add `CacheControl` to `ContentBlock::Text` |

---

## Task 1: PreflightStage Trait + PreflightPipeline Skeleton

**Files:**
- Create: `src/agent_loop/context_budget/preflight.rs`
- Modify: `src/agent_loop/context_budget/mod.rs`
- Test: inline `#[cfg(test)]` in `preflight.rs`

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/context_budget/preflight.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    struct MockStage {
        name: &'static str,
        freed: usize,
        min_ratio: f64,
    }

    #[async_trait::async_trait]
    impl PreflightStage for MockStage {
        fn name(&self) -> &'static str { self.name }
        async fn prepare(
            &self,
            _messages: &mut Vec<UnifiedMessage>,
            pressure: &super::super::ContextPressure,
            _fresh_tail_count: usize,
        ) -> usize {
            if pressure.ratio < self.min_ratio { return 0; }
            self.freed
        }
    }

    #[tokio::test]
    async fn pipeline_runs_stages_in_order() {
        let pipeline = PreflightPipeline::new(vec![
            Box::new(MockStage { name: "a", freed: 100, min_ratio: 0.0 }),
            Box::new(MockStage { name: "b", freed: 200, min_ratio: 0.0 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let pressure = super::super::ContextPressure {
            used_tokens: 800,
            budget_tokens: 1000,
            ratio: 0.8,
            overhead_tokens: 100,
            available_for_messages: 900,
        };
        let freed = pipeline.run(&mut msgs, &pressure, 2).await;
        assert_eq!(freed, 300);
    }

    #[tokio::test]
    async fn pipeline_respects_stage_threshold() {
        let pipeline = PreflightPipeline::new(vec![
            Box::new(MockStage { name: "low", freed: 100, min_ratio: 0.3 }),
            Box::new(MockStage { name: "high", freed: 200, min_ratio: 0.9 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let pressure = super::super::ContextPressure {
            used_tokens: 500,
            budget_tokens: 1000,
            ratio: 0.5,
            overhead_tokens: 100,
            available_for_messages: 900,
        };
        let freed = pipeline.run(&mut msgs, &pressure, 2).await;
        // "low" fires (0.5 >= 0.3), "high" doesn't (0.5 < 0.9)
        assert_eq!(freed, 100);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget::preflight -- --nocapture`
Expected: FAIL — module doesn't exist yet

- [ ] **Step 3: Write the implementation**

In `src/agent_loop/context_budget/preflight.rs`:

```rust
//! PreflightPipeline — async context preparation before each Think turn.
//!
//! Complements the synchronous `CompactionPipeline` (emergency layer) with
//! an async pipeline that runs progressively aggressive stages gated by
//! pressure thresholds.

use async_trait::async_trait;

use super::ContextPressure;
use crate::providers::message::UnifiedMessage;

// =============================================================================
// PreflightStage trait
// =============================================================================

/// A single pre-flight context preparation stage.
///
/// Unlike the synchronous `CompactionStage` (emergency layer), pre-flight
/// stages are async — enabling LLM calls (e.g., Autocompact) and I/O.
#[async_trait]
pub trait PreflightStage: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &'static str;

    /// Prepare context by modifying messages in place.
    ///
    /// Returns estimated tokens freed. Each stage internally checks
    /// `pressure.ratio` against its own threshold and returns 0 if
    /// pressure is below the activation point.
    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize;
}

// =============================================================================
// PreflightPipeline
// =============================================================================

/// Executes an ordered list of `PreflightStage`s before each Think turn.
pub struct PreflightPipeline {
    stages: Vec<Box<dyn PreflightStage>>,
}

impl PreflightPipeline {
    /// Create a new pipeline with the given ordered stages.
    pub fn new(stages: Vec<Box<dyn PreflightStage>>) -> Self {
        Self { stages }
    }

    /// Create an empty pipeline (no-op).
    pub fn empty() -> Self {
        Self { stages: vec![] }
    }

    /// Run all stages in order. Returns total tokens freed.
    pub async fn run(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        let mut total_freed = 0;
        for stage in &self.stages {
            let freed = stage.prepare(messages, pressure, fresh_tail_count).await;
            total_freed += freed;
            if freed > 0 {
                tracing::info!(
                    target: "preflight",
                    stage = stage.name(),
                    tokens_freed = freed,
                    total_freed,
                    "Pre-flight stage completed"
                );
            }
        }
        total_freed
    }
}
```

- [ ] **Step 4: Register module in mod.rs**

In `src/agent_loop/context_budget/mod.rs`, add after existing `pub mod` lines:

```rust
pub mod preflight;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib context_budget::preflight -- --nocapture`
Expected: PASS (2 tests)

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/context_budget/preflight.rs src/agent_loop/context_budget/mod.rs
git commit -m "feat(agent_loop): add PreflightStage trait and PreflightPipeline"
```

---

## Task 2: ToolExecutionContext + CascadePolicy + ToolProgress

**Files:**
- Create: `src/agent_loop/tool_execution_context.rs`
- Modify: `src/agent_loop/mod.rs`
- Test: inline `#[cfg(test)]` in `tool_execution_context.rs`

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/tool_execution_context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bash_as_abort_siblings() {
        assert!(matches!(CascadePolicy::classify("Bash"), CascadePolicy::AbortSiblings));
    }

    #[test]
    fn classify_write_as_abort_siblings() {
        assert!(matches!(CascadePolicy::classify("Write"), CascadePolicy::AbortSiblings));
    }

    #[test]
    fn classify_edit_as_abort_siblings() {
        assert!(matches!(CascadePolicy::classify("Edit"), CascadePolicy::AbortSiblings));
    }

    #[test]
    fn classify_read_as_isolated() {
        assert!(matches!(CascadePolicy::classify("Read"), CascadePolicy::Isolated));
    }

    #[test]
    fn classify_unknown_as_isolated() {
        assert!(matches!(CascadePolicy::classify("SomeCustomTool"), CascadePolicy::Isolated));
    }

    #[tokio::test]
    async fn progress_send_receive() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        tx.send(ToolProgress::Status {
            tool_id: "t1".into(),
            message: "working...".into(),
        }).await.unwrap();
        drop(tx);

        let progress = rx.recv().await.unwrap();
        match progress {
            ToolProgress::Status { tool_id, message } => {
                assert_eq!(tool_id, "t1");
                assert_eq!(message, "working...");
            }
            _ => panic!("expected Status"),
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tool_execution_context -- --nocapture`
Expected: FAIL — module doesn't exist

- [ ] **Step 3: Write the implementation**

In `src/agent_loop/tool_execution_context.rs`:

```rust
//! ToolExecutionContext — per-tool execution context with independent
//! cancellation, progress reporting, and cascade policy.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// =============================================================================
// CascadePolicy
// =============================================================================

/// Determines what happens to concurrent siblings when a tool fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadePolicy {
    /// Failure cancels all concurrent siblings (Bash, Write, Edit).
    AbortSiblings,
    /// Failure is isolated — siblings continue (Read, Grep, Glob, etc.).
    Isolated,
}

impl CascadePolicy {
    /// Classify a tool by name into its cascade policy.
    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "Bash" | "Write" | "Edit" | "NotebookEdit" => Self::AbortSiblings,
            _ => Self::Isolated,
        }
    }
}

// =============================================================================
// ToolProgress
// =============================================================================

/// Progress update emitted during tool execution.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Human-readable status message.
    Status { tool_id: String, message: String },
    /// Partial output chunk (e.g., Bash stdout lines).
    PartialOutput { tool_id: String, chunk: String },
}

// =============================================================================
// ToolExecutionContext
// =============================================================================

/// Per-tool execution context providing independent lifecycle control.
///
/// Created for each tool call in a batch. The `cancel` token is a child of
/// the batch-level token, enabling both individual and batch-wide cancellation.
pub struct ToolExecutionContext {
    /// This tool's cancellation token (child of batch token).
    pub cancel: CancellationToken,
    /// Channel for streaming progress updates (best-effort via `try_send`).
    pub progress_tx: mpsc::Sender<ToolProgress>,
    /// What happens to siblings when this tool fails.
    pub cascade_policy: CascadePolicy,
}

impl ToolExecutionContext {
    /// Create a new context for a tool within a batch.
    pub fn new(
        batch_cancel: &CancellationToken,
        progress_tx: mpsc::Sender<ToolProgress>,
        tool_name: &str,
    ) -> Self {
        Self {
            cancel: batch_cancel.child_token(),
            progress_tx,
            cascade_policy: CascadePolicy::classify(tool_name),
        }
    }

    /// Send a progress update (non-blocking, best-effort).
    pub fn send_progress(&self, progress: ToolProgress) {
        let _ = self.progress_tx.try_send(progress);
    }
}
```

- [ ] **Step 4: Register module in mod.rs**

In `src/agent_loop/mod.rs`, add:

```rust
pub mod tool_execution_context;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tool_execution_context -- --nocapture`
Expected: PASS (6 tests)

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/tool_execution_context.rs src/agent_loop/mod.rs
git commit -m "feat(agent_loop): add ToolExecutionContext with cascade policy and progress"
```

---

## Task 3: Per-tool Result Budget

**Files:**
- Modify: `src/agent_loop/tool.rs` (add `max_result_tokens` field)
- Modify: `src/agent_loop/tool_pipeline.rs` (head+tail truncation)
- Test: existing tests in `tool_pipeline.rs` + new tests

- [ ] **Step 1: Add `max_result_tokens` to ToolDefinition**

In `src/agent_loop/tool.rs`, change:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
```

to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Per-tool result size limit in estimated tokens. Falls back to global default if None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_result_tokens: Option<usize>,
}
```

- [ ] **Step 2: Fix any compilation errors from missing field**

Run: `cargo check -p alephcore 2>&1 | head -30`

Search for all `ToolDefinition {` constructions and add `max_result_tokens: None`. Key locations:
- `src/agent_loop/context_budget/mod.rs` tests
- `src/agent_loop/context_budget/pipeline.rs` (not using it directly, uses `&[ToolDefinition]`)
- `src/agent_loop/tool_info.rs`
- Any other construction sites found by the compiler

- [ ] **Step 3: Write failing test for head+tail truncation**

In `src/agent_loop/tool_pipeline.rs` tests section, add:

```rust
#[test]
fn truncate_preserves_head_and_tail() {
    // Generate a string clearly over 6000 tokens (~15000 chars at 2.5 c/t)
    let mut lines = String::new();
    for i in 0..600 {
        lines.push_str(&format!("Line {:04}: content padding to fill space here\n", i));
    }
    let result = truncate_tool_result_with_budget(&lines, 6000);
    assert!(result.len() < lines.len(), "should be truncated");
    // Should contain early lines (head)
    assert!(result.contains("Line 0000"), "should preserve head");
    // Should contain late lines (tail)
    assert!(result.contains("Line 0599"), "should preserve tail");
    // Should contain truncation marker
    assert!(result.contains("truncated"), "should have truncation marker");
}

#[test]
fn truncate_within_budget_unchanged() {
    let short = "Hello, short result.";
    assert_eq!(truncate_tool_result_with_budget(short, 8000), short);
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tool_pipeline::tests::truncate_preserves -- --nocapture`
Expected: FAIL — function doesn't exist

- [ ] **Step 5: Implement head+tail truncation**

In `src/agent_loop/tool_pipeline.rs`, add the new function and update `map_result`:

```rust
/// Default per-tool result budgets.
fn default_result_budget(tool_name: &str) -> usize {
    match tool_name {
        "Read" => 12_000,
        "WebFetch" => 10_000,
        "Bash" => 8_000,
        "Grep" => 6_000,
        _ => MAX_TOOL_RESULT_TOKENS,
    }
}

/// Truncate a tool result with head+tail preservation.
///
/// Keeps 70% from the head and 30% from the tail, inserting a truncation
/// marker in the middle. This preserves error messages that often appear
/// at the end of tool output.
fn truncate_tool_result_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }

    let chars_per_token = 2.5_f64;
    let total_chars = (budget_tokens as f64 * chars_per_token) as usize;
    let head_chars = (total_chars as f64 * 0.7) as usize;
    let tail_chars = total_chars.saturating_sub(head_chars);

    // Find safe head boundary (last newline before head_chars)
    let head_end = text
        .char_indices()
        .take(head_chars)
        .filter(|(_, c)| *c == '\n')
        .last()
        .map(|(i, _)| i + 1)
        .unwrap_or(head_chars.min(text.len()));

    // Find safe tail boundary (first newline after text.len() - tail_chars)
    let tail_start_approx = text.len().saturating_sub(tail_chars * 4); // rough byte est
    let tail_start = text[tail_start_approx..]
        .find('\n')
        .map(|i| tail_start_approx + i + 1)
        .unwrap_or(tail_start_approx);

    if head_end >= tail_start {
        // Overlap — fall back to head-only truncation
        return truncate_tool_result(text);
    }

    let truncated_tokens = estimated - budget_tokens;
    format!(
        "{}\n\n[... truncated ~{} tokens ...]\n\n{}",
        &text[..head_end],
        truncated_tokens,
        &text[tail_start..],
    )
}
```

Then update `map_result` to use per-tool budget. Change the `ToolResult::Success` arm:

```rust
ToolResult::Success { output } => {
    let raw = value_to_text(output);
    let compressed = compress_tool_output(name, &raw);
    let budget = tool_budget.unwrap_or_else(|| default_result_budget(name));
    let final_text = match store
        .and_then(|s| s.persist_if_large(id, name, &compressed, budget))
    {
        Some(ref_marker) => ref_marker,
        None => truncate_tool_result_with_budget(&compressed, budget),
    };
    // ... rest unchanged
}
```

Update `map_result` signature to accept `tool_budget: Option<usize>` and propagate from the `execute` method using the registry to look up the tool's `max_result_tokens`.

- [ ] **Step 6: Run all tool_pipeline tests**

Run: `cargo test -p alephcore --lib tool_pipeline -- --nocapture`
Expected: PASS (all existing + 2 new tests)

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/tool.rs src/agent_loop/tool_pipeline.rs
git commit -m "feat(agent_loop): per-tool result budget with head+tail truncation"
```

---

## Task 4: Microcompact Stage

**Files:**
- Create: `src/agent_loop/context_budget/microcompact.rs`
- Modify: `src/agent_loop/context_budget/mod.rs` (add `pub mod microcompact`)
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/context_budget/microcompact.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::context_budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;

    fn high_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 800, budget_tokens: 1000, ratio: 0.8,
            overhead_tokens: 100, available_for_messages: 900,
        }
    }

    fn low_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 200, budget_tokens: 1000, ratio: 0.2,
            overhead_tokens: 100, available_for_messages: 900,
        }
    }

    #[tokio::test]
    async fn deduplicates_identical_tool_results() {
        let stage = MicrocompactStage::new();
        let long_output = "fn main() {}\n".repeat(200);
        let mut msgs = vec![
            UnifiedMessage::user("read the file"),
            UnifiedMessage::tool_result("c1", "Read", &long_output, false),
            UnifiedMessage::assistant("I see the code"),
            UnifiedMessage::user("read it again"),
            UnifiedMessage::tool_result("c2", "Read", &long_output, false),
            UnifiedMessage::assistant("same code"),
            UnifiedMessage::user("now do something"),  // fresh tail
        ];
        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;
        assert!(freed > 0, "should free tokens from duplicate");
        // First occurrence (index 1) should be replaced with compact ref
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert!(content.contains("[cached"), "old result should be cached ref, got: {}", content);
        // Second occurrence (index 4) should keep original (closer to fresh tail)
        let (_, content) = msgs[4].tool_result_info().unwrap();
        assert!(content.contains("fn main"), "newer result should keep original");
    }

    #[tokio::test]
    async fn skips_at_low_pressure() {
        let stage = MicrocompactStage::new();
        let long_output = "x".repeat(5000);
        let mut msgs = vec![
            UnifiedMessage::user("q"),
            UnifiedMessage::tool_result("c1", "Read", &long_output, false),
            UnifiedMessage::assistant("a"),
            UnifiedMessage::user("q2"),
            UnifiedMessage::tool_result("c2", "Read", &long_output, false),
            UnifiedMessage::assistant("a2"),
        ];
        let freed = stage.prepare(&mut msgs, &low_pressure(), 2).await;
        assert_eq!(freed, 0, "should skip at low pressure");
    }

    #[tokio::test]
    async fn preserves_different_content() {
        let stage = MicrocompactStage::new();
        let mut msgs = vec![
            UnifiedMessage::user("q"),
            UnifiedMessage::tool_result("c1", "Read", "version 1", false),
            UnifiedMessage::assistant("a"),
            UnifiedMessage::user("q2"),
            UnifiedMessage::tool_result("c2", "Read", "version 2", false),
            UnifiedMessage::assistant("a2"),
            UnifiedMessage::user("latest"),
        ];
        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;
        // Different content — nothing to dedup
        assert_eq!(freed, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget::microcompact -- --nocapture`
Expected: FAIL — module doesn't exist

- [ ] **Step 3: Write the implementation**

In `src/agent_loop/context_budget/microcompact.rs`:

```rust
//! MicrocompactStage — content-addressed dedup for tool results.
//!
//! Replaces older duplicate tool results with compact references,
//! keeping the newest occurrence intact.

use std::collections::HashMap;

use async_trait::async_trait;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

/// Minimum pressure ratio to activate microcompact.
const ACTIVATION_THRESHOLD: f64 = 0.3;

// =============================================================================
// MicrocompactStage
// =============================================================================

pub struct MicrocompactStage {
    /// tool_name:args_hash → (content_hash, first_seen_index)
    cache: std::cell::RefCell<HashMap<u64, CacheEntry>>,
}

struct CacheEntry {
    content_hash: u64,
    newest_index: usize,
}

impl MicrocompactStage {
    pub fn new() -> Self {
        Self {
            cache: std::cell::RefCell::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl PreflightStage for MicrocompactStage {
    fn name(&self) -> &'static str {
        "microcompact"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if pressure.ratio < ACTIVATION_THRESHOLD {
            return 0;
        }

        let partition = partition_fresh_tail(messages, fresh_tail_count);
        if partition == 0 {
            return 0;
        }

        // Pass 1: scan all tool results, build cache of newest occurrence per key
        let mut cache = self.cache.borrow_mut();
        cache.clear();

        for (i, msg) in messages[..partition].iter().enumerate() {
            if let Some((name, content)) = msg.tool_result_info() {
                let key = simple_hash(&format!("{}:{}", name, &content[..content.len().min(50)]));
                let content_hash = simple_hash(&content);
                let entry = cache.entry(key).or_insert(CacheEntry {
                    content_hash,
                    newest_index: i,
                });
                // Track the newest occurrence with matching content
                if content_hash == entry.content_hash && i > entry.newest_index {
                    entry.newest_index = i;
                }
            }
        }

        // Pass 2: replace older duplicates with compact references
        let mut freed = 0;
        for i in 0..partition {
            if let Some((name, content)) = messages[i].tool_result_info() {
                let name = name.to_owned();
                let content = content.to_owned();
                let key = simple_hash(&format!("{}:{}", name, &content[..content.len().min(50)]));
                let content_hash = simple_hash(&content);

                if let Some(entry) = cache.get(&key) {
                    if content_hash == entry.content_hash && i < entry.newest_index {
                        // This is an older duplicate — replace with compact ref
                        let old_tokens = estimate_tokens_smart(&content);
                        let compact_ref = format!(
                            "[cached: {} result, {} tokens, same content appears later in conversation]",
                            name, old_tokens
                        );
                        let new_tokens = estimate_tokens_smart(&compact_ref);
                        messages[i].replace_tool_result_content(compact_ref);
                        freed += old_tokens.saturating_sub(new_tokens);
                    }
                }
            }
        }

        freed
    }
}

/// Simple FNV-1a hash for cache keys (no external dependency needed).
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

- [ ] **Step 4: Register module**

In `src/agent_loop/context_budget/mod.rs`, add:

```rust
pub mod microcompact;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib context_budget::microcompact -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/context_budget/microcompact.rs src/agent_loop/context_budget/mod.rs
git commit -m "feat(agent_loop): add MicrocompactStage for tool result dedup"
```

---

## Task 5: Context Collapse Stage

**Files:**
- Create: `src/agent_loop/context_budget/context_collapse.rs`
- Modify: `src/agent_loop/context_budget/mod.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/context_budget/context_collapse.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::context_budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;

    fn high_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 800, budget_tokens: 1000, ratio: 0.8,
            overhead_tokens: 100, available_for_messages: 900,
        }
    }

    #[tokio::test]
    async fn folds_file_exploration_group() {
        let stage = ContextCollapseStage::new();
        // 4 consecutive Read rounds (assistant+tool_result each)
        let mut msgs = vec![];
        for i in 0..4 {
            msgs.push(UnifiedMessage::user(&format!("read file {}", i)));
            msgs.push(UnifiedMessage::tool_result(
                &format!("c{}", i),
                "Read",
                &format!("// content of src/file_{}.rs\nfn func_{}() {{}}\n", i, i).repeat(50),
                false,
            ));
            msgs.push(UnifiedMessage::assistant(&format!("I read file {}", i)));
        }
        msgs.push(UnifiedMessage::user("now summarize"));  // fresh tail

        let original_len = msgs.len();
        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;

        assert!(freed > 0, "should free tokens");
        assert!(msgs.len() < original_len, "should have fewer messages");
        // Should contain a collapse summary
        let has_collapse = msgs.iter().any(|m| m.text_content().contains("[Context collapsed]"));
        assert!(has_collapse, "should have collapse marker");
    }

    #[tokio::test]
    async fn preserves_write_groups() {
        let stage = ContextCollapseStage::new();
        let mut msgs = vec![];
        for i in 0..4 {
            msgs.push(UnifiedMessage::user(&format!("write file {}", i)));
            msgs.push(UnifiedMessage::tool_result(
                &format!("c{}", i), "Write", "ok", false,
            ));
            msgs.push(UnifiedMessage::assistant(&format!("wrote file {}", i)));
        }
        msgs.push(UnifiedMessage::user("done"));

        let freed = stage.prepare(&mut msgs, &high_pressure(), 1).await;
        assert_eq!(freed, 0, "should not fold Write groups");
    }

    #[tokio::test]
    async fn skips_below_threshold() {
        let stage = ContextCollapseStage::new();
        let low_pressure = ContextPressure {
            used_tokens: 300, budget_tokens: 1000, ratio: 0.3,
            overhead_tokens: 100, available_for_messages: 900,
        };
        let mut msgs = vec![
            UnifiedMessage::user("read"),
            UnifiedMessage::tool_result("c1", "Read", &"x".repeat(2000), false),
            UnifiedMessage::assistant("done"),
        ];
        let freed = stage.prepare(&mut msgs, &low_pressure, 0).await;
        assert_eq!(freed, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget::context_collapse -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Write the implementation**

In `src/agent_loop/context_budget/context_collapse.rs`:

```rust
//! ContextCollapseStage — folds consecutive exploratory message groups
//! into compact summaries.

use async_trait::async_trait;
use std::ops::Range;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

const ACTIVATION_THRESHOLD: f64 = 0.5;
const MIN_SAVINGS_TOKENS: usize = 500;
const MIN_ROUNDS_FILE_EXPLORATION: usize = 3;
const MIN_ROUNDS_SEARCH_SWEEP: usize = 2;

// =============================================================================
// GroupType
// =============================================================================

#[derive(Debug, Clone, Copy)]
enum GroupType {
    FileExploration,
    SearchSweep,
}

struct MessageGroup {
    range: Range<usize>,
    group_type: GroupType,
    total_tokens: usize,
}

// =============================================================================
// ContextCollapseStage
// =============================================================================

pub struct ContextCollapseStage;

impl ContextCollapseStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PreflightStage for ContextCollapseStage {
    fn name(&self) -> &'static str {
        "context_collapse"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        if pressure.ratio < ACTIVATION_THRESHOLD {
            return 0;
        }

        let partition = partition_fresh_tail(messages, fresh_tail_count);
        if partition < 6 {
            return 0;
        }

        let groups = detect_groups(&messages[..partition]);
        let mut freed = 0;

        // Process groups back-to-front to maintain stable indices
        for group in groups.iter().rev() {
            if group.total_tokens < MIN_SAVINGS_TOKENS {
                continue;
            }

            let summary = generate_summary(&messages[group.range.clone()], group.group_type);
            let summary_tokens = estimate_tokens_smart(&summary);
            if summary_tokens >= group.total_tokens {
                continue;
            }

            let collapsed = UnifiedMessage::user(format!("[Context collapsed] {}", summary));

            // Replace range with single collapsed message
            let range = group.range.clone();
            messages.splice(range.clone(), std::iter::once(collapsed));
            freed += group.total_tokens.saturating_sub(summary_tokens);
        }

        freed
    }
}

// =============================================================================
// Group detection
// =============================================================================

/// Detect collapsible groups in the message slice.
fn detect_groups(messages: &[UnifiedMessage]) -> Vec<MessageGroup> {
    let mut groups = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        if let Some(group) = try_match_group(messages, i, is_read_or_glob_round, GroupType::FileExploration, MIN_ROUNDS_FILE_EXPLORATION) {
            let end = group.range.end;
            groups.push(group);
            i = end;
            continue;
        }
        if let Some(group) = try_match_group(messages, i, is_grep_round, GroupType::SearchSweep, MIN_ROUNDS_SEARCH_SWEEP) {
            let end = group.range.end;
            groups.push(group);
            i = end;
            continue;
        }
        i += 1;
    }

    groups
}

/// Try to match consecutive rounds of the same type starting at `start`.
fn try_match_group(
    messages: &[UnifiedMessage],
    start: usize,
    round_matcher: fn(&[UnifiedMessage], usize) -> Option<usize>,
    group_type: GroupType,
    min_rounds: usize,
) -> Option<MessageGroup> {
    let mut end = start;
    let mut round_count = 0;

    while end < messages.len() {
        if let Some(round_end) = round_matcher(messages, end) {
            end = round_end;
            round_count += 1;
        } else {
            break;
        }
    }

    if round_count >= min_rounds {
        let total_tokens: usize = messages[start..end]
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();
        Some(MessageGroup {
            range: start..end,
            group_type,
            total_tokens,
        })
    } else {
        None
    }
}

/// Check if a round starting at `pos` is a Read/Glob round.
/// A round = user + tool_result + assistant (3 messages).
fn is_read_or_glob_round(messages: &[UnifiedMessage], pos: usize) -> Option<usize> {
    if pos + 2 >= messages.len() {
        return None;
    }
    if !messages[pos].is_user() {
        return None;
    }
    if let Some((name, _)) = messages[pos + 1].tool_result_info() {
        let lower = name.to_ascii_lowercase();
        if lower == "read" || lower == "glob" || lower.contains("read_file") {
            if messages[pos + 2].is_assistant() {
                return Some(pos + 3);
            }
        }
    }
    None
}

/// Check if a round starting at `pos` is a Grep round.
fn is_grep_round(messages: &[UnifiedMessage], pos: usize) -> Option<usize> {
    if pos + 2 >= messages.len() {
        return None;
    }
    if !messages[pos].is_user() {
        return None;
    }
    if let Some((name, _)) = messages[pos + 1].tool_result_info() {
        let lower = name.to_ascii_lowercase();
        if lower == "grep" || lower.contains("search") {
            if messages[pos + 2].is_assistant() {
                return Some(pos + 3);
            }
        }
    }
    None
}

// =============================================================================
// Summary generation (pure local, no LLM)
// =============================================================================

fn generate_summary(messages: &[UnifiedMessage], group_type: GroupType) -> String {
    match group_type {
        GroupType::FileExploration => {
            let tools: Vec<String> = messages
                .iter()
                .filter_map(|m| m.tool_result_info().map(|(_, c)| c.to_owned()))
                .collect();
            let count = tools.len();
            // Extract rough size info
            let total_lines: usize = tools.iter().map(|c| c.lines().count()).sum();
            format!("Explored {} files ({} total lines of code)", count, total_lines)
        }
        GroupType::SearchSweep => {
            let count = messages
                .iter()
                .filter(|m| m.tool_result_info().is_some())
                .count();
            format!("Ran {} search queries across the codebase", count)
        }
    }
}

/// Check if any message in the range involves a mutating tool.
#[allow(dead_code)]
fn contains_mutation(messages: &[UnifiedMessage]) -> bool {
    messages.iter().any(|m| {
        if let Some((name, _)) = m.tool_result_info() {
            matches!(name, "Write" | "Edit" | "Bash" | "NotebookEdit")
        } else {
            false
        }
    })
}
```

- [ ] **Step 4: Register module**

In `src/agent_loop/context_budget/mod.rs`, add:

```rust
pub mod context_collapse;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib context_budget::context_collapse -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/context_budget/context_collapse.rs src/agent_loop/context_budget/mod.rs
git commit -m "feat(agent_loop): add ContextCollapseStage for exploratory message folding"
```

---

## Task 6: Autocompact Stage

**Files:**
- Create: `src/agent_loop/context_budget/autocompact.rs`
- Modify: `src/agent_loop/context_budget/mod.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/context_budget/autocompact.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::context_budget::ContextPressure;
    use crate::providers::message::UnifiedMessage;

    fn critical_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 900, budget_tokens: 1000, ratio: 0.9,
            overhead_tokens: 100, available_for_messages: 900,
        }
    }

    fn low_pressure() -> ContextPressure {
        ContextPressure {
            used_tokens: 300, budget_tokens: 1000, ratio: 0.3,
            overhead_tokens: 100, available_for_messages: 900,
        }
    }

    #[tokio::test]
    async fn skips_at_low_pressure() {
        let stage = AutocompactStage::new_with_summarizer(|_| {
            Box::pin(async { Ok("summary".to_string()) })
        });
        let mut msgs = vec![
            UnifiedMessage::user("hi"),
            UnifiedMessage::assistant("hello"),
        ];
        let freed = stage.prepare(&mut msgs, &low_pressure(), 1).await;
        assert_eq!(freed, 0);
    }

    #[tokio::test]
    async fn skips_when_too_few_messages() {
        let stage = AutocompactStage::new_with_summarizer(|_| {
            Box::pin(async { Ok("summary".to_string()) })
        });
        let mut msgs = vec![
            UnifiedMessage::user("q"),
            UnifiedMessage::assistant("a"),
        ];
        let freed = stage.prepare(&mut msgs, &critical_pressure(), 1).await;
        assert_eq!(freed, 0, "too few messages to summarize");
    }

    #[tokio::test]
    async fn compacts_old_messages_preserving_first_and_tail() {
        let stage = AutocompactStage::new_with_summarizer(|_| {
            Box::pin(async { Ok("Discussed file reading and code analysis.".to_string()) })
        });
        let mut msgs = vec![
            UnifiedMessage::user("Please help me refactor this code"),  // first user msg
            UnifiedMessage::assistant("Sure, let me look at the code"),
            UnifiedMessage::user("Here is file A"),
            UnifiedMessage::tool_result("c1", "Read", &"x".repeat(2000), false),
            UnifiedMessage::assistant("I see file A"),
            UnifiedMessage::user("Here is file B"),
            UnifiedMessage::tool_result("c2", "Read", &"y".repeat(2000), false),
            UnifiedMessage::assistant("I see file B"),
            UnifiedMessage::user("Now refactor"),                       // fresh tail
            UnifiedMessage::assistant("Starting refactor"),             // fresh tail
        ];

        let freed = stage.prepare(&mut msgs, &critical_pressure(), 2).await;
        assert!(freed > 0, "should free tokens");
        // First message should be preserved (original task)
        assert_eq!(msgs[0].text_content(), "Please help me refactor this code");
        // Last 2 messages should be preserved (fresh tail)
        let last = &msgs[msgs.len() - 1];
        assert!(last.text_content().contains("Starting refactor"));
        // Should have a summary message
        let has_summary = msgs.iter().any(|m| m.text_content().contains("Conversation summary"));
        assert!(has_summary, "should contain summary message");
    }

    #[tokio::test]
    async fn graceful_on_summarizer_failure() {
        let stage = AutocompactStage::new_with_summarizer(|_| {
            Box::pin(async { Err(anyhow::anyhow!("LLM unavailable")) })
        });
        let mut msgs = vec![];
        for i in 0..10 {
            msgs.push(UnifiedMessage::user(&format!("q{}", i)));
            msgs.push(UnifiedMessage::assistant(&format!("a{}", i)));
        }
        let original_len = msgs.len();
        let freed = stage.prepare(&mut msgs, &critical_pressure(), 2).await;
        assert_eq!(freed, 0, "should return 0 on failure");
        assert_eq!(msgs.len(), original_len, "should not modify messages on failure");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget::autocompact -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Write the implementation**

In `src/agent_loop/context_budget/autocompact.rs`:

```rust
//! AutocompactStage — LLM-based conversation summarization.
//!
//! The heaviest and most aggressive pre-flight stage. Calls a cheap/fast LLM
//! to summarize old conversation segments, replacing them with a compact summary.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use super::preflight::PreflightStage;
use super::pressure::estimate_tokens_smart;
use super::ContextPressure;
use crate::memory::session_compactor::context_window::partition_fresh_tail;
use crate::providers::message::UnifiedMessage;

const ACTIVATION_THRESHOLD: f64 = 0.65;
const MIN_MESSAGES_TO_SUMMARIZE: usize = 6;
const DEFAULT_COOLDOWN_TURNS: usize = 10;

/// Type alias for the summarizer function.
type SummarizerFn = dyn Fn(String) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>>
    + Send
    + Sync;

// =============================================================================
// AutocompactStage
// =============================================================================

pub struct AutocompactStage {
    summarizer: Box<SummarizerFn>,
    cooldown_turns: usize,
    last_compact_turn: AtomicUsize,
}

impl AutocompactStage {
    /// Create with a custom summarizer function (for testing and dependency injection).
    pub fn new_with_summarizer<F, Fut>(f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        Self {
            summarizer: Box::new(move |input| Box::pin(f(input))),
            cooldown_turns: DEFAULT_COOLDOWN_TURNS,
            last_compact_turn: AtomicUsize::new(0),
        }
    }

    /// Create a no-op autocompact (for when no LLM provider is available).
    pub fn noop() -> Self {
        Self::new_with_summarizer(|_| async { Err(anyhow::anyhow!("no summarizer configured")) })
    }
}

#[async_trait]
impl PreflightStage for AutocompactStage {
    fn name(&self) -> &'static str {
        "autocompact"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        // Gate 1: pressure threshold
        if pressure.ratio < ACTIVATION_THRESHOLD {
            return 0;
        }

        // Gate 2: cooldown
        let current_turn = messages.len();
        let last = self.last_compact_turn.load(Ordering::Relaxed);
        if current_turn.saturating_sub(last) < self.cooldown_turns && last > 0 {
            return 0;
        }

        // Gate 3: enough messages to summarize
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        // Reserve first user message (index 0) — summary starts at index 1
        let summary_start = if !messages.is_empty() && messages[0].is_user() {
            1
        } else {
            0
        };

        if partition <= summary_start {
            return 0;
        }

        let summarizable = &messages[summary_start..partition];
        if summarizable.len() < MIN_MESSAGES_TO_SUMMARIZE {
            return 0;
        }

        // Find safe boundary (don't orphan tool_result)
        let summary_end = find_safe_boundary(messages, partition);
        if summary_end <= summary_start {
            return 0;
        }

        // Build conversation text for summarizer
        let conversation_text: String = messages[summary_start..summary_end]
            .iter()
            .map(|m| format!("[{}]: {}", m.role_str(), truncate_for_summary(&m.text_content())))
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = format!(
            "Summarize this conversation segment concisely. Preserve:\n\
             1. All decisions made and their reasoning\n\
             2. Key facts discovered (file paths, function names, error messages)\n\
             3. Current task state and what was accomplished\n\
             4. Any unresolved questions or blockers\n\
             \n\
             Discard: exploratory steps that led nowhere, verbose tool outputs, \
             repeated attempts at the same thing.\n\
             \n\
             Format: dense paragraphs, no bullet lists. Use concrete names and paths.\n\
             \n\
             Conversation to summarize:\n\n{}",
            conversation_text
        );

        // Call summarizer
        let summary = match (self.summarizer)(prompt).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "autocompact", error = %e, "Summarization failed, skipping");
                return 0;
            }
        };

        // Calculate savings
        let old_tokens: usize = messages[summary_start..summary_end]
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();

        let compact_msg = UnifiedMessage::user(format!(
            "[Conversation summary — earlier messages were compressed to save context]\n\n{}",
            summary
        ));
        let new_tokens = estimate_tokens_smart(&compact_msg.text_content());

        if new_tokens >= old_tokens {
            return 0;
        }

        // Replace range with summary
        messages.splice(summary_start..summary_end, std::iter::once(compact_msg));
        self.last_compact_turn.store(current_turn, Ordering::Relaxed);

        old_tokens.saturating_sub(new_tokens)
    }
}

/// Find a safe boundary that doesn't orphan tool_result messages.
fn find_safe_boundary(messages: &[UnifiedMessage], target: usize) -> usize {
    let mut pos = target;
    while pos > 0 && messages[pos.saturating_sub(1)].is_tool_result() {
        pos -= 1;
    }
    pos
}

/// Truncate individual messages for the summary prompt to avoid blowing up the summarizer input.
fn truncate_for_summary(text: &str) -> String {
    const MAX_CHARS_PER_MSG: usize = 500;
    if text.len() <= MAX_CHARS_PER_MSG {
        text.to_string()
    } else {
        format!("{}... [truncated]", &text[..MAX_CHARS_PER_MSG])
    }
}
```

- [ ] **Step 4: Register module**

In `src/agent_loop/context_budget/mod.rs`, add:

```rust
pub mod autocompact;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib context_budget::autocompact -- --nocapture`
Expected: PASS (4 tests)

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/context_budget/autocompact.rs src/agent_loop/context_budget/mod.rs
git commit -m "feat(agent_loop): add AutocompactStage with LLM summarization"
```

---

## Task 7: Integrate PreflightPipeline into loop_core

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Test: manual integration test via `cargo test -p alephcore`

- [ ] **Step 1: Add preflight field to AgentLoop**

In `src/agent_loop/loop_core.rs`, add to `AgentLoop` struct (after `compaction_pipeline` field around line 688):

```rust
    /// Pre-flight context preparation pipeline (microcompact → collapse → autocompact).
    preflight_pipeline: super::context_budget::preflight::PreflightPipeline,
```

- [ ] **Step 2: Initialize preflight in AgentLoop constructor**

In the `AgentLoop::new()` (around line 740), add after `compaction_pipeline` initialization:

```rust
            preflight_pipeline: super::context_budget::preflight::PreflightPipeline::new(vec![
                Box::new(super::context_budget::microcompact::MicrocompactStage::new()),
                Box::new(super::context_budget::context_collapse::ContextCollapseStage::new()),
                Box::new(super::context_budget::autocompact::AutocompactStage::noop()),
            ]),
```

Note: Uses `AutocompactStage::noop()` initially — real summarizer wiring comes when a provider reference is available. This can be upgraded later to pass the actual provider.

- [ ] **Step 3: Call preflight in prepare_turn**

In `prepare_turn()` (around line 936), after `budget_directive = ctx_budget.before_turn(...)` but before the `match budget_directive` block, add:

```rust
        // Pre-flight context preparation (runs before emergency compaction)
        if matches!(
            budget_directive,
            super::context_budget::LoopDirective::Continue
                | super::context_budget::LoopDirective::CompactAndContinue
        ) {
            let pressure = ctx_budget_ref
                .as_ref()
                .and_then(|cb| cb.last_pressure().copied())
                .unwrap_or(ContextPressure {
                    used_tokens: 0,
                    budget_tokens: 1,
                    ratio: 0.0,
                    overhead_tokens: 0,
                    available_for_messages: 1,
                });
            let fresh_tail = ctx_budget_ref
                .as_ref()
                .map(|cb| cb.fresh_tail_count())
                .unwrap_or(6);

            // Release the lock before async call
            drop(ctx_budget_ref);

            let preflight_freed = self
                .preflight_pipeline
                .run(messages, &pressure, fresh_tail)
                .await;

            if preflight_freed > 0 {
                tracing::info!(
                    target: "agent_loop",
                    tokens_freed = preflight_freed,
                    "Pre-flight context preparation freed tokens"
                );
            }

            // Re-acquire for the compaction check below
            ctx_budget_ref = self
                .context_budget
                .lock()
                .unwrap_or_else(|e| e.into_inner());
        }
```

Note: The Mutex must be released before the async preflight call and re-acquired after. This requires restructuring the existing `prepare_turn` to split the lock scope.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (may require adjusting variable scope for the mutex)

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20`
Expected: All existing tests still pass

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): integrate PreflightPipeline into prepare_turn"
```

---

## Task 8: Cascading Abort in StreamingToolExecutor

**Files:**
- Modify: `src/agent_loop/streaming_bridge.rs`
- Test: extend existing tests

- [ ] **Step 1: Write the failing test**

Add to `streaming_bridge.rs` tests:

```rust
    /// A tool that always fails (concurrent-safe for testing cascade).
    struct FailingTool;

    #[async_trait]
    impl LoopTool for FailingTool {
        fn name(&self) -> &str { "Bash" }  // Bash = AbortSiblings policy
        fn description(&self) -> &str { "Always fails" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult::Error {
                error: "command failed".into(),
                retryable: false,
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool { true }
    }

    #[tokio::test]
    async fn bash_failure_cascades_to_siblings() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(FailingTool));
        registry.register(Box::new(VerySlowTool));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel);

        // Bash fails fast, VerySlowTool takes 10s — should be aborted
        feed_tool_call(&mut bridge, "t1", "Bash", "{}");
        feed_tool_call(&mut bridge, "t2", "very_slow", "{}");
        bridge.finish();

        let start = Instant::now();
        let results = executor.run().await;
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_secs(2), "sibling should be aborted quickly, took {:?}", elapsed);
        assert_eq!(results.len(), 2);
        // First result should be the Bash error
        assert!(results[0].outcome.is_error);
        // Second should be aborted
        assert!(results[1].outcome.is_error || results[1].outcome.output_text.contains("Aborted"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib streaming_bridge::tests::bash_failure_cascades -- --nocapture`
Expected: FAIL (takes 10s because no cascade)

- [ ] **Step 3: Implement cascading abort**

Modify `StreamingToolExecutor` to add batch cancel token and cascade logic:

1. Add `batch_cancel: CancellationToken` field to `StreamingToolExecutor`
2. In `StreamingToolBridge::new()`, create `batch_cancel` as child of the provided `cancel`
3. In `spawn_tool_execution()`, create per-tool cancel as child of `batch_cancel`
4. After each tool completes, check if its outcome is an error AND its cascade policy is `AbortSiblings` → cancel `batch_cancel`
5. In Phase 2 (awaiting in-flight), collect results and check for cascade
6. In Phase 3 (exclusive queue), check `batch_cancel` before each tool

Key changes to `StreamingToolExecutor::run()`:

```rust
pub async fn run(mut self) -> Vec<PipelineOutcome> {
    let batch_cancel = self.cancel.child_token();
    // ... Phase 1: receive, spawn with batch_cancel.child_token()
    // ... Phase 2: await, check cascade on error
    for (idx, handle) in in_flight {
        match handle.await {
            Ok(outcome) => {
                if outcome.outcome.is_error {
                    let policy = CascadePolicy::classify(&outcome.outcome.tool_name);
                    if matches!(policy, CascadePolicy::AbortSiblings) {
                        batch_cancel.cancel();
                    }
                }
                results.push((idx, outcome));
            }
            // ... error handling
        }
    }
    // Phase 3: exclusive — check batch_cancel
    for call in exclusive_queue {
        if batch_cancel.is_cancelled() {
            results.push((call.index, synthetic_abort_outcome(&call.name, "sibling failed")));
            continue;
        }
        // ... normal execution
    }
    // ...
}
```

- [ ] **Step 4: Run all streaming_bridge tests**

Run: `cargo test -p alephcore --lib streaming_bridge -- --nocapture`
Expected: PASS (all existing + 1 new)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/streaming_bridge.rs
git commit -m "feat(agent_loop): cascading error abort in streaming tool executor"
```

---

## Task 9: Progress Channel in Streaming Executor

**Files:**
- Modify: `src/agent_loop/streaming_bridge.rs`
- Test: extend existing tests

- [ ] **Step 1: Write the failing test**

Add to `streaming_bridge.rs` tests:

```rust
    #[tokio::test]
    async fn progress_channel_collects_updates() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(SlowTool { delay_ms: 10 }));

        let cancel = CancellationToken::new();
        let (mut bridge, executor) =
            StreamingToolBridge::new(Arc::new(registry), permissive_pipeline(), cancel);

        feed_tool_call(&mut bridge, "t1", "slow", "{}");
        bridge.finish();

        let (results, progress_rx) = executor.run_with_progress().await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].outcome.is_error);
        // progress_rx should be drainable (may be empty if tool didn't send progress)
        drop(progress_rx);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib streaming_bridge::tests::progress_channel -- --nocapture`
Expected: FAIL — `run_with_progress` doesn't exist

- [ ] **Step 3: Implement progress channel**

Add `run_with_progress()` method that returns both results and the progress receiver:

```rust
impl StreamingToolExecutor {
    /// Run with a progress channel. Returns results and the progress receiver
    /// so the caller can drain progress messages.
    pub async fn run_with_progress(
        self,
    ) -> (Vec<PipelineOutcome>, mpsc::Receiver<ToolProgress>) {
        let (progress_tx, progress_rx) = mpsc::channel::<ToolProgress>(64);
        // Store progress_tx for tool contexts to use
        // ... execution logic same as run() but passing progress_tx to each ToolExecutionContext
        let results = self.run_internal(Some(progress_tx)).await;
        (results, progress_rx)
    }
}
```

Refactor `run()` to call `run_internal(None)` for backward compatibility.

- [ ] **Step 4: Run all streaming_bridge tests**

Run: `cargo test -p alephcore --lib streaming_bridge -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/streaming_bridge.rs
git commit -m "feat(agent_loop): add progress channel to streaming tool executor"
```

---

## Task 10: Truncation Recovery Escalation

**Files:**
- Modify: `src/agent_loop/truncation_recovery.rs`
- Test: existing + new tests

- [ ] **Step 1: Write the failing test**

Add to `truncation_recovery.rs` tests:

```rust
#[test]
fn escalation_doubles_max_tokens() {
    let mut recovery = TruncationRecovery::new(4096, 32768);
    recovery.record_truncation("partial output 1");
    match recovery.escalate() {
        EscalateDecision::Retry { new_max_tokens, .. } => {
            assert_eq!(new_max_tokens, 8192, "should double");
        }
        _ => panic!("should retry"),
    }
}

#[test]
fn escalation_caps_at_provider_max() {
    let mut recovery = TruncationRecovery::new(16384, 32768);
    recovery.record_truncation("p1");
    match recovery.escalate() {
        EscalateDecision::Retry { new_max_tokens, .. } => {
            assert_eq!(new_max_tokens, 32768, "should cap at provider max");
        }
        _ => panic!("should retry"),
    }
}

#[test]
fn escalation_gives_up_after_max_attempts() {
    let mut recovery = TruncationRecovery::new(8192, 16384);
    recovery.record_truncation("frag1");
    recovery.escalate(); // 16384
    recovery.record_truncation("frag2");
    recovery.escalate(); // already at cap
    recovery.record_truncation("frag3");
    match recovery.escalate() {
        EscalateDecision::GiveUp { assembled } => {
            assert!(assembled.contains("frag1"));
            assert!(assembled.contains("frag2"));
            assert!(assembled.contains("frag3"));
        }
        _ => panic!("should give up"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib truncation_recovery::tests::escalation -- --nocapture`
Expected: FAIL — `EscalateDecision` doesn't exist

- [ ] **Step 3: Add escalation types and methods**

Add to `src/agent_loop/truncation_recovery.rs`:

```rust
/// Decision from the escalation logic.
pub enum EscalateDecision {
    /// Increase max_tokens and retry.
    Retry {
        new_max_tokens: u32,
        continuation_prompt: String,
    },
    /// Give up — return assembled fragments.
    GiveUp {
        assembled: String,
    },
}

impl TruncationRecovery {
    /// Record a truncated output fragment.
    pub fn record_truncation(&mut self, text: &str) {
        self.fragments.push(text.to_string());
    }

    /// Try to escalate max_tokens. Returns Retry or GiveUp.
    pub fn escalate(&mut self) -> EscalateDecision {
        self.attempts += 1;

        let next_max = (self.provider_max_tokens).min(
            self.original_max_tokens.unwrap_or(self.provider_max_tokens) * 2u32.pow(self.attempts),
        );

        if next_max <= self.provider_max_tokens
            && self.attempts <= 3
            && next_max > self.original_max_tokens.unwrap_or(0)
        {
            EscalateDecision::Retry {
                new_max_tokens: next_max.min(self.provider_max_tokens),
                continuation_prompt: format!(
                    "Your previous response was truncated at the output token limit. \
                     Continue exactly where you left off. Do not repeat content. \
                     Limit increased to {} tokens.",
                    next_max
                ),
            }
        } else {
            EscalateDecision::GiveUp {
                assembled: self.fragments.join(""),
            }
        }
    }
}
```

Adjust the existing fields/logic to support this. The exact implementation will need to fit the existing `TruncationRecovery` struct which already has `phase`, `attempts`, `provider_max_tokens`, `original_max_tokens`, and `fragments`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib truncation_recovery -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/truncation_recovery.rs
git commit -m "feat(agent_loop): add escalation loop to TruncationRecovery"
```

---

## Task 11: Prompt Caching (CacheControl on ContentBlock)

**Files:**
- Modify: `src/providers/message.rs`
- Modify: `src/agent_loop/loop_core.rs` (system prompt block splitting)
- Test: unit test for CacheControl serialization

- [ ] **Step 1: Add CacheControl to ContentBlock**

In `src/providers/message.rs`, add:

```rust
/// Cache control hint for API providers that support prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheControl {
    /// Short-lived cache (Anthropic: ~5 min TTL).
    Ephemeral,
}
```

Change `ContentBlock::Text`:

```rust
pub enum ContentBlock {
    /// Plain text
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    // ... other variants unchanged
}
```

- [ ] **Step 2: Fix all ContentBlock::Text construction sites**

Run: `cargo check -p alephcore 2>&1 | head -50`

Search for all `ContentBlock::Text { text: ... }` and add `cache_control: None`. This will be a widespread change across multiple files.

- [ ] **Step 3: Write test for serialization**

```rust
#[test]
fn cache_control_serializes_correctly() {
    let block = ContentBlock::Text {
        text: "hello".into(),
        cache_control: Some(CacheControl::Ephemeral),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["cache_control"], "ephemeral");
}

#[test]
fn cache_control_none_omitted() {
    let block = ContentBlock::Text {
        text: "hello".into(),
        cache_control: None,
    };
    let json = serde_json::to_value(&block).unwrap();
    assert!(json.get("cache_control").is_none());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore -- --nocapture 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/providers/message.rs
git commit -m "feat(providers): add CacheControl to ContentBlock for prompt caching"
```

---

## Task 12: Enhanced 413 Recovery

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Test: extend existing 413 handling tests

- [ ] **Step 1: Locate existing 413 handling**

Search for the existing `PromptTooLong` or `413` handling in `loop_core.rs`:

Run: `grep -n "413\|PromptTooLong\|CompactAndRetry" src/agent_loop/loop_core.rs | head -20`

- [ ] **Step 2: Enhance the recovery cascade**

In the existing 413 handler, replace simple emergency truncate with multi-tier recovery:

```rust
// Tier 1: Pre-flight pipeline (if not already run this turn)
let preflight_freed = self.preflight_pipeline.run(messages, &pressure, fresh_tail).await;
if preflight_freed >= token_gap {
    return RecoveryResult::Recovered;
}

// Tier 2: Emergency compaction pipeline (existing)
let result = self.compaction_pipeline.run(messages, &sensor, ...);
if result.tokens_freed + preflight_freed >= token_gap {
    return RecoveryResult::Recovered;
}

// Tier 3: Aggressive round drop with reduced fresh_tail
let aggressive_tail = (fresh_tail / 2).max(2);
let stage = RoundDrop { token_budget: ..., ratio: ... };
let extra = stage.compact(messages, aggressive_tail);
if extra > 0 {
    return RecoveryResult::Recovered;
}

// Tier 4: Force emergency — the existing FinalReply path handles this
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): four-tier 413 recovery cascade"
```

---

## Task 13: Cleanup Old Code

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs` (remove old `truncate_tool_result` if fully replaced)
- Run clippy and fix warnings

- [ ] **Step 1: Check if old truncate_tool_result is still used**

Run: `grep -rn "truncate_tool_result[^_]" src/agent_loop/ | grep -v "with_budget"`

If no callers remain, remove the old function and its tests.

- [ ] **Step 2: Remove the old function if unused**

Delete `truncate_tool_result()` and its tests `truncate_short_result_unchanged` and `truncate_large_result_truncated` from `tool_pipeline.rs`.

Update any remaining callers to use `truncate_tool_result_with_budget`.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`
Fix any warnings.

- [ ] **Step 4: Run full test suite**

Run: `cargo test -p alephcore -- --nocapture 2>&1 | tail -20`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add -A src/agent_loop/
git commit -m "refactor(agent_loop): remove old truncation code, fix clippy warnings"
```

---

## Task 14: Final Verification

- [ ] **Step 1: Full build**

Run: `cargo build -p alephcore`
Expected: PASS

- [ ] **Step 2: Full test suite**

Run: `cargo test -p alephcore`
Expected: All tests pass

- [ ] **Step 3: Clippy clean**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Review new module structure**

Run: `find src/agent_loop/context_budget -name "*.rs" | sort`

Expected:
```
src/agent_loop/context_budget/autocompact.rs
src/agent_loop/context_budget/context_collapse.rs
src/agent_loop/context_budget/diagnostics.rs
src/agent_loop/context_budget/microcompact.rs
src/agent_loop/context_budget/mod.rs
src/agent_loop/context_budget/pipeline.rs
src/agent_loop/context_budget/preflight.rs
src/agent_loop/context_budget/pressure.rs
```

- [ ] **Step 5: Final commit with any remaining fixes**

```bash
git add -A
git commit -m "chore(agent_loop): agent loop evolution — all phases complete"
```

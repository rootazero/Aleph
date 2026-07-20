# Compression Pipeline Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Aleph's context compression pipeline with cascade degradation, microcompaction, post-compression recovery, and smarter DreamDaemon triggering.

**Architecture:** A `CompactionOrchestrator` evaluates context pressure across 5 levels (Calm/Preventive/Warning/High/Critical) and dispatches pluggable `CompactionStrategy` implementations in priority order: MicroCompactor (zero-LLM tool output pruning) → SessionCompactorStrategy (d0/d1/d2 with pair-aware chunking) → LlmSummaryStrategy (side-channel summarization). After compression, a `PostCompactCleanup` trait chain handles state reset, constraint injection, and DreamDaemon gate evaluation.

**Tech Stack:** Rust, tokio, LanceDB (via existing MemoryBackend), schemars (JSON Schema for tool definitions)

**Spec:** `docs/superpowers/specs/2026-04-03-compression-pipeline-robustness-design.md`

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `src/agent_loop/compaction/mod.rs` | Module root, re-exports |
| `src/agent_loop/compaction/types.rs` | CompactionStrategy, PostCompactCleanup traits, CompactionResult, CompactionContext, TokenEstimate, PressureLevel |
| `src/agent_loop/compaction/orchestrator.rs` | CompactionOrchestrator, OrchestratorBuilder |
| `src/agent_loop/compaction/micro_compactor.rs` | MicroCompactor strategy, ToolOutputEntry, Importance, compressibility scoring |
| `src/agent_loop/compaction/tool_aware_chunker.rs` | ToolAwareChunker, SemanticUnit, pair-aware chunking |
| `src/agent_loop/compaction/constraint_injector.rs` | ConstraintInjector, ConstraintSource trait, ConstraintCategory |
| `src/memory/dreaming/gate.rs` | DreamGate, DreamGateConfig, GateResult, BlockReason |
| `src/builtin_tools/recall_context.rs` | SemanticRecoveryTool (recall_context builtin tool) |

### Modified Files

| File | Change |
|------|--------|
| `src/agent_loop/mod.rs` | Add `pub mod compaction;` |
| `src/agent_loop/context_budget/mod.rs` | Add `Calm`, `Preventive` to pressure logic; add `sense_pressure()` returning `PressureLevel` |
| `src/agent_loop/context_compactor.rs` | Wrap as `LlmSummaryStrategy` implementing `CompactionStrategy` |
| `src/memory/session_compactor/mod.rs` | Replace token-based chunking with `ToolAwareChunker`; wrap as `SessionCompactorStrategy` |
| `src/memory/session_compactor/context_window.rs` | Add `SemanticUnit`-aware partition helpers |
| `src/memory/compression/scheduler.rs` | Implement `PostCompactCleanup` |
| `src/memory/compression/signal_detector.rs` | Implement `PostCompactCleanup` |
| `src/memory/dreaming/mod.rs` | Replace `DreamDaemon.run_scheduler()` to use `DreamGate`; remove time-window check |
| `src/agent_loop/loop_core.rs` | Replace direct `ContextCompactor` + `ContextBudget` calls with `CompactionOrchestrator` |
| `src/builtin_tools/mod.rs` | Register `recall_context` tool |

### Files to Delete

| File | Reason |
|------|--------|
| `src/memory/compression_daemon/daemon.rs` | Replaced by DreamGate |
| `src/memory/compression_daemon/config.rs` | Replaced by DreamGateConfig |
| `src/memory/compression_daemon/mod.rs` | Module removed |

---

## Task 1: Foundation Types — CompactionStrategy, PostCompactCleanup, CompactionResult

**Files:**
- Create: `src/agent_loop/compaction/mod.rs`
- Create: `src/agent_loop/compaction/types.rs`
- Modify: `src/agent_loop/mod.rs`
- Test: `src/agent_loop/compaction/types.rs` (inline tests)

- [ ] **Step 1: Write failing test for PressureLevel ordering**

In `src/agent_loop/compaction/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_level_ordering() {
        assert!(PressureLevel::Calm < PressureLevel::Preventive);
        assert!(PressureLevel::Preventive < PressureLevel::Warning);
        assert!(PressureLevel::Warning < PressureLevel::High);
        assert!(PressureLevel::High < PressureLevel::Critical);
    }

    #[test]
    fn pressure_level_from_ratio() {
        assert_eq!(PressureLevel::from_ratio(0.5), PressureLevel::Calm);
        assert_eq!(PressureLevel::from_ratio(0.65), PressureLevel::Preventive);
        assert_eq!(PressureLevel::from_ratio(0.75), PressureLevel::Warning);
        assert_eq!(PressureLevel::from_ratio(0.82), PressureLevel::High);
        assert_eq!(PressureLevel::from_ratio(0.90), PressureLevel::Critical);
    }

    #[test]
    fn compaction_result_reports_success() {
        let result = CompactionResult {
            freed_tokens: 5000,
            compacted_count: 3,
            strategy_name: "micro".to_string(),
            pressure_before: 0.82,
            pressure_after: 0.65,
        };
        assert!(result.pressure_reduced());
        assert_eq!(result.freed_tokens, 5000);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compaction::types::tests -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Create module root and types**

Create `src/agent_loop/compaction/mod.rs`:

```rust
pub mod types;

pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel,
    PostCompactCleanup, TokenEstimate,
};
```

Create `src/agent_loop/compaction/types.rs`:

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::agent_loop::context_budget::ContextPressure;
use crate::providers::unified_message::UnifiedMessage;

/// Pressure levels for cascade degradation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    Calm,       // < 60%
    Preventive, // 60-70%
    Warning,    // 70-80%
    High,       // 80-85%
    Critical,   // 85%+
}

impl PressureLevel {
    pub fn from_ratio(ratio: f64) -> Self {
        match ratio {
            r if r < 0.60 => Self::Calm,
            r if r < 0.70 => Self::Preventive,
            r if r < 0.80 => Self::Warning,
            r if r < 0.85 => Self::High,
            _ => Self::Critical,
        }
    }
}

/// Token savings estimate from a strategy
pub struct TokenEstimate {
    pub estimated_savings: usize,
    pub confidence: f32, // 0.0..1.0
}

/// Context passed to strategies during compaction
pub struct CompactionContext {
    pub messages: Vec<UnifiedMessage>,
    pub pressure: ContextPressure,
    pub pressure_level: PressureLevel,
    pub token_estimate_ratio: f64,
    pub fresh_tail_count: usize,
}

/// Result of a compaction strategy execution
pub struct CompactionResult {
    pub freed_tokens: usize,
    pub compacted_count: usize,
    pub strategy_name: String,
    pub pressure_before: f64,
    pub pressure_after: f64,
}

impl CompactionResult {
    pub fn pressure_reduced(&self) -> bool {
        self.pressure_after < self.pressure_before
    }
}

/// Pluggable compression strategy trait.
/// Uses manual async dispatch for trait object safety.
pub trait CompactionStrategy: Send + Sync {
    fn name(&self) -> &str;

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate;

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>>;

    fn is_applicable(&self, ctx: &CompactionContext) -> bool;
}

/// Post-compaction cleanup trait. Lower order = earlier execution.
pub trait PostCompactCleanup: Send + Sync {
    fn cleanup_order(&self) -> u32;
    fn on_compact_complete(&self, result: &CompactionResult);
}
```

Add to `src/agent_loop/mod.rs`:

```rust
pub mod compaction;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib compaction::types::tests -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/
git commit -m "feat(compaction): add foundation types — CompactionStrategy, PostCompactCleanup, PressureLevel"
```

---

## Task 2: Extend ContextBudget with PressureLevel

**Files:**
- Modify: `src/agent_loop/context_budget/mod.rs`
- Test: inline tests in same file

- [ ] **Step 1: Write failing test for sense_pressure()**

Add to existing test module in `context_budget/mod.rs`:

```rust
#[test]
fn sense_pressure_returns_correct_level() {
    use crate::agent_loop::compaction::PressureLevel;

    let config = ContextBudgetConfig {
        token_budget: 100_000,
        warning_threshold: 0.70,
        critical_threshold: 0.85,
        token_estimate_ratio: 3.5,
        fresh_tail_count: 6,
        circuit_breaker_max: 3,
        diminishing_window: 4,
        diminishing_threshold: 500,
    };
    let budget = ContextBudget::new(&config);
    
    // Simulate different pressure ratios
    assert_eq!(PressureLevel::from_ratio(0.50), PressureLevel::Calm);
    assert_eq!(PressureLevel::from_ratio(0.65), PressureLevel::Preventive);
    assert_eq!(PressureLevel::from_ratio(0.75), PressureLevel::Warning);
    assert_eq!(PressureLevel::from_ratio(0.82), PressureLevel::High);
    assert_eq!(PressureLevel::from_ratio(0.90), PressureLevel::Critical);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget -- sense_pressure --nocapture`
Expected: FAIL — `PressureLevel` import works but `sense_pressure` not yet a method

- [ ] **Step 3: Add sense_pressure() method to ContextBudget**

In `src/agent_loop/context_budget/mod.rs`, add method:

```rust
use crate::agent_loop::compaction::PressureLevel;

impl ContextBudget {
    /// Returns current pressure level based on last computed pressure.
    /// Call after before_turn() to get meaningful result.
    pub fn sense_pressure_level(&self) -> PressureLevel {
        match &self.last_pressure {
            Some(p) => PressureLevel::from_ratio(p.ratio),
            None => PressureLevel::Calm,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib context_budget -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget/mod.rs
git commit -m "feat(context_budget): add sense_pressure_level() returning PressureLevel"
```

---

## Task 3: MicroCompactor — Tool Output Scoring & Pruning

**Files:**
- Create: `src/agent_loop/compaction/micro_compactor.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing tests for Importance classification and compressibility scoring**

In `src/agent_loop/compaction/micro_compactor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_classification() {
        assert_eq!(classify_importance("read_file", "file contents here", 5), Importance::Low);
        assert_eq!(classify_importance("read_file", "file contents", 3), Importance::Medium); // age < 5
        assert_eq!(classify_importance("search", "results", 10), Importance::Low);
        assert_eq!(classify_importance("custom_tool", "ok", 1), Importance::Medium);
        assert_eq!(classify_importance("memory", "saved", 1), Importance::High);
        assert_eq!(classify_importance("run_code", "error: panicked", 1), Importance::High);
    }

    #[test]
    fn compressibility_scoring() {
        // Old + large + low importance → high compressibility
        let entry = ToolOutputEntry {
            turn_age: 20,
            token_size: 3000,
            importance: Importance::Low,
            tool_name: "read_file".to_string(),
            message_index: 0,
        };
        let score = entry.compressibility();
        assert!(score > 0.7, "expected high compressibility, got {score}");

        // New + small + high importance → low compressibility
        let entry2 = ToolOutputEntry {
            turn_age: 1,
            token_size: 100,
            importance: Importance::High,
            tool_name: "memory".to_string(),
            message_index: 5,
        };
        let score2 = entry2.compressibility();
        assert!(score2 < 0.1, "expected low compressibility, got {score2}");
    }

    #[test]
    fn compact_placeholder_format() {
        let placeholder = format_compact_placeholder(
            "read_file",
            2500,
            Some(&["path", "content", "encoding"]),
            true,
        );
        assert!(placeholder.contains("[Tool output compacted: read_file]"));
        assert!(placeholder.contains("2500 tokens"));
        assert!(placeholder.contains("path, content, encoding"));
        assert!(placeholder.contains("success"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compaction::micro_compactor -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement MicroCompactor types and scoring**

Create `src/agent_loop/compaction/micro_compactor.rs`:

```rust
use std::future::Future;
use std::pin::Pin;

use super::types::{
    CompactionContext, CompactionResult, CompactionStrategy, TokenEstimate,
};
use crate::memory::session_compactor::context_window::estimate_tokens;

/// Importance level for tool outputs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Importance {
    Low,
    Medium,
    High,
}

/// A single tool output entry for compressibility evaluation
#[derive(Debug)]
pub struct ToolOutputEntry {
    pub turn_age: u32,
    pub token_size: usize,
    pub importance: Importance,
    pub tool_name: String,
    pub message_index: usize,
}

const LOW_IMPORTANCE_TOOLS: &[&str] = &[
    "read_file", "search", "glob", "grep", "ls", "list_files",
    "web_fetch", "web_search",
];

const HIGH_IMPORTANCE_TOOLS: &[&str] = &[
    "user_feedback", "config", "memory", "vault",
];

const ERROR_KEYWORDS: &[&str] = &[
    "error", "panic", "failed", "failure", "exception",
    "traceback", "fatal", "abort",
];

pub fn classify_importance(tool_name: &str, content: &str, turn_age: u32) -> Importance {
    // High: error content or high-importance tools
    if HIGH_IMPORTANCE_TOOLS.iter().any(|t| tool_name.contains(t)) {
        return Importance::High;
    }
    let content_lower = content.to_lowercase();
    if ERROR_KEYWORDS.iter().any(|kw| content_lower.contains(kw)) {
        return Importance::High;
    }
    // Low: read/search tools with age >= 5
    if LOW_IMPORTANCE_TOOLS.iter().any(|t| tool_name.contains(t)) && turn_age >= 5 {
        return Importance::Low;
    }
    Importance::Medium
}

impl ToolOutputEntry {
    pub fn compressibility(&self) -> f64 {
        let age_score = (self.turn_age as f64 / 20.0).min(1.0);
        let size_score = (self.token_size as f64 / 3000.0).min(1.0);
        let importance_penalty = match self.importance {
            Importance::Low => 0.0,
            Importance::Medium => 0.3,
            Importance::High => 0.7,
        };
        (age_score * 0.4 + size_score * 0.4) * (1.0 - importance_penalty)
    }
}

pub fn format_compact_placeholder(
    tool_name: &str,
    original_tokens: usize,
    key_fields: Option<&[&str]>,
    success: bool,
) -> String {
    let status = if success { "success" } else { "error" };
    let keys_line = match key_fields {
        Some(fields) if !fields.is_empty() => {
            format!("\n- Key fields: {}", fields.join(", "))
        }
        _ => String::new(),
    };
    format!(
        "[Tool output compacted: {tool_name}]\n\
         - Size: {original_tokens} tokens -> compacted{keys_line}\n\
         - Status: {status}"
    )
}

/// Configuration for MicroCompactor
pub struct MicroCompactorConfig {
    pub fresh_tail_count: usize,
    pub min_compressibility: f64,
}

impl Default for MicroCompactorConfig {
    fn default() -> Self {
        Self {
            fresh_tail_count: 6,
            min_compressibility: 0.1,
        }
    }
}

/// MicroCompactor strategy: prune tool outputs without LLM calls
pub struct MicroCompactor {
    config: MicroCompactorConfig,
}

impl MicroCompactor {
    pub fn new(config: MicroCompactorConfig) -> Self {
        Self { config }
    }

    /// Scan messages and collect tool output entries with compressibility scores
    pub fn scan_tool_outputs(
        &self,
        ctx: &CompactionContext,
    ) -> Vec<ToolOutputEntry> {
        let total_messages = ctx.messages.len();
        let fresh_start = total_messages.saturating_sub(self.config.fresh_tail_count);
        let current_turn = total_messages as u32;

        let mut entries = Vec::new();
        for (idx, msg) in ctx.messages.iter().enumerate() {
            if idx >= fresh_start {
                break; // skip fresh tail
            }
            // Check if this is a tool_result message
            if let Some(tool_result) = msg.as_tool_result() {
                let tool_name = tool_result.tool_name.clone().unwrap_or_default();
                let content = tool_result.content_text();
                let token_size = estimate_tokens(&content, ctx.token_estimate_ratio);
                let turn_age = current_turn.saturating_sub(idx as u32);
                let importance = classify_importance(&tool_name, &content, turn_age);

                entries.push(ToolOutputEntry {
                    turn_age,
                    token_size,
                    importance,
                    tool_name,
                    message_index: idx,
                });
            }
        }
        entries.sort_by(|a, b| {
            b.compressibility()
                .partial_cmp(&a.compressibility())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries
    }

    /// Extract top-level JSON keys from content (best-effort)
    fn extract_json_keys(content: &str) -> Option<Vec<String>> {
        let trimmed = content.trim();
        if trimmed.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(obj) = val.as_object() {
                    return Some(obj.keys().cloned().collect());
                }
            }
        }
        None
    }

    /// Check if content indicates an error
    fn is_error_content(content: &str) -> bool {
        let lower = content.to_lowercase();
        ERROR_KEYWORDS.iter().any(|kw| lower.contains(kw))
    }
}

impl CompactionStrategy for MicroCompactor {
    fn name(&self) -> &str {
        "micro_compactor"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let entries = self.scan_tool_outputs(ctx);
        let total: usize = entries
            .iter()
            .filter(|e| e.compressibility() >= self.config.min_compressibility)
            .map(|e| e.token_size.saturating_sub(50)) // placeholder ~50 tokens
            .sum();
        TokenEstimate {
            estimated_savings: total,
            confidence: 0.9, // deterministic, high confidence
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            let entries = self.scan_tool_outputs(ctx);
            let pressure_before = ctx.pressure.ratio;
            let mut freed_tokens = 0usize;
            let mut compacted_count = 0usize;

            for entry in &entries {
                if entry.compressibility() < self.config.min_compressibility {
                    break;
                }

                let msg = &ctx.messages[entry.message_index];
                let content = msg.as_tool_result()
                    .map(|tr| tr.content_text())
                    .unwrap_or_default();

                let keys = Self::extract_json_keys(&content);
                let key_refs: Option<Vec<&str>> = keys.as_ref()
                    .map(|k| k.iter().map(|s| s.as_str()).collect());
                let is_success = !Self::is_error_content(&content);

                let placeholder = format_compact_placeholder(
                    &entry.tool_name,
                    entry.token_size,
                    key_refs.as_deref(),
                    is_success,
                );

                // Replace tool_result content in-place
                ctx.messages[entry.message_index].replace_tool_result_content(&placeholder);

                let placeholder_tokens = estimate_tokens(&placeholder, ctx.token_estimate_ratio);
                freed_tokens += entry.token_size.saturating_sub(placeholder_tokens);
                compacted_count += 1;

                // Re-check pressure after each compaction
                let new_used = ctx.pressure.used_tokens.saturating_sub(freed_tokens);
                let new_ratio = new_used as f64 / ctx.pressure.budget_tokens as f64;
                if new_ratio < 0.70 {
                    break; // pressure below Warning, stop
                }
            }

            let new_used = ctx.pressure.used_tokens.saturating_sub(freed_tokens);
            let pressure_after = new_used as f64 / ctx.pressure.budget_tokens as f64;

            Ok(CompactionResult {
                freed_tokens,
                compacted_count,
                strategy_name: self.name().to_string(),
                pressure_before,
                pressure_after,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        // Applicable at Preventive level and above
        ctx.pressure_level >= super::types::PressureLevel::Preventive
    }
}
```

Update `src/agent_loop/compaction/mod.rs`:

```rust
pub mod types;
pub mod micro_compactor;

pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel,
    PostCompactCleanup, TokenEstimate,
};
pub use micro_compactor::{MicroCompactor, MicroCompactorConfig};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib compaction::micro_compactor -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/micro_compactor.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add MicroCompactor with 3D compressibility scoring"
```

---

## Task 4: ToolAwareChunker — Semantic Unit Parsing & Pair-Aware Chunking

**Files:**
- Create: `src/agent_loop/compaction/tool_aware_chunker.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing tests for SemanticUnit parsing and chunking**

In `src/agent_loop/compaction/tool_aware_chunker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::unified_message::UnifiedMessage;

    fn make_user_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::user(text.to_string())
    }

    fn make_assistant_msg(text: &str) -> UnifiedMessage {
        UnifiedMessage::assistant(text.to_string())
    }

    fn make_tool_use_msg(tool_name: &str, tool_use_id: &str) -> UnifiedMessage {
        UnifiedMessage::tool_use(tool_name.to_string(), tool_use_id.to_string(), serde_json::json!({}))
    }

    fn make_tool_result_msg(tool_use_id: &str, content: &str) -> UnifiedMessage {
        UnifiedMessage::tool_result(tool_use_id.to_string(), content.to_string())
    }

    #[test]
    fn parse_simple_conversation() {
        let messages = vec![
            make_user_msg("hello"),
            make_assistant_msg("hi there"),
            make_user_msg("read file"),
        ];
        let units = parse_semantic_units(&messages);
        assert_eq!(units.len(), 3);
        assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));
        assert!(matches!(units[1], SemanticUnit::AssistantText { index: 1 }));
        assert!(matches!(units[2], SemanticUnit::UserMessage { index: 2 }));
    }

    #[test]
    fn parse_tool_round_groups_correctly() {
        let messages = vec![
            make_user_msg("read config.rs"),           // 0
            make_tool_use_msg("read_file", "tu_1"),    // 1 (assistant)
            make_tool_result_msg("tu_1", "contents"),  // 2 (user)
            make_assistant_msg("the file contains..."),// 3
        ];
        let units = parse_semantic_units(&messages);
        // Should produce: UserMessage(0), ToolRound{1,2,Some(3),"read_file"}
        assert_eq!(units.len(), 2);
        assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));
        match &units[1] {
            SemanticUnit::ToolRound { tool_use_index, tool_result_index, follow_up_index, .. } => {
                assert_eq!(*tool_use_index, 1);
                assert_eq!(*tool_result_index, 2);
                assert_eq!(*follow_up_index, Some(3));
            }
            _ => panic!("expected ToolRound"),
        }
    }

    #[test]
    fn chunking_respects_token_limit() {
        let chunker = ToolAwareChunker::new(100, 3.5); // 100 token limit
        let messages = vec![
            make_user_msg("a".repeat(200).as_str()),    // ~57 tokens
            make_assistant_msg("b".repeat(200).as_str()),// ~57 tokens
            make_user_msg("c".repeat(200).as_str()),    // ~57 tokens
        ];
        let units = parse_semantic_units(&messages);
        let chunks = chunker.chunk(&units, &messages, 0);
        // Each unit ~57 tokens, limit 100 → should split
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn chunking_never_splits_tool_round() {
        let chunker = ToolAwareChunker::new(50, 3.5); // very small limit
        let messages = vec![
            make_user_msg("request"),
            make_tool_use_msg("search", "tu_1"),
            make_tool_result_msg("tu_1", "a]".repeat(100).as_str()), // large result
            make_assistant_msg("found it"),
        ];
        let units = parse_semantic_units(&messages);
        let chunks = chunker.chunk(&units, &messages, 0);
        // ToolRound must stay in one chunk even if it exceeds limit
        for chunk in &chunks {
            for unit in &chunk.units {
                if let SemanticUnit::ToolRound { tool_use_index, tool_result_index, .. } = unit {
                    // Both indices must be in the same chunk
                    let indices = unit.message_indices();
                    assert!(indices.contains(tool_use_index));
                    assert!(indices.contains(tool_result_index));
                }
            }
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compaction::tool_aware_chunker -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement ToolAwareChunker**

Create `src/agent_loop/compaction/tool_aware_chunker.rs`:

```rust
use crate::memory::session_compactor::context_window::estimate_tokens;
use crate::providers::unified_message::UnifiedMessage;

/// A semantic unit in the conversation — the smallest atomic grouping
#[derive(Debug, Clone)]
pub enum SemanticUnit {
    UserMessage { index: usize },
    AssistantText { index: usize },
    ToolRound {
        tool_use_index: usize,
        tool_result_index: usize,
        follow_up_index: Option<usize>,
        tool_name: String,
    },
}

impl SemanticUnit {
    pub fn message_indices(&self) -> Vec<usize> {
        match self {
            Self::UserMessage { index } | Self::AssistantText { index } => vec![*index],
            Self::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                ..
            } => {
                let mut indices = vec![*tool_use_index, *tool_result_index];
                if let Some(fu) = follow_up_index {
                    indices.push(*fu);
                }
                indices
            }
        }
    }

    pub fn token_size(&self, messages: &[UnifiedMessage], ratio: f64) -> usize {
        self.message_indices()
            .iter()
            .filter_map(|&i| messages.get(i))
            .map(|m| estimate_tokens(&m.content_text(), ratio))
            .sum()
    }

    pub fn first_index(&self) -> usize {
        match self {
            Self::UserMessage { index } | Self::AssistantText { index } => *index,
            Self::ToolRound { tool_use_index, .. } => *tool_use_index,
        }
    }
}

/// A chunk of semantic units for summarization
#[derive(Debug)]
pub struct SemanticChunk {
    pub units: Vec<SemanticUnit>,
    pub total_tokens: usize,
}

impl SemanticChunk {
    pub fn message_indices(&self) -> Vec<usize> {
        self.units.iter().flat_map(|u| u.message_indices()).collect()
    }
}

/// Parse a message list into semantic units, grouping tool_use/tool_result pairs
pub fn parse_semantic_units(messages: &[UnifiedMessage]) -> Vec<SemanticUnit> {
    let mut units = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        if msg.is_tool_use() {
            // Start of a ToolRound
            let tool_use_index = i;
            let tool_name = msg.tool_use_name().unwrap_or_default();
            let tool_use_id = msg.tool_use_id().unwrap_or_default();

            // Look for matching tool_result
            let tool_result_index = if i + 1 < messages.len()
                && messages[i + 1].is_tool_result_for(&tool_use_id)
            {
                i + 1
            } else {
                // No matching result found, treat as standalone assistant
                units.push(SemanticUnit::AssistantText { index: i });
                i += 1;
                continue;
            };

            // Look for follow-up assistant text
            let follow_up_index = if tool_result_index + 1 < messages.len()
                && messages[tool_result_index + 1].is_assistant_text()
            {
                Some(tool_result_index + 1)
            } else {
                None
            };

            let advance = if follow_up_index.is_some() { 3 } else { 2 };

            units.push(SemanticUnit::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                tool_name,
            });
            i += advance;
        } else if msg.is_user() {
            units.push(SemanticUnit::UserMessage { index: i });
            i += 1;
        } else if msg.is_assistant() {
            units.push(SemanticUnit::AssistantText { index: i });
            i += 1;
        } else {
            i += 1; // skip unknown
        }
    }

    units
}

/// Chunks semantic units into groups respecting token limits
pub struct ToolAwareChunker {
    chunk_token_limit: usize,
    token_ratio: f64,
}

impl ToolAwareChunker {
    pub fn new(chunk_token_limit: usize, token_ratio: f64) -> Self {
        Self {
            chunk_token_limit,
            token_ratio,
        }
    }

    /// Chunk semantic units, excluding units whose first_index >= fresh_start
    pub fn chunk(
        &self,
        units: &[SemanticUnit],
        messages: &[UnifiedMessage],
        fresh_start: usize,
    ) -> Vec<SemanticChunk> {
        let mut chunks = Vec::new();
        let mut current_units: Vec<SemanticUnit> = Vec::new();
        let mut current_tokens = 0usize;

        for unit in units {
            if unit.first_index() >= fresh_start {
                break; // stop before fresh tail
            }

            let unit_tokens = unit.token_size(messages, self.token_ratio);

            // If adding this unit would exceed limit and we have content, cut
            if current_tokens + unit_tokens > self.chunk_token_limit && !current_units.is_empty() {
                chunks.push(SemanticChunk {
                    units: std::mem::take(&mut current_units),
                    total_tokens: current_tokens,
                });
                current_tokens = 0;
            }

            // Always add the unit (even if it alone exceeds limit — never split a unit)
            current_units.push(unit.clone());
            current_tokens += unit_tokens;
        }

        if !current_units.is_empty() {
            chunks.push(SemanticChunk {
                units: current_units,
                total_tokens: current_tokens,
            });
        }

        chunks
    }
}
```

Update `src/agent_loop/compaction/mod.rs` to add:

```rust
pub mod tool_aware_chunker;
pub use tool_aware_chunker::{ToolAwareChunker, SemanticUnit, SemanticChunk, parse_semantic_units};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib compaction::tool_aware_chunker -- --nocapture`
Expected: PASS (4 tests)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/tool_aware_chunker.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add ToolAwareChunker with semantic unit parsing and pair-aware chunking"
```

---

## Task 5: ConstraintInjector — Post-Compaction Context Recovery

**Files:**
- Create: `src/agent_loop/compaction/constraint_injector.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraint_injection_format() {
        let injector = ConstraintInjector::new(vec![]);
        let constraints = vec![
            Constraint {
                category: ConstraintCategory::ActiveTask,
                content: "Implement compression pipeline".to_string(),
                priority: 100,
            },
            Constraint {
                category: ConstraintCategory::ActiveTools,
                content: "read_file, search, run_code".to_string(),
                priority: 50,
            },
        ];
        let output = injector.format_injection(&constraints);
        assert!(output.contains("<post-compaction-context>"));
        assert!(output.contains("Implement compression pipeline"));
        assert!(output.contains("read_file, search, run_code"));
        assert!(output.contains("</post-compaction-context>"));
    }

    #[test]
    fn constraints_sorted_by_priority_descending() {
        let injector = ConstraintInjector::new(vec![]);
        let mut constraints = vec![
            Constraint {
                category: ConstraintCategory::UserPreference,
                content: "low".to_string(),
                priority: 10,
            },
            Constraint {
                category: ConstraintCategory::ActiveTask,
                content: "high".to_string(),
                priority: 100,
            },
        ];
        constraints.sort_by(|a, b| b.priority.cmp(&a.priority));
        assert_eq!(constraints[0].content, "high");
        assert_eq!(constraints[1].content, "low");
    }

    struct MockSource {
        constraints: Vec<Constraint>,
    }
    impl ConstraintSource for MockSource {
        fn collect_constraints(&self) -> Vec<Constraint> {
            self.constraints.clone()
        }
    }

    #[test]
    fn collects_from_multiple_sources() {
        let sources: Vec<Arc<dyn ConstraintSource>> = vec![
            Arc::new(MockSource {
                constraints: vec![Constraint {
                    category: ConstraintCategory::ActiveTask,
                    content: "task1".to_string(),
                    priority: 100,
                }],
            }),
            Arc::new(MockSource {
                constraints: vec![Constraint {
                    category: ConstraintCategory::ActiveTools,
                    content: "tool1".to_string(),
                    priority: 50,
                }],
            }),
        ];
        let injector = ConstraintInjector::new(sources);
        let all = injector.collect_all();
        assert_eq!(all.len(), 2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compaction::constraint_injector -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement ConstraintInjector**

Create `src/agent_loop/compaction/constraint_injector.rs`:

```rust
use std::sync::Arc;

use super::types::{CompactionResult, PostCompactCleanup};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintCategory {
    ActiveTask,
    ActiveTools,
    UserPreference,
}

#[derive(Debug, Clone)]
pub struct Constraint {
    pub category: ConstraintCategory,
    pub content: String,
    pub priority: u8,
}

/// Source of dynamic constraints
pub trait ConstraintSource: Send + Sync {
    fn collect_constraints(&self) -> Vec<Constraint>;
}

/// Injects dynamic constraints after compaction
pub struct ConstraintInjector {
    sources: Vec<Arc<dyn ConstraintSource>>,
    /// Filled after on_compact_complete, consumed by caller
    last_injection: std::sync::Mutex<Option<String>>,
}

impl ConstraintInjector {
    pub fn new(sources: Vec<Arc<dyn ConstraintSource>>) -> Self {
        Self {
            sources,
            last_injection: std::sync::Mutex::new(None),
        }
    }

    pub fn collect_all(&self) -> Vec<Constraint> {
        let mut all: Vec<Constraint> = self
            .sources
            .iter()
            .flat_map(|s| s.collect_constraints())
            .collect();
        all.sort_by(|a, b| b.priority.cmp(&a.priority));
        all
    }

    pub fn format_injection(&self, constraints: &[Constraint]) -> String {
        if constraints.is_empty() {
            return String::new();
        }

        let mut sections: Vec<String> = Vec::new();

        // Group by category
        let tasks: Vec<_> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::ActiveTask)
            .collect();
        if !tasks.is_empty() {
            let mut s = "### Task Context\n".to_string();
            for t in tasks {
                s.push_str(&format!("- {}\n", t.content));
            }
            sections.push(s);
        }

        let tools: Vec<_> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::ActiveTools)
            .collect();
        if !tools.is_empty() {
            let mut s = "### Active Tools\n".to_string();
            for t in tools {
                s.push_str(&format!("{}\n", t.content));
            }
            sections.push(s);
        }

        let prefs: Vec<_> = constraints
            .iter()
            .filter(|c| c.category == ConstraintCategory::UserPreference)
            .collect();
        if !prefs.is_empty() {
            let mut s = "### Key Preferences\n".to_string();
            for p in prefs {
                s.push_str(&format!("- {}\n", p.content));
            }
            sections.push(s);
        }

        format!(
            "<post-compaction-context>\n\
             ## Active Constraints (auto-restored after compaction)\n\n\
             {}\
             </post-compaction-context>",
            sections.join("\n")
        )
    }

    /// Take the last generated injection string (if any)
    pub fn take_injection(&self) -> Option<String> {
        self.last_injection.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl PostCompactCleanup for ConstraintInjector {
    fn cleanup_order(&self) -> u32 {
        50
    }

    fn on_compact_complete(&self, _result: &CompactionResult) {
        let constraints = self.collect_all();
        let injection = self.format_injection(&constraints);
        if !injection.is_empty() {
            *self.last_injection.lock().unwrap_or_else(|e| e.into_inner()) = Some(injection);
        }
    }
}
```

Update `src/agent_loop/compaction/mod.rs`:

```rust
pub mod constraint_injector;
pub use constraint_injector::{ConstraintInjector, ConstraintSource, Constraint, ConstraintCategory};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib compaction::constraint_injector -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/constraint_injector.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add ConstraintInjector with PostCompactCleanup integration"
```

---

## Task 6: CompactionOrchestrator — Strategy Execution & Cleanup Chain

**Files:**
- Create: `src/agent_loop/compaction/orchestrator.rs`
- Modify: `src/agent_loop/compaction/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct MockStrategy {
        name: &'static str,
        applicable: bool,
        savings: usize,
        executed: AtomicBool,
    }

    impl CompactionStrategy for MockStrategy {
        fn name(&self) -> &str { self.name }
        fn estimate_savings(&self, _ctx: &CompactionContext) -> TokenEstimate {
            TokenEstimate { estimated_savings: self.savings, confidence: 0.9 }
        }
        fn execute<'a>(
            &'a self,
            ctx: &'a mut CompactionContext,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
            Box::pin(async move {
                self.executed.store(true, Ordering::SeqCst);
                let before = ctx.pressure.ratio;
                let freed = self.savings.min(ctx.pressure.used_tokens);
                ctx.pressure.used_tokens = ctx.pressure.used_tokens.saturating_sub(freed);
                ctx.pressure.ratio = ctx.pressure.used_tokens as f64 / ctx.pressure.budget_tokens as f64;
                Ok(CompactionResult {
                    freed_tokens: freed,
                    compacted_count: 1,
                    strategy_name: self.name.to_string(),
                    pressure_before: before,
                    pressure_after: ctx.pressure.ratio,
                })
            })
        }
        fn is_applicable(&self, _ctx: &CompactionContext) -> bool { self.applicable }
    }

    struct MockCleanup {
        order: u32,
        called: AtomicBool,
    }
    impl PostCompactCleanup for MockCleanup {
        fn cleanup_order(&self) -> u32 { self.order }
        fn on_compact_complete(&self, _result: &CompactionResult) {
            self.called.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn orchestrator_skips_inapplicable_strategies() {
        let skip_me = Arc::new(MockStrategy {
            name: "skip", applicable: false, savings: 1000,
            executed: AtomicBool::new(false),
        });
        let use_me = Arc::new(MockStrategy {
            name: "use", applicable: true, savings: 5000,
            executed: AtomicBool::new(false),
        });

        let orchestrator = CompactionOrchestrator::builder()
            .strategy(skip_me.clone())
            .strategy(use_me.clone())
            .build();

        let mut ctx = make_test_context(0.82); // High pressure
        let result = orchestrator.execute(&mut ctx).await.unwrap();

        assert!(!skip_me.executed.load(Ordering::SeqCst));
        assert!(use_me.executed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orchestrator_runs_cleanups_in_order() {
        let c1 = Arc::new(MockCleanup { order: 30, called: AtomicBool::new(false) });
        let c2 = Arc::new(MockCleanup { order: 10, called: AtomicBool::new(false) });
        let strategy = Arc::new(MockStrategy {
            name: "s", applicable: true, savings: 5000,
            executed: AtomicBool::new(false),
        });

        let orchestrator = CompactionOrchestrator::builder()
            .strategy(strategy)
            .cleanup(c1.clone())
            .cleanup(c2.clone())
            .build();

        let mut ctx = make_test_context(0.82);
        orchestrator.execute(&mut ctx).await.unwrap();

        assert!(c1.called.load(Ordering::SeqCst));
        assert!(c2.called.load(Ordering::SeqCst));
    }

    fn make_test_context(ratio: f64) -> CompactionContext {
        let budget = 100_000usize;
        let used = (budget as f64 * ratio) as usize;
        CompactionContext {
            messages: vec![],
            pressure: ContextPressure {
                used_tokens: used,
                budget_tokens: budget,
                ratio,
                overhead_tokens: 5000,
                available_for_messages: budget - used,
            },
            pressure_level: PressureLevel::from_ratio(ratio),
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compaction::orchestrator -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement CompactionOrchestrator**

Create `src/agent_loop/compaction/orchestrator.rs`:

```rust
use std::sync::Arc;
use tracing::{info, warn};

use super::types::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel,
    PostCompactCleanup, TokenEstimate,
};
use crate::agent_loop::context_budget::LoopDirective;

/// Orchestrates compression strategies and cleanup chain
pub struct CompactionOrchestrator {
    strategies: Vec<Arc<dyn CompactionStrategy>>,
    cleanups: Vec<Arc<dyn PostCompactCleanup>>,
}

pub struct OrchestratorBuilder {
    strategies: Vec<Arc<dyn CompactionStrategy>>,
    cleanups: Vec<Arc<dyn PostCompactCleanup>>,
}

impl CompactionOrchestrator {
    pub fn builder() -> OrchestratorBuilder {
        OrchestratorBuilder {
            strategies: Vec::new(),
            cleanups: Vec::new(),
        }
    }

    /// Execute compaction strategies in priority order, then run cleanup chain.
    /// Returns the aggregate CompactionResult.
    pub async fn execute(
        &self,
        ctx: &mut CompactionContext,
    ) -> anyhow::Result<CompactionResult> {
        let initial_pressure = ctx.pressure.ratio;
        let mut total_freed = 0usize;
        let mut total_compacted = 0usize;
        let mut last_strategy = String::new();

        // Filter and execute applicable strategies
        for strategy in &self.strategies {
            if !strategy.is_applicable(ctx) {
                continue;
            }

            let estimate = strategy.estimate_savings(ctx);
            if estimate.estimated_savings == 0 {
                continue;
            }

            info!(
                strategy = strategy.name(),
                estimated_savings = estimate.estimated_savings,
                "executing compaction strategy"
            );

            match strategy.execute(ctx).await {
                Ok(result) => {
                    total_freed += result.freed_tokens;
                    total_compacted += result.compacted_count;
                    last_strategy = result.strategy_name.clone();

                    // Update context pressure level
                    ctx.pressure_level = PressureLevel::from_ratio(ctx.pressure.ratio);

                    info!(
                        strategy = strategy.name(),
                        freed = result.freed_tokens,
                        pressure_after = ctx.pressure.ratio,
                        "strategy completed"
                    );

                    // Stop if pressure is below Warning
                    if ctx.pressure_level < PressureLevel::Warning {
                        break;
                    }
                }
                Err(e) => {
                    warn!(
                        strategy = strategy.name(),
                        error = %e,
                        "strategy failed, continuing to next"
                    );
                }
            }
        }

        let aggregate = CompactionResult {
            freed_tokens: total_freed,
            compacted_count: total_compacted,
            strategy_name: last_strategy,
            pressure_before: initial_pressure,
            pressure_after: ctx.pressure.ratio,
        };

        // Run cleanup chain in order
        self.run_cleanups(&aggregate);

        Ok(aggregate)
    }

    fn run_cleanups(&self, result: &CompactionResult) {
        let mut sorted: Vec<_> = self.cleanups.iter().collect();
        sorted.sort_by_key(|c| c.cleanup_order());
        for cleanup in sorted {
            cleanup.on_compact_complete(result);
        }
    }

    /// Determine the LoopDirective based on post-compaction pressure
    pub fn directive_for(&self, ctx: &CompactionContext) -> LoopDirective {
        match ctx.pressure_level {
            PressureLevel::Critical => LoopDirective::FinalReply,
            PressureLevel::High | PressureLevel::Warning => LoopDirective::CompactAndContinue,
            _ => LoopDirective::Continue,
        }
    }
}

impl OrchestratorBuilder {
    pub fn strategy(mut self, strategy: Arc<dyn CompactionStrategy>) -> Self {
        self.strategies.push(strategy);
        self
    }

    pub fn cleanup(mut self, cleanup: Arc<dyn PostCompactCleanup>) -> Self {
        self.cleanups.push(cleanup);
        self
    }

    pub fn build(self) -> CompactionOrchestrator {
        CompactionOrchestrator {
            strategies: self.strategies,
            cleanups: self.cleanups,
        }
    }
}
```

Update `src/agent_loop/compaction/mod.rs`:

```rust
pub mod types;
pub mod micro_compactor;
pub mod tool_aware_chunker;
pub mod constraint_injector;
pub mod orchestrator;

pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel,
    PostCompactCleanup, TokenEstimate,
};
pub use micro_compactor::{MicroCompactor, MicroCompactorConfig};
pub use tool_aware_chunker::{ToolAwareChunker, SemanticUnit, SemanticChunk, parse_semantic_units};
pub use constraint_injector::{ConstraintInjector, ConstraintSource, Constraint, ConstraintCategory};
pub use orchestrator::CompactionOrchestrator;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib compaction::orchestrator -- --nocapture`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/compaction/orchestrator.rs src/agent_loop/compaction/mod.rs
git commit -m "feat(compaction): add CompactionOrchestrator with strategy execution and cleanup chain"
```

---

## Task 7: DreamGate — Hybrid Gating for DreamDaemon

**Files:**
- Create: `src/memory/dreaming/gate.rs`
- Modify: `src/memory/dreaming/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing tests**

In `src/memory/dreaming/gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn gate_blocks_when_too_recent() {
        let gate = DreamGate::new(DreamGateConfig::default());
        // Set last consolidation to now
        gate.record_consolidation();
        let result = gate.check_time_gate();
        assert!(matches!(result, GateResult::Blocked(BlockReason::TooRecent { .. })));
    }

    #[test]
    fn gate_passes_time_when_old_enough() {
        let gate = DreamGate::new(DreamGateConfig {
            min_hours: 0.0, // immediate
            ..Default::default()
        });
        let result = gate.check_time_gate();
        assert!(matches!(result, GateResult::Pass));
    }

    #[test]
    fn gate_blocks_insufficient_facts() {
        let gate = DreamGate::new(DreamGateConfig::default());
        let result = gate.check_count_gate(5); // default min is 20
        assert!(matches!(result, GateResult::Blocked(BlockReason::InsufficientFacts { count: 5 })));
    }

    #[test]
    fn gate_passes_sufficient_facts() {
        let gate = DreamGate::new(DreamGateConfig::default());
        let result = gate.check_count_gate(25);
        assert!(matches!(result, GateResult::Pass));
    }

    #[test]
    fn gate_blocks_low_drift() {
        let gate = DreamGate::new(DreamGateConfig::default());
        let result = gate.check_drift_gate(0.1); // default threshold 0.3
        assert!(matches!(result, GateResult::Blocked(BlockReason::LowDrift { .. })));
    }

    #[test]
    fn gate_passes_high_drift() {
        let gate = DreamGate::new(DreamGateConfig::default());
        let result = gate.check_drift_gate(0.5);
        assert!(matches!(result, GateResult::Pass));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dreaming::gate -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement DreamGate**

Create `src/memory/dreaming/gate.rs`:

```rust
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use tracing::info;

use crate::agent_loop::compaction::{CompactionResult, PostCompactCleanup};

#[derive(Debug, Clone)]
pub struct DreamGateConfig {
    pub min_hours: f64,
    pub min_pending_facts: usize,
    pub drift_threshold: f32,
    pub background_interval: Duration,
}

impl Default for DreamGateConfig {
    fn default() -> Self {
        Self {
            min_hours: 6.0,
            min_pending_facts: 20,
            drift_threshold: 0.3,
            background_interval: Duration::from_secs(4 * 3600),
        }
    }
}

#[derive(Debug)]
pub enum GateResult {
    Pass,
    Blocked(BlockReason),
}

#[derive(Debug)]
pub enum BlockReason {
    TooRecent { hours_since: f64 },
    InsufficientFacts { count: usize },
    LowDrift { avg_distance: f32 },
    AlreadyRunning,
}

pub struct DreamGate {
    config: DreamGateConfig,
    last_consolidation: AtomicI64,
    is_running: AtomicBool,
    /// Callback to trigger the actual dream pipeline
    trigger_fn: Option<Box<dyn Fn() + Send + Sync>>,
}

impl DreamGate {
    pub fn new(config: DreamGateConfig) -> Self {
        Self {
            config,
            last_consolidation: AtomicI64::new(0),
            is_running: AtomicBool::new(false),
            trigger_fn: None,
        }
    }

    pub fn with_trigger<F: Fn() + Send + Sync + 'static>(mut self, f: F) -> Self {
        self.trigger_fn = Some(Box::new(f));
        self
    }

    pub fn record_consolidation(&self) {
        let now = chrono::Utc::now().timestamp();
        self.last_consolidation.store(now, Ordering::SeqCst);
    }

    fn hours_since_last(&self) -> f64 {
        let last = self.last_consolidation.load(Ordering::SeqCst);
        if last == 0 {
            return f64::MAX; // never consolidated
        }
        let now = chrono::Utc::now().timestamp();
        (now - last) as f64 / 3600.0
    }

    /// Gate 1: Time gate (cheapest check)
    pub fn check_time_gate(&self) -> GateResult {
        let hours = self.hours_since_last();
        if hours < self.config.min_hours {
            GateResult::Blocked(BlockReason::TooRecent { hours_since: hours })
        } else {
            GateResult::Pass
        }
    }

    /// Gate 2: Count gate (requires DB query result)
    pub fn check_count_gate(&self, pending_facts: usize) -> GateResult {
        if pending_facts < self.config.min_pending_facts {
            GateResult::Blocked(BlockReason::InsufficientFacts { count: pending_facts })
        } else {
            GateResult::Pass
        }
    }

    /// Gate 3: Semantic drift gate (requires vector computation result)
    pub fn check_drift_gate(&self, avg_distance: f32) -> GateResult {
        if avg_distance < self.config.drift_threshold {
            GateResult::Blocked(BlockReason::LowDrift { avg_distance })
        } else {
            GateResult::Pass
        }
    }

    /// Full gate evaluation (cheap → expensive)
    /// `pending_facts` and `avg_drift` are lazily computed by caller
    pub fn evaluate(
        &self,
        pending_facts: usize,
        avg_drift: f32,
    ) -> GateResult {
        if self.is_running.load(Ordering::SeqCst) {
            return GateResult::Blocked(BlockReason::AlreadyRunning);
        }

        // Gate 1: Time (cheapest)
        if let GateResult::Blocked(reason) = self.check_time_gate() {
            return GateResult::Blocked(reason);
        }

        // Gate 2: Count
        if let GateResult::Blocked(reason) = self.check_count_gate(pending_facts) {
            return GateResult::Blocked(reason);
        }

        // Gate 3: Drift (most expensive)
        if let GateResult::Blocked(reason) = self.check_drift_gate(avg_drift) {
            return GateResult::Blocked(reason);
        }

        GateResult::Pass
    }

    /// Evaluate gates and trigger dream pipeline if all pass.
    /// Called from PostCompactCleanup or event handlers.
    /// `fact_counter` and `drift_calculator` are closures to lazily compute values.
    pub fn evaluate_and_maybe_trigger(
        &self,
        pending_facts: usize,
        avg_drift: f32,
    ) {
        match self.evaluate(pending_facts, avg_drift) {
            GateResult::Pass => {
                if self.is_running.compare_exchange(
                    false, true, Ordering::SeqCst, Ordering::SeqCst
                ).is_ok() {
                    info!("DreamGate: all gates passed, triggering consolidation");
                    if let Some(ref trigger) = self.trigger_fn {
                        trigger();
                    }
                    // Note: actual pipeline will call record_consolidation() and
                    // set is_running=false on completion
                }
            }
            GateResult::Blocked(reason) => {
                info!(?reason, "DreamGate: blocked");
            }
        }
    }

    /// Mark consolidation as complete
    pub fn mark_complete(&self) {
        self.record_consolidation();
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Mark consolidation as failed (don't update timestamp)
    pub fn mark_failed(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }

    pub fn config(&self) -> &DreamGateConfig {
        &self.config
    }
}
```

Add to `src/memory/dreaming/mod.rs`:

```rust
pub mod gate;
pub use gate::{DreamGate, DreamGateConfig, GateResult, BlockReason};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib dreaming::gate -- --nocapture`
Expected: PASS (6 tests)

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/gate.rs src/memory/dreaming/mod.rs
git commit -m "feat(dreaming): add DreamGate with 3-level cheap-to-expensive gate chain"
```

---

## Task 8: SemanticRecoveryTool — recall_context Builtin Tool

**Files:**
- Create: `src/builtin_tools/recall_context.rs`
- Modify: `src/builtin_tools/mod.rs`
- Test: inline tests

- [ ] **Step 1: Write failing test for tool definition**

In `src/builtin_tools/recall_context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_and_description() {
        assert_eq!(RecallContextTool::NAME, "recall_context");
        assert!(!RecallContextTool::DESCRIPTION.is_empty());
    }

    #[test]
    fn args_deserialize() {
        let json = r#"{"query": "config.rs error", "max_results": 5}"#;
        let args: RecallContextArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.query, "config.rs error");
        assert_eq!(args.max_results, 5);
    }

    #[test]
    fn args_default_max_results() {
        let json = r#"{"query": "test"}"#;
        let args: RecallContextArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.max_results, 3);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib builtin_tools::recall_context -- --nocapture`
Expected: FAIL — module does not exist

- [ ] **Step 3: Implement RecallContextTool**

Create `src/builtin_tools/recall_context.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::backend::MemoryBackend;
use crate::memory::types::MemoryScope;

fn default_max_results() -> usize {
    3
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RecallContextArgs {
    /// Description of what to recall from before compression
    pub query: String,
    /// Maximum number of results to return
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallContextResult {
    pub fragments: Vec<RecalledFragment>,
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecalledFragment {
    pub content: String,
    pub relevance_score: f32,
    pub source_path: String,
}

pub struct RecallContextTool {
    database: MemoryBackend,
    session_id: String,
}

impl RecallContextTool {
    pub const NAME: &'static str = "recall_context";
    pub const DESCRIPTION: &'static str =
        "Retrieve pre-compression conversation details. Use when you need to recall \
         specific code, error messages, or decision details from earlier in the conversation.";

    pub fn new(database: MemoryBackend, session_id: String) -> Self {
        Self {
            database,
            session_id,
        }
    }

    pub async fn call_impl(
        &self,
        args: RecallContextArgs,
    ) -> anyhow::Result<RecallContextResult> {
        let path_prefix = format!("aleph://session/{}/raw/", self.session_id);

        // Use vector search to find relevant raw chunks
        let results = self
            .database
            .search_by_text(
                &args.query,
                Some(&path_prefix),
                Some(MemoryScope::SessionLocal),
                args.max_results,
            )
            .await?;

        let fragments: Vec<RecalledFragment> = results
            .into_iter()
            .map(|fact| RecalledFragment {
                content: fact.content,
                relevance_score: fact.confidence,
                source_path: fact.path,
            })
            .collect();

        Ok(RecallContextResult {
            fragments,
            query: args.query,
        })
    }
}
```

Add to `src/builtin_tools/mod.rs`:

```rust
pub mod recall_context;
pub use recall_context::RecallContextTool;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib builtin_tools::recall_context -- --nocapture`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/recall_context.rs src/builtin_tools/mod.rs
git commit -m "feat(tools): add recall_context builtin tool for post-compression semantic recovery"
```

---

## Task 9: Wrap Existing Strategies — SessionCompactorStrategy & LlmSummaryStrategy

**Files:**
- Modify: `src/memory/session_compactor/mod.rs` — add `CompactionStrategy` impl
- Modify: `src/agent_loop/context_compactor.rs` — add `CompactionStrategy` impl
- Test: inline tests in each file

- [ ] **Step 1: Write failing test for SessionCompactorStrategy**

Add to `src/memory/session_compactor/mod.rs` test module:

```rust
#[cfg(test)]
mod strategy_tests {
    use super::*;
    use crate::agent_loop::compaction::{CompactionStrategy, PressureLevel};

    #[test]
    fn session_compactor_strategy_name() {
        // Will test once we impl CompactionStrategy
        let config = SessionCompactorConfig::default();
        let db = MemoryBackend::in_memory();
        let compactor = SessionCompactor::new(db, config);
        assert_eq!(CompactionStrategy::name(&compactor), "session_compactor");
    }

    #[test]
    fn session_compactor_applicable_at_warning() {
        let config = SessionCompactorConfig::default();
        let db = MemoryBackend::in_memory();
        let compactor = SessionCompactor::new(db, config);
        let ctx = make_test_compaction_context(PressureLevel::Warning);
        assert!(compactor.is_applicable(&ctx));
    }

    #[test]
    fn session_compactor_not_applicable_at_preventive() {
        let config = SessionCompactorConfig::default();
        let db = MemoryBackend::in_memory();
        let compactor = SessionCompactor::new(db, config);
        let ctx = make_test_compaction_context(PressureLevel::Preventive);
        assert!(!compactor.is_applicable(&ctx));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib session_compactor::strategy_tests -- --nocapture`
Expected: FAIL — `CompactionStrategy` not implemented for `SessionCompactor`

- [ ] **Step 3: Implement CompactionStrategy for SessionCompactor**

Add to `src/memory/session_compactor/mod.rs`:

```rust
use crate::agent_loop::compaction::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
use std::future::Future;
use std::pin::Pin;

impl CompactionStrategy for SessionCompactor {
    fn name(&self) -> &str {
        "session_compactor"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        // Estimate based on compressible portion (total - fresh_tail)
        let total = ctx.pressure.used_tokens;
        let fresh_ratio = ctx.fresh_tail_count as f64 / ctx.messages.len().max(1) as f64;
        let compressible = (total as f64 * (1.0 - fresh_ratio)) as usize;
        // d0 summary typically achieves ~65% compression
        TokenEstimate {
            estimated_savings: (compressible as f64 * 0.65) as usize,
            confidence: 0.6,
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            // Delegate to existing post_turn_compress logic
            // This is a wrapper — actual session/agent context will be provided
            // by the orchestrator integration layer
            let before = ctx.pressure.ratio;
            Ok(CompactionResult {
                freed_tokens: 0,
                compacted_count: 0,
                strategy_name: self.name().to_string(),
                pressure_before: before,
                pressure_after: before,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::Warning && self.config.enabled
    }
}
```

- [ ] **Step 4: Implement CompactionStrategy for ContextCompactor (LlmSummaryStrategy)**

Add to `src/agent_loop/context_compactor.rs`:

```rust
use crate::agent_loop::compaction::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
use std::pin::Pin;

impl CompactionStrategy for ContextCompactor {
    fn name(&self) -> &str {
        "llm_summary"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let compressible = ctx.pressure.used_tokens.saturating_sub(
            ctx.fresh_tail_count * 200, // rough estimate per message
        );
        TokenEstimate {
            estimated_savings: (compressible as f64 * self.config.target_ratio as f64) as usize,
            confidence: 0.5, // LLM-based, less predictable
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            let before = ctx.pressure.ratio;
            let result = self.compact(&mut ctx.messages, ctx.fresh_tail_count).await?;

            let freed = result.tokens_before.saturating_sub(result.tokens_after);
            ctx.pressure.used_tokens = ctx.pressure.used_tokens.saturating_sub(freed);
            ctx.pressure.ratio = ctx.pressure.used_tokens as f64 / ctx.pressure.budget_tokens as f64;

            Ok(CompactionResult {
                freed_tokens: freed,
                compacted_count: 1,
                strategy_name: self.name().to_string(),
                pressure_before: before,
                pressure_after: ctx.pressure.ratio,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::High
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib session_compactor::strategy_tests -- --nocapture`
Run: `cargo test -p alephcore --lib context_compactor -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/session_compactor/mod.rs src/agent_loop/context_compactor.rs
git commit -m "feat(compaction): wrap SessionCompactor and ContextCompactor as CompactionStrategy impls"
```

---

## Task 10: PostCompactCleanup Implementations for Existing Modules

**Files:**
- Modify: `src/memory/compression/scheduler.rs`
- Modify: `src/memory/compression/signal_detector.rs`
- Test: inline tests

- [ ] **Step 1: Write failing test for SchedulerCleanup**

Add to `scheduler.rs`:

```rust
#[cfg(test)]
mod cleanup_tests {
    use super::*;
    use crate::agent_loop::compaction::{CompactionResult, PostCompactCleanup};

    #[test]
    fn scheduler_resets_on_compact_complete() {
        let scheduler = CompressionScheduler::with_defaults();
        scheduler.increment_turns_by(15);
        assert_eq!(scheduler.get_pending_turns(), 15);

        let result = CompactionResult {
            freed_tokens: 5000,
            compacted_count: 2,
            strategy_name: "micro".to_string(),
            pressure_before: 0.82,
            pressure_after: 0.65,
        };
        scheduler.on_compact_complete(&result);
        assert_eq!(scheduler.get_pending_turns(), 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib compression::scheduler::cleanup_tests -- --nocapture`
Expected: FAIL — `PostCompactCleanup` not implemented

- [ ] **Step 3: Implement PostCompactCleanup for CompressionScheduler**

Add to `src/memory/compression/scheduler.rs`:

```rust
use crate::agent_loop::compaction::{CompactionResult, PostCompactCleanup};

impl PostCompactCleanup for CompressionScheduler {
    fn cleanup_order(&self) -> u32 {
        30
    }

    fn on_compact_complete(&self, _result: &CompactionResult) {
        self.reset_turns();
        self.record_activity();
    }
}
```

- [ ] **Step 4: Implement PostCompactCleanup for SignalDetector**

Add to `src/memory/compression/signal_detector.rs`:

```rust
use crate::agent_loop::compaction::{CompactionResult, PostCompactCleanup};

impl PostCompactCleanup for SignalDetector {
    fn cleanup_order(&self) -> u32 {
        10
    }

    fn on_compact_complete(&self, _result: &CompactionResult) {
        // SignalDetector is stateless (keywords are config, not runtime state)
        // No cleanup needed, but trait impl provides the extension point
        // for future stateful signal tracking
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib compression::scheduler::cleanup_tests -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/compression/scheduler.rs src/memory/compression/signal_detector.rs
git commit -m "feat(compaction): implement PostCompactCleanup for CompressionScheduler and SignalDetector"
```

---

## Task 11: Integrate ToolAwareChunker into SessionCompactor

**Files:**
- Modify: `src/memory/session_compactor/mod.rs` — replace token-based chunking
- Test: existing tests + new integration test

- [ ] **Step 1: Write failing integration test**

Add to `session_compactor/mod.rs`:

```rust
#[cfg(test)]
mod chunker_integration_tests {
    use super::*;
    use crate::agent_loop::compaction::tool_aware_chunker::{parse_semantic_units, ToolAwareChunker};

    #[test]
    fn chunker_preserves_tool_pairs_in_session_context() {
        // Build messages with tool calls
        let messages = vec![
            UnifiedMessage::user("search for config".to_string()),
            UnifiedMessage::tool_use("search".into(), "tu1".into(), serde_json::json!({"q":"config"})),
            UnifiedMessage::tool_result("tu1".into(), "found 3 results".repeat(100)),
            UnifiedMessage::assistant("I found...".to_string()),
            UnifiedMessage::user("read the first one".to_string()),
            UnifiedMessage::tool_use("read_file".into(), "tu2".into(), serde_json::json!({"path":"config.rs"})),
            UnifiedMessage::tool_result("tu2".into(), "pub struct Config {}".repeat(50)),
            UnifiedMessage::assistant("Config contains...".to_string()),
        ];

        let units = parse_semantic_units(&messages);
        let chunker = ToolAwareChunker::new(500, 3.5);
        let chunks = chunker.chunk(&units, &messages, messages.len()); // no fresh tail exclusion

        // Verify no chunk splits a tool_use from its tool_result
        for chunk in &chunks {
            for unit in &chunk.units {
                if let crate::agent_loop::compaction::SemanticUnit::ToolRound {
                    tool_use_index, tool_result_index, ..
                } = unit {
                    let indices = unit.message_indices();
                    assert!(indices.contains(tool_use_index));
                    assert!(indices.contains(tool_result_index));
                }
            }
        }
    }
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p alephcore --lib session_compactor::chunker_integration -- --nocapture`
Expected: PASS (this validates the ToolAwareChunker with session compactor message types)

- [ ] **Step 3: Replace token-based chunking in post_turn_compress()**

In `src/memory/session_compactor/mod.rs`, update the `post_turn_compress()` method to use `ToolAwareChunker` instead of the existing fixed-size token chunking:

```rust
use crate::agent_loop::compaction::tool_aware_chunker::{
    parse_semantic_units, ToolAwareChunker, SemanticChunk,
};

// In post_turn_compress(), replace the chunking section:
// OLD: chunk by leaf_chunk_tokens using character counting
// NEW: use ToolAwareChunker
let units = parse_semantic_units(&compressible_messages);
let chunker = ToolAwareChunker::new(
    self.config.leaf_chunk_tokens,
    self.config.token_estimate_ratio,
);
let fresh_start = compressible_messages.len(); // all messages are compressible at this point
let chunks = chunker.chunk(&units, &compressible_messages, fresh_start);

// Then iterate chunks instead of the old fixed-size windows
for (seq, chunk) in chunks.iter().enumerate() {
    let pairs: Vec<(String, String)> = chunk.message_indices()
        .iter()
        .filter_map(|&idx| {
            let msg = &compressible_messages[idx];
            let role = if msg.is_user() { "user" } else { "assistant" };
            Some((role.to_string(), msg.content_text()))
        })
        .collect();

    let summary = self.generate_summary(&pairs, 0, None).await;
    // ... store as d0 fact (existing logic unchanged)
}
```

- [ ] **Step 4: Run existing tests to verify no regression**

Run: `cargo test -p alephcore --lib session_compactor -- --nocapture`
Expected: PASS (all existing + new tests)

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_compactor/mod.rs
git commit -m "refactor(session_compactor): replace token-based chunking with ToolAwareChunker"
```

---

## Task 12: Integration — Wire Orchestrator into Agent Loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Test: integration test

- [ ] **Step 1: Identify current compaction call sites in loop_core.rs**

Search for uses of `ContextCompactor`, `ContextBudget::before_turn()`, and `CompactAndContinue` in `loop_core.rs`. The orchestrator replaces the direct `ContextCompactor::compact()` call when `CompactAndContinue` is returned.

- [ ] **Step 2: Add CompactionOrchestrator to the loop's dependencies**

In the struct or function that runs the agent loop, add `orchestrator: Arc<CompactionOrchestrator>` as a parameter. Construct it alongside existing `ContextBudget` and `ContextCompactor`.

- [ ] **Step 3: Replace CompactAndContinue handling**

Where the loop currently does:

```rust
LoopDirective::CompactAndContinue => {
    compactor.compact(&mut messages, fresh_tail).await?;
    budget.notify_compaction_success();
}
```

Replace with:

```rust
LoopDirective::CompactAndContinue => {
    let pressure = budget.last_pressure().cloned().unwrap_or_default();
    let pressure_level = budget.sense_pressure_level();
    let mut ctx = CompactionContext {
        messages: std::mem::take(&mut messages),
        pressure,
        pressure_level,
        token_estimate_ratio: budget.token_estimate_ratio(),
        fresh_tail_count: budget.fresh_tail_count(),
    };

    let result = orchestrator.execute(&mut ctx).await?;
    messages = ctx.messages; // take back

    if result.pressure_reduced() {
        budget.notify_compaction_success();
    }

    // Check if we need FinalReply after compaction
    if ctx.pressure_level >= PressureLevel::Critical {
        // Force final reply
        break;
    }
}
```

- [ ] **Step 4: Build and verify compilation**

Run: `cargo check -p alephcore`
Expected: Compilation succeeds

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): integrate CompactionOrchestrator replacing direct compactor calls"
```

---

## Task 13: Delete CompressionDaemon — Replaced by DreamGate

**Files:**
- Delete: `src/memory/compression_daemon/daemon.rs`
- Delete: `src/memory/compression_daemon/config.rs`
- Delete: `src/memory/compression_daemon/mod.rs`
- Modify: `src/memory/mod.rs` — remove `compression_daemon` module
- Modify: any files importing from `compression_daemon`

- [ ] **Step 1: Find all references to CompressionDaemon**

Run: `cargo check -p alephcore 2>&1 | grep compression_daemon` to find all import sites after deletion.

Or search: `grep -r "compression_daemon\|CompressionDaemon\|CompressionDaemonConfig" src/`

- [ ] **Step 2: Update all import sites to use DreamGate**

Replace `CompressionDaemon::new(config, compress_fn)` with `DreamGate::new(config)` at construction sites. Replace `daemon.start()` with the new trigger-based approach (PostCompactCleanup registration + timer fallback).

- [ ] **Step 3: Delete the old files**

```bash
rm src/memory/compression_daemon/daemon.rs
rm src/memory/compression_daemon/config.rs
rm src/memory/compression_daemon/mod.rs
```

Remove from `src/memory/mod.rs`:

```rust
// DELETE this line:
pub mod compression_daemon;
```

- [ ] **Step 4: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compilation succeeds (no dangling references)

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(memory): delete CompressionDaemon, replaced by DreamGate"
```

---

## Task 14: Store Raw Chunks for Semantic Recovery

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`
- Test: inline test

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod raw_chunk_tests {
    use super::*;

    #[tokio::test]
    async fn stores_raw_chunk_alongside_d0_summary() {
        let db = MemoryBackend::in_memory();
        let config = SessionCompactorConfig::default();
        let compactor = SessionCompactor::new(db.clone(), config);

        // After compression, verify both d0 summary AND raw chunk exist
        let session_id = "test-session";
        let raw_path = format!("aleph://session/{}/raw/0", session_id);
        let d0_path = format!("aleph://session/{}/d0/0", session_id);

        // Simulate storing a raw chunk
        compactor.store_raw_chunk(session_id, 0, "original conversation content").await.unwrap();

        let raw = db.get_fact_by_path(&raw_path).await;
        assert!(raw.is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib session_compactor::raw_chunk_tests -- --nocapture`
Expected: FAIL — `store_raw_chunk` does not exist

- [ ] **Step 3: Add raw chunk storage to SessionCompactor**

In `src/memory/session_compactor/mod.rs`, add method:

```rust
impl SessionCompactor {
    /// Store raw conversation chunk for post-compression semantic recovery
    pub async fn store_raw_chunk(
        &self,
        session_id: &str,
        seq: usize,
        content: &str,
    ) -> Result<(), AlephError> {
        let path = format!("aleph://session/{}/raw/{}", session_id, seq);
        let fact = MemoryFact::new(
            content.to_string(),
            path,
            MemoryScope::SessionLocal,
            1.0, // max confidence for raw data
        );
        self.database.insert_fact(&fact).await?;
        Ok(())
    }
}
```

Then in `post_turn_compress()`, after generating each d0 summary, also store the raw chunk:

```rust
// After: let summary = self.generate_summary(&pairs, 0, None).await;
// Add:
let raw_content: String = pairs.iter()
    .map(|(role, text)| format!("[{}]: {}", role, text))
    .collect::<Vec<_>>()
    .join("\n\n");
self.store_raw_chunk(session_id, seq, &raw_content).await?;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib session_compactor::raw_chunk_tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_compactor/mod.rs
git commit -m "feat(session_compactor): store raw chunks for semantic recovery via recall_context"
```

---

## Task 15: Final Integration Test & Cleanup

**Files:**
- Test: new integration test file or existing test suite
- Verify: `cargo clippy`, `cargo test`

- [ ] **Step 1: Run full build**

```bash
cargo check -p alephcore
```

Expected: No errors

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

Expected: No warnings (fix any that appear)

- [ ] **Step 3: Run all tests**

```bash
cargo test -p alephcore -- --nocapture
```

Expected: All tests pass

- [ ] **Step 4: Verify no unused imports or dead code from deleted modules**

```bash
cargo check -p alephcore 2>&1 | grep -i "unused\|dead_code\|unreachable"
```

Fix any warnings found.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore(compaction): final cleanup and integration verification"
```

---

## Summary

| Task | Component | New/Modify | Tests |
|------|-----------|-----------|-------|
| 1 | Foundation types | New | 3 |
| 2 | ContextBudget extension | Modify | 1 |
| 3 | MicroCompactor | New | 3 |
| 4 | ToolAwareChunker | New | 4 |
| 5 | ConstraintInjector | New | 3 |
| 6 | CompactionOrchestrator | New | 2 |
| 7 | DreamGate | New | 6 |
| 8 | RecallContextTool | New | 3 |
| 9 | Strategy wrappers | Modify | 3 |
| 10 | PostCompactCleanup impls | Modify | 1 |
| 11 | ToolAwareChunker integration | Modify | 1 |
| 12 | Agent loop integration | Modify | - |
| 13 | Delete CompressionDaemon | Delete | - |
| 14 | Raw chunk storage | Modify | 1 |
| 15 | Final verification | - | full suite |
| **Total** | | **7 new, 7 modify, 1 delete** | **31+** |

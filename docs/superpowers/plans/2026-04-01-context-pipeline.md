# Context Management Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Aleph's context management from monolithic ContextBudget to a strategy pipeline with API-usage-anchored token estimation, layered compaction (microcompact → tool compact → round drop), context diagnostics, and improved summarization prompts.

**Architecture:** Split `context_budget.rs` into a `context_budget/` directory module with three sub-modules: `pressure.rs` (PressureSensor with API anchoring + content-aware ratio), `pipeline.rs` (CompactionPipeline with 4 ordered stages), `diagnostics.rs` (token breakdown analytics). The pipeline runs cheapest-first and stops when pressure drops below target. `loop_core.rs` simplifies from ~50 lines of inline compaction logic to ~10 lines of pipeline invocation.

**Tech Stack:** Rust, tokio (async), tracing (structured logging)

**Spec:** `docs/superpowers/specs/2026-03-31-context-pipeline-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/agent_loop/context_budget/mod.rs` | Create (from existing `context_budget.rs`) | Re-exports, ContextBudget struct (slimmed), CircuitBreaker, DiminishingReturnsDetector |
| `src/agent_loop/context_budget/pressure.rs` | Create | PressureSensor, AnchoredUsage, content-aware ratio detection |
| `src/agent_loop/context_budget/pipeline.rs` | Create | CompactionStage trait, CompactionPipeline, ImageStripper, MicroCompact, ToolCompactStage, RoundDrop |
| `src/agent_loop/context_budget/diagnostics.rs` | Create | ContextDiagnostics, token breakdown, duplicate detection |
| `src/agent_loop/context_budget.rs` | Delete | Replaced by `context_budget/mod.rs` |
| `src/agent_loop/loop_core.rs` | Modify | Simplify directive handling, remove enforce_context_limit, add sensor/pipeline/diagnostics |
| `src/agent_loop/mod.rs` | Modify | Update module declaration and exports |
| `src/memory/session_compactor/summary_engine.rs` | Modify | Upgrade prompt templates with analysis/summary separation |

---

### Task 1: Convert `context_budget.rs` to Directory Module

**Files:**
- Create: `src/agent_loop/context_budget/mod.rs`
- Delete: `src/agent_loop/context_budget.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Create directory and move file**

```bash
cd /Volumes/TBU/Workspace/Aleph
mkdir -p src/agent_loop/context_budget
mv src/agent_loop/context_budget.rs src/agent_loop/context_budget/mod.rs
```

- [ ] **Step 2: Add sub-module declarations to mod.rs**

Add at the top of `src/agent_loop/context_budget/mod.rs`, after the doc comment:

```rust
pub mod pressure;
pub mod pipeline;
pub mod diagnostics;
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: Compilation errors about missing modules `pressure`, `pipeline`, `diagnostics` — that's correct, we'll create them next.

- [ ] **Step 4: Create stub files to fix compilation**

Create `src/agent_loop/context_budget/pressure.rs`:
```rust
//! PressureSensor — token estimation with API usage anchoring.
```

Create `src/agent_loop/context_budget/pipeline.rs`:
```rust
//! CompactionPipeline — ordered strategy execution.
```

Create `src/agent_loop/context_budget/diagnostics.rs`:
```rust
//! ContextDiagnostics — token breakdown and observability.
```

- [ ] **Step 5: Verify compilation passes**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: PASS (no errors)

- [ ] **Step 6: Run existing tests**

```bash
cargo test -p alephcore --lib context_budget 2>&1 | tail -20
```

Expected: All existing context_budget tests pass unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/context_budget/
git add -u src/agent_loop/
git commit -m "refactor(context_budget): convert to directory module"
```

---

### Task 2: Implement PressureSensor

**Files:**
- Create: `src/agent_loop/context_budget/pressure.rs` (replace stub)

- [ ] **Step 1: Write tests for content-aware ratio detection**

In `src/agent_loop/context_budget/pressure.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ratio_pure_english() {
        let ratio = detect_content_ratio("Hello world, this is a test message.");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_chinese_text() {
        let ratio = detect_content_ratio("这是一段中文文本，用于测试token估算比率。");
        assert!((ratio - 1.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_code_content() {
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{:?}\", x);\n}";
        let ratio = detect_content_ratio(code);
        assert!((ratio - 2.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_empty_string() {
        let ratio = detect_content_ratio("");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_mixed_content() {
        // ~40% CJK should trigger CJK ratio
        let mixed = "Hello world 这是中文 this is mixed 混合内容测试文本比较多";
        let ratio = detect_content_ratio(mixed);
        assert!(ratio < 3.5, "mixed with >30% CJK should use lower ratio, got {ratio}");
    }
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib pressure 2>&1 | tail -20
```

Expected: FAIL — `detect_content_ratio` not defined.

- [ ] **Step 3: Implement content-aware ratio detection**

```rust
//! PressureSensor — token estimation with API usage anchoring.
//!
//! Replaces the fixed `chars / 3.5` heuristic with:
//! 1. Content-aware ratio (CJK=1.5, code=2.5, English=3.5)
//! 2. API usage anchoring (anchor to server-reported usage, estimate only delta)

use crate::providers::message::UnifiedMessage;
use super::ContextPressure;
use crate::agent_loop::tool::ToolDefinition;

// ---- Content-aware ratio detection ----

/// CJK Unicode ranges (common subset, covers CJK Unified Ideographs).
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables (Korean)
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
    )
}

/// Heuristic: does the text look like source code?
fn looks_like_code(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().take(20).collect();
    if lines.is_empty() {
        return false;
    }
    let code_indicators = [
        "fn ", "let ", "pub ", "impl ", "use ", "mod ",  // Rust
        "def ", "class ", "import ", "from ",             // Python
        "const ", "function ", "export ", "return ",      // JS/TS
        "if (", "for (", "while (", "switch (",          // C-like
        "=>", "->", "::", "&&", "||",                    // Operators
    ];
    let indicator_lines = lines.iter().filter(|line| {
        let trimmed = line.trim();
        code_indicators.iter().any(|ind| trimmed.contains(ind))
            || trimmed.ends_with('{')
            || trimmed.ends_with('}')
            || trimmed.ends_with(';')
    }).count();
    // If >40% of sampled lines look like code, classify as code
    indicator_lines * 100 / lines.len() > 40
}

/// Detect the best chars-per-token ratio for a piece of text.
///
/// - CJK-heavy (>30% CJK chars): 1.5
/// - Code-like: 2.5
/// - Default English: 3.5
pub fn detect_content_ratio(text: &str) -> f64 {
    if text.is_empty() {
        return 3.5;
    }
    let char_count = text.chars().count();
    let cjk_count = text.chars().filter(|c| is_cjk(*c)).count();
    let cjk_fraction = cjk_count as f64 / char_count as f64;

    if cjk_fraction > 0.3 {
        1.5
    } else if looks_like_code(text) {
        2.5
    } else {
        3.5
    }
}

/// Estimate token count using content-aware ratio.
pub fn estimate_tokens_smart(content: &str) -> usize {
    let ratio = detect_content_ratio(content);
    if ratio <= 0.0 {
        return 0;
    }
    (content.len() as f64 / ratio) as usize
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib pressure 2>&1 | tail -20
```

Expected: All 5 tests PASS.

- [ ] **Step 5: Write tests for PressureSensor with anchoring**

Add to the `tests` module:

```rust
    #[test]
    fn sensor_without_anchor_estimates_from_scratch() {
        let sensor = PressureSensor::new(3.5);
        let msgs = vec![UnifiedMessage::user("Hello world")];
        let pressure = sensor.measure(&msgs, "system prompt", &[], 10_000);
        assert!(pressure.used_tokens > 0);
        assert!(pressure.ratio < 1.0);
    }

    #[test]
    fn sensor_with_anchor_uses_anchor_plus_delta() {
        let mut sensor = PressureSensor::new(3.5);
        // Simulate: after first LLM call, API reported 5000 input tokens with 2 messages
        sensor.update_anchor(5000, 2);

        let msgs = vec![
            UnifiedMessage::user("old msg 1"),
            UnifiedMessage::assistant("old msg 2"),
            UnifiedMessage::user("new msg after anchor"),
        ];
        let pressure = sensor.measure(&msgs, "system prompt", &[], 10_000);
        // Should be ~5000 (anchor) + estimate("new msg after anchor") + prompt overhead
        // NOT re-estimating the first 2 messages
        assert!(pressure.used_tokens > 5000, "should include anchor, got {}", pressure.used_tokens);
        assert!(pressure.used_tokens < 6000, "delta should be small, got {}", pressure.used_tokens);
    }

    #[test]
    fn sensor_anchor_update_replaces_previous() {
        let mut sensor = PressureSensor::new(3.5);
        sensor.update_anchor(1000, 1);
        sensor.update_anchor(5000, 3);
        // Only the latest anchor matters
        let msgs = vec![
            UnifiedMessage::user("a"),
            UnifiedMessage::assistant("b"),
            UnifiedMessage::user("c"),
            UnifiedMessage::user("new"),
        ];
        let pressure = sensor.measure(&msgs, "", &[], 10_000);
        assert!(pressure.used_tokens > 5000);
    }
```

- [ ] **Step 6: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib pressure 2>&1 | tail -20
```

Expected: FAIL — `PressureSensor` not defined.

- [ ] **Step 7: Implement PressureSensor struct**

Add after the `estimate_tokens_smart` function:

```rust
// ---- API Usage Anchoring ----

/// Anchor point from a previous API response.
#[derive(Debug, Clone)]
struct AnchoredUsage {
    /// Actual input_tokens from the API response.
    input_tokens: usize,
    /// Number of messages in the history when the anchor was taken.
    message_count_at_anchor: usize,
}

/// Pressure sensor that combines API usage anchoring with content-aware estimation.
///
/// After each LLM call, call `update_anchor()` with the API's reported `input_tokens`.
/// Subsequent `measure()` calls use `anchor + estimate(delta)` instead of estimating
/// the entire history, bounding error to only the new messages.
#[derive(Debug)]
pub struct PressureSensor {
    anchor: Option<AnchoredUsage>,
    default_ratio: f64,
}

impl PressureSensor {
    pub fn new(default_ratio: f64) -> Self {
        Self {
            anchor: None,
            default_ratio,
        }
    }

    /// Update the anchor from an API response's reported usage.
    pub fn update_anchor(&mut self, input_tokens: usize, message_count: usize) {
        self.anchor = Some(AnchoredUsage {
            input_tokens,
            message_count_at_anchor: message_count,
        });
    }

    /// Estimate tokens for a single string using content-aware ratio.
    fn estimate_str(&self, s: &str) -> usize {
        let ratio = detect_content_ratio(s);
        (s.len() as f64 / ratio) as usize
    }

    /// Estimate tokens for overhead (system prompt + tool definitions).
    fn estimate_overhead(&self, system_prompt: &str, tool_defs: &[ToolDefinition]) -> usize {
        let prompt_tokens = self.estimate_str(system_prompt);
        let tool_tokens: usize = tool_defs
            .iter()
            .map(|td| {
                self.estimate_str(&td.name)
                    + self.estimate_str(&td.description)
                    + self.estimate_str(&td.parameters.to_string())
            })
            .sum();
        prompt_tokens + tool_tokens
    }

    /// Measure context pressure.
    ///
    /// If an anchor exists, uses `anchor.input_tokens + estimate(messages_since_anchor)`.
    /// Otherwise, estimates the entire history with content-aware ratios.
    pub fn measure(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
    ) -> ContextPressure {
        let overhead = self.estimate_overhead(system_prompt, tool_defs);

        let msg_tokens = if let Some(ref anchor) = self.anchor {
            if anchor.message_count_at_anchor <= messages.len() {
                // Anchor covers messages[0..anchor_count], estimate only the delta
                let delta_tokens: usize = messages[anchor.message_count_at_anchor..]
                    .iter()
                    .map(|m| self.estimate_str(&m.text_content()))
                    .sum();
                anchor.input_tokens + delta_tokens
            } else {
                // Anchor is stale (messages were removed), fall back to full estimation
                self.estimate_all_messages(messages)
            }
        } else {
            self.estimate_all_messages(messages)
        };

        let used = overhead + msg_tokens;
        let budget = token_budget as usize;
        ContextPressure {
            used_tokens: used,
            budget_tokens: budget,
            ratio: if budget == 0 {
                1.0
            } else {
                used as f64 / budget as f64
            },
        }
    }

    /// Full estimation without anchor — uses content-aware ratios per message.
    fn estimate_all_messages(&self, messages: &[UnifiedMessage]) -> usize {
        messages
            .iter()
            .map(|m| self.estimate_str(&m.text_content()))
            .sum()
    }
}
```

- [ ] **Step 8: Run all pressure tests**

```bash
cargo test -p alephcore --lib pressure 2>&1 | tail -20
```

Expected: All 8 tests PASS.

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/context_budget/pressure.rs
git commit -m "feat(context_budget): add PressureSensor with API anchoring and content-aware ratio"
```

---

### Task 3: Implement CompactionPipeline and Stage Trait

**Files:**
- Create: `src/agent_loop/context_budget/pipeline.rs` (replace stub)

- [ ] **Step 1: Write tests for CompactionPipeline**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    /// A test stage that removes N tokens worth of content by truncating user messages.
    struct MockStage {
        name: &'static str,
        tokens_to_free: usize,
    }

    impl CompactionStage for MockStage {
        fn name(&self) -> &'static str { self.name }
        fn compact(
            &self,
            _messages: &mut [UnifiedMessage],
            _fresh_tail_count: usize,
        ) -> usize {
            self.tokens_to_free
        }
    }

    #[test]
    fn pipeline_runs_stages_in_order() {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "stage_a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "stage_b", tokens_to_free: 200 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let sensor = PressureSensor::new(3.5);
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 100, 0.0, 2);
        assert_eq!(result.stages_run.len(), 2);
        assert_eq!(result.stages_run[0].0, "stage_a");
        assert_eq!(result.stages_run[1].0, "stage_b");
    }

    #[test]
    fn pipeline_stops_early_when_pressure_below_target() {
        // Stage A frees enough that pressure drops below target — stage B should not run
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "stage_a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "stage_b", tokens_to_free: 200 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("x")];
        let sensor = PressureSensor::new(3.5);
        // Budget=10000, ratio will be ~0.0001, target=0.70 → already below target before running
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 10_000, 0.70, 2);
        assert_eq!(result.stages_run.len(), 0, "should skip all stages when already under target");
    }

    #[test]
    fn pipeline_result_tracks_total_freed() {
        let pipeline = CompactionPipeline::new(vec![
            Box::new(MockStage { name: "a", tokens_to_free: 100 }),
            Box::new(MockStage { name: "b", tokens_to_free: 250 }),
        ]);
        let mut msgs = vec![UnifiedMessage::user("test")];
        let sensor = PressureSensor::new(3.5);
        let result = pipeline.run(&mut msgs, &sensor, "", &[], 100, 0.0, 2);
        assert_eq!(result.tokens_freed, 300);
    }
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: FAIL — types not defined.

- [ ] **Step 3: Implement CompactionStage trait and CompactionPipeline**

```rust
//! CompactionPipeline — ordered strategy execution.
//!
//! Runs compaction stages cheapest-first. After each stage, re-measures pressure.
//! Stops when pressure drops below the target ratio.

use crate::providers::message::UnifiedMessage;
use crate::agent_loop::tool::ToolDefinition;
use super::pressure::PressureSensor;
use super::ContextPressure;

// ---- Trait ----

/// A single compaction strategy stage.
pub trait CompactionStage: Send + Sync {
    /// Human-readable name for tracing and diagnostics.
    fn name(&self) -> &'static str;

    /// Execute compaction on the message slice.
    /// Returns the estimated number of tokens freed.
    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize;
}

// ---- Pipeline ----

/// Result of a pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub pressure_before: ContextPressure,
    pub pressure_after: ContextPressure,
    pub tokens_freed: usize,
    pub stages_run: Vec<(&'static str, usize)>,
}

/// Ordered sequence of compaction stages, executed cheapest-first.
pub struct CompactionPipeline {
    stages: Vec<Box<dyn CompactionStage>>,
}

impl CompactionPipeline {
    pub fn new(stages: Vec<Box<dyn CompactionStage>>) -> Self {
        Self { stages }
    }

    /// Run the pipeline. Each stage executes in order; after each stage,
    /// pressure is re-measured. Stops when ratio drops below `target_ratio`.
    pub fn run(
        &self,
        messages: &mut [UnifiedMessage],
        sensor: &PressureSensor,
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
        target_ratio: f64,
        fresh_tail_count: usize,
    ) -> PipelineResult {
        let pressure_before = sensor.measure(messages, system_prompt, tool_defs, token_budget);
        let mut total_freed = 0usize;
        let mut stages_run = Vec::new();

        for stage in &self.stages {
            let current = sensor.measure(messages, system_prompt, tool_defs, token_budget);
            if current.ratio < target_ratio {
                break;
            }
            let freed = stage.compact(messages, fresh_tail_count);
            stages_run.push((stage.name(), freed));
            total_freed += freed;
            tracing::info!(
                target: "compaction_pipeline",
                stage = stage.name(),
                tokens_freed = freed,
                "stage completed"
            );
        }

        let pressure_after = sensor.measure(messages, system_prompt, tool_defs, token_budget);
        tracing::info!(
            target: "compaction_pipeline",
            stages = stages_run.len(),
            total_freed,
            ratio_before = pressure_before.ratio,
            ratio_after = pressure_after.ratio,
            "pipeline completed"
        );

        PipelineResult {
            pressure_before,
            pressure_after,
            tokens_freed: total_freed,
            stages_run,
        }
    }
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: All 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget/pipeline.rs
git commit -m "feat(context_budget): add CompactionPipeline with stage trait and early-stop"
```

---

### Task 4: Implement Stage 0 (ImageStripper) and Stage 1 (MicroCompact)

**Files:**
- Modify: `src/agent_loop/context_budget/pipeline.rs`

- [ ] **Step 1: Write tests for ImageStripper**

Add to `pipeline.rs` tests module:

```rust
    #[test]
    fn image_stripper_replaces_image_blocks() {
        use crate::providers::message::ContentBlock;
        let mut msgs = vec![
            UnifiedMessage::user_with_content(vec![
                ContentBlock::Image {
                    data: "base64data".repeat(100),
                    mime_type: "image/png".into(),
                },
            ]),
            UnifiedMessage::assistant("I see the image"),
            UnifiedMessage::user("latest question"),
        ];
        let stage = ImageStripper;
        let freed = stage.compact(&mut msgs, 1); // fresh_tail=1, protects last msg
        // Image in msgs[0] should be replaced with text marker
        let content = msgs[0].text_content();
        assert!(content.contains("[image"), "image should be replaced, got: {content}");
        assert!(freed > 0, "should have freed tokens");
    }

    #[test]
    fn image_stripper_preserves_fresh_tail_images() {
        use crate::providers::message::ContentBlock;
        let mut msgs = vec![
            UnifiedMessage::user("old text"),
            UnifiedMessage::user_with_content(vec![
                ContentBlock::Image {
                    data: "base64data".repeat(100),
                    mime_type: "image/png".into(),
                },
            ]),
        ];
        let stage = ImageStripper;
        // fresh_tail=2 → all messages are in fresh tail
        let freed = stage.compact(&mut msgs, 2);
        assert_eq!(freed, 0, "should not touch fresh tail images");
    }
```

- [ ] **Step 2: Write tests for MicroCompact**

```rust
    #[test]
    fn microcompact_clears_old_consumed_tool_results() {
        let mut msgs = vec![
            UnifiedMessage::user("do something"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(2000), false),
            UnifiedMessage::assistant("I processed it"),
            UnifiedMessage::user("latest"),
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 1); // fresh tail = last 1 msg
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert_eq!(content, "[Old result cleared]");
        assert!(freed > 0);
    }

    #[test]
    fn microcompact_preserves_unconsumed_tool_results() {
        let mut msgs = vec![
            UnifiedMessage::user("do something"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(2000), false),
            // No assistant after → unconsumed
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 0);
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert_eq!(content, "x".repeat(2000), "unconsumed result should be preserved");
        assert_eq!(freed, 0);
    }

    #[test]
    fn microcompact_preserves_fresh_tail() {
        let mut msgs = vec![
            UnifiedMessage::user("old"),
            UnifiedMessage::tool_result("c1", "Bash", "old output", false),
            UnifiedMessage::assistant("old reply"),
            // Fresh tail below
            UnifiedMessage::user("new"),
            UnifiedMessage::tool_result("c2", "Read", &"y".repeat(2000), false),
            UnifiedMessage::assistant("new reply"),
        ];
        let stage = MicroCompact;
        let freed = stage.compact(&mut msgs, 3); // last 3 are fresh
        // Fresh tail tool result should be untouched
        let (_, content) = msgs[4].tool_result_info().unwrap();
        assert_eq!(content, "y".repeat(2000));
        // Old tool result (index 1) should be cleared
        let (_, old) = msgs[1].tool_result_info().unwrap();
        assert_eq!(old, "[Old result cleared]");
        assert!(freed > 0);
    }
```

- [ ] **Step 3: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: FAIL — `ImageStripper`, `MicroCompact` not defined.

- [ ] **Step 4: Implement ImageStripper**

Add to `pipeline.rs` before the tests module:

```rust
use crate::providers::message::ContentBlock;
use super::pressure::estimate_tokens_smart;
use crate::memory::session_compactor::context_window::{
    is_tool_result_consumed, partition_fresh_tail,
};

// ---- Stage 0: ImageStripper ----

/// Replaces image content blocks with text markers.
/// Estimated savings: ~2000 tokens per image.
pub struct ImageStripper;

const IMAGE_TOKEN_ESTIMATE: usize = 2000;
const IMAGE_MARKER: &str = "[image, ~2000 tokens]";

impl CompactionStage for ImageStripper {
    fn name(&self) -> &'static str { "image_stripper" }

    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        let mut freed = 0usize;

        for msg in &mut messages[..partition] {
            let blocks = msg.content_blocks_mut();
            let mut replaced = false;
            for block in blocks.iter_mut() {
                if matches!(block, ContentBlock::Image { .. }) {
                    *block = ContentBlock::Text {
                        text: IMAGE_MARKER.to_string(),
                    };
                    freed += IMAGE_TOKEN_ESTIMATE;
                    replaced = true;
                }
            }
            if replaced {
                tracing::debug!(target: "compaction_pipeline", "replaced image block(s)");
            }
        }
        freed
    }
}
```

- [ ] **Step 5: Implement MicroCompact**

Add after ImageStripper:

```rust
// ---- Stage 1: MicroCompact ----

/// Clears consumed tool results entirely, replacing with "[Old result cleared]".
/// More aggressive than ToolCompact — retains zero information about the original content.
/// Targets oldest results first.
pub struct MicroCompact;

const CLEARED_MARKER: &str = "[Old result cleared]";

impl CompactionStage for MicroCompact {
    fn name(&self) -> &'static str { "micro_compact" }

    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize {
        let partition = partition_fresh_tail(messages, fresh_tail_count);
        let mut total_freed = 0usize;

        // Collect indices of consumed tool results in the compressible zone.
        let candidates: Vec<usize> = (0..partition)
            .filter(|&i| {
                messages[i].is_tool_result() && is_tool_result_consumed(messages, i)
            })
            .collect();

        for idx in candidates {
            let old_content = match messages[idx].tool_result_info() {
                Some((_, c)) => c,
                None => continue,
            };
            // Skip if already cleared
            if old_content == CLEARED_MARKER {
                continue;
            }
            let old_tokens = estimate_tokens_smart(&old_content);
            let new_tokens = estimate_tokens_smart(CLEARED_MARKER);
            let saved = old_tokens.saturating_sub(new_tokens);
            if saved > 0 {
                messages[idx].replace_tool_result_content(CLEARED_MARKER.to_string());
                total_freed += saved;
            }
        }
        total_freed
    }
}
```

- [ ] **Step 6: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: All tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/context_budget/pipeline.rs
git commit -m "feat(context_budget): add ImageStripper and MicroCompact stages"
```

---

### Task 5: Implement Stage 2 (ToolCompactStage) and Stage 3 (RoundDrop)

**Files:**
- Modify: `src/agent_loop/context_budget/pipeline.rs`

- [ ] **Step 1: Write tests for ToolCompactStage**

```rust
    #[test]
    fn tool_compact_stage_delegates_to_existing_compactor() {
        let mut msgs = vec![
            UnifiedMessage::user("request"),
            UnifiedMessage::tool_result("c1", "Read", &"fn main() {}\n".repeat(200), false),
            UnifiedMessage::assistant("I read the file"),
            UnifiedMessage::user("latest"),
        ];
        let stage = ToolCompactStage { token_budget: 100, threshold: 0.01, ratio: 3.5 };
        let freed = stage.compact(&mut msgs, 1);
        let (_, content) = msgs[1].tool_result_info().unwrap();
        assert!(content.starts_with("[Read file,") || content.starts_with("[Old result cleared]"),
            "should be compressed, got: {}", &content[..content.len().min(80)]);
        assert!(freed > 0);
    }
```

- [ ] **Step 2: Write tests for RoundDrop**

```rust
    #[test]
    fn round_drop_removes_oldest_round() {
        let mut msgs = vec![
            // Round 1
            UnifiedMessage::user("old question"),
            UnifiedMessage::assistant("old answer"),
            // Round 2
            UnifiedMessage::user("new question"),
            UnifiedMessage::assistant("new answer"),
        ];
        let stage = RoundDrop { token_budget: 10, ratio: 1.0 }; // tiny budget forces drops
        let freed = stage.compact(&mut msgs, 2); // fresh_tail=2, protects round 2
        // Round 1 should be dropped, replaced with truncation notice
        assert!(freed > 0);
        assert!(msgs[0].text_content().contains("truncated") || msgs[0].text_content().contains("SYSTEM"),
            "first msg should be truncation notice, got: {}", msgs[0].text_content());
    }

    #[test]
    fn round_drop_preserves_tool_pairs() {
        let mut msgs = vec![
            // Round 1 with tool calls
            UnifiedMessage::user("search for X"),
            UnifiedMessage::tool_result("c1", "Grep", "results", false),
            UnifiedMessage::assistant("found X"),
            // Round 2 (fresh tail)
            UnifiedMessage::user("next step"),
            UnifiedMessage::assistant("doing it"),
        ];
        let stage = RoundDrop { token_budget: 10, ratio: 1.0 };
        let freed = stage.compact(&mut msgs, 2);
        // Should drop entire round 1 (user + tool_result + assistant), not leave orphaned tool_result
        assert!(freed > 0);
        // No orphaned tool results should remain
        for msg in &msgs {
            if msg.is_tool_result() {
                // If a tool result remains, it must have an assistant after it
                // (this is the fresh tail, which is protected)
            }
        }
    }

    #[test]
    fn round_drop_noop_when_under_budget() {
        let mut msgs = vec![
            UnifiedMessage::user("hi"),
            UnifiedMessage::assistant("hello"),
        ];
        let stage = RoundDrop { token_budget: 100_000, ratio: 3.5 };
        let freed = stage.compact(&mut msgs, 2);
        assert_eq!(freed, 0);
        assert_eq!(msgs.len(), 2);
    }
```

- [ ] **Step 3: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: FAIL — `ToolCompactStage`, `RoundDrop` not defined.

- [ ] **Step 4: Implement ToolCompactStage**

```rust
// ---- Stage 2: ToolCompactStage ----

/// Wrapper around the existing `tool_compactor::compact_if_needed`.
/// Preserves summary metadata (e.g., "[Read file, 100 lines, rust]").
pub struct ToolCompactStage {
    pub token_budget: u64,
    pub threshold: f64,
    pub ratio: f64,
}

impl CompactionStage for ToolCompactStage {
    fn name(&self) -> &'static str { "tool_compact" }

    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize {
        let before = estimate_tokens_smart(
            &messages.iter().map(|m| m.text_content()).collect::<String>()
        );
        crate::memory::session_compactor::tool_compactor::compact_if_needed(
            messages,
            self.token_budget,
            self.threshold,
            self.ratio,
            fresh_tail_count,
        );
        let after = estimate_tokens_smart(
            &messages.iter().map(|m| m.text_content()).collect::<String>()
        );
        before.saturating_sub(after)
    }
}
```

- [ ] **Step 5: Implement RoundDrop**

```rust
// ---- Stage 3: RoundDrop ----

const TRUNCATION_NOTICE: &str =
    "[SYSTEM] Earlier conversation history was truncated \
     to fit the model's context window. Continue based on the remaining context.";

/// Groups messages into API rounds and drops the oldest complete rounds.
/// An API round = user message + all following messages until the next user message.
pub struct RoundDrop {
    pub token_budget: u64,
    pub ratio: f64,
}

/// A contiguous group of messages forming one API round.
struct ApiRound {
    start_idx: usize,
    end_idx: usize, // exclusive
    estimated_tokens: usize,
}

impl RoundDrop {
    /// Group messages into API rounds.
    fn group_rounds(messages: &[UnifiedMessage], ratio: f64) -> Vec<ApiRound> {
        let mut rounds = Vec::new();
        let mut start = 0;

        for i in 1..messages.len() {
            if messages[i].is_user() {
                let tokens: usize = messages[start..i]
                    .iter()
                    .map(|m| (m.text_content().len() as f64 / ratio) as usize)
                    .sum();
                rounds.push(ApiRound {
                    start_idx: start,
                    end_idx: i,
                    estimated_tokens: tokens,
                });
                start = i;
            }
        }
        // Last round
        if start < messages.len() {
            let tokens: usize = messages[start..]
                .iter()
                .map(|m| (m.text_content().len() as f64 / ratio) as usize)
                .sum();
            rounds.push(ApiRound {
                start_idx: start,
                end_idx: messages.len(),
                estimated_tokens: tokens,
            });
        }
        rounds
    }
}

impl CompactionStage for RoundDrop {
    fn name(&self) -> &'static str { "round_drop" }

    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        fresh_tail_count: usize,
    ) -> usize {
        // We need Vec access for drain — this stage requires the caller to pass &mut Vec.
        // Since the trait takes &mut [UnifiedMessage], we work by replacing dropped messages
        // with a single truncation notice.

        let partition = partition_fresh_tail(messages, fresh_tail_count);
        if partition == 0 {
            return 0;
        }

        let total_tokens: usize = messages.iter()
            .map(|m| (m.text_content().len() as f64 / self.ratio) as usize)
            .sum();

        if total_tokens <= self.token_budget as usize {
            return 0;
        }

        let rounds = Self::group_rounds(&messages[..partition], self.ratio);
        if rounds.is_empty() {
            return 0;
        }

        // Drop oldest rounds until under budget
        let mut tokens_to_free = total_tokens.saturating_sub(self.token_budget as usize);
        let mut freed = 0usize;
        let mut rounds_to_drop = 0usize;

        for round in &rounds {
            if tokens_to_free == 0 {
                break;
            }
            rounds_to_drop += 1;
            let round_tokens = round.estimated_tokens;
            freed += round_tokens;
            tokens_to_free = tokens_to_free.saturating_sub(round_tokens);
        }

        if rounds_to_drop > 0 && !rounds.is_empty() {
            let drop_end = rounds[rounds_to_drop - 1].end_idx;
            // Replace dropped messages with truncation notice
            // We clear content of dropped messages and mark the first one as the notice
            for i in 0..drop_end {
                if i == 0 {
                    // Replace first message with truncation notice
                    *messages.get_mut(i).unwrap() = UnifiedMessage::user(TRUNCATION_NOTICE);
                } else {
                    // Clear subsequent dropped messages (they'll be empty but structurally valid)
                    messages[i].replace_tool_result_content(String::new());
                    // For non-tool-result messages, replace with empty user message
                    if !messages[i].is_tool_result() {
                        *messages.get_mut(i).unwrap() = UnifiedMessage::user("");
                    }
                }
            }

            tracing::warn!(
                target: "compaction_pipeline",
                rounds_dropped = rounds_to_drop,
                tokens_freed = freed,
                "dropped oldest API rounds"
            );
        }

        freed
    }
}
```

- [ ] **Step 6: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib pipeline 2>&1 | tail -20
```

Expected: All tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/context_budget/pipeline.rs
git commit -m "feat(context_budget): add ToolCompactStage and RoundDrop stages"
```

---

### Task 6: Implement ContextDiagnostics

**Files:**
- Create: `src/agent_loop/context_budget/diagnostics.rs` (replace stub)

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    #[test]
    fn analyze_classifies_by_role() {
        let msgs = vec![
            UnifiedMessage::user("hello world"),         // ~3 tokens
            UnifiedMessage::assistant("goodbye world"),   // ~3 tokens
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        assert!(snapshot.user_tokens > 0);
        assert!(snapshot.assistant_tokens > 0);
        assert_eq!(snapshot.tool_result_tokens.len(), 0);
    }

    #[test]
    fn analyze_tracks_tool_results_by_name() {
        let msgs = vec![
            UnifiedMessage::user("search"),
            UnifiedMessage::tool_result("c1", "Bash", "output here", false),
            UnifiedMessage::tool_result("c2", "Read", "file content", false),
            UnifiedMessage::tool_result("c3", "Bash", "more output", false),
            UnifiedMessage::assistant("done"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        assert_eq!(snapshot.tool_result_tokens.len(), 2); // Bash and Read
        assert!(snapshot.tool_result_tokens.contains_key("Bash"));
        assert!(snapshot.tool_result_tokens.contains_key("Read"));
    }

    #[test]
    fn analyze_detects_duplicate_file_reads() {
        let msgs = vec![
            UnifiedMessage::user("read files"),
            UnifiedMessage::tool_result("c1", "Read", "content of src/main.rs", false),
            UnifiedMessage::assistant("ok"),
            UnifiedMessage::user("read again"),
            UnifiedMessage::tool_result("c2", "Read", "content of src/main.rs again", false),
            UnifiedMessage::assistant("ok again"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        // We track tool result counts by tool name, duplicate detection is based on
        // tool name occurrence count
        let read_count = snapshot.tool_result_counts.get("Read").copied().unwrap_or(0);
        assert_eq!(read_count, 2);
    }

    #[test]
    fn format_summary_produces_readable_output() {
        let msgs = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::tool_result("c1", "Bash", &"x".repeat(500), false),
            UnifiedMessage::assistant("done"),
        ];
        let snapshot = ContextDiagnostics::analyze(&msgs);
        let summary = snapshot.format_summary(10_000);
        assert!(summary.contains("Bash"), "summary should mention tool names");
    }
}
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib diagnostics 2>&1 | tail -20
```

Expected: FAIL.

- [ ] **Step 3: Implement ContextDiagnostics**

```rust
//! ContextDiagnostics — token breakdown and observability.
//!
//! Provides per-turn token classification by role and tool name,
//! duplicate file read detection, and formatted summaries for tracing.

use std::collections::HashMap;
use crate::providers::message::UnifiedMessage;
use super::pressure::estimate_tokens_smart;
use super::pipeline::PipelineResult;

/// Snapshot of token distribution across a message list.
#[derive(Debug, Clone)]
pub struct DiagnosticsSnapshot {
    pub user_tokens: usize,
    pub assistant_tokens: usize,
    pub system_tokens: usize,
    pub tool_result_tokens: HashMap<String, usize>,
    pub tool_result_counts: HashMap<String, usize>,
    pub total_tokens: usize,
}

impl DiagnosticsSnapshot {
    /// Format a concise summary string for tracing output.
    pub fn format_summary(&self, budget: u64) -> String {
        let ratio = if budget > 0 {
            self.total_tokens as f64 / budget as f64
        } else {
            1.0
        };

        let mut top_tools: Vec<(&String, &usize)> = self.tool_result_tokens.iter().collect();
        top_tools.sort_by(|a, b| b.1.cmp(a.1));
        let top_str: Vec<String> = top_tools
            .iter()
            .take(5)
            .map(|(name, tokens)| format!("{}={}", name, tokens))
            .collect();

        let mut dupes: Vec<String> = Vec::new();
        for (name, count) in &self.tool_result_counts {
            if *count > 1 {
                dupes.push(format!("{}x{}", name, count));
            }
        }

        let mut s = format!(
            "context: {}/{} ({:.0}%) | user={} asst={} | top: {}",
            self.total_tokens,
            budget,
            ratio * 100.0,
            self.user_tokens,
            self.assistant_tokens,
            top_str.join(", "),
        );
        if !dupes.is_empty() {
            s.push_str(&format!(" | dupes: {}", dupes.join(", ")));
        }
        s
    }
}

/// Accumulates diagnostics across turns.
#[derive(Debug)]
pub struct ContextDiagnostics {
    pipeline_runs: Vec<PipelineResult>,
    circuit_breaker_trips: usize,
}

impl ContextDiagnostics {
    pub fn new() -> Self {
        Self {
            pipeline_runs: Vec::new(),
            circuit_breaker_trips: 0,
        }
    }

    /// Analyze a message list and produce a token distribution snapshot.
    pub fn analyze(messages: &[UnifiedMessage]) -> DiagnosticsSnapshot {
        let mut user_tokens = 0usize;
        let mut assistant_tokens = 0usize;
        let mut system_tokens = 0usize;
        let mut tool_result_tokens: HashMap<String, usize> = HashMap::new();
        let mut tool_result_counts: HashMap<String, usize> = HashMap::new();

        for msg in messages {
            let content = msg.text_content();
            let tokens = estimate_tokens_smart(&content);

            if msg.is_user() {
                if content.starts_with("[SYSTEM]") {
                    system_tokens += tokens;
                } else {
                    user_tokens += tokens;
                }
            } else if msg.is_assistant() {
                assistant_tokens += tokens;
            } else if msg.is_tool_result() {
                if let Some((name, _)) = msg.tool_result_info() {
                    let name = name.to_string();
                    *tool_result_tokens.entry(name.clone()).or_insert(0) += tokens;
                    *tool_result_counts.entry(name).or_insert(0) += 1;
                }
            }
        }

        let total_tokens = user_tokens + assistant_tokens + system_tokens
            + tool_result_tokens.values().sum::<usize>();

        DiagnosticsSnapshot {
            user_tokens,
            assistant_tokens,
            system_tokens,
            tool_result_tokens,
            tool_result_counts,
            total_tokens,
        }
    }

    /// Record a pipeline run result.
    pub fn record_pipeline(&mut self, result: PipelineResult) {
        self.pipeline_runs.push(result);
    }

    /// Record a circuit breaker trip.
    pub fn record_circuit_breaker_trip(&mut self) {
        self.circuit_breaker_trips += 1;
    }

    /// Get pipeline run history.
    pub fn pipeline_runs(&self) -> &[PipelineResult] {
        &self.pipeline_runs
    }
}
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib diagnostics 2>&1 | tail -20
```

Expected: All 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget/diagnostics.rs
git commit -m "feat(context_budget): add ContextDiagnostics with token breakdown"
```

---

### Task 7: Upgrade Summary Prompt

**Files:**
- Modify: `src/memory/session_compactor/summary_engine.rs`

- [ ] **Step 1: Write test for analysis/summary separation**

Add to `summary_engine.rs` tests:

```rust
    #[test]
    fn test_build_prompt_leaf_has_analysis_scratchpad() {
        let messages = msgs(&[("user", "Fix the bug in auth.rs"), ("assistant", "I found the issue")]);
        let prompt = build_summary_prompt(&messages, 0, None, FallbackLevel::Normal);
        assert!(prompt.contains("<analysis>"), "leaf prompt should have analysis scratchpad");
        assert!(prompt.contains("</analysis>"), "leaf prompt should close analysis tag");
        assert!(prompt.contains("<summary>"), "leaf prompt should have summary section");
    }

    #[test]
    fn test_strip_analysis_block() {
        let input = "Some preamble\n<analysis>\nDetailed reasoning here\n</analysis>\n<summary>\nThe actual summary\n</summary>";
        let stripped = strip_analysis_block(input);
        assert!(!stripped.contains("<analysis>"));
        assert!(!stripped.contains("Detailed reasoning"));
        assert!(stripped.contains("The actual summary"));
    }

    #[test]
    fn test_strip_analysis_block_no_analysis() {
        let input = "Just a plain summary with no analysis block.";
        let stripped = strip_analysis_block(input);
        assert_eq!(stripped, input);
    }
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
cargo test -p alephcore --lib summary_engine 2>&1 | tail -20
```

Expected: FAIL — new tests reference updated prompt and `strip_analysis_block`.

- [ ] **Step 3: Update LEAF_PROMPT with analysis/summary structure**

Replace the `LEAF_PROMPT` constant:

```rust
const LEAF_PROMPT: &str = "\
You are a conversation compressor. Condense the following conversation into a structured summary.

First, analyze the conversation in an <analysis> block (this will be stripped before the summary enters context):

<analysis>
1. User's primary request and intent
2. Key technical concepts and decisions made
3. Files and code sections involved (preserve exact paths)
4. Errors encountered and how they were resolved
5. Problem-solving approaches tried (what worked, what didn't)
</analysis>

Then produce the final summary in a <summary> block:

<summary>
- Preserve ALL user message key points (never lose user intent)
- Key decisions and their rationale
- File operations with paths
- Current work state (most recent operations, detailed)
- Pending tasks and unresolved problems
- Next steps (quote from conversation where relevant)
</summary>

Omit: greetings, filler, repeated information, verbose tool outputs already summarized.";
```

- [ ] **Step 4: Implement strip_analysis_block function**

Add after the `depth_prompt` function:

```rust
/// Strip the `<analysis>...</analysis>` scratchpad from LLM summary output.
///
/// The analysis block gives the LLM reasoning space but should not enter
/// the context window. If no analysis block is found, returns input unchanged.
pub fn strip_analysis_block(text: &str) -> String {
    if let Some(start) = text.find("<analysis>") {
        if let Some(end) = text.find("</analysis>") {
            let after_end = end + "</analysis>".len();
            let mut result = String::new();
            result.push_str(text[..start].trim());
            if after_end < text.len() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(text[after_end..].trim());
            }
            return result;
        }
    }
    text.to_string()
}
```

- [ ] **Step 5: Run tests — verify they pass**

```bash
cargo test -p alephcore --lib summary_engine 2>&1 | tail -20
```

Expected: All tests PASS (existing + new).

- [ ] **Step 6: Commit**

```bash
git add src/memory/session_compactor/summary_engine.rs
git commit -m "feat(summary_engine): upgrade prompt with analysis/summary separation"
```

---

### Task 8: Integrate Pipeline into loop_core.rs

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Modify: `src/agent_loop/context_budget/mod.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Add PressureSensor and Pipeline fields to AgentLoop**

In `loop_core.rs`, update the `AgentLoop` struct (around line 255):

```rust
use super::context_budget::pressure::PressureSensor;
use super::context_budget::pipeline::{
    CompactionPipeline, ImageStripper, MicroCompact, ToolCompactStage, RoundDrop, PipelineResult,
};
use super::context_budget::diagnostics::ContextDiagnostics;
```

Add fields to `AgentLoop<P>`:

```rust
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    context_budget: Mutex<Option<super::context_budget::ContextBudget>>,
    pressure_sensor: Mutex<PressureSensor>,
    compaction_pipeline: CompactionPipeline,
    diagnostics: Mutex<ContextDiagnostics>,
    delta_sink: Box<dyn DeltaSink>,
    cancel_token: CancellationToken,
}
```

- [ ] **Step 2: Update constructor to initialize new fields**

Update `AgentLoop::new()`:

```rust
pub fn new(
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    cancel_token: CancellationToken,
) -> Self {
    let pipeline = CompactionPipeline::new(vec![
        Box::new(ImageStripper),
        Box::new(MicroCompact),
        Box::new(ToolCompactStage {
            token_budget: config.token_budget as u64,
            threshold: 0.70,
            ratio: 3.5,
        }),
        Box::new(RoundDrop {
            token_budget: config.token_budget as u64,
            ratio: 3.5,
        }),
    ]);
    Self {
        provider,
        tool_registry,
        prompt_builder,
        safety_guard,
        config,
        context_budget: Mutex::new(None),
        pressure_sensor: Mutex::new(PressureSensor::new(3.5)),
        compaction_pipeline: pipeline,
        diagnostics: Mutex::new(ContextDiagnostics::new()),
        delta_sink: Box::new(NoopSink),
        cancel_token,
    }
}
```

- [ ] **Step 3: Simplify the directive match block**

Replace the ~50-line directive handling block (lines 390-444 of the original) with:

```rust
match budget_directive {
    super::context_budget::LoopDirective::CompactAndContinue => {
        let result = {
            let sensor = self.pressure_sensor.lock().unwrap_or_else(|e| e.into_inner());
            self.compaction_pipeline.run(
                &mut messages,
                &sensor,
                &system_prompt,
                &tool_defs,
                ctx_budget.token_budget(),
                ctx_budget.warning_threshold() as f64,
                ctx_budget.fresh_tail_count(),
            )
        };
        if result.pressure_after.ratio < ctx_budget.warning_threshold()
            || result.tokens_freed > 500
        {
            ctx_budget.notify_compaction_success();
        }
        self.diagnostics.lock().unwrap_or_else(|e| e.into_inner())
            .record_pipeline(result);
    }
    super::context_budget::LoopDirective::FinalReply => {
        let result = {
            let sensor = self.pressure_sensor.lock().unwrap_or_else(|e| e.into_inner());
            self.compaction_pipeline.run(
                &mut messages,
                &sensor,
                &system_prompt,
                &tool_defs,
                ctx_budget.token_budget(),
                0.5,
                ctx_budget.fresh_tail_count(),
            )
        };
        self.diagnostics.lock().unwrap_or_else(|e| e.into_inner())
            .record_pipeline(result);
        messages.push(UnifiedMessage::user(CRITICAL_CONTEXT_NOTICE));
    }
    super::context_budget::LoopDirective::StopDiminishing => {
        messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
    }
    super::context_budget::LoopDirective::Continue => {}
}
```

- [ ] **Step 4: Remove enforce_context_limit call and functions**

Delete the `enforce_context_limit()` function call from the loop (around line 447).

Delete these three functions from `loop_core.rs`:
- `find_safe_cut_point()`
- `remove_oldest_complete_round()`
- `enforce_context_limit()`

Keep `TRUNCATION_NOTICE`, `CRITICAL_CONTEXT_NOTICE`, `DIMINISHING_RETURNS_NOTICE` constants — they're still used.

Remove `TRUNCATION_NOTICE` (it's now in `pipeline.rs` as `RoundDrop::TRUNCATION_NOTICE`). Keep the other two.

- [ ] **Step 5: Add API usage anchoring after LLM response**

After the existing `total_tokens` tracking block (around line 494):

```rust
// Track tokens
if let Some(usage) = &response.usage {
    total_tokens += (usage.input_tokens + usage.output_tokens) as usize;
    // Anchor pressure sensor to API-reported usage
    self.pressure_sensor.lock().unwrap_or_else(|e| e.into_inner())
        .update_anchor(usage.input_tokens as usize, messages.len());
}
```

- [ ] **Step 6: Add diagnostics logging each turn**

Add after the budget evaluation block, before the LLM call:

```rust
// Diagnostics: log token distribution
{
    let snapshot = ContextDiagnostics::analyze(&messages);
    tracing::info!(
        target: "context_diagnostics",
        "{}",
        snapshot.format_summary(self.config.token_budget as u64)
    );
}
```

- [ ] **Step 7: Update mod.rs exports**

In `src/agent_loop/mod.rs`, update:

```rust
pub use context_budget::{
    ContextBudget, ContextBudgetConfig, ContextPressure, LoopDirective, TurnMetrics,
};
pub use context_budget::pressure::PressureSensor;
pub use context_budget::pipeline::{CompactionPipeline, CompactionStage, PipelineResult};
pub use context_budget::diagnostics::{ContextDiagnostics, DiagnosticsSnapshot};
```

- [ ] **Step 8: Remove token_estimate_ratio from ContextBudgetConfig**

In `context_budget/mod.rs`, remove the `token_estimate_ratio` field from `ContextBudgetConfig` and `ContextBudget`. Update `ContextBudget::new()` accordingly. The `before_turn` method should now accept a `&PressureSensor` parameter instead of computing pressure inline. Alternatively, keep `before_turn` signature unchanged for now and delegate to `PressureSensor` internally — this minimizes changes to the loop.

**Recommended approach**: Keep `before_turn` unchanged for now. The `ContextPressure::compute` function remains but internally uses a fixed ratio. The `PressureSensor` in the loop handles the smart estimation separately. This avoids a large signature refactor.

- [ ] **Step 9: Verify compilation**

```bash
cargo check -p alephcore 2>&1 | head -40
```

Expected: PASS.

- [ ] **Step 10: Run all tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -30
```

Expected: All tests PASS.

- [ ] **Step 11: Commit**

```bash
git add src/agent_loop/
git commit -m "feat(agent_loop): integrate CompactionPipeline, PressureSensor, and Diagnostics into loop"
```

---

### Task 9: Cleanup and Final Verification

**Files:**
- All modified files

- [ ] **Step 1: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20
```

Expected: No warnings.

- [ ] **Step 2: Run full test suite**

```bash
cargo test -p alephcore 2>&1 | tail -30
```

Expected: All tests PASS.

- [ ] **Step 3: Verify no dead code**

Search for any remaining references to deleted functions:

```bash
grep -rn "enforce_context_limit\|find_safe_cut_point\|remove_oldest_complete_round" src/ --include="*.rs"
```

Expected: No matches (except in comments or tests that reference the old behavior).

- [ ] **Step 4: Run cargo fmt**

```bash
cargo fmt -p alephcore
```

- [ ] **Step 5: Final commit**

```bash
git add -u core/
git commit -m "chore(context_budget): cleanup dead code and format"
```

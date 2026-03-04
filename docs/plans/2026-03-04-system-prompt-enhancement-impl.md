# System Prompt Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add PromptMode, Bootstrap files, token budget, truncation warnings, and heartbeat layer to Aleph's PromptPipeline.

**Architecture:** Extend the existing PromptLayer trait with `supports_mode()`, add 3 new layers (Bootstrap, Heartbeat, BudgetGuard post-processing), and enhance PromptPipeline::execute() to accept PromptMode and return PromptResult with truncation stats.

**Tech Stack:** Rust, existing PromptPipeline/PromptLayer framework in `core/src/thinker/`

**Design Doc:** `docs/plans/2026-03-04-system-prompt-enhancement-design.md`

---

### Task 1: Add PromptMode enum and extend PromptLayer trait

**Files:**
- Create: `core/src/thinker/prompt_mode.rs`
- Modify: `core/src/thinker/prompt_layer.rs`
- Modify: `core/src/thinker/mod.rs` (re-export)

**Step 1: Write the failing test in prompt_mode.rs**

Create `core/src/thinker/prompt_mode.rs`:

```rust
/// Prompt rendering mode — controls which layers participate in assembly.
///
/// Orthogonal to `AssemblyPath`: path selects *which variant* (Basic/Soul/Context),
/// mode selects *how verbose* (Full/Compact/Minimal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PromptMode {
    /// All layers included (primary agent).
    #[default]
    Full,
    /// Essential layers only (sub-agent, saves ~60% tokens).
    Compact,
    /// Identity + tools + response format only (ultra-lightweight).
    Minimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_full() {
        assert_eq!(PromptMode::default(), PromptMode::Full);
    }

    #[test]
    fn modes_are_distinct() {
        assert_ne!(PromptMode::Full, PromptMode::Compact);
        assert_ne!(PromptMode::Compact, PromptMode::Minimal);
        assert_ne!(PromptMode::Full, PromptMode::Minimal);
    }
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test -p alephcore --lib thinker::prompt_mode -- --nocapture`

**Step 3: Extend PromptLayer trait with `supports_mode()`**

In `core/src/thinker/prompt_layer.rs`, add import and default method:

```rust
// Add import at top
use super::prompt_mode::PromptMode;

// Add to PromptLayer trait (after inject()):
    /// Whether this layer participates in the given prompt mode.
    /// Default: true (all modes). Override to exclude from Compact/Minimal.
    fn supports_mode(&self, _mode: PromptMode) -> bool {
        true
    }
```

**Step 4: Add re-export in mod.rs**

In `core/src/thinker/mod.rs`, add:
```rust
pub mod prompt_mode;
pub use prompt_mode::PromptMode;
```

**Step 5: Run all tests to verify nothing breaks**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`

**Step 6: Commit**

```bash
git add core/src/thinker/prompt_mode.rs core/src/thinker/prompt_layer.rs core/src/thinker/mod.rs
git commit -m "thinker: add PromptMode enum and supports_mode() to PromptLayer trait"
```

---

### Task 2: Override `supports_mode()` in existing layers

**Files:**
- Modify: `core/src/thinker/layers/runtime_context.rs`
- Modify: `core/src/thinker/layers/environment.rs`
- Modify: `core/src/thinker/layers/runtime_capabilities.rs`
- Modify: `core/src/thinker/layers/security.rs`
- Modify: `core/src/thinker/layers/protocol_tokens.rs`
- Modify: `core/src/thinker/layers/operational_guidelines.rs`
- Modify: `core/src/thinker/layers/citation_standards.rs`
- Modify: `core/src/thinker/layers/generation_models.rs`
- Modify: `core/src/thinker/layers/skill_instructions.rs`
- Modify: `core/src/thinker/layers/special_actions.rs`
- Modify: `core/src/thinker/layers/guidelines.rs`
- Modify: `core/src/thinker/layers/thinking_guidance.rs`
- Modify: `core/src/thinker/layers/skill_mode.rs`
- Modify: `core/src/thinker/layers/soul.rs`
- Modify: `core/src/thinker/layers/custom_instructions.rs`

**Step 1: Write a test that validates mode support matrix**

Add to a new test section in `core/src/thinker/prompt_pipeline.rs`:

```rust
#[cfg(test)]
mod mode_tests {
    use super::*;
    use crate::thinker::prompt_mode::PromptMode;

    #[test]
    fn full_mode_includes_all_layers() {
        let pipeline = PromptPipeline::default_layers();
        for layer in &pipeline.layers {
            assert!(
                layer.supports_mode(PromptMode::Full),
                "Layer '{}' should support Full mode",
                layer.name()
            );
        }
    }

    #[test]
    fn compact_mode_excludes_heavy_layers() {
        let pipeline = PromptPipeline::default_layers();
        let excluded_in_compact = [
            "runtime_context",
            "environment",
            "runtime_capabilities",
            "poe_success_criteria",
            "protocol_tokens",
            "operational_guidelines",
            "citation_standards",
            "generation_models",
            "skill_instructions",
            "special_actions",
            "guidelines",
            "thinking_guidance",
            "skill_mode",
        ];
        for layer in &pipeline.layers {
            if excluded_in_compact.contains(&layer.name()) {
                assert!(
                    !layer.supports_mode(PromptMode::Compact),
                    "Layer '{}' should NOT support Compact mode",
                    layer.name()
                );
            } else {
                assert!(
                    layer.supports_mode(PromptMode::Compact),
                    "Layer '{}' SHOULD support Compact mode",
                    layer.name()
                );
            }
        }
    }

    #[test]
    fn minimal_mode_only_core_layers() {
        let pipeline = PromptPipeline::default_layers();
        let included_in_minimal = ["soul", "tools", "hydrated_tools", "response_format", "language"];
        for layer in &pipeline.layers {
            if included_in_minimal.contains(&layer.name()) {
                assert!(
                    layer.supports_mode(PromptMode::Minimal),
                    "Layer '{}' SHOULD support Minimal mode",
                    layer.name()
                );
            } else {
                assert!(
                    !layer.supports_mode(PromptMode::Minimal),
                    "Layer '{}' should NOT support Minimal mode",
                    layer.name()
                );
            }
        }
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::mode_tests -- --nocapture`
Expected: FAIL — all layers return `true` for all modes (default impl)

**Step 3: Add `supports_mode()` overrides to Full-only layers**

For each layer that should be **Full-only** (excluded from Compact AND Minimal), add:

```rust
use crate::thinker::prompt_mode::PromptMode;

fn supports_mode(&self, mode: PromptMode) -> bool {
    matches!(mode, PromptMode::Full)
}
```

Apply to: `RuntimeContextLayer`, `EnvironmentLayer`, `RuntimeCapabilitiesLayer`, `PoePromptLayer`, `ProtocolTokensLayer`, `OperationalGuidelinesLayer`, `CitationStandardsLayer`, `GenerationModelsLayer`, `SkillInstructionsLayer`, `SpecialActionsLayer`, `GuidelinesLayer`, `ThinkingGuidanceLayer`, `SkillModeLayer`.

For layers that support **Full + Compact** but NOT Minimal:

```rust
fn supports_mode(&self, mode: PromptMode) -> bool {
    !matches!(mode, PromptMode::Minimal)
}
```

Apply to: `ProfileLayer`, `RoleLayer`, `SecurityLayer`, `CustomInstructionsLayer`.

For `SoulLayer` — supports all modes but renders differently in Minimal:

```rust
fn supports_mode(&self, _mode: PromptMode) -> bool {
    true  // default, but Minimal renders identity-only (handled in inject())
}
```

In `SoulLayer::inject()`, add at the start:
```rust
// In Minimal mode, only emit identity line
// (This requires PromptMode to be passed via LayerInput — see Task 4)
```

Note: SoulLayer Minimal behavior deferred to Task 4 when LayerInput gains mode field.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::mode_tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/thinker/layers/ core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: add supports_mode() overrides to all existing layers"
```

---

### Task 3: Update PromptPipeline::execute() to filter by mode

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs`
- Modify: `core/src/thinker/prompt_layer.rs` (add mode to LayerInput)

**Step 1: Write the failing test**

Add to `prompt_pipeline.rs`:

```rust
#[test]
fn execute_with_mode_filters_layers() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools = vec![];
    let input = LayerInput::basic(&config, &tools);

    let full = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Full);
    let compact = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Compact);
    let minimal = pipeline.execute_with_mode(AssemblyPath::Basic, &input, PromptMode::Minimal);

    // Full should be longest
    assert!(full.len() > compact.len(), "Full ({}) should be longer than Compact ({})", full.len(), compact.len());
    // Compact should be longer than Minimal
    assert!(compact.len() > minimal.len(), "Compact ({}) should be longer than Minimal ({})", compact.len(), minimal.len());
    // Minimal should still have some content (response format, language)
    assert!(!minimal.is_empty(), "Minimal should not be empty");
}
```

**Step 2: Run test to verify it fails (method doesn't exist)**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::mode_tests::execute_with_mode -- --nocapture`
Expected: FAIL — `execute_with_mode` not found

**Step 3: Implement `execute_with_mode()`**

In `core/src/thinker/prompt_pipeline.rs`:

```rust
use crate::thinker::prompt_mode::PromptMode;

impl PromptPipeline {
    /// Execute pipeline with mode filtering (path + mode).
    pub fn execute_with_mode(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
    ) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                layer.inject(&mut output, input);
            }
        }
        output
    }
}
```

Keep the existing `execute()` method as-is (it defaults to Full mode behavior).

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::mode_tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: add execute_with_mode() to PromptPipeline"
```

---

### Task 4: Add PromptMode to LayerInput and PromptBuilder

**Files:**
- Modify: `core/src/thinker/prompt_layer.rs` (LayerInput)
- Modify: `core/src/thinker/prompt_builder/mod.rs` (PromptBuilder)
- Modify: `core/src/thinker/layers/soul.rs` (Minimal mode identity-only)

**Step 1: Write failing test for SoulLayer minimal mode**

Add to `core/src/thinker/layers/soul.rs` tests:

```rust
#[test]
fn soul_layer_minimal_mode_identity_only() {
    let layer = SoulLayer;
    let config = PromptConfig::default();
    let soul = SoulManifest {
        identity: "I am Aleph.".to_string(),
        voice: SoulVoice {
            tone: "Warm and professional".to_string(),
            ..Default::default()
        },
        directives: vec!["Always help.".to_string()],
        ..Default::default()
    };
    let input = LayerInput::soul(&config, &[], &soul)
        .with_mode(PromptMode::Minimal);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    // Should contain identity
    assert!(out.contains("I am Aleph"), "Should contain identity");
    // Should NOT contain communication style or directives
    assert!(!out.contains("Communication Style"), "Should not contain style in Minimal");
    assert!(!out.contains("Always help"), "Should not contain directives in Minimal");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::layers::soul -- --nocapture`
Expected: FAIL — `with_mode` method doesn't exist

**Step 3: Add `mode` field to LayerInput**

In `core/src/thinker/prompt_layer.rs`:

```rust
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
    pub poe: Option<&'a PoePromptContext>,
    pub profile: Option<&'a crate::config::ProfileConfig>,
    /// Prompt mode for this assembly (default: Full)
    pub mode: PromptMode,
}
```

Add `mode: PromptMode::Full` to all existing factory methods (`basic`, `hydration`, `soul`, `context`).

Add builder method:
```rust
pub fn with_mode(mut self, mode: PromptMode) -> Self {
    self.mode = mode;
    self
}
```

**Step 4: Update SoulLayer for Minimal mode**

In `core/src/thinker/layers/soul.rs`, in `inject()`, after the identity block:

```rust
// In Minimal mode, only emit identity line
if input.mode == PromptMode::Minimal {
    return;
}
```

**Step 5: Add mode-aware build methods to PromptBuilder**

In `core/src/thinker/prompt_builder/mod.rs`, add:

```rust
/// Build system prompt with explicit mode control.
pub fn build_system_prompt_with_mode(
    &self,
    tools: &[ToolInfo],
    soul: &SoulManifest,
    profile: Option<&ProfileConfig>,
    mode: PromptMode,
) -> String {
    let input = LayerInput::soul(&self.config, tools, soul)
        .with_profile(profile)
        .with_mode(mode);
    self.pipeline.execute_with_mode(AssemblyPath::Soul, &input, mode)
}
```

**Step 6: Run all thinker tests**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_layer.rs core/src/thinker/prompt_builder/mod.rs core/src/thinker/layers/soul.rs
git commit -m "thinker: add PromptMode to LayerInput and PromptBuilder"
```

---

### Task 5: Add TokenBudget and PromptResult types

**Files:**
- Create: `core/src/thinker/prompt_budget.rs`
- Modify: `core/src/thinker/mod.rs` (re-export)

**Step 1: Write types and tests**

Create `core/src/thinker/prompt_budget.rs`:

```rust
//! Token budget management for system prompt assembly.
//!
//! Prevents system prompt bloat by enforcing character limits
//! and providing truncation statistics.

use super::prompt_mode::PromptMode;

/// Budget configuration for system prompt assembly.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum total characters for assembled system prompt.
    /// Default: 80_000 (~20K tokens).
    pub max_total_chars: usize,
    /// Bootstrap section total budget.
    /// Default: 100_000.
    pub max_bootstrap_chars: usize,
    /// Per-bootstrap-file character limit.
    /// Default: 20_000.
    pub max_per_file_chars: usize,
    /// Warning mode for truncation events.
    pub truncation_warning: TruncationWarning,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max_total_chars: 80_000,
            max_bootstrap_chars: 100_000,
            max_per_file_chars: 20_000,
            truncation_warning: TruncationWarning::default(),
        }
    }
}

/// Warning mode for truncation events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TruncationWarning {
    /// Never warn.
    Off,
    /// Warn once per session per unique truncation state.
    #[default]
    Once,
    /// Warn every time.
    Always,
}

/// Result of prompt assembly with truncation metadata.
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// The assembled system prompt string.
    pub prompt: String,
    /// Truncation statistics (empty if nothing was truncated).
    pub truncation_stats: Vec<TruncationStat>,
    /// Which mode was used.
    pub mode: PromptMode,
}

/// Per-section truncation statistics.
#[derive(Debug, Clone)]
pub struct TruncationStat {
    /// Layer name that was truncated or removed.
    pub layer_name: String,
    /// Original character count before truncation.
    pub original_chars: usize,
    /// Final character count (0 if fully removed).
    pub final_chars: usize,
    /// Whether the section was fully removed.
    pub fully_removed: bool,
}

/// Truncate content preserving head and tail, UTF-8 safe.
///
/// Keeps `head_ratio` of chars from the start and `tail_ratio` from the end,
/// inserting a truncation marker in between.
pub fn truncate_with_head_tail(content: &str, max_chars: usize, head_ratio: f64, tail_ratio: f64) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let head_chars = (max_chars as f64 * head_ratio) as usize;
    let tail_chars = (max_chars as f64 * tail_ratio) as usize;
    let marker = format!("\n\n[... {} chars truncated ...]\n\n", content.len() - head_chars - tail_chars);

    // UTF-8 safe boundary finding
    let head_end = content.char_indices()
        .take_while(|(i, _)| *i < head_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);

    let tail_start = content.char_indices()
        .rev()
        .take_while(|(i, _)| content.len() - *i <= tail_chars)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    format!("{}{}{}", &content[..head_end], marker, &content[tail_start..])
}

/// Enforce total budget by removing sections from lowest priority.
///
/// Returns (trimmed prompt, truncation stats).
/// Sections with priority in `protected_priorities` are never removed.
pub fn enforce_budget(
    sections: &[(u32, &str, &str)],  // (priority, layer_name, content)
    max_total: usize,
    protected_priorities: &[u32],
) -> (String, Vec<TruncationStat>) {
    let total: usize = sections.iter().map(|(_, _, c)| c.len()).sum();
    if total <= max_total {
        let prompt = sections.iter().map(|(_, _, c)| *c).collect::<Vec<_>>().join("\n\n");
        return (prompt, vec![]);
    }

    let mut stats = Vec::new();
    let mut excess = total - max_total;

    // Sort sections by priority descending (lowest priority = highest number = removed first)
    let mut removal_order: Vec<usize> = (0..sections.len()).collect();
    removal_order.sort_by(|a, b| sections[*b].0.cmp(&sections[*a].0));

    let mut included = vec![true; sections.len()];

    for idx in removal_order {
        if excess == 0 {
            break;
        }
        let (priority, name, content) = &sections[idx];
        if protected_priorities.contains(priority) {
            continue;
        }
        let saved = content.len();
        included[idx] = false;
        stats.push(TruncationStat {
            layer_name: name.to_string(),
            original_chars: saved,
            final_chars: 0,
            fully_removed: true,
        });
        excess = excess.saturating_sub(saved);
    }

    let prompt = sections.iter()
        .enumerate()
        .filter(|(i, _)| included[*i])
        .map(|(_, (_, _, c))| *c)
        .collect::<Vec<_>>()
        .join("\n\n");

    (prompt, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_values() {
        let b = TokenBudget::default();
        assert_eq!(b.max_total_chars, 80_000);
        assert_eq!(b.max_bootstrap_chars, 100_000);
        assert_eq!(b.max_per_file_chars, 20_000);
        assert_eq!(b.truncation_warning, TruncationWarning::Once);
    }

    #[test]
    fn truncate_short_content_unchanged() {
        let content = "Hello, world!";
        let result = truncate_with_head_tail(content, 100, 0.7, 0.2);
        assert_eq!(result, content);
    }

    #[test]
    fn truncate_long_content_preserves_head_tail() {
        let content = "A".repeat(1000);
        let result = truncate_with_head_tail(&content, 100, 0.7, 0.2);
        assert!(result.len() < 1000);
        assert!(result.contains("[..."));
        assert!(result.contains("truncated ...]"));
        // Head should be ~70 chars of 'A'
        assert!(result.starts_with("AAAA"));
        // Tail should be ~20 chars of 'A'
        assert!(result.ends_with("AAAA"));
    }

    #[test]
    fn truncate_multibyte_utf8_safe() {
        let content = "你好世界".repeat(100);  // 4 chars * 100 = 400 chars, ~1200 bytes
        let result = truncate_with_head_tail(&content, 50, 0.7, 0.2);
        // Should not panic on multi-byte boundary
        assert!(result.contains("[..."));
    }

    #[test]
    fn enforce_budget_under_limit_no_stats() {
        let sections = vec![
            (100, "role", "You are an AI."),
            (500, "tools", "Available tools: none"),
        ];
        let (prompt, stats) = enforce_budget(&sections, 1000, &[]);
        assert!(stats.is_empty());
        assert!(prompt.contains("You are an AI."));
        assert!(prompt.contains("Available tools"));
    }

    #[test]
    fn enforce_budget_removes_lowest_priority_first() {
        let sections = vec![
            (100, "role", "Role content"),
            (500, "tools", "Tools content"),
            (1600, "language", "Language content"),
            (1500, "custom", "Custom content"),
        ];
        // Total ~60 chars, limit to 40 — must remove some
        let (prompt, stats) = enforce_budget(&sections, 40, &[100, 500]);
        // Should remove language (1600) first, then custom (1500)
        assert!(!stats.is_empty());
        assert!(prompt.contains("Role content"));
        assert!(prompt.contains("Tools content"));
        // At least language should be removed (highest priority number)
        let removed: Vec<_> = stats.iter().map(|s| s.layer_name.as_str()).collect();
        assert!(removed.contains(&"language"));
    }

    #[test]
    fn enforce_budget_protects_layers() {
        let sections = vec![
            (100, "role", &"A".repeat(100)),
            (500, "tools", &"B".repeat(100)),
        ];
        // Both protected, can't remove anything
        let (_, stats) = enforce_budget(&sections, 50, &[100, 500]);
        assert!(stats.is_empty());  // Nothing removed (both protected)
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_budget -- --nocapture`
Expected: PASS

**Step 3: Add re-export in mod.rs**

In `core/src/thinker/mod.rs`:
```rust
pub mod prompt_budget;
pub use prompt_budget::{TokenBudget, TruncationWarning, PromptResult, TruncationStat};
```

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_budget.rs core/src/thinker/mod.rs
git commit -m "thinker: add TokenBudget, PromptResult, and truncation utilities"
```

---

### Task 6: Add BootstrapLayer

**Files:**
- Create: `core/src/thinker/layers/bootstrap.rs`
- Modify: `core/src/thinker/layers/mod.rs` (re-export)
- Modify: `core/src/thinker/prompt_pipeline.rs` (add to default_layers)

**Step 1: Write BootstrapLayer with tests**

Create `core/src/thinker/layers/bootstrap.rs`:

```rust
//! Bootstrap file injection layer.
//!
//! Loads workspace-level context files (CONTEXT.md, INSTRUCTIONS.md, etc.)
//! and injects them into the system prompt with truncation management.

use std::path::{Path, PathBuf};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_budget::truncate_with_head_tail;

/// Ordered list of bootstrap files by priority (highest first).
const BOOTSTRAP_FILES: &[&str] = &[
    "CONTEXT.md",
    "INSTRUCTIONS.md",
    "TOOLS.md",
    "MEMORY.md",
];

/// Layer that injects workspace bootstrap files into the system prompt.
pub struct BootstrapLayer {
    workspace: PathBuf,
    max_chars_per_file: usize,
    max_chars_total: usize,
}

impl BootstrapLayer {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            max_chars_per_file: 20_000,
            max_chars_total: 100_000,
        }
    }

    pub fn with_limits(mut self, per_file: usize, total: usize) -> Self {
        self.max_chars_per_file = per_file;
        self.max_chars_total = total;
        self
    }

    /// Load and format bootstrap files within budget.
    fn load_files(&self) -> Option<String> {
        let mut sections = Vec::new();
        let mut total_chars = 0;

        for &filename in BOOTSTRAP_FILES {
            if total_chars >= self.max_chars_total {
                break;
            }

            let path = self.resolve_path(filename);
            let content = match std::fs::read_to_string(&path) {
                Ok(c) if !c.trim().is_empty() => c,
                _ => continue,
            };

            // Per-file truncation
            let content = if content.len() > self.max_chars_per_file {
                truncate_with_head_tail(&content, self.max_chars_per_file, 0.7, 0.2)
            } else {
                content
            };

            // Total budget check
            let remaining = self.max_chars_total - total_chars;
            let content = if content.len() > remaining {
                truncate_with_head_tail(&content, remaining, 0.7, 0.2)
            } else {
                content
            };

            total_chars += content.len();
            sections.push(format!("### {}\n{}", filename, content));
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("## Workspace Context\n\n{}", sections.join("\n\n")))
        }
    }

    /// Resolve bootstrap file path. Check .aleph/ first, then workspace root.
    fn resolve_path(&self, filename: &str) -> PathBuf {
        let aleph_path = self.workspace.join(".aleph").join(filename);
        if aleph_path.exists() {
            return aleph_path;
        }
        self.workspace.join(filename)
    }
}

impl PromptLayer for BootstrapLayer {
    fn name(&self) -> &'static str { "bootstrap" }
    fn priority(&self) -> u32 { 55 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        if let Some(content) = self.load_files() {
            output.push_str(&content);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn create_bootstrap_file(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn layer_metadata() {
        let layer = BootstrapLayer::new(PathBuf::from("/tmp"));
        assert_eq!(layer.name(), "bootstrap");
        assert_eq!(layer.priority(), 55);
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn loads_existing_files() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "CONTEXT.md", "# Project\nRust AI assistant");
        create_bootstrap_file(dir.path(), "INSTRUCTIONS.md", "Always use Chinese");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("## Workspace Context"));
        assert!(content.contains("### CONTEXT.md"));
        assert!(content.contains("Rust AI assistant"));
        assert!(content.contains("### INSTRUCTIONS.md"));
        assert!(content.contains("Always use Chinese"));
    }

    #[test]
    fn skips_missing_files() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "CONTEXT.md", "Only context file");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("CONTEXT.md"));
        assert!(!content.contains("INSTRUCTIONS.md"));
        assert!(!content.contains("TOOLS.md"));
    }

    #[test]
    fn returns_none_when_no_files() {
        let dir = tempdir().unwrap();
        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        assert!(layer.load_files().is_none());
    }

    #[test]
    fn truncates_large_files() {
        let dir = tempdir().unwrap();
        let large_content = "X".repeat(30_000);
        create_bootstrap_file(dir.path(), "CONTEXT.md", &large_content);

        let layer = BootstrapLayer::new(dir.path().to_path_buf())
            .with_limits(20_000, 100_000);
        let content = layer.load_files().unwrap();

        assert!(content.contains("[..."));
        assert!(content.len() < 30_000);
    }

    #[test]
    fn respects_total_budget() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "CONTEXT.md", &"A".repeat(80_000));
        create_bootstrap_file(dir.path(), "INSTRUCTIONS.md", &"B".repeat(80_000));

        let layer = BootstrapLayer::new(dir.path().to_path_buf())
            .with_limits(80_000, 100_000);
        let content = layer.load_files().unwrap();

        // Total should be around 100K, not 160K
        assert!(content.len() <= 110_000);  // some overhead from headers/markers
    }

    #[test]
    fn prefers_aleph_dir() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "CONTEXT.md", "root version");
        fs::create_dir_all(dir.path().join(".aleph")).unwrap();
        create_bootstrap_file(&dir.path().join(".aleph"), "CONTEXT.md", "aleph version");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("aleph version"));
        assert!(!content.contains("root version"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::bootstrap -- --nocapture`
Expected: PASS

**Step 3: Add to layers/mod.rs and default_layers**

In `core/src/thinker/layers/mod.rs`:
```rust
pub mod bootstrap;
pub use bootstrap::BootstrapLayer;
```

In `core/src/thinker/prompt_pipeline.rs`, `default_layers()`:
- Do NOT add BootstrapLayer to default_layers (it requires a workspace path)
- Instead, add a `with_bootstrap(workspace: PathBuf)` builder method:

```rust
impl PromptPipeline {
    /// Add bootstrap layer for workspace context injection.
    pub fn with_bootstrap(mut self, workspace: PathBuf, per_file: usize, total: usize) -> Self {
        let layer = BootstrapLayer::new(workspace).with_limits(per_file, total);
        self.layers.push(Box::new(layer));
        self.layers.sort_by_key(|l| l.priority());
        self
    }
}
```

**Step 4: Run all tests**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/thinker/layers/bootstrap.rs core/src/thinker/layers/mod.rs core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: add BootstrapLayer for workspace context file injection"
```

---

### Task 7: Add HeartbeatLayer

**Files:**
- Create: `core/src/thinker/layers/heartbeat.rs`
- Modify: `core/src/thinker/layers/mod.rs`
- Modify: `core/src/thinker/prompt_pipeline.rs`

**Step 1: Write HeartbeatLayer with tests**

Create `core/src/thinker/layers/heartbeat.rs`:

```rust
//! Heartbeat layer — progress reporting guidance for long-running tasks.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct HeartbeatLayer;

impl PromptLayer for HeartbeatLayer {
    fn name(&self) -> &'static str { "heartbeat" }
    fn priority(&self) -> u32 { 710 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Progress Reporting\n\n");
        output.push_str("For long-running tasks (multi-step plans, large file operations):\n");
        output.push_str("- Report progress after completing each major step\n");
        output.push_str("- Use structured progress format: [step N/total] description\n");
        output.push_str("- If a step takes unusually long, report intermediate status\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn layer_metadata() {
        let layer = HeartbeatLayer;
        assert_eq!(layer.name(), "heartbeat");
        assert_eq!(layer.priority(), 710);
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn injects_progress_guidance() {
        let layer = HeartbeatLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Progress Reporting"));
        assert!(out.contains("[step N/total]"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::heartbeat -- --nocapture`
Expected: PASS

**Step 3: Register in layers/mod.rs and default_layers**

In `core/src/thinker/layers/mod.rs`:
```rust
pub mod heartbeat;
pub use heartbeat::HeartbeatLayer;
```

In `core/src/thinker/prompt_pipeline.rs`, `default_layers()`, add after `ProtocolTokensLayer` (700):
```rust
Box::new(HeartbeatLayer),
```

**Step 4: Run all tests**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/thinker/layers/heartbeat.rs core/src/thinker/layers/mod.rs core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: add HeartbeatLayer for long-task progress guidance"
```

---

### Task 8: Integrate TokenBudget into PromptPipeline

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs`
- Modify: `core/src/thinker/prompt_builder/mod.rs`

**Step 1: Write failing test for budget-enforced assembly**

Add to `prompt_pipeline.rs`:

```rust
#[test]
fn assemble_with_budget_trims_when_over() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools = vec![];
    let input = LayerInput::basic(&config, &tools);

    // Very small budget to force trimming
    let budget = TokenBudget {
        max_total_chars: 200,
        ..Default::default()
    };

    let result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Full, &budget);
    assert!(result.prompt.len() <= 250);  // Some overhead tolerance
    assert!(!result.truncation_stats.is_empty(), "Should have truncation stats");
}
```

**Step 2: Run test — should fail (method doesn't exist)**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline -- --nocapture`

**Step 3: Implement `assemble()` method**

In `core/src/thinker/prompt_pipeline.rs`:

```rust
use crate::thinker::prompt_budget::{TokenBudget, PromptResult, enforce_budget};

impl PromptPipeline {
    /// Assemble system prompt with mode filtering and budget enforcement.
    pub fn assemble(
        &self,
        path: AssemblyPath,
        input: &LayerInput,
        mode: PromptMode,
        budget: &TokenBudget,
    ) -> PromptResult {
        // 1. Collect sections from matching layers
        let mut sections: Vec<(u32, &str, String)> = Vec::new();
        for layer in &self.layers {
            if layer.paths().contains(&path) && layer.supports_mode(mode) {
                let mut section = String::new();
                layer.inject(&mut section, input);
                if !section.is_empty() {
                    sections.push((layer.priority(), layer.name(), section));
                }
            }
        }

        // 2. Check total size
        let total: usize = sections.iter().map(|(_, _, c)| c.len()).sum();
        if total <= budget.max_total_chars {
            let prompt = sections.iter().map(|(_, _, c)| c.as_str()).collect::<Vec<_>>().join("");
            return PromptResult {
                prompt,
                truncation_stats: vec![],
                mode,
            };
        }

        // 3. Enforce budget — protected priorities: Soul(50), Role(100), Tools(500/501), ResponseFormat(1200)
        let refs: Vec<(u32, &str, &str)> = sections.iter()
            .map(|(p, n, c)| (*p, *n, c.as_str()))
            .collect();
        let protected = &[50u32, 75, 100, 500, 501, 1200];
        let (prompt, stats) = enforce_budget(&refs, budget.max_total_chars, protected);

        PromptResult {
            prompt,
            truncation_stats: stats,
            mode,
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline -- --nocapture`
Expected: PASS

**Step 5: Add budget-aware build method to PromptBuilder**

In `core/src/thinker/prompt_builder/mod.rs`:

```rust
/// Build system prompt with mode and budget control.
pub fn build_with_budget(
    &self,
    tools: &[ToolInfo],
    soul: &SoulManifest,
    profile: Option<&ProfileConfig>,
    mode: PromptMode,
    budget: &TokenBudget,
) -> PromptResult {
    let input = LayerInput::soul(&self.config, tools, soul)
        .with_profile(profile)
        .with_mode(mode);
    self.pipeline.assemble(AssemblyPath::Soul, &input, mode, budget)
}
```

**Step 6: Run all tests**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_pipeline.rs core/src/thinker/prompt_builder/mod.rs
git commit -m "thinker: integrate TokenBudget into PromptPipeline assembly"
```

---

### Task 9: Add TokenBudget to PromptConfig and ThinkerConfig

**Files:**
- Modify: `core/src/thinker/prompt_builder/mod.rs` (PromptConfig)
- Modify: `core/src/thinker/mod.rs` (ThinkerConfig)

**Step 1: Add `token_budget` field to PromptConfig**

In `core/src/thinker/prompt_builder/mod.rs`:

```rust
use crate::thinker::prompt_budget::TokenBudget;

pub struct PromptConfig {
    // ... existing fields ...
    /// Token budget for system prompt assembly.
    pub token_budget: TokenBudget,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            token_budget: TokenBudget::default(),
        }
    }
}
```

**Step 2: Add `bootstrap_workspace` to ThinkerConfig**

In `core/src/thinker/mod.rs`:

```rust
pub struct ThinkerConfig {
    // ... existing fields ...
    /// Workspace root for bootstrap file loading. When set, enables BootstrapLayer.
    pub bootstrap_workspace: Option<PathBuf>,
}
```

**Step 3: Run all tests to ensure nothing breaks**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_builder/mod.rs core/src/thinker/mod.rs
git commit -m "thinker: add TokenBudget to PromptConfig and bootstrap_workspace to ThinkerConfig"
```

---

### Task 10: Wire up truncation warnings in Thinker

**Files:**
- Modify: `core/src/thinker/mod.rs`

**Step 1: Add truncation warning helper**

In `core/src/thinker/mod.rs`, add a helper function (not inside Thinker struct — keep it simple):

```rust
use prompt_budget::{PromptResult, TruncationStat, TruncationWarning};

/// Format truncation stats into a human-readable warning message.
pub fn format_truncation_warning(stats: &[TruncationStat]) -> String {
    let parts: Vec<String> = stats.iter().map(|s| {
        if s.fully_removed {
            format!("{} fully removed", s.layer_name)
        } else {
            let pct = if s.original_chars > 0 {
                ((s.original_chars - s.final_chars) as f64 / s.original_chars as f64 * 100.0) as u32
            } else {
                0
            };
            format!("{} {}→{} chars (-{}%)", s.layer_name, s.original_chars, s.final_chars, pct)
        }
    }).collect();
    format!("[System] Context truncated: {}", parts.join(", "))
}
```

**Step 2: Write test**

```rust
#[test]
fn format_truncation_warning_message() {
    let stats = vec![
        TruncationStat {
            layer_name: "CONTEXT.md".to_string(),
            original_chars: 45000,
            final_chars: 20000,
            fully_removed: false,
        },
        TruncationStat {
            layer_name: "guidelines".to_string(),
            original_chars: 500,
            final_chars: 0,
            fully_removed: true,
        },
    ];
    let msg = format_truncation_warning(&stats);
    assert!(msg.contains("[System] Context truncated"));
    assert!(msg.contains("CONTEXT.md"));
    assert!(msg.contains("45000→20000"));
    assert!(msg.contains("guidelines fully removed"));
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib thinker::format_truncation_warning -- --nocapture`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/thinker/mod.rs
git commit -m "thinker: add truncation warning formatter"
```

---

### Task 11: Final integration test

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs` (integration test)

**Step 1: Write end-to-end integration test**

```rust
#[test]
fn full_pipeline_with_mode_and_budget() {
    use crate::thinker::prompt_budget::TokenBudget;

    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools = vec![];

    // Full mode, generous budget — no truncation
    let input = LayerInput::basic(&config, &tools);
    let budget = TokenBudget::default();
    let result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Full, &budget);
    assert!(result.truncation_stats.is_empty());
    assert_eq!(result.mode, PromptMode::Full);

    // Compact mode — shorter prompt
    let compact_result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Compact, &budget);
    assert!(compact_result.prompt.len() < result.prompt.len(),
        "Compact ({}) should be shorter than Full ({})",
        compact_result.prompt.len(), result.prompt.len());

    // Minimal mode — shortest
    let minimal_result = pipeline.assemble(AssemblyPath::Basic, &input, PromptMode::Minimal, &budget);
    assert!(minimal_result.prompt.len() < compact_result.prompt.len(),
        "Minimal ({}) should be shorter than Compact ({})",
        minimal_result.prompt.len(), compact_result.prompt.len());
}
```

**Step 2: Run test**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::full_pipeline_with_mode_and_budget -- --nocapture`
Expected: PASS

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib thinker -- --nocapture`
Expected: PASS (with pre-existing markdown_skill failures as known)

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: add integration tests for PromptMode and TokenBudget"
```

---

## Summary

| Task | Component | New Files | Modified Files |
|------|-----------|-----------|----------------|
| 1 | PromptMode enum | `prompt_mode.rs` | `prompt_layer.rs`, `mod.rs` |
| 2 | Layer mode overrides | — | 15 layer files, `prompt_pipeline.rs` |
| 3 | execute_with_mode() | — | `prompt_pipeline.rs` |
| 4 | Mode in LayerInput | — | `prompt_layer.rs`, `prompt_builder/mod.rs`, `soul.rs` |
| 5 | TokenBudget types | `prompt_budget.rs` | `mod.rs` |
| 6 | BootstrapLayer | `layers/bootstrap.rs` | `layers/mod.rs`, `prompt_pipeline.rs` |
| 7 | HeartbeatLayer | `layers/heartbeat.rs` | `layers/mod.rs`, `prompt_pipeline.rs` |
| 8 | Budget in Pipeline | — | `prompt_pipeline.rs`, `prompt_builder/mod.rs` |
| 9 | Config integration | — | `prompt_builder/mod.rs`, `mod.rs` |
| 10 | Truncation warnings | — | `mod.rs` |
| 11 | Integration tests | — | `prompt_pipeline.rs` |

**Total: 4 new files, ~15 modified files, 11 commits**

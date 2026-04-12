# Prompt System Unification & Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify Aleph's dual prompt systems into PromptPipeline, add tool usage grammar, section caching, and hybrid memory injection.

**Architecture:** Migrate all `agent_loop::PromptBuilder` call sites to `thinker::PromptBuilder` (which wraps `PromptPipeline`). Add `AgentRoleLayer` for sub-agent support. Then enhance with three Claude Code-inspired capabilities. Finally delete all old system code (~1000 lines).

**Tech Stack:** Rust, `serde_json`, `parking_lot::RwLock`, LanceDB (existing)

---

## File Structure

### Phase 1: Migration

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `src/thinker/layers/agent_role.rs` | AgentRoleLayer — sub-agent role + protocol injection |
| Modify | `src/thinker/layers/mod.rs` | Register AgentRoleLayer |
| Modify | `src/thinker/prompt_layer.rs` | Add `agent_def` to `LayerInput` |
| Modify | `src/thinker/prompt_pipeline.rs` | Add AgentRoleLayer to `default_layers()`, update protected list |
| Modify | `src/thinker/prompt_builder/mod.rs` | Add `build_for_agent()` method |
| Modify | `src/agent_loop/loop_core.rs` | Switch from old PromptBuilder to thinker::PromptBuilder |
| Modify | `src/agent_loop/subagent_runner.rs` | Use thinker::PromptBuilder::build_for_agent() |
| Modify | `src/agent_loop/mod.rs` | Remove old prompt_builder/prompt_sections re-exports, keep ToolInfo |
| Delete | `src/agent_loop/prompt_builder.rs` | Old PromptBuilder (~776 lines) |
| Delete | `src/agent_loop/prompt_sections/*.rs` (26 files) | Old section renderers |

### Phase 2: Enhancements

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `src/thinker/layers/tool_usage_grammar.rs` | ToolUsageGrammarLayer — tool preference rules |
| Modify | `src/thinker/layers/mod.rs` | Register ToolUsageGrammarLayer |
| Modify | `src/thinker/prompt_pipeline.rs` | Add ToolUsageGrammarLayer to default_layers(), section cache |
| Modify | `src/agent_loop/prompt_builder.rs` (ToolInfo only — see note) | Add `usage_hint` field to ToolInfo |
| Modify | `src/thinker/memory_context.rs` | Enhanced hybrid format_for_prompt() |
| Modify | `src/thinker/layers/memory_augmentation.rs` | Structured index + vector dual-path |

**Note on ToolInfo:** `ToolInfo` is defined in `agent_loop/prompt_builder.rs` and widely used. Before deleting the old file, we must relocate `ToolInfo` to its own module. This is handled in Task 1.

---

## Phase 1: Unify to PromptPipeline

### Task 1: Relocate ToolInfo to standalone module

`ToolInfo` lives in `src/agent_loop/prompt_builder.rs` and is re-exported from `agent_loop/mod.rs`. It's used across the entire codebase. We must extract it before deleting the old file.

**Files:**
- Create: `src/agent_loop/tool_info.rs`
- Modify: `src/agent_loop/mod.rs`
- Modify: `src/agent_loop/prompt_builder.rs`
- Test: `cargo check -p alephcore`

- [ ] **Step 1: Create `src/agent_loop/tool_info.rs`**

```rust
//! Lightweight tool info for prompt building.

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Optional JSON Schema for tool parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
}
```

- [ ] **Step 2: Update `src/agent_loop/prompt_builder.rs` to re-export from new location**

Remove the `ToolInfo` struct definition (lines 10-19) and replace with:

```rust
// Re-export ToolInfo from its new home
pub use super::tool_info::ToolInfo;
```

- [ ] **Step 3: Update `src/agent_loop/mod.rs` to declare new module**

Add after line 28 (`mod tool;`):

```rust
pub mod tool_info;
```

And update the re-export block (line 50-52) to also re-export from `tool_info`:

```rust
pub use tool_info::ToolInfo;
```

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: compiles successfully — all existing `use crate::agent_loop::ToolInfo` paths still work.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool_info.rs src/agent_loop/prompt_builder.rs src/agent_loop/mod.rs
git commit -m "refactor(prompt): extract ToolInfo to standalone module for migration"
```

---

### Task 2: Create AgentRoleLayer

This layer replaces the old `prompt_sections::agent_role` + `prompt_sections::resolve()` mechanism. It reads `AgentDef.prompt_sections` and injects the appropriate protocol content.

**Files:**
- Create: `src/thinker/layers/agent_role.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the test**

```rust
// At the bottom of agent_role.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};

    #[test]
    fn skips_when_no_agent_def() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn injects_role_header_for_subagent() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let agent = AgentDef::new("explore", AgentMode::SubAgent)
            .with_prompt_sections(vec!["explore_constraints".into()]);
        let input = LayerInput::basic(&config, &[]).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("**explore**"), "should contain agent id");
        assert!(out.contains("Sub-Agent Role"), "should have role header");
        assert!(
            out.contains("read-only exploration specialist"),
            "should resolve explore_constraints"
        );
    }

    #[test]
    fn skips_role_header_for_primary() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let agent = AgentDef::new("main", AgentMode::Primary);
        let input = LayerInput::basic(&config, &[]).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(!out.contains("Sub-Agent Role"));
    }

    #[test]
    fn unknown_sections_are_skipped() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let agent = AgentDef::new("custom", AgentMode::SubAgent)
            .with_prompt_sections(vec!["nonexistent".into()]);
        let input = LayerInput::basic(&config, &[]).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Sub-Agent Role"));
        assert!(!out.contains("nonexistent"));
    }

    #[test]
    fn verify_protocol_injected() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let agent = AgentDef::new("verify", AgentMode::SubAgent)
            .with_prompt_sections(vec!["verify_protocol".into()]);
        let input = LayerInput::basic(&config, &[]).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("adversarial verifier"));
        assert!(out.contains("VERDICT:"));
    }

    #[test]
    fn metadata_correct() {
        let layer = AgentRoleLayer;
        assert_eq!(layer.name(), "agent_role");
        assert_eq!(layer.priority(), 55);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
        assert!(layer.paths().contains(&AssemblyPath::Soul));
        assert!(layer.paths().contains(&AssemblyPath::Context));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib thinker::layers::agent_role 2>&1 | tail -5`
Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Write the implementation**

Create `src/thinker/layers/agent_role.rs`:

```rust
//! AgentRoleLayer — inject sub-agent role description and protocol constraints (priority 55)
//!
//! Replaces the old `prompt_sections::agent_role` + `prompt_sections::resolve()` mechanism.
//! Reads `AgentDef.prompt_sections` to determine which protocol blocks to inject.

use crate::agents::{AgentDef, AgentMode};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct AgentRoleLayer;

impl AgentRoleLayer {
    /// Resolve a section name to its static content block.
    /// Migrated from `agent_loop::prompt_sections::resolve()`.
    fn resolve_section(name: &str) -> Option<&'static str> {
        match name {
            "explore_constraints" => Some(EXPLORE_CONSTRAINTS),
            "coder_guidelines" => Some(CODER_GUIDELINES),
            "researcher_protocol" => Some(RESEARCHER_PROTOCOL),
            "verify_protocol" => Some(VERIFY_PROTOCOL),
            "plan_protocol" => Some(PLAN_PROTOCOL),
            _ => None,
        }
    }

    /// Render the shared sub-agent role header.
    fn render_role_header(agent_id: &str) -> String {
        format!(
            r#"# Sub-Agent Role

You are **{agent_id}**, a specialized sub-agent of Aleph.

## Contract
- Complete the delegated task fully — do not leave partial work.
- Stay within your declared tool set. Do not attempt to use tools you are not given.
- End with a concise report: what you did, key findings or changes, and recommended next steps.
- If you cannot complete the task with your available tools, explain what is missing rather than guessing.

## Communication
- Be direct and factual. No filler, no apologies.
- Structure output for machine readability when the caller is another agent.

"#
        )
    }
}

impl PromptLayer for AgentRoleLayer {
    fn name(&self) -> &'static str {
        "agent_role"
    }

    fn priority(&self) -> u32 {
        55
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Soul, AssemblyPath::Context]
    }

    fn supports_mode(&self, _mode: PromptMode) -> bool {
        true
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let agent = match input.agent_def {
            Some(a) => a,
            None => return,
        };

        // Sub-agents get the shared role header
        if agent.mode == AgentMode::SubAgent {
            output.push_str(&Self::render_role_header(&agent.id));
        }

        // Resolve and inject agent-specific protocol sections
        for section_name in &agent.prompt_sections {
            if let Some(block) = Self::resolve_section(section_name) {
                output.push_str(block);
                output.push_str("\n\n");
            }
        }
    }
}

// =============================================================================
// Static protocol content blocks (migrated from prompt_sections/*.rs)
// =============================================================================

const EXPLORE_CONSTRAINTS: &str = r#"# Explore Agent Constraints

## Role
You are a read-only exploration specialist. Your sole purpose is gathering information — you NEVER modify, create, or delete anything.

## Behavioral Rules
- Prefer parallel tool calls for speed (glob + grep simultaneously).
- Start broad (directory structure), then narrow (specific files).
- When searching code, try multiple patterns before reporting "not found".
- Read only the parts of files you need — use offset/limit for large files.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 20 iterations — be efficient.

## Output Format
End with a structured summary:
- **Findings**: what you found, with file paths and line numbers.
- **Relevance**: how each finding relates to the request.
- **Next steps**: recommended actions for the caller."#;

const CODER_GUIDELINES: &str = r#"# Coder Agent Guidelines

## Role
You are a code writing specialist. You read, write, and edit code with precision.

## Behavioral Rules
- Read existing code before modifying — understand context and conventions first.
- Make minimal, focused changes. Do not refactor unrelated code.
- One concern per edit. If a file needs multiple changes, make them in separate edits.
- Verify changes compile: run `cargo check` after significant edits.
- Follow the project's existing patterns, naming, and style.

## Hard Constraints
- Maximum 30 iterations — plan your work efficiently.
- Do not introduce new dependencies without explicit approval.

## Output Format
End with a summary:
- **Changes made**: list each file modified with a one-line description.
- **Compilation**: whether `cargo check` passes.
- **Notes**: anything the caller should review or test."#;

const RESEARCHER_PROTOCOL: &str = r#"# Researcher Agent Protocol

## Role
You are an information gathering specialist. You search, fetch, and synthesize information from multiple sources.

## Behavioral Rules
- Cross-reference multiple sources before making claims.
- Distinguish facts from inference — label speculation clearly.
- Cite sources: include URLs, file paths, or document names.
- Prefer primary sources (official docs, source code) over secondary.
- When web results are ambiguous, try different search queries.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 15 iterations — prioritize high-value sources.

## Output Format
End with a structured research report:
- **Summary**: 2-3 sentence answer to the research question.
- **Findings**: detailed evidence organized by topic.
- **Sources**: list of all sources consulted.
- **Confidence**: high / medium / low, with reasoning."#;

const VERIFY_PROTOCOL: &str = r#"# Verification Agent Protocol

## Mindset
You are an adversarial verifier. Your job is to TRY TO BREAK IT, not to confirm it works. Assume the implementation has bugs until proven otherwise.

## Mandatory Checks
For every verification request, you MUST run all applicable checks:
1. **Build check**: `cargo check` — compilation must pass.
2. **Test suite**: `cargo test` — all tests must pass.
3. **Lint check**: `cargo clippy` — no errors.

Do NOT skip a check. If a check cannot run, the verdict is PARTIAL.

## Change-Type Specific Checks
- **Code changes**: read the diff, verify logic correctness, check edge cases.
- **Refactoring**: verify public API surface is unchanged.
- **New features**: verify test coverage exists for new code paths.
- **Bug fixes**: verify the specific bug scenario is covered by a test.

## Adversarial Probes
After mandatory checks pass, actively look for:
- Edge cases the tests don't cover.
- Error handling gaps (unwrap, expect in non-test code).
- Assumptions that could break under different inputs.
- Off-by-one errors, empty collection handling, None/null paths.

## Output Format
Always end with a verdict block exactly in this format:

```
VERDICT: PASS | FAIL | PARTIAL
REASON: <one-line summary>
CHECKS:
- [x] build: <result>
- [x] tests: <N passed, M failed>
- [x] lint: <result>
ISSUES:
- <issue 1>
- <issue 2>
```

## Hard Rules
- NEVER modify, create, or delete source files. You are a read-only verifier.
- NEVER output PASS without actually running the mandatory checks.
- NEVER skip a mandatory check — if it can't run, verdict is PARTIAL.
- Report what you OBSERVED, not what you expected.
- Maximum 25 iterations."#;

const PLAN_PROTOCOL: &str = r#"# Plan Agent Protocol

## Role
You are a read-only planning specialist. You analyze codebases and produce
step-by-step implementation plans without modifying any files.

## Behavioral Rules
- Read code thoroughly before proposing changes.
- Produce structured, actionable plans with specific file paths and line numbers.
- Identify dependencies, risks, and implementation order.
- Break complex tasks into phases with clear milestones.

## Hard Constraints (enforced by system)
- File write and edit tools are blocked at runtime.
- Maximum 20 iterations — focus on high-value analysis.

## Output Format
End with a structured plan:
- **Goal**: one-sentence summary of what the plan achieves.
- **Steps**: numbered list with file paths, changes, and rationale.
- **Risks**: potential issues and mitigations.
- **Dependencies**: ordering constraints between steps."#;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib thinker::layers::agent_role 2>&1 | tail -10`
Expected: all 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/agent_role.rs
git commit -m "feat(prompt): add AgentRoleLayer for sub-agent protocol injection"
```

---

### Task 3: Add `agent_def` to LayerInput and register AgentRoleLayer

**Files:**
- Modify: `src/thinker/prompt_layer.rs` — add `agent_def` field and `with_agent_def()` method
- Modify: `src/thinker/layers/mod.rs` — declare and re-export `AgentRoleLayer`
- Modify: `src/thinker/prompt_pipeline.rs` — add to `default_layers()` and update protected list
- Test: `cargo test -p alephcore --lib thinker::prompt_pipeline`

- [ ] **Step 1: Add `agent_def` to `LayerInput` in `src/thinker/prompt_layer.rs`**

Add import at top (after line 9):

```rust
use crate::agents::AgentDef;
```

Add field to `LayerInput` struct (after `has_session_summaries` field, line 67):

```rust
    /// Agent definition for sub-agent prompt injection.
    pub agent_def: Option<&'a AgentDef>,
```

Add `agent_def: None,` to all four constructors (`basic`, `hydration`, `soul`, `context`) — insert after the `has_session_summaries: false,` line in each.

Add builder method (after `with_session_summaries`, ~line 194):

```rust
    /// Attach agent definition for sub-agent prompt injection.
    pub fn with_agent_def(mut self, agent_def: &'a AgentDef) -> Self {
        self.agent_def = Some(agent_def);
        self
    }
```

- [ ] **Step 2: Register AgentRoleLayer in `src/thinker/layers/mod.rs`**

Add module declaration (after `mod soul;`, line 22):

```rust
// --- Agent role layer ---
mod agent_role;
```

Add re-export (after `pub use soul::SoulLayer;`, line 69):

```rust
pub use agent_role::AgentRoleLayer;
```

- [ ] **Step 3: Add to `default_layers()` in `src/thinker/prompt_pipeline.rs`**

In `default_layers()` (line 220-249), add after `Box::new(SoulLayer),`:

```rust
            Box::new(AgentRoleLayer),
```

Update the docstring layer count from "25 default layers" to "26 default layers" (line 186).

Update `protected` list in `assemble()` (line 102) to include priority 55:

```rust
        let protected = &[50u32, 55, 75, 100, 500, 501, 1200];
```

- [ ] **Step 4: Fix test assertions**

In `test_default_layers_count` (line 374): change `27` to `28`.

In `dynamic_layers_are_correctly_classified` (line 681): keep as `6` (AgentRoleLayer is Stable by default).

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline 2>&1 | tail -15`
Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/thinker/prompt_layer.rs src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "feat(prompt): register AgentRoleLayer in pipeline, add agent_def to LayerInput"
```

---

### Task 4: Add `build_for_agent()` to thinker::PromptBuilder

**Files:**
- Modify: `src/thinker/prompt_builder/mod.rs`
- Test: `src/thinker/prompt_builder/tests.rs` (if exists) or inline

- [ ] **Step 1: Write the test**

Add to `src/thinker/prompt_builder/tests.rs` (or create inline `#[cfg(test)]` at bottom of `mod.rs`):

```rust
#[test]
fn build_for_agent_includes_role_and_protocol() {
    use crate::agents::{AgentDef, AgentMode};
    use crate::thinker::soul::SoulManifest;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let soul = SoulManifest::default();
    let agent = AgentDef::new("explore", AgentMode::SubAgent)
        .with_prompt_sections(vec!["explore_constraints".into()]);

    let prompt = builder.build_for_agent(&agent, &[], &soul);
    assert!(prompt.contains("**explore**"), "should contain agent id");
    assert!(prompt.contains("read-only exploration specialist"), "should have explore constraints");
}

#[test]
fn build_for_agent_primary_has_no_role() {
    use crate::agents::{AgentDef, AgentMode};
    use crate::thinker::soul::SoulManifest;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let soul = SoulManifest::default();
    let agent = AgentDef::new("main", AgentMode::Primary);

    let prompt = builder.build_for_agent(&agent, &[], &soul);
    assert!(!prompt.contains("Sub-Agent Role"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::prompt_builder::tests::build_for_agent 2>&1 | tail -5`
Expected: FAIL — method doesn't exist.

- [ ] **Step 3: Implement `build_for_agent()`**

Add to `src/thinker/prompt_builder/mod.rs`, inside `impl PromptBuilder` (after `build_system_prompt_with_context`, ~line 252):

```rust
    /// Build system prompt for a sub-agent.
    ///
    /// Injects the agent's role header and protocol sections via `AgentRoleLayer`.
    /// This replaces the old `agent_loop::PromptBuilder::for_agent()`.
    pub fn build_for_agent(
        &self,
        agent_def: &crate::agents::AgentDef,
        tools: &[ToolInfo],
        soul: &SoulManifest,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_agent_def(agent_def);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    /// Build system prompt for a sub-agent with full context.
    pub fn build_for_agent_with_context(
        &self,
        agent_def: &crate::agents::AgentDef,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        workspace: Option<&IdentityFiles>,
        inbound: Option<&InboundContext>,
        memory_context: Option<&super::memory_context::MemoryContext>,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_agent_def(agent_def)
            .with_profile(profile)
            .with_workspace_opt(workspace)
            .with_inbound_opt(inbound)
            .with_memory_context_opt(memory_context);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_builder 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/prompt_builder/mod.rs
git commit -m "feat(prompt): add build_for_agent() to thinker::PromptBuilder"
```

---

### Task 5: Migrate `subagent_runner.rs` to thinker::PromptBuilder

**Files:**
- Modify: `src/agent_loop/subagent_runner.rs`
- Test: `cargo test -p alephcore --lib agent_loop::subagent`

- [ ] **Step 1: Update imports in `subagent_runner.rs`**

Replace line 9:

```rust
use super::prompt_builder::{PromptBuilder, PromptSection, Stability};
```

With:

```rust
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};
use crate::thinker::soul::SoulManifest;
```

- [ ] **Step 2: Replace PromptBuilder usage**

Replace the prompt building section (around lines 59-77):

Old:
```rust
    let mut prompt_builder = PromptBuilder::for_agent(&agent_def);
    if let Some(summary) = context_summary {
        prompt_builder.register(PromptSection {
            name: "parent_context".to_string(),
            stability: Stability::Dynamic,
            priority: 500,
            protected: false,
            content: format!("## Context from parent agent\n\n{}", summary),
        });
    }
```

New:
```rust
    let config = PromptConfig::default();
    let prompt_builder = PromptBuilder::new(config);
    let soul = SoulManifest::default();
    let mut prompt = prompt_builder.build_for_agent(&agent_def, &[], &soul);

    // Inject parent context if provided
    if let Some(summary) = context_summary {
        prompt.push_str(&format!("\n\n## Context from parent agent\n\n{}", summary));
    }
```

Update the `AgentLoop::new()` call to pass the prompt string directly instead of the old builder. This requires the AgentLoop constructor to accept a system prompt string — see Task 6.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: may have errors if AgentLoop constructor hasn't changed yet. That's OK — Task 6 handles this.

- [ ] **Step 4: Commit (WIP)**

```bash
git add src/agent_loop/subagent_runner.rs
git commit -m "refactor(prompt): migrate subagent_runner to thinker::PromptBuilder (WIP)"
```

---

### Task 6: Migrate `loop_core.rs` to accept system prompt string

This is the largest migration task. `AgentLoop` currently stores an old `PromptBuilder` and calls `.register()` + `.build()` at runtime. We need to change the constructor to accept a pre-built system prompt string (or the new thinker::PromptBuilder).

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Test: `cargo test -p alephcore --lib agent_loop::loop_core`

- [ ] **Step 1: Change AgentLoop constructor**

In `loop_core.rs`, change the `prompt_builder` field (line 657) from:

```rust
    prompt_builder: PromptBuilder,
```

To:

```rust
    prompt_builder: crate::thinker::prompt_builder::PromptBuilder,
```

Update the import (line 25) from:

```rust
use super::prompt_builder::{PromptBuilder, ToolInfo};
```

To:

```rust
use super::tool_info::ToolInfo;
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
```

Update `AgentLoop::new()` signature (line 701-707) to accept the new type.

- [ ] **Step 2: Replace dynamic `.register()` calls**

The old code does dynamic `self.prompt_builder.register(section)` at three points:

**Point A** (line ~1716): discovered skills injection
```rust
// OLD:
self.prompt_builder.register(
    crate::agent_loop::prompt_sections::discovered_skills::render(&new_skills),
);
*system_prompt = self.prompt_builder.build().prompt;
```

Replace with: update the PromptConfig's `skill_instructions` field and rebuild:
```rust
// NEW:
// Skills are already handled by SkillInstructionsLayer in the pipeline.
// Update config and rebuild system prompt.
let skills_text = new_skills.iter()
    .map(|s| format!("- {}: {}", s.name, s.description))
    .collect::<Vec<_>>()
    .join("\n");
// Inject via custom instructions append or dedicated mechanism
tracing::info!(chain_id = %self.chain.chain_id, "Skill discovery complete, {} new skills", new_skills.len());
```

**Point B** (line ~2006-2012): tools and session guidance registration
```rust
// OLD:
self.prompt_builder.register(prompt_sections::tools::render(&tool_infos));
self.prompt_builder.register(prompt_sections::session_guidance::render(&tool_names));
```

Replace with: build system prompt via the new builder which handles tools via ToolsLayer:
```rust
// NEW: tools are passed to build_system_prompt_with_soul() directly
```

**Point C** (line ~2025-2028): environment registration
```rust
// OLD:
self.prompt_builder.register(prompt_sections::environment::render(&env));
```

Replace with: environment handled by EnvironmentLayer via ResolvedContext.

The key refactor: instead of registering sections one-by-one and calling `.build()`, collect all context data upfront and call `self.prompt_builder.build_system_prompt_with_full_context(...)` once.

- [ ] **Step 3: Update `prepare_runtime()` or equivalent init method**

Replace the register-then-build pattern with a single build call:

```rust
let prompt = self.prompt_builder.build_system_prompt_with_full_context(
    &tool_infos,
    &soul,
    profile.as_ref(),
    workspace.as_ref(),
    inbound.as_ref(),
    memory_context.as_ref(),
);
```

- [ ] **Step 4: Update test constructors**

All test `AgentLoop::new()` calls (lines ~2320, 2435, 2478, 2571, etc.) pass `PromptBuilder::new()`. Update them to:

```rust
AgentLoop::new(
    provider,
    registry,
    PromptBuilder::new(PromptConfig::default()),
    // ... rest unchanged
)
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib agent_loop 2>&1 | tail -20`
Expected: PASS (may need iteration on exact API).

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "refactor(prompt): migrate loop_core to thinker::PromptBuilder"
```

---

### Task 7: Delete old prompt system

Now that all call sites use the new system, delete the old files.

**Files:**
- Delete: `src/agent_loop/prompt_builder.rs` (776 lines)
- Delete: `src/agent_loop/prompt_sections/` (26 files)
- Modify: `src/agent_loop/mod.rs` — remove old mod/use declarations

- [ ] **Step 1: Remove mod and use declarations from `src/agent_loop/mod.rs`**

Remove line 16: `mod prompt_builder;`
Remove line 17: `pub mod prompt_sections;`

Update the re-export block (lines 50-52). Remove everything except `ToolInfo`:

```rust
pub use tool_info::ToolInfo;
```

Remove: `PromptBudget, PromptBuilder, PromptResult, PromptSection, Stability` from re-exports.

- [ ] **Step 2: Delete old files**

```bash
rm src/agent_loop/prompt_builder.rs
rm -r src/agent_loop/prompt_sections/
```

- [ ] **Step 3: Fix any remaining references**

Run: `cargo check -p alephcore 2>&1 | grep "error" | head -20`

Fix any remaining `use crate::agent_loop::prompt_builder::` or `use crate::agent_loop::prompt_sections::` references. They should be replaced with:
- `PromptBuilder` → `crate::thinker::prompt_builder::PromptBuilder`
- `PromptSection` / `Stability` → delete (no longer needed)
- `ToolInfo` → `crate::agent_loop::ToolInfo` (unchanged)

- [ ] **Step 4: Fix integration_probe.rs**

`src/agent_loop/integration_probe.rs` imports old PromptBuilder. Update:

```rust
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
```

And update any `PromptBuilder::new()` calls to `PromptBuilder::new(PromptConfig::default())`.

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(prompt): delete old PromptBuilder and 26 prompt_sections (~1000 lines removed)"
```

---

## Phase 2: Claude Code-Inspired Enhancements

### Task 8: Add `usage_hint` to ToolInfo

**Files:**
- Modify: `src/agent_loop/tool_info.rs`
- Test: inline

- [ ] **Step 1: Write test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_info_with_usage_hint() {
        let tool = ToolInfo {
            name: "file_read".into(),
            description: "Read a file".into(),
            parameters_schema: None,
            usage_hint: Some(ToolUsageHint {
                prefer_for: vec!["reading file contents".into()],
                prefer_over: vec!["cat".into(), "head".into(), "tail".into()],
            }),
        };
        assert_eq!(tool.usage_hint.as_ref().unwrap().prefer_over.len(), 3);
    }

    #[test]
    fn tool_info_without_hint_serializes() {
        let tool = ToolInfo {
            name: "test".into(),
            description: "test".into(),
            parameters_schema: None,
            usage_hint: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(!json.contains("usage_hint"));
    }
}
```

- [ ] **Step 2: Add `ToolUsageHint` and extend `ToolInfo`**

In `src/agent_loop/tool_info.rs`:

```rust
//! Lightweight tool info for prompt building.

/// Hint for tool usage grammar generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolUsageHint {
    /// Scenarios where this tool should be preferred
    pub prefer_for: Vec<String>,
    /// Alternative tools/commands this tool supersedes
    pub prefer_over: Vec<String>,
}

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
    /// Optional usage hint for grammar layer generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_hint: Option<ToolUsageHint>,
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::tool_info 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/tool_info.rs
git commit -m "feat(prompt): add ToolUsageHint to ToolInfo for grammar layer"
```

---

### Task 9: Create ToolUsageGrammarLayer

**Files:**
- Create: `src/thinker/layers/tool_usage_grammar.rs`
- Modify: `src/thinker/layers/mod.rs`
- Modify: `src/thinker/prompt_pipeline.rs`

- [ ] **Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::tool_info::{ToolInfo, ToolUsageHint};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};

    #[test]
    fn skips_when_no_tools_have_hints() {
        let layer = ToolUsageGrammarLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "test".into(),
            description: "test".into(),
            parameters_schema: None,
            usage_hint: None,
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn generates_grammar_from_hints() {
        let layer = ToolUsageGrammarLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "file_read".into(),
            description: "Read a file".into(),
            parameters_schema: None,
            usage_hint: Some(ToolUsageHint {
                prefer_for: vec!["reading file contents".into()],
                prefer_over: vec!["cat".into(), "head".into()],
            }),
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool Usage Guidelines"));
        assert!(out.contains("file_read"));
        assert!(out.contains("cat"));
    }

    #[test]
    fn metadata() {
        let layer = ToolUsageGrammarLayer;
        assert_eq!(layer.name(), "tool_usage_grammar");
        assert_eq!(layer.priority(), 550);
    }
}
```

- [ ] **Step 2: Implement the layer**

Create `src/thinker/layers/tool_usage_grammar.rs`:

```rust
//! ToolUsageGrammarLayer — encode tool usage conventions (priority 550)
//!
//! Reads `ToolInfo.usage_hint` from registered tools and generates
//! behavioral guidelines ("prefer tool X over Y for task Z").

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ToolUsageGrammarLayer;

impl PromptLayer for ToolUsageGrammarLayer {
    fn name(&self) -> &'static str {
        "tool_usage_grammar"
    }

    fn priority(&self) -> u32 {
        550
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let tools = match input.tools {
            Some(t) => t,
            None => return,
        };

        let hints: Vec<_> = tools
            .iter()
            .filter_map(|t| t.usage_hint.as_ref().map(|h| (&t.name, h)))
            .collect();

        if hints.is_empty() {
            return;
        }

        output.push_str("## Tool Usage Guidelines\n\n");

        for (name, hint) in &hints {
            if !hint.prefer_over.is_empty() {
                let alternatives = hint.prefer_over.join(", ");
                let scenario = hint.prefer_for.first().map(|s| s.as_str()).unwrap_or("this task");
                output.push_str(&format!(
                    "- For {}, use `{}` instead of {}\n",
                    scenario, name, alternatives
                ));
            }
        }

        output.push_str("- Prefer parallel tool calls when tasks are independent\n");
        output.push('\n');
    }
}
```

- [ ] **Step 3: Register in `layers/mod.rs` and `prompt_pipeline.rs`**

In `src/thinker/layers/mod.rs`, add:
```rust
mod tool_usage_grammar;
pub use tool_usage_grammar::ToolUsageGrammarLayer;
```

In `src/thinker/prompt_pipeline.rs` `default_layers()`, add after `Box::new(HydratedToolsLayer),`:
```rust
            Box::new(ToolUsageGrammarLayer),
```

Update layer count in docstring and test assertion (`28` → `29`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::tool_usage_grammar 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/tool_usage_grammar.rs src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "feat(prompt): add ToolUsageGrammarLayer for tool preference encoding"
```

---

### Task 10: Add section-level caching to PromptPipeline

**Files:**
- Modify: `src/thinker/prompt_pipeline.rs`
- Test: inline

- [ ] **Step 1: Write the test**

Add to `prompt_pipeline.rs` tests module:

```rust
#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn cached_execute_returns_same_result() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let r1 = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let r2 = pipeline.execute_cached(AssemblyPath::Basic, &input);
        assert_eq!(r1, r2);
    }

    #[test]
    fn invalidate_clears_cache() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let stats_before = pipeline.cache_stats();
        assert!(stats_before.hits > 0 || stats_before.misses > 0);

        pipeline.invalidate_all();
        let stats_after = pipeline.cache_stats();
        assert_eq!(stats_after.entries, 0);
    }

    #[test]
    fn cache_stats_tracks_hits_and_misses() {
        let pipeline = PromptPipeline::default_layers();
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);

        // First call — all misses
        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let s1 = pipeline.cache_stats();
        assert!(s1.misses > 0);
        assert_eq!(s1.hits, 0);

        // Second call — stable layers hit cache
        let _ = pipeline.execute_cached(AssemblyPath::Basic, &input);
        let s2 = pipeline.cache_stats();
        assert!(s2.hits > 0);
    }
}
```

- [ ] **Step 2: Implement caching**

Add to `src/thinker/prompt_pipeline.rs`:

```rust
use std::sync::RwLock;
use std::collections::HashMap;

/// Cache statistics for monitoring.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
    cache: RwLock<HashMap<&'static str, String>>,
    stats: RwLock<CacheStats>,
}
```

Update `PromptPipeline::new()`:

```rust
    pub fn new(mut layers: Vec<Box<dyn PromptLayer>>) -> Self {
        layers.sort_by_key(|l| l.priority());
        Self {
            layers,
            cache: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
        }
    }
```

Add cached execution method:

```rust
    /// Execute with section-level caching for Stable layers.
    ///
    /// Stable layers' output is cached and reused across calls.
    /// Dynamic layers are always re-executed.
    pub fn execute_cached(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(16384);
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());

        for layer in &self.layers {
            if !layer.paths().contains(&path) {
                continue;
            }

            if layer.stability() == LayerStability::Stable {
                // Try cache hit
                if let Some(cached) = cache.get(layer.name()) {
                    output.push_str(cached);
                    if let Ok(mut s) = self.stats.write() {
                        s.hits += 1;
                    }
                    continue;
                }
            }

            // Cache miss or dynamic — execute
            let mut section = String::new();
            layer.inject(&mut section, input);

            if layer.stability() == LayerStability::Stable && !section.is_empty() {
                // Release read lock, acquire write lock
                drop(cache);
                if let Ok(mut w) = self.cache.write() {
                    w.insert(layer.name(), section.clone());
                }
                // Re-acquire read for next iteration — but we can just
                // push to output and get cache again next loop
            }

            if let Ok(mut s) = self.stats.write() {
                s.misses += 1;
            }

            output.push_str(&section);

            // Re-acquire read lock for next iteration
            // Actually, restructure to avoid lock juggling:
        }

        output
    }
```

**Note:** The exact lock strategy may need refinement. A simpler approach: compute all sections first, then cache stable ones in a single write pass:

```rust
    pub fn execute_cached(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let mut output = String::with_capacity(16384);
        let mut to_cache: Vec<(&'static str, String)> = Vec::new();

        for layer in &self.layers {
            if !layer.paths().contains(&path) {
                continue;
            }

            if layer.stability() == LayerStability::Stable {
                if let Some(cached) = cache.get(layer.name()) {
                    output.push_str(cached);
                    if let Ok(mut s) = self.stats.write() { s.hits += 1; }
                    continue;
                }
            }

            let mut section = String::new();
            layer.inject(&mut section, input);
            if let Ok(mut s) = self.stats.write() { s.misses += 1; }

            if layer.stability() == LayerStability::Stable && !section.is_empty() {
                to_cache.push((layer.name(), section.clone()));
            }

            output.push_str(&section);
        }

        drop(cache);
        if !to_cache.is_empty() {
            if let Ok(mut w) = self.cache.write() {
                for (name, content) in to_cache {
                    w.insert(name, content);
                }
            }
        }

        output
    }

    /// Invalidate a specific layer's cache.
    pub fn invalidate(&self, layer_name: &str) {
        if let Ok(mut w) = self.cache.write() {
            w.retain(|k, _| *k != layer_name);
        }
    }

    /// Invalidate all cached sections.
    pub fn invalidate_all(&self) {
        if let Ok(mut w) = self.cache.write() {
            w.clear();
        }
        if let Ok(mut s) = self.stats.write() {
            *s = CacheStats::default();
        }
    }

    /// Cache hit/miss statistics.
    pub fn cache_stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let stats = self.stats.read().unwrap_or_else(|e| e.into_inner());
        CacheStats {
            hits: stats.hits,
            misses: stats.misses,
            entries: cache.len(),
        }
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::cache_tests 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/thinker/prompt_pipeline.rs
git commit -m "feat(prompt): add section-level caching to PromptPipeline"
```

---

### Task 11: Enhance MemoryAugmentationLayer with hybrid injection

**Files:**
- Modify: `src/thinker/memory_context.rs`
- Modify: `src/thinker/layers/memory_augmentation.rs`
- Modify: `src/thinker/prompt_layer.rs` — add structured memory field
- Test: inline

- [ ] **Step 1: Add structured memory index to MemoryContext**

In `src/thinker/memory_context.rs`, add:

```rust
/// Structured memory index content (e.g., from .aleph/MEMORY.md)
#[derive(Debug, Clone, Default)]
pub struct StructuredMemoryIndex {
    /// Raw content from MEMORY.md (truncated to 200 lines / 25KB)
    pub content: String,
    /// Whether the content was truncated
    pub truncated: bool,
}

// Update MemoryContext:
pub struct MemoryContext {
    pub facts: Vec<ScoredFact>,
    pub memory_summaries: Vec<MemorySummary>,
    /// Optional structured memory index (from workspace .aleph/MEMORY.md)
    pub structured_index: Option<StructuredMemoryIndex>,
}
```

- [ ] **Step 2: Update `format_for_prompt()` for hybrid output**

Replace the existing `format_for_prompt()`:

```rust
    pub fn format_for_prompt(&self) -> String {
        let has_structured = self.structured_index.as_ref().map_or(false, |s| !s.content.is_empty());
        let has_vector = !self.facts.is_empty() || !self.memory_summaries.is_empty();

        if !has_structured && !has_vector {
            return String::new();
        }

        let mut output = String::from("## Memory Context\n\n");

        // Path 1: Structured index
        if let Some(ref index) = self.structured_index {
            if !index.content.is_empty() {
                output.push_str("### Index (structured)\n\n");
                output.push_str(&index.content);
                if index.truncated {
                    output.push_str("\n[... truncated ...]\n");
                }
                output.push_str("\n\n");
            }
        }

        // Path 2: Vector retrieval results
        if has_vector {
            output.push_str("### Relevant Memories (semantic)\n\n");

            for sf in &self.facts {
                output.push_str(&format!("- [{}] {}\n", format_score(sf.score), sf.fact.content));
            }

            for ms in &self.memory_summaries {
                output.push_str(&format!(
                    "- [{}] [{}] Q: {} → A: {}\n",
                    format_score(ms.score), ms.date, ms.user_input, ms.ai_output
                ));
            }
            output.push('\n');
        }

        // Taxonomy guidelines
        output.push_str("### Memory Guidelines\n\n");
        output.push_str("Memory categories: user (preferences), project (goals/status), feedback (corrections), reference (external pointers).\n");
        output.push_str("Save important context. Update stale memories. Delete outdated ones.\n\n");

        output
    }
```

Add helper:

```rust
fn format_score(score: f32) -> String {
    format!("{:.2}", score)
}
```

- [ ] **Step 3: Update Default for MemoryContext**

```rust
impl Default for MemoryContext {
    fn default() -> Self {
        Self {
            facts: Vec::new(),
            memory_summaries: Vec::new(),
            structured_index: None,
        }
    }
}
```

- [ ] **Step 4: Update tests**

```rust
#[test]
fn test_hybrid_format() {
    let fact = ScoredFact {
        fact: MemoryFact::new("User prefers dark mode".to_string(), FactType::Preference, vec![]),
        score: 0.92,
    };
    let ctx = MemoryContext {
        facts: vec![fact],
        memory_summaries: vec![],
        structured_index: Some(StructuredMemoryIndex {
            content: "- [Role](user/role.md) — data scientist".into(),
            truncated: false,
        }),
    };
    let prompt = ctx.format_for_prompt();
    assert!(prompt.contains("## Memory Context"));
    assert!(prompt.contains("### Index (structured)"));
    assert!(prompt.contains("data scientist"));
    assert!(prompt.contains("### Relevant Memories (semantic)"));
    assert!(prompt.contains("[0.92]"));
    assert!(prompt.contains("### Memory Guidelines"));
}

#[test]
fn test_structured_only() {
    let ctx = MemoryContext {
        facts: vec![],
        memory_summaries: vec![],
        structured_index: Some(StructuredMemoryIndex {
            content: "- [Project](project/arch.md) — Rust core".into(),
            truncated: true,
        }),
    };
    let prompt = ctx.format_for_prompt();
    assert!(prompt.contains("### Index (structured)"));
    assert!(prompt.contains("[... truncated ...]"));
    assert!(!prompt.contains("### Relevant Memories (semantic)"));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib thinker::memory_context 2>&1 | tail -10`
Expected: PASS.

Run: `cargo test -p alephcore --lib thinker::layers::memory_augmentation 2>&1 | tail -10`
Expected: PASS (MemoryAugmentationLayer delegates to `format_for_prompt()`).

- [ ] **Step 6: Commit**

```bash
git add src/thinker/memory_context.rs src/thinker/layers/memory_augmentation.rs
git commit -m "feat(prompt): hybrid memory injection — structured index + vector retrieval"
```

---

## Phase 3: Final Cleanup & Verification

### Task 12: Full integration verification

**Files:**
- No new files
- Test: full test suite

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -30`
Expected: all tests PASS.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 3: Verify no dead imports**

Run: `cargo check -p alephcore 2>&1 | grep "unused" | head -20`
Expected: no unused imports related to old prompt system.

- [ ] **Step 4: Run build**

Run: `cargo build -p alephcore 2>&1 | tail -5`
Expected: BUILD SUCCEEDED.

- [ ] **Step 5: Commit any cleanup fixes**

```bash
git add -A
git commit -m "chore(prompt): final cleanup — remove dead imports and fix warnings"
```

---

### Task 13: Update documentation

**Files:**
- Modify: `docs/reference/ARCHITECTURE.md` — prompt system section

- [ ] **Step 1: Update ARCHITECTURE.md**

Find the prompt system section and update to reflect:
- Sole entry point: `thinker::PromptBuilder` wrapping `PromptPipeline`
- 29 layers, sorted by priority
- Stable/Dynamic cache boundary
- Section-level caching
- AgentRoleLayer for sub-agent support
- ToolUsageGrammarLayer for tool preference encoding
- Hybrid memory injection (structured + vector)

- [ ] **Step 2: Commit**

```bash
git add docs/reference/ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md for unified prompt system"
```

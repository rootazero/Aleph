# Skill Scope Filtering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate v2 scope-aware skill filtering so only relevant skills are injected into LLM prompts, reducing token waste.

**Architecture:** Add `bound_tool` to `SkillManifest`, store eligible manifests in `SkillSnapshot`, add `eligible_skills` to both prompt builders, and implement scope filtering in `SkillInstructionsLayer` (thinker path) and `agent_loop::PromptBuilder` (production path).

**Tech Stack:** Rust, serde_yaml, tracing

**Spec:** `docs/superpowers/specs/2026-03-23-skill-scope-filtering-design.md`

---

### Task 1: Add `bound_tool` to SkillManifest

**Files:**
- Modify: `src/domain/skill.rs:363-384` (SkillManifest struct + impl)

- [ ] **Step 1: Write failing test**

Add to the existing `tests` module in `src/domain/skill.rs`:

```rust
#[test]
fn test_skill_manifest_bound_tool() {
    let mut manifest = SkillManifest::new(
        "docker:build",
        "Docker Build",
        "Builds Docker images",
        SkillContent::new("content"),
        SkillSource::Bundled,
    );

    // Default: no bound tool
    assert!(manifest.bound_tool().is_none());

    // Set bound tool
    manifest.set_bound_tool("docker_cli".to_string());
    assert_eq!(manifest.bound_tool(), Some("docker_cli"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_skill_manifest_bound_tool`
Expected: FAIL — `bound_tool()` and `set_bound_tool()` don't exist yet

- [ ] **Step 3: Implement**

In `src/domain/skill.rs`, add `bound_tool: Option<String>` field to `SkillManifest` struct (after `scope`):

```rust
/// Tool name this skill is bound to (for Tool scope filtering).
bound_tool: Option<String>,
```

Add to `SkillManifest::new()` initializer: `bound_tool: None,`

Add accessor and setter in the `impl SkillManifest` block:

```rust
/// Bound tool name (for Tool scope filtering).
pub fn bound_tool(&self) -> Option<&str> {
    self.bound_tool.as_deref()
}

/// Set the bound tool name.
pub fn set_bound_tool(&mut self, tool: String) {
    self.bound_tool = Some(tool);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_skill_manifest_bound_tool`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/domain/skill.rs
git commit -m "skill: add bound_tool field to SkillManifest"
```

---

### Task 2: Parse `bound-tool` from SKILL.md frontmatter

**Files:**
- Modify: `src/skill/manifest.rs:62-77` (RawFrontmatter) and `src/skill/manifest.rs:126-216` (parse_skill_content)

- [ ] **Step 1: Write failing test**

Add to the existing `tests` module in `src/skill/manifest.rs`:

```rust
#[test]
fn parse_bound_tool_from_frontmatter() {
    let content = r#"---
name: Docker Build
description: Builds Docker images
scope: tool
bound-tool: docker_cli
---
Docker expert."#;

    let manifest = parse_skill_content(content, SkillSource::Global).unwrap();
    assert_eq!(*manifest.scope(), PromptScope::Tool);
    assert_eq!(manifest.bound_tool(), Some("docker_cli"));
}

#[test]
fn parse_no_bound_tool_defaults_to_none() {
    let content = r#"---
name: Simple Skill
description: No bound tool
---
Content."#;

    let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
    assert!(manifest.bound_tool().is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib parse_bound_tool`
Expected: FAIL — `bound_tool` field not in `RawFrontmatter`, not set during parsing

- [ ] **Step 3: Implement**

In `src/skill/manifest.rs`, add to `RawFrontmatter` struct:

```rust
#[serde(default)]
bound_tool: Option<String>,
```

In `parse_skill_content()`, after the scope-setting block (around line 151), add:

```rust
// Bound tool (for Tool scope)
if let Some(bound_tool) = raw.bound_tool {
    manifest.set_bound_tool(bound_tool);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib parse_bound_tool`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/skill/manifest.rs
git commit -m "skill: parse bound-tool from SKILL.md frontmatter"
```

---

### Task 3: Add `eligible_manifests` to SkillSnapshot

**Files:**
- Modify: `src/skill/snapshot.rs:17-82` (SkillSnapshot struct + build())

- [ ] **Step 1: Write failing test**

Add to the existing `tests` module in `src/skill/snapshot.rs`:

```rust
#[test]
fn eligible_manifests_populated() {
    let mut registry = SkillRegistry::new();
    let eligibility = EligibilityService::new();

    // Eligible + model-visible
    let m1 = make_manifest("visible:skill", SkillSource::Bundled);
    registry.register(m1);

    // Eligible but model-invisible (disabled scope)
    let mut m2 = make_manifest("disabled:skill", SkillSource::Bundled);
    m2.set_scope(PromptScope::Disabled);
    registry.register(m2);

    // Eligible but model-invisible (disable_model_invocation)
    let mut m3 = make_manifest("hidden:skill", SkillSource::Bundled);
    m3.set_invocation(InvocationPolicy {
        disable_model_invocation: true,
        ..Default::default()
    });
    registry.register(m3);

    let snap = SkillSnapshot::build(&registry, &eligibility, 1);

    // All 3 eligible, but only 1 model-visible in eligible_manifests
    assert_eq!(snap.eligible.len(), 3);
    assert_eq!(snap.eligible_manifests.len(), 1);
    assert_eq!(snap.eligible_manifests[0].name(), "visible:skill");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib eligible_manifests_populated`
Expected: FAIL — `eligible_manifests` field doesn't exist

- [ ] **Step 3: Implement**

In `src/skill/snapshot.rs`, add field to `SkillSnapshot`:

```rust
/// Eligible + model-visible skill manifests for prompt injection.
pub eligible_manifests: Vec<SkillManifest>,
```

Update `SkillSnapshot::empty()` to include: `eligible_manifests: Vec::new(),`

Update `SkillSnapshot::build()` — change the `model_visible` collection to also build `eligible_manifests`:

```rust
let mut model_visible: Vec<&SkillManifest> = Vec::new();
let mut eligible_manifests: Vec<SkillManifest> = Vec::new();

// ... inside the Eligible arm:
if manifest.is_model_visible() {
    model_visible.push(manifest);
    eligible_manifests.push(manifest.clone());
}
```

Add `eligible_manifests` to the `Self { ... }` return.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib eligible_manifests_populated`
Expected: PASS

- [ ] **Step 5: Run all snapshot tests to check no regression**

Run: `cargo test -p alephcore --lib snapshot`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add src/skill/snapshot.rs
git commit -m "skill: add eligible_manifests to SkillSnapshot"
```

---

### Task 4: Add `eligible_skills` to PromptConfig

**Files:**
- Modify: `src/thinker/prompt_builder/mod.rs:39-98` (PromptConfig struct + Default)

- [ ] **Step 1: Add field**

In `src/thinker/prompt_builder/mod.rs`, add to `PromptConfig` struct:

```rust
/// Eligible skills from SkillSystem v2 snapshot for scope-aware filtering.
/// When set, SkillInstructionsLayer filters by scope + active tools before injection.
pub eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>>,
```

Add to `Default::default()`: `eligible_skills: None,`

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: OK (no breaking changes — new field is `None` by default, existing code uses `..Default::default()`)

- [ ] **Step 3: Commit**

```bash
git add src/thinker/prompt_builder/mod.rs
git commit -m "thinker: add eligible_skills field to PromptConfig"
```

---

### Task 5: Rewrite SkillInstructionsLayer with scope filtering

**Files:**
- Modify: `src/thinker/layers/skill_instructions.rs` (full rewrite)

- [ ] **Step 1: Write failing tests**

Replace the existing test module in `src/thinker/layers/skill_instructions.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        InvocationPolicy, PromptScope, SkillContent, SkillManifest, SkillSource,
    };
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::agent_loop::ToolInfo;

    fn make_skill(name: &str, scope: PromptScope) -> SkillManifest {
        let mut m = SkillManifest::new(
            name.to_lowercase().replace(' ', "-"),
            name,
            &format!("{} description", name),
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        m.set_scope(scope);
        m
    }

    fn make_tool(name: &str) -> ToolInfo {
        ToolInfo {
            name: name.to_string(),
            description: "tool desc".to_string(),
            parameters_schema: None,
        }
    }

    #[test]
    fn explicit_skill_instructions_take_priority() {
        let layer = SkillInstructionsLayer;
        let system_skill = make_skill("System Skill", PromptScope::System);
        let config = PromptConfig {
            skill_instructions: Some("Explicit instructions here".to_string()),
            eligible_skills: Some(vec![system_skill]),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Explicit instructions here"));
        // Should NOT contain the auto skill list
        assert!(!out.contains("System Skill"));
    }

    #[test]
    fn system_scope_always_included() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            eligible_skills: Some(vec![
                make_skill("Git Commit", PromptScope::System),
            ]),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Git Commit"));
        assert!(out.contains("## Available Skills"));
    }

    #[test]
    fn tool_scope_included_when_bound_tool_active() {
        let layer = SkillInstructionsLayer;
        let mut docker_skill = make_skill("Docker Build", PromptScope::Tool);
        docker_skill.set_bound_tool("docker_cli".to_string());

        let config = PromptConfig {
            eligible_skills: Some(vec![docker_skill]),
            ..Default::default()
        };
        let tools = vec![make_tool("docker_cli")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Docker Build"));
    }

    #[test]
    fn tool_scope_excluded_when_bound_tool_not_active() {
        let layer = SkillInstructionsLayer;
        let mut docker_skill = make_skill("Docker Build", PromptScope::Tool);
        docker_skill.set_bound_tool("docker_cli".to_string());

        let config = PromptConfig {
            eligible_skills: Some(vec![docker_skill]),
            ..Default::default()
        };
        let tools = vec![make_tool("web_search")]; // no docker_cli
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn standalone_and_disabled_excluded() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            eligible_skills: Some(vec![
                make_skill("Standalone Skill", PromptScope::Standalone),
                make_skill("Disabled Skill", PromptScope::Disabled),
            ]),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn mixed_scopes_filtered_correctly() {
        let layer = SkillInstructionsLayer;
        let mut tool_skill = make_skill("Docker Build", PromptScope::Tool);
        tool_skill.set_bound_tool("docker_cli".to_string());

        let config = PromptConfig {
            eligible_skills: Some(vec![
                make_skill("Git Commit", PromptScope::System),
                tool_skill,
                make_skill("Hidden", PromptScope::Standalone),
                make_skill("Off", PromptScope::Disabled),
            ]),
            ..Default::default()
        };
        let tools = vec![make_tool("docker_cli")];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Git Commit"));
        assert!(out.contains("Docker Build"));
        assert!(!out.contains("Hidden"));
        assert!(!out.contains("Off"));
    }

    #[test]
    fn empty_eligible_skills_no_output() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            eligible_skills: Some(vec![]),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn none_eligible_skills_no_output() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn paths_include_all_assembly_paths() {
        let paths = SkillInstructionsLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib skill_instructions`
Expected: Multiple failures — new fields don't exist in inject(), paths wrong, etc.

- [ ] **Step 3: Implement SkillInstructionsLayer**

Rewrite `src/thinker/layers/skill_instructions.rs`:

```rust
//! SkillInstructionsLayer — skill system v2 instructions with scope filtering (priority 1050)

use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::build_skills_prompt_xml;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct SkillInstructionsLayer;

impl PromptLayer for SkillInstructionsLayer {
    fn name(&self) -> &'static str { "skill_instructions" }
    fn priority(&self) -> u32 { 1050 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
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
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // 1. Explicit /skill invocation takes priority (backward compat)
        if let Some(ref instructions) = input.config.skill_instructions {
            if !instructions.is_empty() {
                let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
                let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
                output.push_str("## Available Skills\n\n");
                output.push_str("You can invoke skills using the `skill` tool. ");
                output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
                output.push_str(&instructions);
                output.push_str("\n\n");
                return;
            }
        }

        // 2. Auto skill list with scope filtering
        let skills = match input.config.eligible_skills {
            Some(ref skills) if !skills.is_empty() => skills,
            _ => return,
        };

        // Collect active tool names from LayerInput
        let active_tool_names: Vec<&str> = input
            .tools
            .map(|tools| tools.iter().map(|t| t.name.as_str()).collect())
            .unwrap_or_default();

        // Filter by scope
        let filtered: Vec<&SkillManifest> = skills
            .iter()
            .filter(|s| match *s.scope() {
                PromptScope::System => true,
                PromptScope::Tool => s.bound_tool().map_or(false, |bound| {
                    active_tool_names.iter().any(|t| *t == bound)
                }),
                PromptScope::Standalone | PromptScope::Disabled => false,
            })
            .collect();

        tracing::debug!(
            total = skills.len(),
            after_filter = filtered.len(),
            "skill_instructions: scope filtering applied"
        );

        if filtered.is_empty() {
            return;
        }

        let xml = build_skills_prompt_xml(&filtered);
        let xml = sanitize_for_prompt(&xml, SanitizeLevel::Moderate);
        output.push_str("## Available Skills\n\n");
        output.push_str("You can invoke skills using the `skill` tool. ");
        output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
        output.push_str(&xml);
        output.push_str("\n\n");
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib skill_instructions`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/skill_instructions.rs
git commit -m "thinker: rewrite SkillInstructionsLayer with scope filtering"
```

---

### Task 6: Integrate into production agent loop PromptBuilder

The production path uses `agent_loop::prompt_builder::PromptBuilder`, NOT the thinker's `PromptPipeline`. Skills must also be injected here.

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:56-202` (PromptBuilder struct + build())

- [ ] **Step 1: Write failing test**

Add to the existing `tests` module in `src/agent_loop/prompt_builder.rs`:

```rust
#[test]
fn test_build_with_skill_instructions() {
    use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};

    let mut system_skill = SkillManifest::new(
        "git-commit", "Git Commit", "Helps write commit messages",
        SkillContent::new("content"), SkillSource::Bundled,
    );
    system_skill.set_scope(PromptScope::System);

    let mut tool_skill = SkillManifest::new(
        "docker-build", "Docker Build", "Builds Docker images",
        SkillContent::new("content"), SkillSource::Bundled,
    );
    tool_skill.set_scope(PromptScope::Tool);
    tool_skill.set_bound_tool("docker_cli".to_string());

    let mut standalone_skill = SkillManifest::new(
        "hidden", "Hidden", "Hidden skill",
        SkillContent::new("content"), SkillSource::Bundled,
    );
    standalone_skill.set_scope(PromptScope::Standalone);

    let builder = PromptBuilder::new()
        .with_eligible_skills(vec![system_skill, tool_skill, standalone_skill]);

    let tools = vec![
        ToolInfo {
            name: "docker_cli".to_string(),
            description: "Docker CLI".to_string(),
            parameters_schema: None,
        },
    ];

    let prompt = builder.build(&tools, None);

    assert!(prompt.contains("# Available Skills"));
    assert!(prompt.contains("Git Commit"));
    assert!(prompt.contains("Docker Build"));
    assert!(!prompt.contains("Hidden"));
}

#[test]
fn test_build_no_skills_no_section() {
    let prompt = PromptBuilder::new().build(&[], None);
    assert!(!prompt.contains("# Available Skills"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib test_build_with_skill`
Expected: FAIL — `with_eligible_skills` doesn't exist

- [ ] **Step 3: Implement**

In `src/agent_loop/prompt_builder.rs`:

Add import at the top:
```rust
use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::build_skills_prompt_xml;
```

Add field to `PromptBuilder` struct:
```rust
eligible_skills: Option<Vec<SkillManifest>>,
```

Initialize in `new()`: `eligible_skills: None,`

Add builder method:
```rust
/// Set eligible skills for scope-aware filtering.
pub fn with_eligible_skills(mut self, skills: Vec<SkillManifest>) -> Self {
    self.eligible_skills = Some(skills);
    self
}
```

In `build()`, add a new section between "5. Available Tools" and "6. Context from Memory" (will become section 6, shifting the rest):

```rust
// 6. Available Skills (scope-filtered from SkillSystem v2)
if let Some(ref skills) = self.eligible_skills {
    let active_tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let filtered: Vec<&SkillManifest> = skills
        .iter()
        .filter(|s| match *s.scope() {
            PromptScope::System => true,
            PromptScope::Tool => s.bound_tool().map_or(false, |bound| {
                active_tool_names.iter().any(|t| *t == bound)
            }),
            PromptScope::Standalone | PromptScope::Disabled => false,
        })
        .collect();

    if !filtered.is_empty() {
        let xml = build_skills_prompt_xml(&filtered);
        sections.push(format!(
            "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
             Skills provide specialized instructions for specific tasks.\n\n{}",
            xml
        ));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib test_build_with_skill`
Expected: PASS

- [ ] **Step 5: Run all prompt_builder tests to check no regression**

Run: `cargo test -p alephcore --lib agent_loop::prompt_builder`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "agent_loop: add scope-filtered skill injection to PromptBuilder"
```

---

### Task 7: Wire upstream — populate eligible_skills from SkillSystem

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:114-121` (prompt_builder construction)

- [ ] **Step 1: Read the file to understand context**

Read `src/gateway/execution_engine/run_loop.rs` around lines 1-20 (imports) and 114-121 (prompt_builder construction).

**Key context:** `ExecutionEngine` does NOT have an `extension_manager` field. However, there is a global `ExtensionManager` accessible via `crate::gateway::handlers::plugins::get_extension_manager()` (a `OnceCell`-backed static initialized at gateway startup). This is the correct access path.

- [ ] **Step 2: Implement wiring**

In `src/gateway/execution_engine/run_loop.rs`, add import:

```rust
use crate::gateway::handlers::plugins::get_extension_manager;
```

After the `prompt_builder` is constructed (around line 121), add skill population:

```rust
// Populate eligible skills from SkillSystem v2 for scope filtering
let prompt_builder = if let Ok(ext_manager) = get_extension_manager() {
    if ext_manager.is_loaded().await {
        let snapshot = ext_manager.skill_system().current_snapshot().await;
        if !snapshot.eligible_manifests.is_empty() {
            prompt_builder.with_eligible_skills(snapshot.eligible_manifests)
        } else {
            prompt_builder
        }
    } else {
        prompt_builder
    }
} else {
    prompt_builder
};
```

Note: `get_extension_manager()` returns `Result<&'static Arc<ExtensionManager>, JsonRpcResponse>`. If it fails (manager not initialized), we gracefully fall back to no skill injection — this is the correct behavior during tests or early startup.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: OK

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: wire SkillSystem eligible_manifests into prompt builder"
```

---

### Task 8: Full build + test verification

- [ ] **Step 1: Run full core test suite**

Run: `cargo test -p alephcore --lib`
Expected: All pass (check for pre-existing failures in `tools::markdown_skill::loader::tests` — those are known and not ours)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No new warnings

- [ ] **Step 3: Verify dead code cleanup**

Check if `build_skill_tool_description_v2` and `filter_skills_by_scope` in `src/extension/skill_tool.rs` still have `#[allow(dead_code)]` — they should remain untouched per spec (v1 path not modified).

- [ ] **Step 4: Final commit if any fixups**

```bash
git add -A
git commit -m "skill: scope filtering cleanup and verification"
```

---

### Task 9: Update prompt pipeline test (compact mode)

**Files:**
- Modify: `src/thinker/prompt_pipeline.rs:390-424` (compact mode test)

The `compact_mode_excludes_heavy_layers` test checks that `skill_instructions` layer does NOT support Compact mode. The paths change should not break this. Verify.

- [ ] **Step 1: Run the specific test**

Run: `cargo test -p alephcore --lib compact_mode_excludes_heavy_layers`
Expected: PASS (the `supports_mode` check is about PromptMode, not AssemblyPath — no change needed)

- [ ] **Step 2: If test fails, fix by updating test expectations**

The paths change (adding Soul/Context/Cached) does NOT affect `supports_mode(Compact)` — it should still return `false`. If the test still passes, no action needed.

---

## Summary

| Task | Component | Type |
|------|-----------|------|
| 1 | `SkillManifest.bound_tool` | Domain model |
| 2 | SKILL.md frontmatter parsing | Parser |
| 3 | `SkillSnapshot.eligible_manifests` | Snapshot |
| 4 | `PromptConfig.eligible_skills` | Thinker config |
| 5 | `SkillInstructionsLayer` rewrite | Thinker layer |
| 6 | `agent_loop::PromptBuilder` integration | Production path |
| 7 | Upstream wiring in gateway | Integration |
| 8 | Full verification | Quality |
| 9 | Pipeline test check | Regression |

## Phase 2 Reminder

After this is complete, the next task is **Semantic Skill Selection** (deferred loading). See spec section "Phase 2 Roadmap" for the design direction:
- Two-level list: summary (always sent) + full content (on demand via `read_skill` tool)
- Phase 1's scope filtering becomes the pre-filter
- Critical when skill count exceeds ~30

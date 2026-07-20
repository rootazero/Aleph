# Skill Scope Filtering — V2 接入设计

**Date**: 2026-03-23
**Status**: Approved
**Scope**: Phase 1 — Scope-aware skill filtering at prompt injection time

## Problem

当前 Aleph 的 Skill 系统将所有 auto-invocable skills 的 name + description 全量发送给 LLM，无论 scope 设置。随着安装的 skills 增多，token 开销持续膨胀。

SkillSystem v2 已定义了 `PromptScope`（System / Tool / Standalone / Disabled）和 `filter_skills_by_scope()` 函数，但这些过滤逻辑未接入 prompt 组装流程（标记为 `#[allow(dead_code)]`）。

## Decision

在 **SkillInstructionsLayer**（prompt injection 时）做动态 scope 过滤，而非在 snapshot 构建时或 ExtensionManager 层。

理由：
- 活跃工具集是每次 LLM 调用时才确定的动态状态
- 在 v2 路径（SkillSystem + thinker layers）接入，推动两套系统的统一
- 改动集中，不影响 v1 ExtensionSkill 路径

## Architecture

### Data Flow (Before)

```
SkillSnapshot::build()
  → pre-render prompt_xml (ALL model-visible skills)
    → PromptConfig.skill_instructions = snapshot.prompt_xml
      → SkillInstructionsLayer outputs verbatim
```

### Data Flow (After)

```
SkillSnapshot::build()
  → store eligible_manifests: Vec<SkillManifest>
    → PromptConfig.eligible_skills = snapshot.eligible_manifests
      → SkillInstructionsLayer.inject():
          1. Read skills from input.config.eligible_skills
          2. Read active tool names from input.tools
          3. Filter by scope:
             - System → keep
             - Tool → keep if bound_tool in active tools
             - Standalone → exclude
             - Disabled → exclude
          4. build_skills_prompt_xml(&filtered)
          5. Inject into prompt
```

### Scope Filtering Rules

| Scope | Behavior | Rationale |
|-------|----------|-----------|
| `System` | Always injected | Core skills the LLM should always know about |
| `Tool` | Injected only when `bound_tool` is in active tool set | No point showing a skill if its tool isn't available |
| `Standalone` | Never auto-injected | User must explicitly invoke via `/skill` command |
| `Disabled` | Never injected | Completely hidden |

### Assembly Paths

**Bug fix**: The existing `SkillInstructionsLayer` only participates in `[Basic, Hydration]`, but the Soul path is the primary production path. This change adds `Soul` (and `Context`, `Cached` for completeness):

```rust
fn paths(&self) -> &'static [AssemblyPath] {
    &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
}
```

### Hydration Path Behavior

On the Hydration path, `input.tools` is `None`, so `active_tool_names` will be empty. This means all `Tool`-scoped skills are automatically excluded on Hydration — this is acceptable because the Hydration path uses semantic retrieval for tool discovery rather than explicit tool lists.

### Explicit `/skill` Invocation

The `skill_instructions: Option<String>` field in `PromptConfig` is **preserved** for the capability layer's explicit skill invocation (`/skill` command). When present and non-empty, it takes priority over the automatic skill list — they are mutually exclusive in a single prompt.

## Changes

### 1. `src/domain/skill.rs` — SkillManifest

Add `bound_tool: Option<String>` field:

```rust
pub struct SkillManifest {
    // ... existing fields ...
    /// Tool name this skill is bound to (for Tool scope filtering)
    bound_tool: Option<String>,
}

impl SkillManifest {
    pub fn bound_tool(&self) -> Option<&str> {
        self.bound_tool.as_deref()
    }

    pub fn set_bound_tool(&mut self, tool: String) {
        self.bound_tool = Some(tool);
    }
}
```

### 2. `src/skill/manifest.rs` — Frontmatter Parsing

Parse `bound-tool` from YAML frontmatter into `SkillManifest.bound_tool`.

### 3. `src/skill/snapshot.rs` — SkillSnapshot

Add `eligible_manifests: Vec<SkillManifest>`:

```rust
pub struct SkillSnapshot {
    pub version: u64,
    pub prompt_xml: String,              // deprecated, kept for backward compat
    pub eligible: Vec<SkillId>,
    pub eligible_manifests: Vec<SkillManifest>,  // NEW
    pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>,
    pub built_at: DateTime<Utc>,
}
```

In `build()`, collect eligible + model-visible manifests (cloned) into `eligible_manifests`.

### 4. `src/thinker/prompt_builder/mod.rs` — PromptConfig

Add `eligible_skills: Option<Vec<SkillManifest>>`:

```rust
pub struct PromptConfig {
    // ... existing fields ...
    pub eligible_skills: Option<Vec<SkillManifest>>,
}
```

### 5. `src/thinker/layers/skill_instructions.rs` — SkillInstructionsLayer

Rewrite `inject()`:

```rust
fn inject(&self, output: &mut String, input: &LayerInput) {
    // 1. Explicit /skill invocation takes priority
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

    let active_tool_names: Vec<&str> = input.tools
        .map(|tools| tools.iter().map(|t| t.name.as_str()).collect())
        .unwrap_or_default();

    let filtered: Vec<&SkillManifest> = skills.iter().filter(|s| {
        match *s.scope() {
            PromptScope::System => true,
            PromptScope::Tool => {
                s.bound_tool().map_or(false, |bound|
                    active_tool_names.iter().any(|t| *t == bound)
                )
            }
            PromptScope::Standalone | PromptScope::Disabled => false,
        }
    }).collect();

    tracing::debug!(
        total = skills.len(),
        after_filter = filtered.len(),
        "skill_instructions: scope filtering applied"
    );

    if filtered.is_empty() { return; }

    let xml = build_skills_prompt_xml(&filtered);
    let xml = sanitize_for_prompt(&xml, SanitizeLevel::Moderate);
    output.push_str("## Available Skills\n\n");
    output.push_str("You can invoke skills using the `skill` tool. ");
    output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
    output.push_str(&xml);
    output.push_str("\n\n");
}
```

### 6. Upstream Integration

The caller that builds `PromptConfig` must populate `eligible_skills` from `SkillSystem.current_snapshot().eligible_manifests`. This integration point is in the gateway/execution engine or thinker caller.

### 7. `agent_loop::prompt_builder::PromptBuilder` — Production Path

The production agent loop uses `agent_loop::prompt_builder::PromptBuilder`, not the thinker's `PromptPipeline`. This builder also needs scope-filtered skill injection. Add `eligible_skills: Option<Vec<SkillManifest>>` field, `with_eligible_skills()` builder method, and scope filtering in `build()`.

### 8. `gateway/execution_engine/run_loop.rs` — Upstream Wiring

Use the global `get_extension_manager()` accessor to obtain the `SkillSystem` snapshot and populate `eligible_skills` on the `PromptBuilder` at construction time.

### Files Not Changed

- `ExtensionSkill` path (v1) — untouched, natural deprecation
- `filter_skills_by_scope()` in `skill_tool.rs` — remains dead code (v1 path)

## Testing

- Unit test: `SkillInstructionsLayer` with mixed scopes (System + Tool + Standalone + Disabled) → only System and matching Tool appear in output
- Unit test: Tool scope skill with `bound_tool="web_search"` — included when `web_search` in tools, excluded when not
- Unit test: Explicit `skill_instructions` takes priority over `eligible_skills`
- Unit test: Empty `eligible_skills` produces no output
- Unit test: `SkillSnapshot::build()` populates `eligible_manifests` correctly

## Phase 2 Roadmap — Semantic Skill Selection (Next Task)

Phase 1 solves scope filtering, but `System` scope skills are still sent in full. With 50+ system skills, token cost remains high.

### Core Idea: Two-Level List (Deferred Loading)

1. **Summary list** (always sent) — All eligible skills as `name + one-line description`, minimal tokens
2. **Full content** (on demand) — LLM calls `read_skill` tool to load full instructions when needed

This mirrors Claude Code's deferred tools pattern: send an index, load on demand.

### How It Connects to Phase 1

- Phase 1's scope filtering is preserved as **pre-filter** before building the summary list
- `SkillInstructionsLayer` responsibility unchanged — output format changes from "full XML list" to "summary index"
- `read_skill` tool already exists in the dispatcher

### Expected Token Savings

| Scenario | Phase 1 | Phase 2 |
|----------|---------|---------|
| 10 skills, all System | 10 × ~50 tokens = ~500 | Same (not worth deferring) |
| 50 skills, 30 System + 20 Tool | ~1500 (if 10 tools active) | ~500 summary + ~100 per loaded skill |
| 100 skills | ~5000 | ~1000 summary + on-demand |

Phase 2 becomes critical when skill count exceeds ~30.

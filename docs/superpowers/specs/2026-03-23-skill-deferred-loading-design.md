# Phase 2: Skill Deferred Loading Design

**Date**: 2026-03-23
**Status**: Approved
**Depends on**: Phase 1 — Skill Scope Filtering (2026-03-23, completed)

## Problem

After Phase 1 scope filtering, some paths still inject full skill content into the LLM prompt. Specifically, the ExtensionSkill v1 path (`build_skill_instructions()` in `extension/mod.rs`) sends the complete `skill.content` for every plugin skill. With 50+ skills installed, this causes significant token overhead (~5000+ tokens).

## Goal

Implement deferred loading: only send a summary index (name + description) to the LLM. When the LLM determines it needs a skill, it calls the `read_skill` tool to load full instructions on-demand.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Scope | All System scope skills unified deferred | Simplicity, no special cases |
| Caching | No special mechanism — conversation history is the cache | P6 simplicity; `read_skill` results stay in context naturally |
| Prompt style | Structured: name+description index + usage guidance | LLM needs enough info to decide when to call `read_skill` |
| Paths to change | Both agent_loop PromptBuilder and thinker SkillInstructionsLayer | Consistency — both paths must behave the same |
| Explicit `/skill` | Unchanged — still injects full content | User-initiated, should be immediate |
| Approach | Minimal — fix v1 full-content path, add guidance, register tool | Smallest change, lowest risk |

## Current State Analysis

### Paths that inject skill content into prompts

| Path | Location | Current behavior | Phase 2 action |
|------|----------|-----------------|----------------|
| ExtensionSkill v1 | `extension/mod.rs:340` `build_skill_instructions()` | Full content for all plugin skills | **Change to name+description only + guidance** |
| Explicit `/skill` | `capability/mod.rs:421,445` → `payload.context.skill_instructions` | Full instructions (user-initiated) | **No change** |
| v2 auto skill list | `build_skills_prompt_xml()` via SkillInstructionsLayer / agent_loop PromptBuilder | Name+description only (Phase 1) | **Add guidance text** |

### Critical gap: `read_skill` tool not registered

`ReadSkillTool` (`skill_read`) exists in `builtin_tools/skill_reader.rs` but is **not registered** in `BuiltinToolRegistry`. Only `ListSkillsTool` (`skill_list`) is registered. The builder comment says "read_skill replaced by read_config_guide". Phase 2 must register this tool for deferred loading to work.

## Changes

### 1. Shared guidance constant

**File**: `core/src/skill/prompt.rs`

Add a constant for the deferred loading guidance text, used by all three injection points:

```rust
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `read_skill` tool with the skill name \
     to load its full instructions, then follow those instructions.";
```

### 2. Fix ExtensionSkill v1 path

**File**: `core/src/extension/mod.rs` — `build_skill_instructions()`

Change from injecting full `skill.content` to name+description index only:

```rust
// Before:
output.push_str(&format!(
    "### /{}\n**Description**: {}\n\n{}\n\n---\n\n",
    skill.qualified_name(),
    skill.description,
    skill.content          // ← full content, removed
));

// After:
output.push_str(&format!(
    "- **{}**: {}\n",
    skill.qualified_name(),
    skill.description,
));
```

Append `DEFERRED_LOADING_GUIDANCE` after the skill list.

### 3. Add guidance to v2 thinker path

**File**: `core/src/thinker/layers/skill_instructions.rs` — `inject()`

After `build_skills_prompt_xml()` output, append `DEFERRED_LOADING_GUIDANCE` to the prompt. The existing header text ("You can invoke skills using the `skill` tool...") is kept, with the guidance appended after it.

### 4. Add guidance to agent_loop path

**File**: `core/src/agent_loop/prompt_builder.rs` — `build()`

Same change as thinker path — append `DEFERRED_LOADING_GUIDANCE` after the skills XML section.

### 5. Register `ReadSkillTool` in BuiltinToolRegistry

**Files**:
- `core/src/executor/builtin_registry/registry.rs` — add `read_skill_tool: ReadSkillTool` field
- `core/src/executor/builtin_registry/builder.rs` — create `ReadSkillTool::default()`, register `"skill_read"` in tools map with schema, add to struct initialization
- `core/src/executor/builtin_registry/registry.rs` — add `"skill_read"` match arm in `execute_tool()`

## What does NOT change

- **Explicit `/skill` invocation** — `capability/mod.rs` payload.context.skill_instructions path stays as-is (user-initiated, full content is expected)
- **`SkillSnapshot.prompt_xml`** — backward compat field, untouched
- **`build_skills_prompt_xml()` function** — already outputs name+description only, no changes needed to the function itself
- **Phase 1 scope filtering logic** — remains as pre-filter in both paths
- **`read_skill` tool implementation** — already feature-complete with multi-directory discovery, path traversal protection, etc.

## Testing

- Update existing `build_skill_instructions()` tests to verify content is no longer emitted
- Update `skill_instructions.rs` tests to verify `DEFERRED_LOADING_GUIDANCE` appears in output
- Update `agent_loop/prompt_builder.rs` tests to verify guidance appears in output
- Add integration test for `skill_read` tool registration (tool metadata exists and is executable)

## Token impact estimate

- **Before**: 50 plugin skills × ~100 tokens avg content = ~5000 tokens
- **After**: 50 skills × ~15 tokens (name+description) + ~30 tokens guidance = ~780 tokens
- **Savings**: ~4200 tokens per prompt (~84% reduction)

## Architecture alignment

- **R8 LLM Sovereignty**: LLM decides when to load a skill, no deterministic pre-filtering of content
- **R9 Everything is a Tool**: Skill loading is a tool call (`read_skill`)
- **R10 Intelligence in Prompt**: Guidance text tells LLM the mechanism; LLM decides usage
- **P6 Simplicity**: Minimal changes, no new abstractions, leverages existing `ReadSkillTool`

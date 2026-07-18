# Phase 2: Skill Deferred Loading Design

**Date**: 2026-03-23
**Status**: Approved
**Depends on**: Phase 1 — Skill Scope Filtering (2026-03-23, completed)

## Problem

After Phase 1 scope filtering, the v2 auto skill list path (`build_skills_prompt_xml()`) already only sends name+description. However, the `skill_read` tool is not registered in `BuiltinToolRegistry`, so even though the prompt tells the LLM about available skills, the LLM has no way to load a skill's full instructions on-demand.

Additionally, there are dead-code v1 paths (`build_skill_instructions()`, `build_skill_tool_description()`) that still contain full-content injection logic. While currently unreachable, they should be cleaned up to prevent future accidental use.

## Goal

Complete the deferred loading mechanism:
1. Register the `skill_read` tool so the LLM can actually load skill content on-demand
2. Add deferred loading guidance to the prompt so the LLM knows to call `skill_read`
3. Clean up dead-code v1 full-content injection functions

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Scope | All System scope skills unified deferred | Simplicity, no special cases |
| Caching | No special mechanism — conversation history is the cache | P6 simplicity; `skill_read` results stay in context naturally |
| Prompt style | Structured: name+description index + usage guidance | LLM needs enough info to decide when to call `skill_read` |
| Paths to change | Both agent_loop PromptBuilder and thinker SkillInstructionsLayer | Consistency — both paths must behave the same |
| Explicit `/skill` | Unchanged — still injects full content | User-initiated, should be immediate |
| Approach | Minimal — register tool, add guidance, clean up dead code | Smallest change, lowest risk |

## Current State Analysis

### Paths that inject skill content into prompts

| Path | Location | Current behavior | Status |
|------|----------|-----------------|--------|
| v2 auto skill list | `build_skills_prompt_xml()` via SkillInstructionsLayer / agent_loop PromptBuilder | Name+description only | **Working — needs guidance text** |
| Explicit `/skill` | `capability/mod.rs` → `payload.context.skill_instructions` | Full instructions (user-initiated) | **No change needed** |
| ~~ExtensionSkill v1~~ | `extension/mod.rs:340` `build_skill_instructions()` | Dead code — defined but never called | **Clean up** |
| ~~ExtensionSkill v1~~ | `extension/skill_tool.rs:246` `build_skill_tool_description()` | Dead code — only called by `get_skill_tool_description()` which itself has no callers | **Clean up** |

### Critical gap: `skill_read` tool not registered

`ReadSkillTool` (tool name = `skill_read`) exists in `builtin_tools/skill_reader.rs` but is **not registered** in `BuiltinToolRegistry`. Only `ListSkillsTool` (`skill_list`) is registered. The builder comment says "read_skill replaced by read_config_guide". Phase 2 must register this tool for deferred loading to work.

### `ReadSkillTool` coverage

`ReadSkillTool` discovers skills from filesystem directories:
- Project level: `.aleph/skills/`, `.claude/skills/` (traverse up to git root)
- Global level: `~/.aleph/skills`, `~/.claude/skills`

This covers v2 `SkillManifest` skills (loaded from the same directories by `SkillSystem`). Plugin skills (loaded via `ExtensionManager` from plugin directories) are NOT covered, but this is acceptable because:
- Plugin skills currently go through the `skill` tool (ExtensionManager's `invoke_skill_tool`), not `skill_read`
- The v2 SkillSystem is the primary path for skill index injection
- Future unification of v1/v2 is out of scope for Phase 2

## Changes

### 1. Shared guidance constant

**File**: `src/skill/prompt.rs`

Add a constant for the deferred loading guidance text, used by both injection points:

```rust
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.";
```

Note: tool name is `skill_read` (not `read_skill`) — matches `ReadSkillTool::NAME`.

### 2. Add guidance to v2 thinker path

**File**: `src/thinker/layers/skill_instructions.rs` — `inject()`

After `build_skills_prompt_xml()` output, append `DEFERRED_LOADING_GUIDANCE` to the prompt. The existing header text ("You can invoke skills using the `skill` tool...") is kept, with the guidance appended after it.

### 3. Add guidance to agent_loop path

**File**: `src/agent_loop/prompt_builder.rs` — `build()`

Same change as thinker path — append `DEFERRED_LOADING_GUIDANCE` after the skills XML section.

### 4. Register `ReadSkillTool` in BuiltinToolRegistry

**Files**:
- `src/executor/builtin_registry/registry.rs` — add `read_skill_tool: ReadSkillTool` field
- `src/executor/builtin_registry/builder.rs` — create `ReadSkillTool::default()`, register `"skill_read"` in tools map with schema, add to struct initialization
- `src/executor/builtin_registry/registry.rs` — add `"skill_read"` match arm in `execute_tool()`

`ReadSkillTool::default()` calls `with_auto_discover(None)` which discovers global skill directories. This matches the existing `ListSkillsTool::default()` behavior and is sufficient because:
- Most skills are installed globally (`~/.aleph/skills`)
- Project-level skill discovery would require project directory injection (future enhancement)

### 5. Clean up dead code

**Files**:
- `src/extension/mod.rs` — remove `build_skill_instructions()` function (dead code, never called)
- `src/extension/skill_tool.rs` — remove `build_skill_tool_description()`, `build_skill_tool_description_v2()`, and `filter_skills_by_scope()` (dead code / unused prepared v2 API)
- `src/extension/skill_ops.rs` — remove `get_skill_tool_description()` method (only caller of dead code)
- `src/extension/mod.rs` — remove `build_skill_tool_description` from `pub use` exports

## What does NOT change

- **Explicit `/skill` invocation** — `capability/mod.rs` payload.context.skill_instructions path stays as-is (user-initiated, full content is expected)
- **`SkillSnapshot.prompt_xml`** — backward compat field, untouched
- **`build_skills_prompt_xml()` function** — already outputs name+description only, no changes needed to the function itself
- **Phase 1 scope filtering logic** — remains as pre-filter in both paths
- **`ReadSkillTool` implementation** — already feature-complete with multi-directory discovery, path traversal protection, etc.

## Testing

- Update `skill_instructions.rs` tests to verify `DEFERRED_LOADING_GUIDANCE` appears in output
- Update `agent_loop/prompt_builder.rs` tests to verify guidance appears in output
- Add test for `skill_read` tool registration (tool metadata exists in BuiltinToolRegistry)
- Remove tests for deleted dead-code functions
- Verify `cargo test -p alephcore --lib` passes after dead code removal

## Architecture alignment

- **R8 LLM Sovereignty**: LLM decides when to load a skill, no deterministic pre-filtering of content
- **R9 Everything is a Tool**: Skill loading is a tool call (`skill_read`)
- **R10 Intelligence in Prompt**: Guidance text tells LLM the mechanism; LLM decides usage
- **P6 Simplicity**: Minimal changes, no new abstractions, leverages existing `ReadSkillTool`; dead code cleaned up per "deletion over commenting"

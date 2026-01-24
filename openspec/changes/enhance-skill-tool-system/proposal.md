# Change: Enhance Skill Tool System

## Why

Comparative analysis with OpenCode (Claude Code open-source implementation) revealed that Aether's Skill system lacks critical features: Skills cannot be dynamically invoked by LLM as a Tool, there's no instance-level caching, template system is limited, and permission checks are not integrated. These gaps prevent Skills from being a first-class citizen in the agent loop.

## What Changes

- **ADDED** `SkillTool` - Skill as LLM-callable Tool with structured result
- **ADDED** Instance-level caching via `ensure_loaded()` and `reload()` methods
- **ADDED** Template processor with `@file` reference support
- **ADDED** Permission integration in `invoke_skill_tool()`
- **ADDED** `SkillContext`, `SkillToolResult`, `SkillMetadata` types
- **MODIFIED** `ExtensionManager` to support lazy-loading and skill tool invocation
- **MODIFIED** `ExtensionError` to include `PermissionDenied` variant

## Impact

- Affected specs: `extension-system` (new capability to be created)
- Affected code:
  - `core/src/extension/mod.rs` - caching, new exports
  - `core/src/extension/skill_tool.rs` - new file
  - `core/src/extension/template.rs` - new file
  - `core/src/extension/types.rs` - new types
  - `core/src/extension/error.rs` - new error variant
  - `core/src/agent_loop/` - tool integration (future)

## Key Design Decisions

1. **Incremental Enhancement**: Build on existing `ExtensionManager` and `ComponentRegistry`
2. **Backward Compatibility**: Preserve `execute_skill()` for existing callers
3. **Security First**: No shell execution in templates; file refs limited to skill directory
4. **Lazy Loading**: Only scan filesystem when first accessed, cache thereafter

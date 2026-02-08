# Phase 1: Extension Types Refactoring Design

**Date:** 2026-02-08
**Status:** Approved
**Author:** Architecture Review

## Executive Summary

This document outlines the first phase of a four-phase architecture refactoring initiative to address code bloat in the Aleph codebase. Phase 1 focuses on refactoring `core/src/extension/types.rs` (2037 lines) into a modular directory structure while maintaining complete backward compatibility.

## Background

### Problem Statement

The current `core/src/extension/types.rs` file exhibits the "God Object" anti-pattern, containing type definitions for multiple distinct domains:
- Skills and Commands
- Agents and Permissions
- Plugins and Hooks
- Runtime Services (Channels, Providers, HTTP)
- MCP Configuration
- Frontmatter Parsing

This monolithic structure creates several issues:
1. **Maintainability** - Difficult to navigate and modify
2. **IDE Performance** - Slow indexing and autocomplete
3. **Cognitive Load** - Developers must understand all domains to modify any
4. **Testing** - Hard to isolate and test individual domains

### Architecture Audit Context

This refactoring is part of a broader initiative to improve code cohesion and reduce coupling across four major subsystems:
1. **Phase 1** (This Document): `extension/types.rs` - Type definitions (leaf node)
2. **Phase 2**: `engine/atomic_executor.rs` - Execution logic (core)
3. **Phase 3**: `browser/mod.rs` - Infrastructure services
4. **Phase 4**: `gateway/handlers/poe.rs` - Business logic layer

## Design Goals

### Primary Objectives

1. **Improve Maintainability** - Reduce file size to manageable units (200-300 lines each)
2. **Enhance Readability** - Clear domain boundaries and responsibilities
3. **Preserve Stability** - Zero breaking changes to existing code
4. **Enable Evolution** - Create foundation for future architectural improvements

### Core Principles

1. **Complete Backward Compatibility** - Use `pub use` to maintain existing API surface
2. **Zero Invasiveness** - No modifications to calling code required
3. **Atomic Migration** - Move code without logic changes
4. **Documentation Consistency** - Preserve all module-level documentation

## Proposed Architecture

### File Structure

```
core/src/extension/types/
├── mod.rs          # Unified export layer (~50 lines)
├── skills.rs       # Skills + Commands (~250 lines)
├── agents.rs       # Agents + Permissions (~150 lines)
├── plugins.rs      # Plugin core types (~250 lines)
├── hooks.rs        # Hooks + MCP (~230 lines)
└── runtime.rs      # Runtime interactions (~300 lines)
```

### Module Responsibilities

#### `skills.rs` - Skill & Command System
**Domain:** Skill execution and command handling

**Types:**
- `SkillToolResult` - Result of skill tool invocation
- `SkillMetadata` - Skill metadata and discovery info
- `SkillContext` - Execution context for skills
- `DirectCommandResult` - Synchronous command results
- `SkillType` - Skill type enumeration
- `ExtensionSkill` - Skill definition and manifest
- `ExtensionCommand` - Type alias for commands
- `SkillFrontmatter` - YAML frontmatter parsing

**Rationale:** Commands are conceptually "synchronous skills" without LLM involvement, so they belong in the same module.

#### `agents.rs` - Agent System
**Domain:** Agent modes, permissions, and lifecycle

**Types:**
- `AgentMode` - Agent execution modes
- `PermissionRule` - Permission rule definitions
- `PermissionAction` - Permission actions (Allow/Deny/Ask)
- `ExtensionAgent` - Agent definition and manifest
- `AgentFrontmatter` - Agent YAML frontmatter

**Rationale:** Agents represent a distinct domain with their own lifecycle and permission model.

#### `plugins.rs` - Plugin Core
**Domain:** Plugin metadata, status, and lifecycle

**Types:**
- `ExtensionPlugin` - Plugin definition and manifest
- `PluginInfo` - Plugin metadata
- `PluginOrigin` - Plugin source (Local/Remote/Builtin)
- `PluginKind` - Plugin type (Wasm/Node/Python)
- `PluginStatus` - Plugin lifecycle state
- `PluginRecord` - Plugin registry record

**Rationale:** Plugin core types form the foundation of the extension system.

#### `hooks.rs` - Extension Mechanisms
**Domain:** Hooks, events, and MCP integration

**Types:**
- `HookEvent` - Hook event types
- `HookKind` - Hook categories (Tool/Prompt/Lifecycle)
- `HookPriority` - Hook execution priority
- `PromptScope` - Prompt injection scope
- `HookAction` - Hook action definitions
- `HookConfig` - Hook configuration
- `McpServerConfig` - MCP server configuration

**Rationale:** Hooks and MCP are both extension mechanisms that allow plugins to integrate with the system.

#### `runtime.rs` - Runtime Interactions
**Domain:** Service, channel, provider, and HTTP types

**Types:**
- **Service Types:** `ServiceState`, `ServiceInfo`, `ServiceResult`
- **Channel Types:** `ChannelMessage`, `ChannelSendRequest`, `ChannelState`, `ChannelInfo`
- **Provider Types:** `ProviderChatRequest`, `ProviderMessage`, `ProviderChatResponse`, `ProviderUsage`, `ProviderStreamChunk`, `ProviderModelInfo`
- **HTTP Types:** `HttpRequest`, `HttpResponse`

**Rationale:** These types represent runtime interactions with external systems. They are grouped together as infrastructure concerns.

**⚠️ Architecture Warning:** This module may become a future maintenance pressure point. If any subdomain (especially Providers) grows significantly, it should be extracted into its own file.

### Export Strategy

The `mod.rs` file will use the "Unified Flat Namespace" pattern (similar to `tokio` and `serde`):

```rust
// core/src/extension/types/mod.rs

//! Extension system type definitions
//!
//! Core data structures for skills, commands, agents, and plugins.

mod skills;
mod agents;
mod plugins;
mod hooks;
mod runtime;

// Flatten exports to maintain existing API
pub use skills::*;
pub use agents::*;
pub use plugins::*;
pub use hooks::*;
pub use runtime::*;
```

This approach ensures that existing imports continue to work:

```rust
// Existing code - no changes required
use crate::extension::types::SkillMetadata;
use crate::extension::types::ExtensionAgent;
use crate::extension::types::PluginRecord;
```

## Implementation Plan

### Step 1: Create Directory Structure

```bash
mkdir -p core/src/extension/types
```

### Step 2: Create Module Files

Create files in dependency order to minimize compilation errors:

1. **`skills.rs`** - Most independent, minimal dependencies
2. **`agents.rs`** - Depends on `PermissionRule` from skills
3. **`plugins.rs`** - Plugin core types
4. **`hooks.rs`** - Depends on plugin types
5. **`runtime.rs`** - Runtime interaction types
6. **`mod.rs`** - Final step, unified exports

Each file must include:
- Module-level documentation (`//!` comments)
- Necessary imports (`use` statements)
- Type definitions moved from original `types.rs`
- All `impl` blocks for the types
- All derive macros and attributes

### Step 3: Create Export Layer

Create `mod.rs` with module declarations and `pub use` statements to flatten the namespace.

### Step 4: Remove Original File

```bash
git rm core/src/extension/types.rs
```

### Step 5: Compilation Verification

```bash
cargo check -p alephcore
cargo test -p alephcore
cargo doc -p alephcore --no-deps
```

## Verification Strategy

### Compilation Checks

1. **Full compilation:** `cargo check -p alephcore`
2. **Test suite:** `cargo test -p alephcore`
3. **Documentation:** `cargo doc -p alephcore --no-deps`

### Import Path Validation

Verify that existing import paths continue to work:

```rust
use crate::extension::types::SkillMetadata;      // ✓ Should work
use crate::extension::types::ExtensionAgent;     // ✓ Should work
use crate::extension::types::PluginRecord;       // ✓ Should work
```

### Architecture Review Checklist

- [ ] All type definitions migrated completely
- [ ] No `impl` blocks left behind
- [ ] Module-level documentation preserved
- [ ] All derive macros and attributes preserved
- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] No logic modifications (only code movement)
- [ ] Git history preserved (use `git mv` where possible)

### Rollback Strategy

If verification fails, quick rollback is available:

```bash
git checkout core/src/extension/types.rs
rm -rf core/src/extension/types/
```

## Risk Assessment

### Low Risk

- **Type definitions are leaf nodes** - They are imported by many modules but have minimal dependencies themselves
- **Rust's type system** - Compilation errors will catch any issues immediately
- **`pub use` pattern** - Well-established in Rust ecosystem (tokio, serde)

### Mitigation Strategies

1. **Incremental approach** - Create files one at a time, verify compilation after each
2. **Comprehensive testing** - Run full test suite before and after
3. **Git safety** - Commit each step separately for easy rollback
4. **Documentation** - Preserve all comments and documentation

## Future Considerations

### Phase 2 Preview

After Phase 1 completion, Phase 2 will address `engine/atomic_executor.rs` (1547 lines) using the Strategy Pattern to separate:
- File operations (`file_ops.rs`)
- Edit operations (`edit.rs`)
- Shell execution (`shell.rs`)
- Search/replace (`search.rs`)

### Evolution Path

If `runtime.rs` grows beyond 500 lines, consider splitting into:
- `services.rs` - Service lifecycle types
- `channels.rs` - Channel communication types
- `providers.rs` - AI provider types
- `http.rs` - HTTP endpoint types

## Success Criteria

1. ✅ `cargo check` passes without errors
2. ✅ `cargo test` passes all tests
3. ✅ No changes required in calling code
4. ✅ File sizes reduced to manageable units (<350 lines each)
5. ✅ Module responsibilities clearly defined
6. ✅ Documentation quality maintained
7. ✅ Git history preserved

## Conclusion

This refactoring represents a "zero-invasive" architectural improvement that enhances code organization without disrupting existing functionality. By following the principle of "physical organization ≠ logical API boundary," we achieve better maintainability while preserving the developer experience.

The use of `pub use` for backward compatibility is a well-established Rust pattern that allows us to evolve the internal structure without forcing ecosystem-wide changes. This approach minimizes merge conflicts and allows the refactoring to proceed independently of other development work.

---

**Next Steps:**
1. Obtain architecture review approval
2. Create implementation branch
3. Execute refactoring following the implementation plan
4. Submit PR with comprehensive testing evidence
5. Proceed to Phase 2 (atomic_executor.rs) after Phase 1 stabilizes

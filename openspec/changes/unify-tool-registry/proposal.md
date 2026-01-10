# Change: Unify Tool Registry as Single Source of Truth

## Status
- **Stage**: Proposed
- **Created**: 2026-01-10
- **Depends on**: implement-dispatcher-layer (deployed)

## Why

The current Aether system has multiple hardcoded tool/command lists that are not synchronized:

1. **Rust Core `Config::builtin_rules()`** (config/mod.rs:359-459)
   - Hardcodes 5 builtin commands: `/search`, `/mcp`, `/skill`, `/video`, `/chat`
   - Used by Router for regex-based routing

2. **Rust Core `CommandRegistry::get_builtin_hint()`** (command/registry.rs:23-29)
   - Hardcodes hints for the same 5 commands with localization
   - Used by command completion system

3. **Swift UI `PresetRules.all`** (RoutingView.swift:695-774)
   - Hardcodes the same 5 preset rules with descriptions, icons, subcommands
   - Used by Settings > Routing > Builtin Rules UI

4. **Rust Core `ToolRegistry::register_native_tools()`** (dispatcher/registry.rs:70-127)
   - Hardcodes only 2 native tools: `native:search`, `native:video`
   - Missing `/mcp`, `/skill`, `/chat` as tools

**Problems:**
- Adding a new builtin command requires updating 3-4 different places
- MCP tools, Skills, and Custom rules don't appear in UI's builtin rules list
- Command completion only shows config rules, not dynamically loaded MCP/Skills
- LLM intent detection can only use tools in ToolRegistry, not all available commands
- No single source of truth for "what commands/tools are available"

## What Changes

### Core Architecture

1. **ToolRegistry becomes the Single Source of Truth**
   - All tool types registered: Native, MCP, Skill, Custom, **Builtin**
   - New `ToolSource::Builtin` variant for system commands
   - Remove hardcoded lists from Config and CommandRegistry
   - ToolRegistry provides data to UI and command completion

2. **New UniFFI APIs for UI**
   - `list_builtin_tools()` - Returns builtin commands for Settings UI
   - `list_all_tools_for_ui()` - Returns all tools with UI metadata
   - `get_command_completions(prefix)` - Returns filtered commands for completion
   - `get_tool_metadata(id)` - Returns detailed metadata for a tool

3. **Dynamic Command Completion**
   - CommandRegistry queries ToolRegistry instead of parsing config
   - Supports nested completions for `/mcp <server> <tool>`
   - Supports skill completions for `/skill <skill_name>`
   - Real-time updates when MCP servers connect/disconnect

4. **Unified Metadata Structure**
   - `UnifiedTool` extended with UI metadata:
     - `icon: Option<String>` - SF Symbol name
     - `usage: Option<String>` - Usage example
     - `subcommands: Vec<UnifiedTool>` - Nested tools
     - `localized_hint: Option<String>` - Localized short description
     - `localized_description: Option<String>` - Localized full description

### Files to Create/Modify

**Modified Files (Rust Core):**
- `core/src/dispatcher/registry.rs` - Add Builtin tools, UI metadata, completion APIs
- `core/src/dispatcher/types.rs` - Extend UnifiedTool, add ToolSource::Builtin
- `core/src/command/registry.rs` - Use ToolRegistry as data source
- `core/src/config/mod.rs` - Remove `builtin_rules()`, load from ToolRegistry
- `core/src/aether.udl` - Add UniFFI exports for UI APIs
- `core/src/core.rs` - Wire up new APIs

**Modified Files (Swift UI):**
- `Sources/RoutingView.swift` - Remove PresetRules, use ToolRegistry APIs
- `Sources/Utils/CommandCompletionManager.swift` - Use new completion APIs

## Impact

### Affected Specs
- New spec: `unified-tool-registry` - Unified tool aggregation requirements
- New spec: `command-completion` - Command completion from registry requirements

### Affected Code
- `core/src/dispatcher/` - Extended registry and types
- `core/src/command/` - CommandRegistry refactored
- `core/src/config/` - Builtin rules moved to registry
- `Aether/Sources/RoutingView.swift` - Dynamic preset rules
- `Aether/Sources/Utils/CommandCompletionManager.swift` - Dynamic completion

### Breaking Changes
- **None externally** - This is an internal refactoring
- UI components will use new APIs but behavior unchanged
- Config format unchanged - user-defined rules still work

### Migration
- No user migration required
- Internal code migration handled in implementation

## Success Criteria

1. **Single Source of Truth**: All tool types (Native, MCP, Skill, Custom, Builtin) queryable from ToolRegistry
2. **Dynamic UI**: Settings > Routing shows all available tools, not just hardcoded presets
3. **Dynamic Completion**: `/` shows all root commands, `/mcp ` shows MCP tools, `/skill ` shows skills
4. **LLM Awareness**: L3 intent detection sees all available tools for routing
5. **Real-time Updates**: MCP server connect/disconnect updates completion list immediately
6. **Localization**: Tool hints and descriptions support i18n via existing localization system
7. **No Hardcoding**: Removing a tool from registry removes it everywhere

## References

- Depends on: `implement-dispatcher-layer` (deployed)
- Related code: `dispatcher/registry.rs`, `command/registry.rs`, `RoutingView.swift`
- Design document: See `design.md` for architectural decisions

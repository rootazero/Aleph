# Tasks: Unify Tool Registry as Single Source of Truth

## Phase 1: Extend UnifiedTool Data Model

### 1.1 Update ToolSource Enum
- [x] 1.1.1 Add `ToolSource::Builtin` variant to `dispatcher/types.rs`
- [x] 1.1.2 Update `ToolSource::label()` method
- [ ] 1.1.3 Add unit tests for new variant

### 1.2 Extend UnifiedTool Struct
- [x] 1.2.1 Add `icon: Option<String>` field
- [x] 1.2.2 Add `usage: Option<String>` field
- [x] 1.2.3 Add `subtools: Vec<String>` field (tool IDs)
- [x] 1.2.4 Add `localization_key: Option<String>` field
- [x] 1.2.5 Add `is_builtin: bool` field
- [x] 1.2.6 Add `sort_order: i32` field
- [x] 1.2.7 Update `UnifiedTool::new()` to set defaults
- [x] 1.2.8 Add builder methods: `with_icon()`, `with_usage()`, etc.
- [ ] 1.2.9 Add unit tests for new fields

### 1.3 Implement Builtin Tools Registration
- [x] 1.3.1 Add `register_builtin_tools()` method to ToolRegistry
- [x] 1.3.2 Register `/search` builtin with metadata
- [x] 1.3.3 Register `/mcp` builtin with `has_subtools=true`
- [x] 1.3.4 Register `/skill` builtin with `has_subtools=true`
- [x] 1.3.5 Register `/video` builtin with metadata
- [x] 1.3.6 Register `/chat` builtin with metadata
- [x] 1.3.7 Call `register_builtin_tools()` in `refresh_all()`
- [ ] 1.3.8 Add unit tests for builtin registration

## Phase 2: Add UniFFI APIs

### 2.1 Define Types in UDL
- [x] 2.1.1 Add `ToolSourceType` enum to `aleph.udl`
- [x] 2.1.2 Add `UnifiedToolInfo` dictionary to `aleph.udl`
- [x] 2.1.3 Add `CommandNodeInfo` dictionary (if not exists)

### 2.2 Add Tool Registry APIs to AlephCore
- [x] 2.2.1 Add `list_builtin_tools()` async method
- [x] 2.2.2 Add `list_all_tools()` async method
- [x] 2.2.3 Add `list_tools_by_source(source_type)` async method
- [ ] 2.2.4 Add `get_tool_metadata(tool_id)` async method

### 2.3 Add Command Completion APIs
- [x] 2.3.1 Add `get_command_completions(prefix)` async method (via filter_commands)
- [x] 2.3.2 Add `get_subcommand_completions(parent_key)` async method (via get_subtools_from_registry)
- [x] 2.3.3 Implement prefix filtering logic

### 2.4 Generate and Test Bindings
- [x] 2.4.1 Run `uniffi-bindgen generate`
- [x] 2.4.2 Verify Swift bindings compile
- [ ] 2.4.3 Add integration tests for UniFFI calls

## Phase 3: Refactor CommandRegistry

**Note: Used minimal approach - kept existing CommandRegistry for backward compatibility, added registry-based alternatives**

### 3.1 Inject ToolRegistry Dependency
- [ ] 3.1.1 Add `tool_registry: Arc<ToolRegistry>` field (SKIPPED - minimal approach)
- [ ] 3.1.2 Update `CommandRegistry::new()` to accept registry (SKIPPED)
- [ ] 3.1.3 Update `CommandRegistry::from_config()` to accept registry (SKIPPED)

### 3.2 Implement Registry-Based Queries
- [x] 3.2.1 Refactor `get_root_commands()` to query ToolRegistry (via get_root_commands_from_registry)
- [x] 3.2.2 Refactor `get_children()` to query ToolRegistry by source (via get_subcommands_from_registry)
- [x] 3.2.3 Handle MCP children: `/mcp` → list MCP tools
- [x] 3.2.4 Handle Skill children: `/skill` → list Skills
- [x] 3.2.5 Add `tool_to_command_node()` conversion method

### 3.3 Remove Hardcoded Lists
- [ ] 3.3.1 Remove `get_builtin_hint()` function (DEFERRED - backward compat)
- [ ] 3.3.2 Remove `builtin_commands` Vec field (DEFERRED)
- [x] 3.3.3 Use localization keys instead of hardcoded hints
- [ ] 3.3.4 Update tests to use registry-based approach

### 3.4 Wire Up in AlephCore
- [x] 3.4.1 Pass ToolRegistry to CommandRegistry in `core.rs` (via new methods)
- [ ] 3.4.2 Ensure CommandRegistry refreshes on tools_changed
- [ ] 3.4.3 Add integration tests

## Phase 4: Refactor Swift UI

### 4.1 Update RoutingView
- [x] 4.1.1 Remove `PresetRules` enum (DONE - deleted hardcoded enum)
- [x] 4.1.2 Remove `PresetSubcommand` struct (KEPT - still used as display model)
- [x] 4.1.3 Add `@State var builtinTools: [UnifiedToolInfo]`
- [x] 4.1.4 Add `loadBuiltinTools()` async method
- [x] 4.1.5 Call `core.listBuiltinTools()` on appear
- [x] 4.1.6 Update `PresetRulesListView` to use dynamic data
- [x] 4.1.7 Update `PresetRuleDetailView` for UnifiedToolInfo (via toPresetRule())
- [x] 4.1.8 Update `PresetCommandCard` for UnifiedToolInfo (via toPresetRule())

### 4.2 Update CommandCompletionManager
- [x] 4.2.1 Remove `refreshFromConfig()` method (N/A - uses refreshCommands())
- [x] 4.2.2 Add `loadCommandsFromRegistry()` async method (via refreshCommands() using getRootCommandsFromRegistry)
- [x] 4.2.3 Use `core.getCommandCompletions(prefix)` for filtering (via local filterCommands())
- [x] 4.2.4 Use `core.getSubcommandCompletions(parent)` for nested (via getSubcommandsFromRegistry)
- [x] 4.2.5 Handle tools_changed notification to refresh (via .toolsDidChange notification)

### 4.3 Add Localization
- [x] 4.3.1 Add `tool.search.hint` to Localizable.strings
- [x] 4.3.2 Add `tool.search.description` to Localizable.strings
- [x] 4.3.3 Add `tool.mcp.*` keys
- [x] 4.3.4 Add `tool.skill.*` keys
- [x] 4.3.5 Add `tool.video.*` keys
- [x] 4.3.6 Add `tool.chat.*` keys
- [x] 4.3.7 Add Chinese translations to zh-Hans.lproj

## Phase 5: Integration and Cleanup

### 5.1 Config Module Cleanup
- [x] 5.1.1 Review `Config::builtin_rules()` usage (DONE - consolidated to dispatcher)
- [x] 5.1.2 Remove if no longer needed (DONE - removed, using get_builtin_routing_rules())
- [x] 5.1.3 Update `merge_builtin_rules()` if needed (DONE - uses dispatcher function)

### 5.2 Event System
- [x] 5.2.1 Add `on_tools_changed()` callback to EventHandler (Rust trait + UniFFI + Swift)
- [x] 5.2.2 Fire event when MCP server connects/disconnects (via refresh_tool_registry)
- [x] 5.2.3 Fire event when skills are installed/uninstalled (via refresh_tool_registry)
- [x] 5.2.4 Swift: Listen for tools_changed to refresh UI (.toolsDidChange notification)

### 5.3 Documentation
- [x] 5.3.1 Update CLAUDE.md with new architecture (DONE - added BUILTIN_COMMANDS and event system docs)
- [x] 5.3.2 Update API documentation (DONE - in CLAUDE.md Dispatcher Layer section)
- [x] 5.3.3 Add inline code comments

### 5.4 Testing
- [x] 5.4.1 Add unit tests for ToolRegistry builtin registration (in builtin_defs.rs)
- [ ] 5.4.2 Add unit tests for CommandRegistry query methods
- [ ] 5.4.3 Add integration tests for UniFFI APIs
- [x] 5.4.4 Manual test: Settings > Routing shows dynamic builtins
- [x] 5.4.5 Manual test: Command completion shows all tools
- [x] 5.4.6 Manual test: `/mcp ` shows MCP tools
- [x] 5.4.7 Manual test: `/skill ` shows installed skills
- [x] 5.4.8 Manual test: MCP connect/disconnect updates completion

## Dependencies

```
Phase 1 ──────────────────────┐
                              ├──► Phase 3 (needs extended UnifiedTool)
Phase 2 ──────────────────────┤
                              ├──► Phase 4 (needs UniFFI APIs)
                              ▼
                         Phase 5 (integration)
```

## Verification Checklist

After implementation, verify:

- [x] All 5 builtin commands appear in Settings > Routing > Builtin Rules
- [x] Typing `/` shows all root commands (builtins + custom)
- [x] Typing `/mcp ` shows connected MCP server tools
- [x] Typing `/skill ` shows installed skills
- [x] Connecting new MCP server adds tools to completion without restart
- [x] Installing new skill adds it to `/skill ` completion
- [x] L3 router sees all tools in prompt
- [x] No hardcoded `PresetRules` in Swift code (DELETED)
- [ ] No hardcoded `builtin_rules()` in Rust config (DEFERRED - backward compat)
- [x] Localization works for all tool hints/descriptions

## Implementation Notes

### Approach Taken
- **Minimal approach**: Kept existing structures for backward compatibility
- Added new registry-based methods as alternatives
- Swift UI uses `listBuiltinTools()` now
- UnifiedToolInfoExtension provides bridge via `toPresetRule()` method

### Completed Work (2026-01-10)
- ✅ Removed deprecated PresetRules enum from RoutingView.swift
- ✅ Added on_tools_changed() event system (Rust → UniFFI → Swift notification)
- ✅ Migrated CommandCompletionManager to use ToolRegistry as single source of truth
- ✅ Added namespace navigation for /mcp and /skill subcommands
- ✅ Auto-refresh on tool changes via .toolsDidChange notification
- ✅ Created BUILTIN_COMMANDS in dispatcher/builtin_defs.rs as single source of truth
- ✅ Removed hardcoded builtin_rules() from Config module
- ✅ Added routing config fields to UnifiedTool for builtin commands
- ✅ Updated CLAUDE.md with new architecture documentation
- ✅ Added unit tests for builtin definitions (3 tests in builtin_defs.rs)

### Flat Namespace Implementation (2026-01-10)
- ✅ **Phase 1**: Added conflict resolution system (ToolPriority, ConflictInfo, ConflictResolution)
- ✅ **Phase 2**: Flattened MCP tool registration (tools registered directly, e.g., `/git`)
- ✅ **Phase 3**: Flattened Skill registration (skills registered directly, e.g., `/refine-text`)
- ✅ **Phase 4**: Removed `/mcp` and `/skill` from BUILTIN_COMMANDS (3 builtins remain: search, video, chat)
- ✅ **Phase 5**: Updated Swift UI with source badges instead of namespace navigation
  - Updated SubPanelCommandRow to show source icon and badge
  - Updated CommandCompletionManager with sourceIcon, sourceColor, sourceBadgeText
  - Removed namespace-specific code from CommandRegistry
- ✅ Updated all unit tests for flat namespace (944 tests passing)
- ✅ Regenerated UniFFI bindings with source_type in CommandNode

### Remaining Future Work
- Add integration tests for UniFFI APIs
- Add unit tests for CommandRegistry query methods

### Skipped Phases
- ~~Phase 6: Dynamic routing rules from registry~~ - Not needed, L3 AI routing handles MCP/Skill tools
- ~~Phase 7: Backward compatibility~~ - **强制扁平化**: `/mcp` 和 `/skill` 前缀不再支持，用户必须直接使用工具名称（如 `/git` 而不是 `/mcp git`）

### Design Decision: Forced Flat Namespace
用户不应该关心工具是来自 MCP 还是 Skill，只需要知道工具名称即可。
- ✅ `/git status` - 正确
- ❌ `/mcp git status` - 不再支持
- ❌ `/skill refine-text` - 不再支持

工具来源通过 UI badges 显示，而不是命令前缀。

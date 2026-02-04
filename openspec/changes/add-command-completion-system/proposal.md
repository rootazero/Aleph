# Change: Add Command Completion System (Dual-Mode Unified Interface)

**Status**: Draft
**Author**: AI Assistant
**Created**: 2026-01-06

## Why

Current Aleph only supports natural language chat mode. Users cannot efficiently invoke structured commands (like `/draw`, `/translate`, MCP tools) without manually typing full command strings. This lacks discoverability and makes the system feel like a simple chat wrapper rather than a powerful AI middleware.

This proposal transforms Aleph into a **Semantic OS Launcher** with:
1. **Chat Mode** (default): Natural language conversation with AI
2. **Command Mode** (triggered by hotkey): Hierarchical command browsing with auto-completion

## What Changes

### New Capabilities

1. **Command Registry** (Rust Core)
   - Unified command tree structure for all features (builtins, MCP tools, user prompts)
   - Dynamic lazy-loading of child nodes (MCP clients, tool lists)
   - Platform-agnostic command data model exposed via UniFFI

2. **Halo Command Mode** (Swift UI)
   - Dual-mode input interface (Chat/Command toggle)
   - Chip-style breadcrumb navigation for command path
   - Filtered suggestion list with keyboard navigation
   - Visual mode indicator (purple border = Chat, cyan border = Command)
   - **Command hints**: Short descriptions next to each command (max 80px width)
     - Builtin hints: Localized (English/Chinese) in Localizable.strings
     - User-defined hints: Via `hint` field in config.toml routing rules
     - Toggle: "Show command hints" in General Settings (default: on)

3. **Keyboard Interceptor** (Swift UI)
   - `Cmd+Opt+/`: Force enter Command Mode
   - `Tab`: Select suggestion and drill down
   - `Backspace` (on empty input): Pop path stack, go up one level
   - `Escape`: Exit Command Mode, return to Chat Mode
   - `Enter`: Execute selected command

### Modified Capabilities

- **HaloState**: Add new `.commandMode(...)` state with path stack and suggestions
- **HaloView**: Extend to render command mode UI with breadcrumbs
- **HaloWindow**: Handle command mode interaction (disable `ignoresMouseEvents` for command mode)

## Impact

- **Affected specs**:
  - `command-registry` (NEW)
  - `halo-command-mode` (NEW)
  - Will extend existing `ai-routing` spec in future phases

- **Affected code**:
  - `Aleph/core/src/` - New `command/` module
  - `Aleph/core/src/aleph.udl` - New UniFFI exports
  - `Aleph/core/src/config/mod.rs` - Add `hint` field to RoutingRuleConfig, `show_command_hints` to GeneralConfig
  - `Aleph/Sources/HaloState.swift` - New command mode state
  - `Aleph/Sources/HaloView.swift` - Command mode UI
  - `Aleph/Sources/Components/` - New command mode components
  - `Aleph/Resources/en.lproj/Localizable.strings` - Builtin hint translations
  - `Aleph/Resources/zh-Hans.lproj/Localizable.strings` - Builtin hint translations

- **Breaking changes**: None (additive feature)

## Design Decisions

See `design.md` for detailed technical architecture.

## Success Criteria

1. User can toggle between Chat and Command modes seamlessly
2. Command suggestions load within 50ms
3. Tab/Backspace navigation feels responsive (<16ms frame time)
4. MCP tools discoverable through hierarchical browsing
5. Existing Chat mode functionality unchanged
6. Command hints display correctly with 80px max width truncation
7. Builtin hints show in correct language (English/Chinese)
8. User can toggle hint visibility in Settings

# Tasks: Add Command Completion System

## Phase 1: Rust Core - Command Registry

### 1.1 Data Structures
- [ ] 1.1.1 Create `Aether/core/src/command/mod.rs` module
- [ ] 1.1.2 Define `CommandType` enum (Action, Prompt, Namespace)
- [ ] 1.1.3 Define `CommandNode` struct with all fields (including `hint: Option<String>`)
- [ ] 1.1.4 Define `CommandExecutionResult` for command outcomes
- [ ] 1.1.5 Add unit tests for data structures

### 1.2 Command Registry Implementation
- [ ] 1.2.1 Create `CommandRegistry` struct to manage command tree
- [ ] 1.2.2 Implement `get_root_commands()` - returns top-level commands
- [ ] 1.2.3 Implement `get_children(parent_key)` - returns children of a node
- [ ] 1.2.4 Implement `filter_by_prefix(nodes, prefix)` - filters suggestions
- [ ] 1.2.5 Add unit tests for registry operations

### 1.3 Builtin Command Loading
- [ ] 1.3.1 Parse config.toml `[[rules]]` with `^/` prefix into CommandNodes
- [ ] 1.3.2 Extract icon from rule metadata (add `icon` field to RoutingRuleConfig)
- [ ] 1.3.3 Extract hint from rule metadata (add `hint` field to RoutingRuleConfig)
- [ ] 1.3.4 Map rule capabilities to command descriptions
- [ ] 1.3.5 Add integration tests with sample config

### 1.4 Builtin Hint Localization
- [ ] 1.4.1 Define builtin hint keys mapping (command key → hint i18n key)
- [ ] 1.4.2 Implement hint lookup with language fallback
- [ ] 1.4.3 Add `show_command_hints` to GeneralConfig
- [ ] 1.4.4 Unit tests for hint localization

### 1.5 MCP Command Integration (Stub)
- [ ] 1.5.1 Create `McpCommandSource` trait for MCP tool discovery
- [ ] 1.5.2 Implement stub `MockMcpSource` for testing
- [ ] 1.5.3 Add `/mcp` namespace node to root commands
- [ ] 1.5.4 Define interface for future MCP client integration

## Phase 2: UniFFI Bridge

### 2.1 Type Exports
- [ ] 2.1.1 Add `CommandType` enum to `aether.udl`
- [ ] 2.1.2 Add `CommandNode` dictionary to `aether.udl`
- [ ] 2.1.3 Add `CommandExecutionResult` dictionary to `aether.udl`
- [ ] 2.1.4 Generate Swift bindings with `uniffi-bindgen`

### 2.2 API Methods
- [ ] 2.2.1 Add `get_root_commands()` to AetherCore interface
- [ ] 2.2.2 Add `get_command_children(parent_key)` to AetherCore interface
- [ ] 2.2.3 Add `execute_command(path, argument)` to AetherCore interface
- [ ] 2.2.4 Add `filter_commands(nodes, prefix)` helper method
- [ ] 2.2.5 Add `get_show_command_hints()` / `set_show_command_hints()` methods
- [ ] 2.2.6 Rebuild `libaethecore.dylib` with new exports

## Phase 3: Swift UI - Command Mode State

### 3.1 State Management
- [ ] 3.1.1 Create `CommandSession` class (ObservableObject)
- [ ] 3.1.2 Add `pathStack: [CommandNode]` property
- [ ] 3.1.3 Add `currentInput: String` property
- [ ] 3.1.4 Add `suggestions: [CommandNode]` property
- [ ] 3.1.5 Add `selectedIndex: Int` property
- [ ] 3.1.6 Implement `pushNode()`, `popNode()` navigation methods

### 3.2 HaloState Extension
- [ ] 3.2.1 Add `.commandMode(session: CommandSession)` case to HaloState
- [ ] 3.2.2 Update `HaloState` Equatable conformance
- [ ] 3.2.3 Update `HaloView` switch statement for command mode

## Phase 4: Swift UI - Command Mode Components

### 4.1 BreadcrumbView
- [ ] 4.1.1 Create `BreadcrumbChip` view (icon + label + background)
- [ ] 4.1.2 Create `BreadcrumbView` HStack of chips
- [ ] 4.1.3 Add delete button on each chip (optional, for click-to-pop)
- [ ] 4.1.4 Style with cyan accent color

### 4.2 SuggestionListView
- [ ] 4.2.1 Create `SuggestionRow` view (icon + key + hint + arrow)
- [ ] 4.2.2 Create `SuggestionListView` with keyboard selection
- [ ] 4.2.3 Implement highlight style for selected row
- [ ] 4.2.4 Add loading state for async children fetch
- [ ] 4.2.5 Add empty state ("No matching commands")

### 4.3 Hint Display
- [ ] 4.3.1 Add hint label with 80px max width and ellipsis truncation
- [ ] 4.3.2 Style hint with secondary color and smaller font (11pt)
- [ ] 4.3.3 Conditionally hide hint based on `show_command_hints` setting
- [ ] 4.3.4 Add VoiceOver accessibility label including hint

### 4.4 CommandModeView
- [ ] 4.4.1 Create `CommandModeView` composing Breadcrumb + Input + Suggestions
- [ ] 4.4.2 Apply cyan border and command mode styling
- [ ] 4.4.3 Add mode indicator icon (laptop/terminal)
- [ ] 4.4.4 Integrate with CommandSession for state

### 4.5 Input Field
- [ ] 4.5.1 Create `CommandInputField` with NSViewRepresentable
- [ ] 4.5.2 Implement prefix filtering on text change
- [ ] 4.5.3 Style cursor and placeholder text

## Phase 5: Keyboard Interceptor

### 5.1 Mode Switching
- [ ] 5.1.1 Register `Cmd+Opt+/` as command mode trigger
- [ ] 5.1.2 Implement mode toggle in AppDelegate or EventHandler
- [ ] 5.1.3 Clear path stack on mode enter
- [ ] 5.1.4 Fetch and display root commands on mode enter

### 5.2 Navigation Keys
- [ ] 5.2.1 Handle `Tab` key for suggestion selection
- [ ] 5.2.2 Handle `Backspace` for path pop (when input empty)
- [ ] 5.2.3 Handle `Enter` for command execution
- [ ] 5.2.4 Handle `Escape` to exit command mode
- [ ] 5.2.5 Handle `↑/↓` arrow keys for suggestion navigation

### 5.3 Input Handling
- [ ] 5.3.1 Forward character input to CommandInputField
- [ ] 5.3.2 Update suggestions on input change
- [ ] 5.3.3 Reset selected index on input change

## Phase 6: Integration & Polish

### 6.1 HaloWindow Updates
- [ ] 6.1.1 Enable mouse events for command mode (`ignoresMouseEvents = false`)
- [ ] 6.1.2 Add dynamic sizing for command mode (wider for suggestions)
- [ ] 6.1.3 Position window appropriately for command UI

### 6.2 Visual Polish
- [ ] 6.2.1 Add transition animation between Chat/Command modes
- [ ] 6.2.2 Implement smooth suggestion list scrolling
- [ ] 6.2.3 Add hover effects for clickable elements
- [ ] 6.2.4 Ensure accessibility labels for VoiceOver

### 6.3 Hint Localization (i18n)
- [ ] 6.3.1 Add builtin hint keys to `en.lproj/Localizable.strings`
- [ ] 6.3.2 Add builtin hint keys to `zh-Hans.lproj/Localizable.strings`
- [ ] 6.3.3 Implement Swift helper to fetch localized hint for builtin commands

### 6.4 Settings UI for Hints
- [ ] 6.4.1 Add "Show command hints" toggle to General Settings view
- [ ] 6.4.2 Add "Hint" input field to routing rule edit panel (for user commands)
- [ ] 6.4.3 Connect toggle to `show_command_hints` config setting
- [ ] 6.4.4 Limit hint input to reasonable length (50 chars max)

### 6.5 Error Handling
- [ ] 6.5.1 Handle empty command registry gracefully
- [ ] 6.5.2 Handle MCP connection errors (future)
- [ ] 6.5.3 Add toast notification for command execution errors

## Phase 7: Testing

### 7.1 Unit Tests
- [ ] 7.1.1 Test CommandRegistry with mock config
- [ ] 7.1.2 Test prefix filtering logic
- [ ] 7.1.3 Test path stack navigation
- [ ] 7.1.4 Test hint localization fallback logic

### 7.2 Integration Tests
- [ ] 7.2.1 Test UniFFI bridge for command methods
- [ ] 7.2.2 Test command mode state transitions
- [ ] 7.2.3 Test hint visibility toggle setting

### 7.3 Manual Testing
- [ ] 7.3.1 Verify Cmd+Opt+/ triggers command mode
- [ ] 7.3.2 Verify keyboard navigation (Tab, Backspace, Enter, Escape, Arrows)
- [ ] 7.3.3 Verify command execution for builtin commands
- [ ] 7.3.4 Verify visual styling and animations
- [ ] 7.3.5 Test on multiple displays and screen positions
- [ ] 7.3.6 Verify hints display correctly with truncation
- [ ] 7.3.7 Verify hints toggle works in Settings
- [ ] 7.3.8 Verify hint localization in Chinese and English

## Phase 8: Documentation

### 8.1 User Documentation
- [ ] 8.1.1 Add command mode section to README or user guide
- [ ] 8.1.2 Document keyboard shortcuts
- [ ] 8.1.3 Add screenshots/GIFs of command mode

### 8.2 Developer Documentation
- [ ] 8.2.1 Document CommandRegistry API in code comments
- [ ] 8.2.2 Update CLAUDE.md with command mode architecture
- [ ] 8.2.3 Add inline documentation for new SwiftUI components

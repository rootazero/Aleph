# Design: Command Completion System

## Context

Aleph currently operates in a single-mode paradigm where users type natural language, which is routed to AI providers. While effective for chat, this model lacks:

1. **Discoverability**: Users must know command syntax (e.g., `/draw`, `/en`) in advance
2. **Hierarchy**: No way to browse nested commands (MCP tools, skills)
3. **Efficiency**: Full typing required even for frequently used commands

### Constraints

- Must not steal focus from active application (Halo design principle)
- Must coexist with Chat mode without interference
- Rust Core handles all business logic; Swift UI is rendering only
- UniFFI boundary limits data structures (no closures, simple types)

## Goals / Non-Goals

### Goals

1. Provide hierarchical command browsing with auto-completion
2. Support three command sources: Builtins, MCP Tools, User Prompts
3. Enable keyboard-only navigation (Tab, Backspace, Enter, Escape)
4. Visual distinction between Chat and Command modes
5. Sub-100ms response for command suggestions

### Non-Goals

1. Fuzzy search across all commands (future enhancement)
2. Command history/recent commands (future enhancement)
3. Custom command creation via UI (use config.toml)
4. Touch/mouse-first interaction (keyboard-first design)

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        Swift UI Layer                            │
├─────────────────────────────────────────────────────────────────┤
│  HaloWindow                                                      │
│  ├── HaloView (mode switching based on HaloState)                │
│  │   ├── ChatModeView (existing)                                 │
│  │   └── CommandModeView (new)                                   │
│  │       ├── BreadcrumbView (chip-style path)                    │
│  │       ├── InputField (filtered input)                         │
│  │       └── SuggestionList (keyboard navigable)                 │
│  └── KeyboardInterceptor (handles mode-specific keys)            │
├─────────────────────────────────────────────────────────────────┤
│                     UniFFI Bridge                                │
├─────────────────────────────────────────────────────────────────┤
│                      Rust Core Layer                             │
├─────────────────────────────────────────────────────────────────┤
│  CommandRegistry                                                 │
│  ├── BuiltinCommands (static, from config.toml rules)            │
│  ├── McpCommands (dynamic, from connected MCP clients)           │
│  └── PromptCommands (static, from config.toml rules)             │
│                                                                  │
│  CommandNode (tree structure)                                    │
│  ├── key: String                                                 │
│  ├── description: String                                         │
│  ├── icon: String (SF Symbol name)                               │
│  ├── node_type: CommandType (Action | Prompt | Namespace)        │
│  ├── has_children: bool                                          │
│  └── source_id: Option<String> (for dynamic loading)             │
└─────────────────────────────────────────────────────────────────┘
```

## Decisions

### Decision 1: Mode Switching via Explicit Hotkey

**What**: Command Mode is ONLY entered via `Cmd+Opt+/` hotkey, not by typing `/`

**Why**:
- Prevents accidental mode switches during normal typing
- Clear physical feedback (muscle memory)
- Allows `/` character in Chat mode without triggering commands

**Alternative considered**: Auto-detect `/` at start of input
- Rejected: Too easy to accidentally trigger; confuses users who want literal `/`

### Decision 2: Rust Core Owns Command Tree

**What**: All command tree logic lives in Rust; Swift only renders

**Why**:
- Consistent with Aleph's architecture (no business logic in Swift)
- Enables future cross-platform support (Windows/Linux)
- Single source of truth for command availability

**Alternative considered**: Swift-side command filtering
- Rejected: Duplicates logic; harder to maintain consistency

### Decision 3: Lazy Loading for Dynamic Commands

**What**: MCP tool lists are fetched on-demand when user navigates to that namespace

**Why**:
- Avoids startup delay from querying all MCP servers
- Allows MCP connections to be established lazily
- Reduces memory footprint for unused commands

**Implementation**:
```rust
// When user selects "mcp" namespace
fn fetch_children(&self, node: &CommandNode) -> Vec<CommandNode> {
    match &node.source_id {
        Some(id) if id.starts_with("mcp:") => {
            // Query connected MCP client for available tools
            self.mcp_manager.list_tools(id)
        }
        _ => self.static_children.get(&node.key).unwrap_or_default()
    }
}
```

### Decision 4: Chip-Style Breadcrumbs (Not Plain Text)

**What**: Each path segment is rendered as a visual "chip" with icon + label

**Why**:
- Clear visual separation between confirmed path and pending input
- Easier to understand current context at a glance
- Consistent with modern launcher UIs (Raycast, Alfred)

**Visual Design**:
```
┌─────────────────────────────────────────────┐
│  [ mcp] [🔧 git] █commit message here       │
│    ↑        ↑         ↑                     │
│   chip    chip    cursor + input            │
└─────────────────────────────────────────────┘
```

### Decision 5: CommandType Enum for Node Behavior

**What**: Three types determining what happens on selection

| Type      | On Tab/Enter          | Icon Style      |
|-----------|----------------------|-----------------|
| Namespace | Push to path stack   | Folder + arrow  |
| Action    | Execute immediately  | Bolt/play icon  |
| Prompt    | Load system prompt   | Text icon       |

**Why**:
- Clear mental model for users
- Simplifies UI logic (behavior based on type)
- Extensible for future command types

### Decision 6: Command Hint Display Strategy

**What**: Each command displays a short hint (备注) after the key label, with fixed pixel width control.

**Why**:
- Improves discoverability without cluttering the UI
- Helps users understand command purpose at a glance
- Optional feature controllable via settings

**Width Control Strategy**:

| Approach | Pros | Cons |
|----------|------|------|
| Character limit (e.g., 4 Chinese, 8 English) | Simple | Inconsistent visual width |
| Word count limit | English-friendly | Doesn't work for CJK |
| **Fixed pixel width (80px) + ellipsis** | Consistent visual, adaptive | Requires UI truncation |

**Chosen**: **Fixed pixel width (80px)** with `.truncationMode(.tail)` in SwiftUI.

**Implementation**:
```swift
Text(hint)
    .font(.system(size: 11))
    .foregroundColor(.secondary)
    .lineLimit(1)
    .truncationMode(.tail)
    .frame(maxWidth: 80, alignment: .leading)
```

**Hint Sources**:

| Command Type | Hint Source | Localization |
|--------------|-------------|--------------|
| Builtin | Hardcoded in code | Yes (Localizable.strings) |
| User-defined | `hint` field in config.toml | No (user's language) |
| MCP Tools | From MCP tool description | No (server-provided) |

**Settings Toggle**:
- Location: General Settings → "Show command hints"
- Default: Enabled
- Stored in: `config.toml` → `[general]` → `show_command_hints = true`

### Decision 7: Localized Builtin Hints

**What**: Builtin command hints are hardcoded with localization keys.

**Builtin Hints Table**:

| Command | English | 简体中文 |
|---------|---------|---------|
| search | Web search | 网页搜索 |
| en | To English | 译成英文 |
| zh | To Chinese | 译成中文 |
| draw | Gen image | 生成图片 |
| mcp | MCP tools | MCP工具 |
| code | Code help | 代码助手 |

**Why**:
- Consistent experience for international users
- No burden on users to translate system commands
- Follows Aleph's existing i18n pattern (Localizable.strings)

## Data Structures

### UniFFI Types (Rust → Swift)

```rust
// In aleph.udl

enum CommandType {
    "Action",     // Execute immediately
    "Prompt",     // Load system prompt, then await user input
    "Namespace",  // Container, has children
};

dictionary CommandNode {
    string key;
    string description;
    string icon;
    string? hint;           // Short hint for display (max ~80px width)
    CommandType node_type;
    boolean has_children;
    string? source_id;
};

// Extended AlephCore interface
interface AlephCore {
    // ... existing methods ...

    // Command Registry API
    sequence<CommandNode> get_root_commands();
    sequence<CommandNode> get_children(string parent_key);
    CommandExecutionResult execute_command(string command_path, string? argument);

    // Settings API for hint visibility
    boolean get_show_command_hints();
    void set_show_command_hints(boolean enabled);
};
```

### Swift State Model

```swift
// CommandSession manages command mode state
class CommandSession: ObservableObject {
    @Published var pathStack: [CommandNode] = []
    @Published var currentInput: String = ""
    @Published var suggestions: [CommandNode] = []
    @Published var selectedIndex: Int = 0

    var currentPath: String {
        pathStack.map { $0.key }.joined(separator: "/")
    }
}

// Extended HaloState
enum HaloState {
    // ... existing cases ...
    case commandMode(session: CommandSession)
}
```

## Keyboard Interaction Flow

```
┌──────────────────────────────────────────────────────────────┐
│                      Keyboard Events                          │
├──────────────────────────────────────────────────────────────┤
│  Cmd+Opt+/  →  Enter Command Mode (clear path, show root)    │
│  Escape     →  Exit Command Mode (return to Chat/Idle)       │
│  Tab        →  Select highlighted suggestion, push to path   │
│  Enter      →  Execute if Action/Prompt, else same as Tab    │
│  Backspace  →  (empty input) Pop path stack, go up one level │
│              →  (has input) Delete last character            │
│  ↑ / ↓      →  Navigate suggestion list                      │
│  Any char   →  Filter suggestions by prefix                  │
└──────────────────────────────────────────────────────────────┘
```

## Command Sources

### 1. Builtin Commands (Static)

Derived from `config.toml` rules with `^/` prefix:

```toml
[[rules]]
regex = "^/search"
provider = "openai"
capabilities = ["search"]
system_prompt = "Search the web and provide answers."
```

Generates:
```
CommandNode {
    key: "search",
    description: "Web search",
    icon: "magnifyingglass",
    node_type: Action,
    has_children: false,
    source_id: Some("builtin:search"),
}
```

### 2. MCP Commands (Dynamic)

Structure:
```
/mcp                    → Namespace (lists connected MCP clients)
/mcp/git                → Namespace (lists git tools)
/mcp/git/commit         → Action (executes git_commit tool)
/mcp/git/commit <msg>   → Action with argument
```

MCP tools discovered via `mcp_client.list_tools()` at runtime.

### 3. Prompt Commands (Static)

User-defined prompts from `config.toml`:

```toml
[[rules]]
regex = "^/en"
provider = "openai"
system_prompt = "Translate to English."
icon = "globe"
hint = "译英文"   # User-defined hint (displayed in command mode)
```

Generates:
```
CommandNode {
    key: "en",
    description: "Translate to English",
    icon: "globe",
    hint: Some("译英文"),  # From config, no auto-localization
    node_type: Prompt,
    has_children: false,
    source_id: Some("prompt:en"),
}
```

**Note**: User-defined `hint` in config.toml is used as-is (no localization). Users should write hints in their preferred language.

## Visual Design

### Mode Indicators

| Mode    | Border Color | Icon        | Background      |
|---------|-------------|-------------|-----------------|
| Chat    | Purple      | ✨ (sparkle)| Frosted glass   |
| Command | Cyan        | 💻 (laptop) | Frosted glass   |

### Suggestion List

```
┌──────────────────────────────────────────────────┐
│ 🔍 search   网页搜索                           → │  ← selected (highlighted)
│ 🌐 en       译成英文                             │
│ 🎨 draw     生成图片                           → │
│ 🔧 mcp      MCP工具                            → │
└──────────────────────────────────────────────────┘
     ↑          ↑         ↑                      ↑
    icon      label     hint (80px max)     has_children
```

**Hint Display Rules**:
- Hint appears after label, styled as secondary text (gray, smaller font)
- Width capped at 80px, overflow truncated with "..."
- Hidden entirely if `show_command_hints = false` in settings
- For builtins: uses localized string from Localizable.strings
- For user commands: uses `hint` field from config.toml rule

## Risks / Trade-offs

### Risk 1: MCP Connection Latency

**Problem**: First access to `/mcp/xxx` may be slow if MCP client not connected

**Mitigation**:
- Show loading spinner in suggestion list
- Cache tool lists after first fetch
- Pre-warm connections for frequently used MCP servers

### Risk 2: Command Namespace Collisions

**Problem**: User prompt `/git` conflicts with MCP tool namespace `/mcp/git`

**Mitigation**:
- Builtins and user prompts are top-level
- MCP tools always under `/mcp/` namespace
- Clear documentation of namespace hierarchy

### Risk 3: UniFFI Callback Limitations

**Problem**: Can't use closures for dynamic behavior across FFI

**Mitigation**:
- Use polling model: Swift calls `get_children(path)` synchronously
- Async operations return immediately, Swift polls for completion
- Keep state in Rust, Swift only renders

## Migration Plan

1. **Phase 1**: Implement CommandRegistry in Rust (no UI changes)
2. **Phase 2**: Add UniFFI exports for command API
3. **Phase 3**: Create CommandModeView in Swift
4. **Phase 4**: Integrate with HaloWindow and keyboard handling
5. **Phase 5**: Connect MCP tool discovery (requires MCP client implementation)

Phases 1-4 can ship independently of Phase 5 (MCP).

## Open Questions

1. **Q**: Should command history be persisted across sessions?
   **A**: Defer to future enhancement. Start with no history.

2. **Q**: How to handle commands that require multi-field input (e.g., `git commit -m "message"`)?
   **A**: For MVP, show argument hint after command selection. Complex forms are future work.

3. **Q**: Should suggestions be fuzzy-matched or prefix-only?
   **A**: Prefix-only for MVP. Fuzzy matching is future enhancement.

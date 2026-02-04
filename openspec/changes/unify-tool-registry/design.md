# Design: Unify Tool Registry as Single Source of Truth

## Context

Aleph has evolved with multiple independent systems for managing commands and tools:

1. **ToolRegistry** (dispatcher/registry.rs) - Aggregates tools for L3 intent detection
2. **CommandRegistry** (command/registry.rs) - Provides command completion
3. **Config::builtin_rules()** (config/mod.rs) - Hardcodes builtin slash commands
4. **PresetRules.all** (Swift UI) - Hardcodes preset rules for Settings UI

These systems evolved separately and have divergent data:
- ToolRegistry knows about MCP/Skills but not builtin commands
- CommandRegistry parses config rules but doesn't know about MCP tools
- PresetRules is completely static and can't show MCP/Skills

This design unifies all sources into ToolRegistry as the single source of truth.

## Goals

1. **Single Source of Truth**: ToolRegistry contains all available commands/tools
2. **Dynamic Updates**: MCP connects → tools appear in completion and UI
3. **Consistent Data**: Same metadata across all consumers (Router, Completion, UI)
4. **Backward Compatible**: Existing APIs continue to work
5. **Localization Support**: Tool metadata supports i18n

## Non-Goals

1. **Change config format**: User-defined rules remain in config.toml
2. **Replace Router**: Router still does regex matching, just uses registry for fallback
3. **Real-time config sync**: Config changes still require hot-reload
4. **Persistent tool state**: Tool active/inactive state is runtime-only

## Decisions

### Decision 1: ToolSource Variant for Builtin Commands

**Options:**
- A) Use `ToolSource::Native` for all builtin commands
- B) Create `ToolSource::Builtin` variant
- C) Use `ToolSource::Custom` with special flag

**Decision: Option B** - Create `ToolSource::Builtin` variant

**Rationale:**
- Distinguishes system commands (`/chat`) from capabilities (`/search`)
- Native = capabilities with execution logic (Search, Video)
- Builtin = system commands without special capability execution
- Custom = user-defined rules from config.toml
- Clear separation enables different UI treatments

**Updated ToolSource:**
```rust
pub enum ToolSource {
    Native,                           // Search, Video (has capability)
    Builtin,                          // /chat, /mcp, /skill (system commands)
    Mcp { server: String },           // MCP server tools
    Skill { id: String },             // Claude Agent Skills
    Custom { rule_index: usize },     // User-defined rules
}
```

### Decision 2: UnifiedTool UI Metadata Extension

**Options:**
- A) Add all UI fields directly to UnifiedTool
- B) Create separate UIMetadata struct
- C) Use generic metadata HashMap

**Decision: Option A** - Add fields directly to UnifiedTool

**Rationale:**
- UnifiedTool is already the primary data structure
- All consumers need the same metadata
- Avoids additional indirection
- Fields are all optional (backward compatible)

**Extended UnifiedTool:**
```rust
pub struct UnifiedTool {
    // Existing fields
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub source: ToolSource,
    pub parameters_schema: Option<Value>,
    pub is_active: bool,
    pub requires_confirmation: bool,
    pub service_name: Option<String>,

    // NEW: UI Metadata
    pub icon: Option<String>,           // SF Symbol name (e.g., "magnifyingglass")
    pub usage: Option<String>,          // Usage example (e.g., "/search <query>")
    pub subtools: Vec<String>,          // IDs of nested tools (for /mcp, /skill)
    pub localization_key: Option<String>, // Key for i18n lookup
    pub is_builtin: bool,               // Quick check for builtin status
    pub sort_order: i32,                // Display order (lower = first)
}
```

### Decision 3: Builtin Tools Registration

**Options:**
- A) Hardcode in `register_native_tools()`
- B) Load from embedded JSON/TOML
- C) Define in code with builder pattern

**Decision: Option C** - Define in code with builder pattern

**Rationale:**
- Compile-time type safety
- Easy to add/remove builtins
- Builder pattern makes intent clear
- No external file parsing needed

**Implementation:**
```rust
impl ToolRegistry {
    pub async fn register_builtin_tools(&self) {
        let builtins = vec![
            UnifiedTool::builtin("search")
                .with_display_name("Web Search")
                .with_description("Search the web for real-time information")
                .with_icon("magnifyingglass")
                .with_usage("/search <query>")
                .with_localization_key("tool.search")
                .with_sort_order(1)
                .build(),

            UnifiedTool::builtin("mcp")
                .with_display_name("MCP Tools")
                .with_description("Invoke Model Context Protocol tools")
                .with_icon("puzzlepiece.extension")
                .with_usage("/mcp <tool> [params]")
                .with_localization_key("tool.mcp")
                .with_sort_order(2)
                .with_has_subtools(true)  // Dynamic subtools from MCP servers
                .build(),

            UnifiedTool::builtin("skill")
                .with_display_name("Skills")
                .with_description("Execute predefined skill workflows")
                .with_icon("wand.and.stars")
                .with_usage("/skill <name>")
                .with_localization_key("tool.skill")
                .with_sort_order(3)
                .with_has_subtools(true)  // Dynamic subtools from installed skills
                .build(),

            UnifiedTool::builtin("video")
                .with_display_name("Video Transcript")
                .with_description("Analyze YouTube video content")
                .with_icon("play.rectangle")
                .with_usage("/video <YouTube URL>")
                .with_localization_key("tool.video")
                .with_sort_order(4)
                .build(),

            UnifiedTool::builtin("chat")
                .with_display_name("Chat")
                .with_description("Start a multi-turn conversation")
                .with_icon("bubble.left.and.bubble.right")
                .with_usage("/chat <message>")
                .with_localization_key("tool.chat")
                .with_sort_order(5)
                .build(),
        ];

        let mut tools = self.tools.write().await;
        for tool in builtins {
            tools.insert(tool.id.clone(), tool);
        }
    }
}
```

### Decision 4: CommandRegistry Refactoring

**Options:**
- A) Remove CommandRegistry entirely, use ToolRegistry directly
- B) CommandRegistry becomes a thin wrapper over ToolRegistry
- C) CommandRegistry queries ToolRegistry for data

**Decision: Option C** - CommandRegistry queries ToolRegistry

**Rationale:**
- Preserves existing CommandRegistry API (backward compatible)
- CommandRegistry adds command-specific logic (path parsing, execution)
- ToolRegistry remains focused on tool storage and query
- Clear separation of concerns

**Refactored CommandRegistry:**
```rust
pub struct CommandRegistry {
    tool_registry: Arc<ToolRegistry>,
    language: String,
    show_hints: bool,
}

impl CommandRegistry {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            tool_registry,
            language: "en".to_string(),
            show_hints: true,
        }
    }

    /// Get root-level commands from ToolRegistry
    pub async fn get_root_commands(&self) -> Vec<CommandNode> {
        let tools = self.tool_registry.list_all().await;

        tools.iter()
            .filter(|t| !t.id.contains(':') || t.is_builtin)  // Root = no namespace or builtin
            .map(|t| self.tool_to_command_node(t))
            .collect()
    }

    /// Get children of a namespace (e.g., /mcp -> list MCP tools)
    pub async fn get_children(&self, parent_key: &str) -> Vec<CommandNode> {
        match parent_key {
            "mcp" => {
                self.tool_registry.list_by_source_type("Mcp").await
                    .iter()
                    .map(|t| self.tool_to_command_node(t))
                    .collect()
            }
            "skill" => {
                self.tool_registry.list_by_source_type("Skill").await
                    .iter()
                    .map(|t| self.tool_to_command_node(t))
                    .collect()
            }
            _ => Vec::new()
        }
    }

    fn tool_to_command_node(&self, tool: &UnifiedTool) -> CommandNode {
        let node_type = if tool.subtools.is_empty() {
            CommandType::Action
        } else {
            CommandType::Namespace
        };

        let mut node = CommandNode::new(
            &tool.name,
            &tool.description,
            node_type,
        );

        if let Some(icon) = &tool.icon {
            node = node.with_icon(icon);
        }

        if self.show_hints {
            if let Some(key) = &tool.localization_key {
                let hint = self.get_localized_hint(key);
                node = node.with_hint(hint);
            }
        }

        node
    }
}
```

### Decision 5: UniFFI API Design

**Options:**
- A) Expose ToolRegistry directly via UniFFI
- B) Create dedicated API methods on AlephCore
- C) Create separate ToolAPI interface

**Decision: Option B** - Create dedicated API methods on AlephCore

**Rationale:**
- AlephCore is the existing entry point for all Swift calls
- Keeps UniFFI interface simple
- Can apply business logic before returning data

**New UniFFI Methods:**
```rust
// In aleph.udl
interface AlephCore {
    // ... existing methods ...

    // Tool Registry APIs
    [Async]
    sequence<UnifiedToolInfo> list_builtin_tools();

    [Async]
    sequence<UnifiedToolInfo> list_all_tools();

    [Async]
    sequence<UnifiedToolInfo> list_tools_by_source(ToolSourceType source_type);

    [Async]
    sequence<CommandNode> get_command_completions(string prefix);

    [Async]
    sequence<CommandNode> get_subcommand_completions(string parent_key);

    [Async]
    UnifiedToolInfo? get_tool_metadata(string tool_id);
}

// Export types for Swift
dictionary UnifiedToolInfo {
    string id;
    string name;
    string display_name;
    string description;
    ToolSourceType source_type;
    string? icon;
    string? usage;
    boolean is_builtin;
    boolean has_subtools;
    i32 sort_order;
};

enum ToolSourceType {
    "Native",
    "Builtin",
    "Mcp",
    "Skill",
    "Custom",
};
```

### Decision 6: Localization Strategy

**Options:**
- A) Embed all translations in Rust code
- B) Use localization keys, translate in Swift
- C) Pass language to Rust, return translated strings

**Decision: Option B** - Use localization keys, translate in Swift

**Rationale:**
- Swift already has `LocalizedStringKey` infrastructure
- Rust doesn't need to know about all languages
- Consistent with existing Aleph localization pattern
- Keys like `tool.search.hint` map to Localizable.strings

**Implementation:**
```rust
// Rust returns localization keys
pub struct UnifiedTool {
    pub localization_key: Option<String>,  // e.g., "tool.search"
    // ...
}

// Swift uses the key to get localized string
extension UnifiedToolInfo {
    var localizedHint: String {
        guard let key = localization_key else { return description }
        return NSLocalizedString("\(key).hint", comment: "")
    }

    var localizedDescription: String {
        guard let key = localization_key else { return description }
        return NSLocalizedString("\(key).description", comment: "")
    }
}
```

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                         ToolRegistry                                 │
│                   (Single Source of Truth)                           │
│                                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────┐│
│  │ Builtin  │  │  Native  │  │   MCP    │  │  Skill   │  │ Custom ││
│  │ /search  │  │ search   │  │ fs:read  │  │ refine   │  │ /en    ││
│  │ /mcp     │  │ video    │  │ git:stat │  │ code-rev │  │ /zh    ││
│  │ /skill   │  │          │  │ ...      │  │ ...      │  │ ...    ││
│  │ /video   │  │          │  │          │  │          │  │        ││
│  │ /chat    │  │          │  │          │  │          │  │        ││
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘  └────────┘│
└───────────────────────────┬─────────────────────────────────────────┘
                            │
         ┌──────────────────┼──────────────────┐
         │                  │                  │
         ▼                  ▼                  ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│ CommandRegistry │ │  L3 Router      │ │   Swift UI      │
│ (Completion)    │ │ (Intent)        │ │  (Settings)     │
│                 │ │                 │ │                 │
│ get_root_cmds() │ │ to_prompt_block │ │ list_builtin()  │
│ get_children()  │ │ get_all_tools() │ │ list_all()      │
│ filter_prefix() │ │                 │ │ get_metadata()  │
└─────────────────┘ └─────────────────┘ └─────────────────┘
```

## Data Flow

### Initialization Flow

```
AlephCore::init()
    │
    ├── Config::load()
    │       └── Parse user-defined [[rules]]
    │
    └── ToolRegistry::refresh_all()
            ├── register_builtin_tools()      // /search, /mcp, /skill, /video, /chat
            ├── register_native_tools()       // search, video (capabilities)
            ├── register_mcp_tools()          // From McpClient
            ├── register_skills()             // From SkillRegistry
            └── register_custom_commands()    // From config.rules
```

### Command Completion Flow

```
User types "/" in input
    │
    ├── Swift: CommandCompletionManager.refreshCommands()
    │
    ├── UniFFI: core.get_command_completions("/")
    │
    ├── Rust: CommandRegistry::get_root_commands()
    │       └── ToolRegistry::list_all().filter(is_root)
    │
    └── Swift: Display completion list
            ├── /search (magnifyingglass) Web Search
            ├── /mcp (puzzlepiece) MCP Tools  >
            ├── /skill (wand) Skills  >
            ├── /video (play) Video Transcript
            ├── /chat (bubble) Chat
            ├── /en (globe) Translate to English
            └── /zh (globe) 译中文
```

### Settings UI Flow

```
User opens Settings > Routing
    │
    ├── Swift: RoutingView.onAppear()
    │
    ├── UniFFI: core.list_builtin_tools()
    │
    ├── Rust: ToolRegistry::list_builtin()
    │       └── filter(|t| t.is_builtin)
    │
    └── Swift: Display builtin rules section
            ├── Builtin Commands (5)
            │   ├── /search - Web Search
            │   ├── /mcp - MCP Tools
            │   └── ...
            │
            └── User Commands (dynamic)
                ├── /en - Translate to English
                └── /zh - 译中文
```

### MCP Dynamic Update Flow

```
McpClient connects to new server
    │
    ├── McpClient::list_tools() returns tools
    │
    ├── ToolRegistry::register_mcp_tools(tools)
    │
    ├── EventHandler::on_tools_changed()
    │
    └── Swift: CommandCompletionManager.refreshCommands()
            └── New MCP tools now appear in completion
```

## Risks / Trade-offs

### Risk 1: Performance - Async ToolRegistry Queries

**Problem**: ToolRegistry uses `Arc<RwLock>`, async queries add latency
**Mitigation**:
- Cache command list in CommandCompletionManager
- Refresh only on tools_changed event
- Use RwLock for concurrent reads

### Risk 2: Data Duplication - Builtin vs Native

**Problem**: `/search` (Builtin) and `search` (Native) are different entries
**Mitigation**:
- Builtin = user-facing command (`/search`)
- Native = capability execution (`search`)
- Router maps `/search` → `search` capability
- No user confusion (only see `/search` in UI)

### Risk 3: Localization Key Mismatches

**Problem**: Missing localization keys cause fallback to English
**Mitigation**:
- Use description as fallback
- CI checks for missing keys
- Log warnings for missing translations

### Risk 4: Tool ID Conflicts

**Problem**: Custom rule `/search` conflicts with Builtin `/search`
**Mitigation**:
- Builtin registered first (lower sort_order)
- Custom rules use `custom:` prefix in ID
- Router prioritizes by specificity (exact match first)

## Migration Plan

### Phase 1: Extend UnifiedTool (No Breaking Changes)
1. Add new fields to UnifiedTool (all optional)
2. Add `ToolSource::Builtin` variant
3. Implement `register_builtin_tools()`
4. Keep existing code working

### Phase 2: Add UniFFI APIs
1. Add new methods to AlephCore
2. Add UnifiedToolInfo type to UDL
3. Generate bindings
4. Write unit tests

### Phase 3: Refactor CommandRegistry
1. Inject ToolRegistry dependency
2. Implement `get_root_commands()` from registry
3. Implement `get_children()` from registry
4. Remove `get_builtin_hint()` (use localization keys)
5. Remove hardcoded builtin_commands list

### Phase 4: Refactor Swift UI
1. Remove `PresetRules.all` enum
2. Call `list_builtin_tools()` in RoutingView
3. Update PresetRulesListView to use dynamic data
4. Update CommandCompletionManager to use new APIs

### Phase 5: Clean Up
1. Remove `Config::builtin_rules()` (if no longer needed)
2. Remove `get_builtin_hint()` function
3. Update documentation
4. Add localization keys to Localizable.strings

### Rollback
- All changes are additive until Phase 3-4
- Can revert to static lists by restoring removed code
- No data migration needed

## Open Questions

1. **Should we merge Builtin and Native?**
   - Currently: `/search` (Builtin) maps to `search` (Native)
   - Alternative: Just have one `search` with both command and capability
   - Decision: Keep separate for clarity (command vs capability)

2. **Should custom rules override builtins?**
   - If user defines `/search` rule, what happens?
   - Proposal: User rules can customize but not remove builtins
   - Implementation: Merge user config into builtin defaults

3. **How to handle MCP server disconnects?**
   - Current: Tools remain in registry
   - Alternative: Remove tools on disconnect
   - Proposal: Mark inactive (`is_active = false`) on disconnect

4. **Should tool sort_order be configurable?**
   - Current: Hardcoded in register_builtin_tools
   - Future: Allow user to reorder in Settings UI
   - Decision: Hardcoded for MVP, configurable later

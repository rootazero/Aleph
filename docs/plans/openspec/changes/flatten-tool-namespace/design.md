# Design: Flatten Tool Namespace

## Overview

This document describes the architectural changes required to flatten the tool namespace, removing `/mcp` and `/skill` prefixes while maintaining source visibility through UI badges.

## Current Architecture

### Command Structure (Before)

```
Root Commands:
├─ /search        [Builtin]  ← Direct access
├─ /video         [Builtin]  ← Direct access
├─ /chat          [Builtin]  ← Direct access
├─ /mcp           [Builtin, Namespace]
│   ├─ git        [MCP: git-server]      ← Requires /mcp prefix
│   ├─ fs         [MCP: filesystem]      ← Requires /mcp prefix
│   └─ github     [MCP: github]          ← Requires /mcp prefix
├─ /skill         [Builtin, Namespace]
│   ├─ refine-text    [Skill]            ← Requires /skill prefix
│   └─ code-review    [Skill]            ← Requires /skill prefix
└─ /en            [Custom]   ← Direct access
```

### Problems

1. **Namespace navigation required**: User must know `/mcp` contains git tools
2. **Two-step discovery**: Type `/`, see `/mcp`, select, then see actual tools
3. **Mental overhead**: Remember which source contains which tool
4. **Inconsistent UX**: Some tools direct (`/search`), others nested (`/mcp git`)

## Target Architecture

### Command Structure (After)

```
Root Commands (Flat):
├─ /search        [System]   icon: magnifyingglass    badge: "System"
├─ /video         [System]   icon: play.rectangle     badge: "System"
├─ /chat          [System]   icon: bubble.left        badge: "System"
├─ /git           [MCP]      icon: bolt.fill          badge: "MCP: git-server"
├─ /fs            [MCP]      icon: bolt.fill          badge: "MCP: filesystem"
├─ /github        [MCP]      icon: bolt.fill          badge: "MCP"
├─ /refine-text   [Skill]    icon: lightbulb.fill     badge: "Skill"
├─ /code-review   [Skill]    icon: lightbulb.fill     badge: "Skill"
└─ /en            [Custom]   icon: command            badge: "Custom"
```

### Key Principles

1. **Flat list**: All commands at root level, no namespace navigation
2. **Source via badge**: Visual indicator shows origin (System, MCP, Skill, Custom)
3. **Icon differentiation**: Each source type has consistent icon style
4. **Consistent UX**: All tools invoked the same way: `/{name} {args}`

## Component Changes

### 1. ToolRegistry Changes

#### 1.1 Remove Namespace Builtins

**File:** `dispatcher/builtin_defs.rs`

```rust
// REMOVE these from BUILTIN_COMMANDS:
// - BuiltinCommandDef { name: "mcp", has_subtools: true, ... }
// - BuiltinCommandDef { name: "skill", has_subtools: true, ... }

// KEEP these (direct access tools):
pub const BUILTIN_COMMANDS: &[BuiltinCommandDef] = &[
    BuiltinCommandDef { name: "search", ... },
    BuiltinCommandDef { name: "video", ... },
    BuiltinCommandDef { name: "chat", ... },
];
```

#### 1.2 Flatten MCP Tool Registration

**File:** `dispatcher/registry.rs`

```rust
// BEFORE: MCP tools registered under parent
impl ToolRegistry {
    pub async fn register_mcp_tools(&self, tools: &[McpToolInfo], server: &str) {
        for tool in tools {
            let id = format!("mcp:{}:{}", server, tool.name);
            // Tool accessible via /mcp {tool.name}
        }
    }
}

// AFTER: MCP tools registered as root commands
impl ToolRegistry {
    pub async fn register_mcp_tools(&self, tools: &[McpToolInfo], server: &str) {
        for tool in tools {
            let id = format!("mcp:{}:{}", server, tool.name);

            // Check for conflicts
            let command_name = &tool.name;
            if let Some(conflict) = self.check_conflict(command_name).await {
                // Apply conflict resolution
                let resolved_name = self.resolve_conflict(command_name, conflict);
                // Register with resolved name
            } else {
                // Register directly as root command
            }

            // Tool accessible via /{tool.name} directly
            // routing_regex = format!("^/{}\\s+", tool.name)
        }
    }
}
```

#### 1.3 Flatten Skill Registration

**File:** `dispatcher/registry.rs`

```rust
// BEFORE: Skills registered under /skill namespace
impl ToolRegistry {
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        for skill in skills {
            let id = format!("skill:{}", skill.id);
            // Skill accessible via /skill {skill.id}
        }
    }
}

// AFTER: Skills registered as root commands
impl ToolRegistry {
    pub async fn register_skills(&self, skills: &[SkillInfo]) {
        for skill in skills {
            let id = format!("skill:{}", skill.id);

            // Check for conflicts
            if let Some(conflict) = self.check_conflict(&skill.id).await {
                let resolved_name = self.resolve_conflict(&skill.id, conflict);
            } else {
                // Register directly as root command
            }

            // Skill accessible via /{skill.id} directly
            // routing_regex = format!("^/{}\\s*", skill.id)
        }
    }
}
```

### 2. Conflict Resolution System

#### 2.1 Priority Levels

```rust
/// Tool priority for conflict resolution (higher = wins)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolPriority {
    Skill = 1,      // Lowest - can be shadowed by anything
    Mcp = 2,        // External tools
    Custom = 3,     // User-defined rules
    Native = 4,     // System capabilities
    Builtin = 5,    // Highest - system commands
}

impl ToolSource {
    pub fn priority(&self) -> ToolPriority {
        match self {
            ToolSource::Builtin => ToolPriority::Builtin,
            ToolSource::Native => ToolPriority::Native,
            ToolSource::Custom { .. } => ToolPriority::Custom,
            ToolSource::Mcp { .. } => ToolPriority::Mcp,
            ToolSource::Skill { .. } => ToolPriority::Skill,
        }
    }
}
```

#### 2.2 Conflict Detection and Resolution

```rust
pub struct ConflictInfo {
    pub existing_id: String,
    pub existing_source: ToolSource,
    pub existing_priority: ToolPriority,
}

impl ToolRegistry {
    /// Check if a command name conflicts with existing registration
    pub async fn check_conflict(&self, name: &str) -> Option<ConflictInfo> {
        let tools = self.tools.read().await;

        // Search for tool with same command name
        for tool in tools.values() {
            if tool.name == name {
                return Some(ConflictInfo {
                    existing_id: tool.id.clone(),
                    existing_source: tool.source.clone(),
                    existing_priority: tool.source.priority(),
                });
            }
        }
        None
    }

    /// Resolve naming conflict based on priority
    pub fn resolve_conflict(
        &self,
        name: &str,
        conflict: ConflictInfo,
        new_source: &ToolSource,
    ) -> ConflictResolution {
        let new_priority = new_source.priority();

        if new_priority > conflict.existing_priority {
            // New tool wins, rename existing
            ConflictResolution::RenameExisting {
                old_name: name.to_string(),
                new_name: format!("{}-{}", name, conflict.existing_source.suffix()),
            }
        } else {
            // Existing wins, rename new
            ConflictResolution::RenameNew {
                old_name: name.to_string(),
                new_name: format!("{}-{}", name, new_source.suffix()),
            }
        }
    }
}

pub enum ConflictResolution {
    RenameExisting { old_name: String, new_name: String },
    RenameNew { old_name: String, new_name: String },
}

impl ToolSource {
    /// Suffix for renamed tools
    pub fn suffix(&self) -> &'static str {
        match self {
            ToolSource::Builtin => "system",
            ToolSource::Native => "native",
            ToolSource::Custom { .. } => "custom",
            ToolSource::Mcp { server } => "mcp",
            ToolSource::Skill { .. } => "skill",
        }
    }
}
```

#### 2.3 Conflict Examples

| Scenario | Winner | Loser Renamed To |
|----------|--------|------------------|
| System `/search` vs MCP `search` | System | `search-mcp` |
| Custom `/translate` vs Skill `translate` | Custom | `translate-skill` |
| MCP `git` vs Skill `git` | MCP | `git-skill` |
| Two MCP servers both have `status` | First registered | `status-{server2}` |

### 3. UI Changes

#### 3.1 Command Completion Badge Display

**File:** `SubPanelView.swift`

```swift
struct CommandRowView: View {
    let command: CommandNode

    var body: some View {
        HStack {
            // Icon (left)
            Image(systemName: command.icon)
                .foregroundColor(iconColor)
                .frame(width: 20)

            // Command name
            Text("/\(command.key)")
                .font(.system(.body, design: .monospaced))

            // Description
            Text(command.description)
                .foregroundColor(.secondary)
                .lineLimit(1)

            Spacer()

            // Source Badge (right)
            SourceBadge(source: command.sourceType)
        }
    }

    var iconColor: Color {
        switch command.sourceType {
        case .builtin, .native: return .blue
        case .mcp: return .orange
        case .skill: return .purple
        case .custom: return .green
        }
    }
}

struct SourceBadge: View {
    let source: ToolSourceType

    var body: some View {
        Text(badgeText)
            .font(.caption2)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(badgeColor.opacity(0.15))
            .foregroundColor(badgeColor)
            .clipShape(Capsule())
    }

    var badgeText: String {
        switch source {
        case .builtin, .native: return "System"
        case .mcp: return "MCP"
        case .skill: return "Skill"
        case .custom: return "Custom"
        }
    }

    var badgeColor: Color {
        switch source {
        case .builtin, .native: return .blue
        case .mcp: return .orange
        case .skill: return .purple
        case .custom: return .green
        }
    }
}
```

#### 3.2 Settings Preset Rules List

**File:** `RoutingView.swift`

```swift
struct PresetRulesListView: View {
    @State private var tools: [UnifiedToolInfo] = []

    var body: some View {
        List {
            // Group by source type with section headers
            Section("System Commands") {
                ForEach(tools.filter { $0.sourceType == .builtin || $0.sourceType == .native }) { tool in
                    ToolRowView(tool: tool)
                }
            }

            Section("MCP Tools") {
                ForEach(tools.filter { $0.sourceType == .mcp }) { tool in
                    ToolRowView(tool: tool, showServer: true)
                }
            }

            Section("Skills") {
                ForEach(tools.filter { $0.sourceType == .skill }) { tool in
                    ToolRowView(tool: tool)
                }
            }

            Section("Custom Commands") {
                ForEach(tools.filter { $0.sourceType == .custom }) { tool in
                    ToolRowView(tool: tool)
                }
            }
        }
    }
}

struct ToolRowView: View {
    let tool: UnifiedToolInfo
    var showServer: Bool = false

    var body: some View {
        HStack {
            Image(systemName: tool.icon ?? "command")

            VStack(alignment: .leading) {
                Text("/\(tool.name)")
                    .font(.headline)

                if showServer, let server = tool.serviceName {
                    Text("from \(server)")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Text(tool.description)
                    .font(.subheadline)
                    .foregroundColor(.secondary)
            }

            Spacer()

            SourceBadge(source: tool.sourceType)
        }
    }
}
```

### 4. Routing Changes

#### 4.1 Dynamic Routing Rule Generation

**File:** `dispatcher/registry.rs`

```rust
impl ToolRegistry {
    /// Generate routing rules for all registered tools
    pub async fn generate_routing_rules(&self) -> Vec<RoutingRuleConfig> {
        let tools = self.tools.read().await;
        let mut rules = Vec::new();

        for tool in tools.values() {
            match &tool.source {
                ToolSource::Builtin => {
                    // Use predefined routing from builtin_defs.rs
                    if let Some(regex) = &tool.routing_regex {
                        rules.push(RoutingRuleConfig {
                            regex: regex.clone(),
                            system_prompt: tool.routing_system_prompt.clone(),
                            capabilities: tool.routing_capabilities.clone(),
                            ..Default::default()
                        });
                    }
                }
                ToolSource::Mcp { server } => {
                    // Generate routing rule for MCP tool
                    rules.push(RoutingRuleConfig {
                        rule_type: Some("mcp".to_string()),
                        regex: format!(r"^/{}\s*", regex::escape(&tool.name)),
                        intent_type: Some(format!("mcp:{}", tool.name)),
                        mcp_server: Some(server.clone()),
                        mcp_tool: Some(tool.name.clone()),
                        strip_prefix: Some(true),
                        ..Default::default()
                    });
                }
                ToolSource::Skill { id } => {
                    // Generate routing rule for Skill
                    rules.push(RoutingRuleConfig {
                        rule_type: Some("skill".to_string()),
                        regex: format!(r"^/{}\s*", regex::escape(&tool.name)),
                        intent_type: Some("skills".to_string()),
                        skill_id: Some(id.clone()),
                        capabilities: Some(vec!["skills".to_string(), "memory".to_string()]),
                        strip_prefix: Some(true),
                        ..Default::default()
                    });
                }
                ToolSource::Custom { .. } | ToolSource::Native => {
                    // Use existing routing config
                }
            }
        }

        rules
    }
}
```

#### 4.2 Router Integration

**File:** `router/mod.rs`

```rust
impl Router {
    /// Initialize router with flattened tool registry
    pub async fn init_with_registry(&mut self, registry: &ToolRegistry) {
        // Get all routing rules from registry
        let rules = registry.generate_routing_rules().await;

        // Compile regex patterns
        self.compiled_rules = rules.iter()
            .filter_map(|rule| {
                Regex::new(&rule.regex).ok().map(|regex| CompiledRule {
                    regex,
                    config: rule.clone(),
                })
            })
            .collect();
    }

    /// Route input to appropriate handler
    pub async fn route(&self, input: &str) -> RoutingDecision {
        // L1: Check all compiled patterns (flat, no namespace nesting)
        for rule in &self.compiled_rules {
            if rule.regex.is_match(input) {
                return self.create_decision(&rule.config, input);
            }
        }

        // L2, L3, Default...
    }
}
```

### 5. CommandCompletionManager Simplification

**File:** `CommandCompletionManager.swift`

```swift
final class CommandCompletionManager: ObservableObject {
    @Published var displayedCommands: [CommandNode] = []

    // REMOVE: namespace navigation state
    // @Published var currentParentKey: String?  ← DELETE

    // REMOVE: namespace navigation methods
    // func navigateIntoNamespace(_ key: String)  ← DELETE
    // func navigateBack()  ← DELETE
    // var isInNamespace: Bool  ← DELETE

    func refreshCommands() async {
        // Get ALL commands as flat list
        let commands = try? await core.getRootCommandsFromRegistry()

        await MainActor.run {
            self.allCommands = commands ?? []
            self.applyFilter()
        }
    }

    func applyFilter() {
        if currentFilter.isEmpty {
            displayedCommands = allCommands
        } else {
            displayedCommands = allCommands.filter {
                $0.key.localizedCaseInsensitiveContains(currentFilter) ||
                $0.description.localizedCaseInsensitiveContains(currentFilter)
            }
        }

        // Sort by source priority, then alphabetically
        displayedCommands.sort { a, b in
            if a.sourceType != b.sourceType {
                return a.sourceType.sortOrder < b.sourceType.sortOrder
            }
            return a.key < b.key
        }
    }
}

extension ToolSourceType {
    var sortOrder: Int {
        switch self {
        case .builtin, .native: return 0  // System first
        case .custom: return 1            // User custom second
        case .mcp: return 2               // MCP third
        case .skill: return 3             // Skills last
        }
    }
}
```

## Data Flow

### Registration Flow

```
App Launch
    ↓
ToolRegistry.refresh_all()
    ↓
┌─────────────────────────────────────────────────────────┐
│ 1. Register Builtins (3): search, video, chat          │
│    - Highest priority, always at root                   │
│                                                         │
│ 2. Register Native (2): search, video capabilities     │
│    - Internal implementations, not user-facing         │
│                                                         │
│ 3. Register MCP Tools (dynamic)                         │
│    - Each tool becomes root command: /git, /fs, etc.   │
│    - Check conflicts, rename if needed                 │
│                                                         │
│ 4. Register Skills (dynamic)                            │
│    - Each skill becomes root command: /refine-text     │
│    - Check conflicts, rename if needed                 │
│                                                         │
│ 5. Register Custom Rules (from config.toml)            │
│    - User-defined prompts: /en, /zh, etc.              │
└─────────────────────────────────────────────────────────┘
    ↓
Generate routing rules from registered tools
    ↓
Router.init_with_registry()
    ↓
Swift: CommandCompletionManager.refreshCommands()
```

### User Interaction Flow

```
User types "/"
    ↓
CommandCompletionManager shows flat list:
┌────────────────────────────────────────────────────┐
│ [🔍] /search     Search the web...        [System] │
│ [▶️] /video      Analyze YouTube...       [System] │
│ [💬] /chat       Multi-turn chat...       [System] │
│ [⚡] /git        Git operations...        [MCP]    │
│ [⚡] /fs         File system...           [MCP]    │
│ [💡] /refine     Refine text...           [Skill]  │
│ [⌘] /en         Translate to English     [Custom] │
└────────────────────────────────────────────────────┘

User selects "/git"
    ↓
Input field shows: "/git "
    ↓
User types "status"
    ↓
Input: "/git status"
    ↓
Submit → Router matches "^/git\s+" → Execute MCP tool
```

## Migration Strategy

### Phase 1: Add Flat Registration (Parallel)

1. Keep existing namespace registration
2. Add new flat registration as alternative
3. Feature flag: `flat_namespace = false` (default)

### Phase 2: Enable by Default

1. Set `flat_namespace = true` (default)
2. Deprecation warning for `/mcp` and `/skill` prefixes
3. Auto-translate old commands: `/mcp git` → `/git`

### Phase 3: Remove Namespace Code

1. Remove `/mcp` and `/skill` from BUILTIN_COMMANDS
2. Remove namespace navigation from UI
3. Remove compatibility layer

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_conflict_resolution_builtin_wins() {
    let registry = ToolRegistry::new();

    // Register builtin /search
    registry.register_builtin_tools().await;

    // Try to register MCP tool named "search"
    registry.register_mcp_tool("search", "some-server").await;

    // Verify MCP tool renamed to "search-mcp"
    assert!(registry.get_tool("search").await.source.is_builtin());
    assert!(registry.get_tool("search-mcp").await.source.is_mcp());
}

#[test]
fn test_flat_command_routing() {
    let router = Router::new();

    // Register MCP tool as root command
    registry.register_mcp_tool("git", "git-server").await;
    router.init_with_registry(&registry).await;

    // Verify routing
    let decision = router.route("/git status").await;
    assert_eq!(decision.intent_type, "mcp:git");
    assert_eq!(decision.mcp_tool, Some("git".to_string()));
}
```

### Integration Tests

1. **Command Completion**: Verify all tools appear in flat list
2. **Routing**: Verify `/git status` routes correctly
3. **Conflict**: Verify conflict resolution works
4. **UI Badges**: Verify source badges display correctly

### Manual Testing

1. Start app with MCP servers connected
2. Type `/` and verify all tools in flat list
3. Select `/git` and verify it executes MCP tool
4. Add custom rule `/git-custom`, verify coexistence
5. Verify Settings shows all tools with correct badges

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Too many root commands | Smart grouping in UI, fuzzy search |
| Naming conflicts | Clear conflict resolution, renamed tools visible |
| User confusion (transition) | Migration tips, deprecation warnings |
| Breaking existing shortcuts | Backward compat layer during transition |

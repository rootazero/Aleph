# Design: Claude Code Compatible Plugin System

## Context

Claude Code CLI (https://code.claude.com) has a well-designed plugin system that allows users to extend Claude's capabilities with custom skills, hooks, agents, and MCP servers. By making Aether's plugin system compatible with this format, we can:

1. Leverage existing Claude Code plugins without modification
2. Allow plugin developers to create plugins that work in both environments
3. Benefit from Claude Code's plugin marketplace ecosystem

### Stakeholders
- End users who want to extend Aether
- Plugin developers
- Aether maintainers

### Constraints
- Must parse Claude Code plugin format exactly
- Must integrate with Aether's existing systems (EventBus, AgentRegistry, McpClient)
- Must use Aether's runtime managers (fnm, uv) instead of requiring system-installed Node.js/Python

## Goals / Non-Goals

### Goals
- Full compatibility with Claude Code plugin directory structure
- Support all Claude Code plugin components: commands, skills, agents, hooks, MCP
- Seamless integration with Aether's existing architecture
- Zero additional dependencies for users (use Aether's runtimes)

### Non-Goals
- LSP server support (deferred to future phase)
- Plugin marketplace integration (future)
- Plugin sandboxing/security (future)
- Hot-reload during runtime (future)

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Plugin Manager                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │
│  │   Scanner   │───▶│   Loader    │───▶│  Registry   │───▶│ Integrator  │  │
│  │             │    │             │    │             │    │             │  │
│  │ - Find dirs │    │ - Parse     │    │ - Store     │    │ - Connect   │  │
│  │ - Validate  │    │   manifests │    │   plugins   │    │   to core   │  │
│  │   structure │    │ - Load      │    │ - Query     │    │   systems   │  │
│  └─────────────┘    │   components│    │   enable/   │    └─────────────┘  │
│                     └─────────────┘    │   disable   │                      │
│                                        └─────────────┘                      │
│                            │                                                 │
│           ┌────────────────┼────────────────────────┐                       │
│           │                │                        │                       │
│           ▼                ▼                        ▼                       │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐       │
│  │ SkillLoader │  │ HookLoader  │  │ AgentLoader │  │  McpLoader  │       │
│  │             │  │             │  │             │  │             │       │
│  │ - SKILL.md  │  │ - hooks.json│  │ - agent.md  │  │ - .mcp.json │       │
│  │ - frontmatter│ │ - Events    │  │ - Markdown  │  │ - Runtime   │       │
│  │ - $ARGUMENTS│  │ - Matchers  │  │ - Capabilities│ │   paths     │       │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Aether Core Integration                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐       │
│  │   Thinker   │  │  EventBus   │  │   Agent     │  │    MCP      │       │
│  │             │  │             │  │  Registry   │  │   Client    │       │
│  │ Skills →    │  │ Hooks →     │  │ Agents →    │  │ MCP →       │       │
│  │ prompt      │  │ subscribers │  │ register()  │  │ start()     │       │
│  │ injection   │  │             │  │             │  │             │       │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Module Structure

```
core/src/plugins/
├── mod.rs                 # Public API exports
├── error.rs               # Error types
├── manifest.rs            # plugin.json parsing
├── scanner.rs             # Plugin discovery
├── loader.rs              # Plugin loading orchestration
├── registry.rs            # Plugin storage and state
├── integrator.rs          # Core system integration
├── types.rs               # Shared types
├── components/
│   ├── mod.rs
│   ├── skill.rs           # SKILL.md parsing (commands + skills)
│   ├── hook.rs            # hooks.json parsing and event mapping
│   ├── agent.rs           # agent.md parsing
│   └── mcp.rs             # .mcp.json handling
└── runtime.rs             # Runtime path resolution (fnm, uv)
```

## Decisions

### Decision 1: Event Mapping Strategy

**Decision**: Create a bidirectional mapping between Claude Code hook events and Aether EventBus events.

**Mapping Table**:
| Claude Code Event | Aether EventType | Notes |
|-------------------|------------------|-------|
| PreToolUse | ToolCall | Before tool execution |
| PostToolUse | ToolCallCompleted | After successful execution |
| SessionStart | LoopStarted | Session begins |
| SessionEnd | LoopStop | Session ends |
| UserPromptSubmit | InputReceived | User input |
| SubagentStart | SubAgentStarted | Subagent launched |
| SubagentStop | SubAgentCompleted | Subagent finished |
| Stop | LoopStop | Explicit stop |

**Alternatives considered**:
1. Create new event types for Claude Code events → Rejected (duplicates existing events)
2. Direct passthrough without mapping → Rejected (naming inconsistency)

### Decision 2: Skill Injection Method

**Decision**: Inject plugin skills into the Thinker's prompt system via a dedicated `plugin_skills` field in PromptConfig.

```rust
pub struct PromptConfig {
    // ... existing fields
    pub plugin_skills: Vec<PluginSkill>,  // NEW
}
```

Skills with `disable_model_invocation: false` (default) are included in the system prompt, allowing the LLM to invoke them automatically. Skills with `disable_model_invocation: true` are only triggered by explicit user commands.

**Alternatives considered**:
1. Separate skill system → Rejected (unnecessary duplication)
2. Inject as tools → Rejected (skills are prompts, not tools)

### Decision 3: Runtime Path Resolution

**Decision**: Resolve runtime commands at MCP server start time using Aether's RuntimeRegistry.

```rust
fn resolve_command(&self, cmd: &str) -> PathBuf {
    match cmd {
        "npx" | "node" => self.runtime.require("fnm")?.node_path(),
        "uvx" | "python" | "python3" => self.runtime.require("uv")?.python_path(),
        other => which::which(other)?,
    }
}
```

**Alternatives considered**:
1. Require system-installed runtimes → Rejected (worse UX)
2. Detect at load time → Rejected (runtime might not be ready)

### Decision 4: Plugin Storage Location

**Decision**: Store plugins in `~/.aether/plugins/` with state in `~/.aether/plugins.json`.

```
~/.aether/
├── plugins/
│   ├── plugin-a/
│   └── plugin-b/
├── plugins.json           # { "plugin-a": { "enabled": true } }
└── config.toml            # [plugins] section
```

**Alternatives considered**:
1. System-wide location → Rejected (permission issues)
2. Per-project plugins → Future enhancement

## Data Structures

### Plugin Manifest

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<PluginAuthor>,
    #[serde(default)]
    pub license: Option<String>,
    // Custom paths (optional)
    #[serde(default)]
    pub commands: Option<PathBuf>,
    #[serde(default)]
    pub skills: Option<PathBuf>,
    #[serde(default)]
    pub agents: Option<PathBuf>,
    #[serde(default)]
    pub hooks: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}
```

### Plugin Skill

```rust
#[derive(Debug, Clone)]
pub struct PluginSkill {
    pub plugin_name: String,
    pub skill_name: String,
    pub skill_type: SkillType,
    pub description: String,
    pub content: String,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillType {
    Command,  // From commands/ directory
    Skill,    // From skills/ directory
}
```

### Plugin Hook

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHooksConfig {
    pub hooks: HashMap<HookEvent, Vec<HookMatcher>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    #[serde(default)]
    pub matcher: Option<String>,
    pub hooks: Vec<HookAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    Command { command: String },
    Prompt { prompt: String },
    Agent { agent: String },
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    Stop,
    SubagentStart,
    SubagentStop,
    Setup,
    PreCompact,
    Notification,
}
```

### Plugin Agent

```rust
#[derive(Debug, Clone)]
pub struct PluginAgent {
    pub plugin_name: String,
    pub agent_name: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub system_prompt: String,
}
```

## Risks / Trade-offs

### Risk 1: Claude Code Format Evolution
Claude Code plugin format may change over time.
- **Mitigation**: Version detection, maintain compatibility matrix, document supported versions.

### Risk 2: LLM-Specific Prompt Instructions
SKILL.md content is optimized for Claude, may not work perfectly with other LLMs.
- **Mitigation**: Most instructions are generic enough. Add documentation noting this limitation.

### Risk 3: Hook Execution Performance
Hooks running on every tool call could impact performance.
- **Mitigation**: Async hook execution, matcher optimization, optional hook disabling.

### Risk 4: Tool Name Mismatch
Claude Code uses specific tool names (Write, Edit, Bash) that may differ from Aether's.
- **Mitigation**: Maintain tool name alias mapping, document differences.

## Migration Plan

N/A - This is a new capability, no migration needed.

## Open Questions

1. **Plugin Versioning**: Should we support multiple versions of the same plugin?
   - Current decision: No, one version per plugin name.

2. **Conflict Resolution**: How to handle skill name conflicts between plugins?
   - Current decision: Namespace with plugin name (e.g., `plugin-a:skill-name`).

3. **Plugin Dependencies**: Should plugins be able to depend on other plugins?
   - Current decision: Not in initial implementation.

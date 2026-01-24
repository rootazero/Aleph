# Aether Plugin System

Aether's plugin system is fully compatible with Claude Code CLI plugins. Any Claude Code plugin (official or third-party) can be installed and used in Aether without modification.

## Overview

The plugin system supports:

- **Skills**: Commands and auto-invocable prompts (SKILL.md files)
- **Agents**: Custom sub-agents with specialized system prompts
- **Hooks**: Event-driven actions (PreToolUse, PostToolUse, SessionStart, etc.)
- **MCP Servers**: Model Context Protocol server integration

## Plugin Directory Structure

Plugins are installed in `~/.config/aether/plugins/`. Each plugin follows the Claude Code directory structure:

```
~/.config/aether/plugins/
└── my-plugin/
    ├── .claude-plugin/
    │   └── plugin.json       # Plugin manifest (required)
    ├── commands/             # User-triggered commands
    │   └── commit.md         # /commit command
    ├── skills/               # Auto-invocable skills
    │   └── code-review.md    # Skill for code review
    ├── agents/               # Custom agents
    │   └── reviewer.md       # Code reviewer agent
    ├── hooks.json            # Hook configurations
    └── .mcp.json             # MCP server configurations
```

## Plugin Manifest

The plugin manifest (`.claude-plugin/plugin.json`) defines the plugin:

```json
{
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "My awesome plugin",
  "author": "Your Name"
}
```

## Skills and Commands

Skills are markdown files with YAML frontmatter:

```markdown
---
name: commit
description: Create a git commit with AI-generated message
---

Analyze the staged changes and create a commit message following conventional commits format.

$ARGUMENTS
```

### Skill Types

- **Commands** (`commands/` directory): User-triggered via `/plugin:command`
- **Skills** (`skills/` directory): Auto-invocable by the AI when relevant

The `$ARGUMENTS` placeholder is replaced with user input.

## Agents

Custom agents are defined in markdown files:

```markdown
---
name: reviewer
description: Code review specialist
capabilities:
  - Review code for bugs and issues
  - Suggest improvements
---

You are a code review specialist. Analyze code for:
- Logic errors
- Security vulnerabilities
- Performance issues
- Best practices violations
```

## Hooks

Hooks respond to events during AI processing:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "action": {
          "type": "prompt",
          "prompt": "Verify this command is safe before execution."
        }
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Edit",
        "action": {
          "type": "command",
          "command": "npm run lint ${FILE}"
        }
      }
    ]
  }
}
```

### Hook Events

| Event | Description | Aether EventType |
|-------|-------------|------------------|
| `PreToolUse` | Before tool execution | `ToolCallRequested` |
| `PostToolUse` | After tool execution | `ToolCallCompleted` |
| `SessionStart` | Session begins | `SessionCreated` |
| `Stop` | Processing complete | `ProcessingComplete` |

### Hook Actions

- **command**: Execute a shell command
- **prompt**: Inject a prompt for LLM evaluation
- **agent**: Invoke a custom agent

### Variable Substitution

- `${CLAUDE_PLUGIN_ROOT}`: Plugin root directory
- `$ARGUMENTS`: User-provided arguments
- `${FILE}`: Current file path (if applicable)

## MCP Servers

Configure MCP servers in `.mcp.json`:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-server-filesystem", "/path/to/allowed"]
    },
    "custom-server": {
      "command": "uvx",
      "args": ["my-mcp-server"]
    }
  }
}
```

### Runtime Resolution

MCP server commands are resolved through Aether's runtime system:

- `npx`, `node` → Resolved via fnm (Node.js)
- `uvx`, `python`, `python3` → Resolved via uv (Python)

## FFI API

Aether provides both synchronous and asynchronous FFI APIs for plugin management. The async API (UniFFI 0.31+) is recommended for new code.

### Swift Integration (Async - Recommended)

```swift
// Load all extensions
let summary = try await extensionLoadAll()
print("Loaded \(summary.pluginsLoaded) plugins")

// List installed plugins
let plugins = try await extensionListPlugins()

// List skills from enabled plugins
let skills = try await extensionListSkills()

// Get auto-invocable skills (for AI prompt injection)
let autoSkills = try await extensionGetAutoSkills()

// Execute a skill
let result = try await extensionExecuteSkill(
    qualifiedName: "my-plugin:commit",
    arguments: "--amend"
)

// Execute a command
let cmdResult = try await extensionExecuteCommand(
    name: "commit",
    arguments: "--amend"
)

// Get skill instructions for prompt injection
let instructions = try await extensionGetSkillInstructions()

// Get default model from configuration
let model = try await extensionGetDefaultModel()

// Get custom instructions
let customInstructions = try await extensionGetInstructions()

// Load plugin from custom path (development)
let pluginInfo = try await extensionLoadPluginFromPath(path: "/path/to/dev/plugin")

// Sync functions (no await needed)
let pluginsDir = extensionGetPluginsDir()
let isValid = extensionIsValidPluginDir(path: "/path/to/plugin")
```

### Swift Integration (Sync - Legacy)

```swift
// List installed plugins
let plugins = try core.list_plugins()

// Enable/disable plugins
try core.enable_plugin(name: "my-plugin")
try core.disable_plugin(name: "my-plugin")

// List skills from enabled plugins
let skills = try core.list_plugin_skills()

// Execute a skill
let result = try core.execute_plugin_skill(
    plugin_name: "my-plugin",
    skill_name: "commit",
    arguments: "--amend"
)

// Get plugins directory
let pluginsDir = core.get_plugins_dir()

// Refresh plugins from disk
let count = try core.refresh_plugins()

// Get skill instructions for prompt injection
let instructions = try core.get_plugin_skill_instructions()
```

### Available FFI Types

```swift
// Plugin information (both sync and async APIs)
struct PluginInfoFfi {
    let name: String
    let version: String
    let description: String
    let enabled: Bool
    let path: String
    let skillsCount: UInt32
    let agentsCount: UInt32
    let hooksCount: UInt32
    let mcpServersCount: UInt32
}

// Skill information (both sync and async APIs)
struct PluginSkillFfi {
    let qualifiedName: String  // "plugin:skill"
    let pluginName: String
    let skillName: String
    let description: String
    let isCommand: Bool
}

// Load summary (async API only)
struct LoadSummaryFfi {
    let skillsLoaded: UInt32
    let commandsLoaded: UInt32
    let agentsLoaded: UInt32
    let pluginsLoaded: UInt32
    let hooksLoaded: UInt32
    let errors: [String]
}

// Async error type
enum ExtensionAsyncError: Error {
    case initError(String)
    case loadError(String)
    case notFound(String)
    case ioError(String)
}
```

## Plugin State Persistence

Plugin enable/disable state is stored in `~/.config/aether/plugins.json`:

```json
{
  "disabled_plugins": ["plugin-to-disable"]
}
```

## Development Mode

Load plugins from custom paths for development:

```swift
let pluginInfo = try core.load_plugin_from_path(path: "/path/to/dev/plugin")
```

## Architecture

The plugin system uses a layered architecture with separate discovery and extension modules:

```
┌─────────────────────────────────────────────────────────────┐
│                    Extension System                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              discovery/ module                        │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│  │  │ scanner.rs  │  │  paths.rs   │  │  types.rs   │   │  │
│  │  │             │  │             │  │             │   │  │
│  │  │ Discover:   │  │ Path utils: │  │ Component   │   │  │
│  │  │ - ~/.claude/│  │ - aether_   │  │ types and   │   │  │
│  │  │ - ~/.aether/│  │   home()    │  │ discovery   │   │  │
│  │  │ - .claude/  │  │ - git_root()│  │ sources     │   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │  │
│  └──────────────────────────────────────────────────────┘  │
│                            ↓                                │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              extension/ module                        │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│  │  │ loader.rs   │  │ registry.rs │  │ config/     │   │  │
│  │  │             │  │             │  │             │   │  │
│  │  │ Load:       │  │ Store:      │  │ aether.jsonc│   │  │
│  │  │ - skills    │  │ - plugins   │  │ config      │   │  │
│  │  │ - commands  │  │ - skills    │  │ merging     │   │  │
│  │  │ - agents    │  │ - commands  │  │             │   │  │
│  │  │ - plugins   │  │ - agents    │  │             │   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │  │
│  │                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│  │  │ hooks/      │  │ runtime/    │  │ sync_api.rs │   │  │
│  │  │             │  │             │  │             │   │  │
│  │  │ Hook exec:  │  │ Node.js     │  │ Sync FFI    │   │  │
│  │  │ - PreToolUse│  │ runtime:    │  │ wrapper     │   │  │
│  │  │ - PostTool  │  │ - fnm       │  │ (legacy)    │   │  │
│  │  │ - Stop      │  │ - npm       │  │             │   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘   │  │
│  └──────────────────────────────────────────────────────┘  │
│                            ↓                                │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              ffi/ module                              │  │
│  │  ┌────────────────────┐  ┌────────────────────┐      │  │
│  │  │ async_extension.rs │  │ plugins.rs         │      │  │
│  │  │                    │  │                    │      │  │
│  │  │ Async FFI:         │  │ Sync FFI:          │      │  │
│  │  │ extensionLoadAll() │  │ listPlugins()      │      │  │
│  │  │ extensionListXxx() │  │ executePluginSkill │      │  │
│  │  │ (UniFFI 0.31+)     │  │ (legacy)           │      │  │
│  │  └────────────────────┘  └────────────────────┘      │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│                     Integration Layer                       │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Skills → Thinker (prompt injection)                       │
│  Agents → AgentRegistry                                    │
│  Hooks  → EventBus (PreToolUse, PostToolUse, Stop, etc.)   │
│  MCP    → MCP Client (runtime path resolution via fnm/uv)  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Module Structure

```
core/src/
├── discovery/           # Multi-level component discovery
│   ├── mod.rs          # DiscoveryManager
│   ├── scanner.rs      # Directory scanning
│   ├── paths.rs        # Path utilities
│   └── types.rs        # DiscoveredComponent, DiscoverySource
│
├── extension/          # Extension system
│   ├── mod.rs          # ExtensionManager
│   ├── loader.rs       # ComponentLoader
│   ├── registry.rs     # ComponentRegistry
│   ├── types.rs        # ExtensionSkill, ExtensionAgent, etc.
│   ├── config/         # aether.jsonc configuration
│   ├── hooks/          # HookExecutor
│   ├── runtime/        # Node.js plugin runtime
│   └── sync_api.rs     # SyncExtensionManager (legacy sync wrapper)
│
└── ffi/
    ├── async_extension.rs  # Async FFI exports (UniFFI 0.31+)
    └── plugins.rs          # Sync FFI exports (legacy)
```

## Compatibility

The plugin system is designed to be 100% compatible with Claude Code plugins:

| Feature | Claude Code | Aether |
|---------|-------------|--------|
| plugin.json manifest | ✅ | ✅ |
| SKILL.md commands | ✅ | ✅ |
| SKILL.md skills | ✅ | ✅ |
| Agent definitions | ✅ | ✅ |
| hooks.json | ✅ | ✅ |
| .mcp.json | ✅ | ✅ |
| $ARGUMENTS substitution | ✅ | ✅ |
| ${CLAUDE_PLUGIN_ROOT} | ✅ | ✅ |

## Best Practices

1. **Version your plugins**: Use semantic versioning in plugin.json
2. **Document skills**: Provide clear descriptions in frontmatter
3. **Test hooks carefully**: Hooks run automatically, ensure they're safe
4. **Use relative paths**: Use `${CLAUDE_PLUGIN_ROOT}` for portability
5. **Keep MCP servers lightweight**: Avoid long-running initialization

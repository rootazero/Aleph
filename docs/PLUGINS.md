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

### Swift Integration

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
// Plugin information
struct PluginInfoFFI {
    let name: String
    let version: String
    let description: String
    let enabled: Bool
    let path: String
    let skills_count: UInt32
    let agents_count: UInt32
    let hooks_count: UInt32
    let mcp_servers_count: UInt32
}

// Skill information
struct PluginSkillFFI {
    let qualified_name: String  // "plugin:skill"
    let plugin_name: String
    let skill_name: String
    let description: String
    let is_command: Bool
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

```
┌─────────────────────────────────────────────────────────────┐
│                    Plugin System                            │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ PluginScanner│→ │PluginLoader  │→ │PluginRegistry│      │
│  │              │  │              │  │              │      │
│  │ Discover     │  │ Parse:       │  │ Store state  │      │
│  │ plugins in   │  │ - manifest   │  │ Enable/      │      │
│  │ ~/.config/   │  │ - skills     │  │ disable      │      │
│  │ aether/      │  │ - hooks      │  │ Persist to   │      │
│  │ plugins/     │  │ - agents     │  │ plugins.json │      │
│  └──────────────┘  │ - mcp.json   │  └──────────────┘      │
│                    └──────────────┘                         │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                   PluginManager                      │   │
│  │                                                      │   │
│  │ - load_all() - Load all plugins from directory      │   │
│  │ - get_all_skills() - Get skills for prompt injection│   │
│  │ - prepare_skill_execution() - Execute a skill       │   │
│  │ - set_enabled() - Enable/disable plugins            │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│                     Integration Layer                       │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Skills → Thinker (prompt injection)                       │
│  Agents → AgentRegistry                                    │
│  Hooks  → EventBus                                         │
│  MCP    → MCP Client (runtime path resolution)             │
│                                                             │
└─────────────────────────────────────────────────────────────┘
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

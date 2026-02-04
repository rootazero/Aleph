# Aleph Plugin System 2.0 Design

> Date: 2026-01-24
> Status: Draft
> Reference: OpenCode plugin implementation at `/Users/zouguojun/Workspace/opencode`

## Overview

This document describes a complete rewrite of Aleph's plugin system, inspired by OpenCode's implementation while maintaining Claude Code compatibility and leveraging Aleph's unique architecture.

## Goals

1. **Full Claude Code Compatibility** - Read from `.claude/` directories
2. **Multi-level Configuration** - Global → Project → Inline config merging
3. **Enhanced Agent Configuration** - model, temperature, steps, permissions
4. **Expanded Hook System** - More event types, JS plugin support
5. **Node.js Plugin Runtime** - npm package installation, TypeScript plugins
6. **Clean Architecture** - Modular, testable, maintainable

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Aleph Plugin System 2.0                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        ConfigManager                                 │   │
│  │  - Multi-level config discovery (global → project → inline)         │   │
│  │  - aleph.jsonc central configuration                               │   │
│  │  - Config merging with priority                                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────┴───────────────────────────────────┐   │
│  │                        DiscoveryManager                              │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐               │   │
│  │  │ SkillScanner │  │ AgentScanner │  │ PluginScanner│               │   │
│  │  │ .claude/     │  │ agents/      │  │ plugins/     │               │   │
│  │  │ skills/      │  │              │  │              │               │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘               │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────┴───────────────────────────────────┐   │
│  │                     PluginRuntime (Node.js)                          │   │
│  │  ┌─────────────────┐    ┌─────────────────┐    ┌────────────────┐   │   │
│  │  │  NpmInstaller   │    │  JsPluginHost   │    │  PluginBridge  │   │   │
│  │  │  - fnm/node     │    │  - Load .ts/.js │    │  Rust ↔ Node   │   │   │
│  │  │  - npm install  │    │  - Execute hooks│    │  IPC (JSON-RPC)│   │   │
│  │  └─────────────────┘    └─────────────────┘    └────────────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────┴───────────────────────────────────┐   │
│  │                          ComponentRegistry                           │   │
│  │  Skills │ Agents │ Hooks │ MCP Servers │ Commands │ Tools            │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────┴───────────────────────────────────┐   │
│  │                         HookExecutor                                 │   │
│  │  PreToolUse │ PostToolUse │ ChatMessage │ ChatParams │ Permission    │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Directory Strategy

### Read Paths (by priority, later overrides earlier)

**Global:**
```
~/.claude/skills/          ← Claude Code compatible (read-only)
~/.claude/commands/        ← Claude Code compatible (read-only)
~/.aleph/skills/          ← Aleph native
~/.aleph/commands/        ← Aleph native
~/.aleph/plugins/         ← Aleph native
```

**Project-level (find_up to git root):**
```
./.claude/skills/          ← Claude Code compatible (read-only)
./.claude/commands/        ← Claude Code compatible (read-only)
```

### Write Paths (always use Aleph native directory)

```
~/.aleph/
├── aleph.jsonc         # Global configuration
├── plugins/             # Installed plugins
├── skills/              # User skills
├── commands/            # User commands
├── agents/              # User agents
├── plugins.json         # Plugin state persistence
└── node_modules/        # JS plugin dependencies
```

### Configuration File Priority

1. `~/.aleph/aleph.jsonc` (global)
2. `./aleph.jsonc` (project-level, find_up)
3. `ALEPH_CONFIG_CONTENT` (environment variable, highest priority)

## Configuration Format

### aleph.jsonc

```jsonc
{
  "$schema": "https://aleph.ai/config.json",

  // Plugins (npm packages or local files)
  "plugin": [
    "@anthropic/aleph-tools@1.0.0",
    "file:///path/to/local/plugin.ts"
  ],

  // Additional instruction files
  "instructions": ["STYLE_GUIDE.md", "CONVENTIONS.md"],

  // Agent configuration overrides
  "agent": {
    "build": {
      "model": "anthropic/claude-sonnet-4",
      "temperature": 0.7,
      "steps": 20
    },
    "code-reviewer": {
      "mode": "subagent",
      "model": "anthropic/claude-haiku",
      "color": "#44BA81",
      "description": "Review code for bugs and style issues"
    }
  },

  // MCP servers
  "mcp": {
    "filesystem": {
      "type": "local",
      "command": ["npx", "-y", "@anthropic/mcp-server-filesystem"]
    }
  },

  // Permission configuration
  "permission": {
    "edit": "allow",
    "bash": { "*": "ask", "git *": "allow" }
  }
}
```

## Agent Configuration

### Agent Definition Format (agents/*.md)

```yaml
---
# Basic configuration
mode: subagent          # primary | subagent | all
description: "Code review specialist"
hidden: false           # Whether to hide in UI
color: "#44BA81"        # UI color identifier

# LLM configuration
model: anthropic/claude-haiku
temperature: 0.3
top_p: 0.9
steps: 10               # Max iterations

# Tool permissions (fine-grained control)
tools:
  "*": false            # Disable all by default
  Read: true
  Grep: true
  Glob: true

# Or use permission format (more flexible)
permission:
  read: allow
  edit:
    "*.md": allow
    "*": deny
  bash:
    "git *": allow
    "*": ask

# Provider-specific options
options:
  thinking: true        # Claude extended thinking
  cache: true           # Enable prompt cache
---

# System Prompt (Markdown body)
You are a code review specialist...
```

### Rust Type Definition

```rust
pub struct AgentConfig {
    pub name: String,
    pub mode: AgentMode,           // Primary | Subagent | All
    pub description: Option<String>,
    pub hidden: bool,
    pub color: Option<String>,

    // LLM configuration
    pub model: Option<ModelRef>,   // provider/model
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub steps: Option<u32>,

    // Permissions
    pub permission: PermissionRuleset,
    pub options: HashMap<String, Value>,

    // Content
    pub system_prompt: String,
}
```

## Hook System

### Hook Event Types (Complete List)

```rust
pub enum HookEvent {
    // Tool lifecycle
    PreToolUse,           // Before tool call, can modify args or block
    PostToolUse,          // After successful tool call
    PostToolUseFailure,   // After failed tool call

    // Session lifecycle
    SessionStart,         // Session started
    SessionEnd,           // Session ended
    PreCompact,           // Before context compaction

    // User interaction
    UserPromptSubmit,     // User submits prompt
    PermissionRequest,    // Permission request

    // Sub-agent
    SubagentStart,        // Sub-agent started
    SubagentStop,         // Sub-agent completed

    // Chat (new, for JS plugins)
    ChatMessage,          // Before message send, can modify content
    ChatParams,           // LLM params, can modify temperature etc
    ChatResponse,         // After LLM response

    // Command
    CommandExecuteBefore, // Before command execution
    CommandExecuteAfter,  // After command execution
}
```

### Static Hook Format (hooks.json)

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/format.sh $FILE"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "prompt",
            "prompt": "Verify this command is safe: $ARGUMENTS"
          }
        ]
      }
    ]
  }
}
```

### Dynamic Hook Format (JS Plugin)

```typescript
// plugin.ts
import { plugin } from "@anthropic/aleph-plugin"

export default plugin(async (ctx) => ({
  // Modify LLM params
  "ChatParams": async (input, output) => {
    output.temperature = 0.5
  },

  // Post-tool processing
  "PostToolUse": async (input, output) => {
    if (input.tool === "Write") {
      await ctx.$`prettier --write ${input.file}`
    }
  },

  // Custom tools
  tool: {
    "github-search": {
      description: "Search GitHub issues",
      args: { query: z.string() },
      execute: async (args) => { ... }
    }
  }
}))
```

## Plugin Types

### 1. Static Plugins (Markdown/JSON)
- `commands/*.md`, `skills/*/SKILL.md`
- `agents/*.md`, `hooks/hooks.json`
- `.mcp.json`

### 2. Dynamic Plugins (TypeScript/JavaScript)
- `tool/*.ts` (custom tools)
- `plugin/*.ts` (hook plugins)
- npm packages (`@scope/plugin@version`)

## Module Structure

```
core/src/
├── plugins/              ← DELETE (old implementation)
│
├── config/               ← NEW: Unified configuration system
│   ├── mod.rs            # ConfigManager entry
│   ├── loader.rs         # aleph.jsonc parsing
│   ├── merger.rs         # Multi-level config merging
│   ├── types.rs          # AlephConfig, AgentConfig, etc
│   └── env.rs            # Environment variable handling
│
├── discovery/            ← NEW: Component discovery system
│   ├── mod.rs            # DiscoveryManager entry
│   ├── scanner.rs        # Directory scanning, find_up
│   ├── skill.rs          # Skill/Command discovery
│   ├── agent.rs          # Agent discovery
│   ├── plugin.rs         # Plugin directory discovery
│   ├── claude_md.rs      # CLAUDE.md discovery (migrated)
│   └── paths.rs          # Path constants and helpers
│
├── extension/            ← NEW: Extension system (replaces plugins)
│   ├── mod.rs            # ExtensionManager entry
│   ├── types.rs          # Extension, Skill, Hook types
│   ├── registry.rs       # Component registry
│   ├── manifest.rs       # plugin.json parsing
│   ├── loader.rs         # Static component loading
│   │
│   ├── runtime/          # Node.js plugin runtime
│   │   ├── mod.rs        # PluginRuntime entry
│   │   ├── npm.rs        # npm install wrapper
│   │   ├── bridge.rs     # Rust ↔ Node IPC
│   │   ├── host.rs       # Node.js process management
│   │   └── protocol.rs   # JSON-RPC protocol definition
│   │
│   ├── hooks/            # Hook system
│   │   ├── mod.rs        # HookExecutor
│   │   ├── events.rs     # HookEvent enum
│   │   ├── matcher.rs    # Pattern matching
│   │   └── executor.rs   # Executors (command/prompt/agent)
│   │
│   └── mcp.rs            # MCP config processing
│
└── ... (other existing modules)
```

## Implementation Plan

### Phase 1: Infrastructure (config/ + discovery/)

**1.1 Create config/ module**
- AlephConfig type definitions
- aleph.jsonc parser
- Multi-level config merging logic

**1.2 Create discovery/ module**
- find_up mechanism
- Multi-directory scanner
- Migrate claude_md.rs

**1.3 Unit tests**

### Phase 2: Extension System Core (extension/)

**2.1 Type definitions and registry**
- Extension, Skill, Agent, Hook types
- ComponentRegistry

**2.2 Static component loading**
- Skill/Command loading (migrate existing code)
- Agent loading (enhanced config)
- Hook loading (expanded event types)
- MCP config processing

**2.3 Integration tests**

### Phase 3: Node.js Plugin Runtime (extension/runtime/)

**3.1 npm installer**
- Leverage RuntimeRegistry (fnm/node)
- Package version resolution

**3.2 Plugin Bridge**
- JSON-RPC protocol
- Rust ↔ Node IPC

**3.3 JS Plugin Host**
- Node.js process management
- Hook call forwarding
- Tool execution

**3.4 End-to-end tests**

### Phase 4: System Integration

**4.1 Agent Loop integration**
- ExtensionManager initialization
- Hook event subscription

**4.2 Thinker integration**
- Dynamic tool registration
- Agent config application

**4.3 FFI exposure**
- Plugin management API

**4.4 Regression tests**

### Phase 5: Cleanup & Documentation

**5.1 Delete old plugins/ module**
**5.2 Update documentation**
**5.3 Migration guide**

## Key Design Decisions

1. **Read .claude/, Write ~/.aleph/** - Respect Claude Code's directory while maintaining Aleph's identity

2. **Node.js Runtime via fnm** - Leverage existing Aleph infrastructure for JS plugin support

3. **JSON-RPC Bridge** - Standard protocol for Rust ↔ Node.js communication, similar to MCP

4. **Backward Compatibility** - Existing plugins continue to work during migration

5. **No Project-level Write** - All writes go to `~/.aleph/` for simplicity

## References

- OpenCode Plugin Implementation: `/Users/zouguojun/Workspace/opencode`
- OpenCode Plugin SDK: `@opencode-ai/plugin`
- Claude Code Plugin Format: `.claude-plugin/plugin.json`
- Aleph Current Implementation: `core/src/plugins/`

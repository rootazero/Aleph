# Change: Add Claude Code Compatible Plugin System

## Why

Aleph needs a plugin system to extend its capabilities. By making it fully compatible with Claude Code CLI plugins, we can leverage the existing Claude Code plugin ecosystem (official and third-party plugins) without requiring plugin developers to create separate implementations for Aleph.

## What Changes

- **ADDED** Plugin system module in `core/src/plugins/`
- **ADDED** Plugin manifest parser (compatible with Claude Code's `plugin.json`)
- **ADDED** SKILL.md parser for commands and skills
- **ADDED** Hook event system (mapping Claude Code hooks to Aleph EventBus)
- **ADDED** Plugin agent support (converting Claude Code agents to Aleph agents)
- **ADDED** MCP server integration (using Aleph's runtime managers)
- **ADDED** Plugin CLI commands and FFI exports
- **MODIFIED** Thinker module to support plugin skill injection
- **MODIFIED** EventBus to support hook subscriptions from plugins

## Impact

- Affected specs: New `plugin-system` capability
- Affected code:
  - `core/src/plugins/` (new module)
  - `core/src/thinker/` (skill injection)
  - `core/src/event/` (hook integration)
  - `core/src/agents/` (agent registration)
  - `core/src/mcp/` (runtime path resolution)
  - `core/src/ffi/` (FFI exports)

## Compatibility Strategy

The plugin system will be 100% compatible with Claude Code plugin directory structure:
```
plugin-root/
├── .claude-plugin/plugin.json    # Manifest
├── commands/*/SKILL.md           # User-triggered commands
├── skills/*/SKILL.md             # AI-invoked skills
├── agents/*/agent.md             # Custom agents
├── hooks/hooks.json              # Event hooks
├── .mcp.json                     # MCP servers
└── .lsp.json                     # LSP servers (future)
```

## Key Design Decisions

1. **Direct Format Compatibility**: Parse Claude Code plugin formats natively, no conversion needed
2. **Runtime Leverage**: Use Aleph's existing fnm/uv runtime managers for MCP servers
3. **Event Mapping**: Map Claude Code hook events to Aleph's EventBus events
4. **Skill Injection**: Inject SKILL.md content into Thinker's prompt system
5. **Agent Conversion**: Convert Claude Code agent definitions to Aleph's AgentDef format

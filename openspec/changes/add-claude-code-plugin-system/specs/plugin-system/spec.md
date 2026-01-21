## ADDED Requirements

### Requirement: Plugin Discovery

The system SHALL discover plugins from the configured plugins directory (`~/.config/aether/plugins/` by default).

#### Scenario: Plugin directory scanning
- **WHEN** the plugin manager initializes
- **THEN** it SHALL scan the plugins directory for subdirectories containing `.claude-plugin/plugin.json`
- **AND** each valid plugin directory SHALL be registered for loading

#### Scenario: Invalid plugin structure
- **WHEN** a directory does not contain `.claude-plugin/plugin.json`
- **THEN** it SHALL be skipped with a warning log
- **AND** scanning SHALL continue with remaining directories

---

### Requirement: Plugin Manifest Parsing

The system SHALL parse Claude Code compatible `plugin.json` manifest files.

#### Scenario: Minimal manifest
- **WHEN** a plugin.json contains only `{"name": "my-plugin"}`
- **THEN** the manifest SHALL be parsed successfully
- **AND** version, description, and other fields SHALL default to None

#### Scenario: Full manifest
- **WHEN** a plugin.json contains all optional fields (version, description, author, license, custom paths)
- **THEN** all fields SHALL be parsed and stored

#### Scenario: Invalid manifest
- **WHEN** a plugin.json is malformed or missing required `name` field
- **THEN** the plugin SHALL fail to load with a descriptive error

---

### Requirement: SKILL.md Parsing

The system SHALL parse SKILL.md files from `commands/` and `skills/` directories.

#### Scenario: Command skill parsing
- **WHEN** a file exists at `commands/hello/SKILL.md`
- **THEN** it SHALL be parsed as a command skill
- **AND** the skill name SHALL be "hello"
- **AND** the namespace SHALL be "plugin-name:hello"

#### Scenario: Agent skill parsing
- **WHEN** a file exists at `skills/code-review/SKILL.md`
- **THEN** it SHALL be parsed as an agent-invocable skill
- **AND** the `description` and `name` frontmatter fields SHALL be extracted

#### Scenario: Frontmatter extraction
- **WHEN** a SKILL.md contains YAML frontmatter
- **THEN** `description`, `name`, and `disable-model-invocation` fields SHALL be extracted
- **AND** the remaining markdown content SHALL be stored as the skill body

#### Scenario: Argument substitution
- **WHEN** a skill body contains `$ARGUMENTS`
- **THEN** it SHALL be replaced with user-provided arguments at execution time

---

### Requirement: Hook Event Handling

The system SHALL process hooks defined in `hooks/hooks.json`.

#### Scenario: Hook registration
- **WHEN** a plugin defines hooks in hooks.json
- **THEN** each hook SHALL be registered with the EventBus

#### Scenario: Event mapping
- **WHEN** a Claude Code hook event (e.g., PostToolUse) is registered
- **THEN** it SHALL be mapped to the corresponding Aether EventType (e.g., ToolCallCompleted)

#### Scenario: Matcher filtering
- **WHEN** a hook has a `matcher` pattern like "Write|Edit"
- **THEN** the hook SHALL only trigger for events matching that pattern

#### Scenario: Command hook execution
- **WHEN** a hook action has type "command"
- **THEN** the command SHALL be executed via shell
- **AND** `${CLAUDE_PLUGIN_ROOT}` SHALL be substituted with the plugin's root path

#### Scenario: Prompt hook execution
- **WHEN** a hook action has type "prompt"
- **THEN** the prompt SHALL be evaluated by the LLM
- **AND** `$ARGUMENTS` SHALL contain the event context

#### Scenario: Agent hook execution
- **WHEN** a hook action has type "agent"
- **THEN** the specified agent SHALL be invoked

---

### Requirement: Plugin Agent Support

The system SHALL parse and register agents from `agents/` directory.

#### Scenario: Agent parsing
- **WHEN** a file exists at `agents/reviewer/agent.md`
- **THEN** the agent SHALL be parsed with frontmatter (description, capabilities) and body (system prompt)

#### Scenario: Agent registration
- **WHEN** a plugin agent is loaded
- **THEN** it SHALL be converted to Aether's AgentDef format
- **AND** registered with the AgentRegistry
- **AND** the agent ID SHALL be namespaced as "plugin-name:agent-name"

---

### Requirement: MCP Server Integration

The system SHALL start MCP servers defined in `.mcp.json`.

#### Scenario: MCP config parsing
- **WHEN** a plugin contains `.mcp.json`
- **THEN** the server configurations SHALL be parsed

#### Scenario: Runtime path resolution
- **WHEN** an MCP server command is "npx" or "node"
- **THEN** the command path SHALL be resolved to Aether's fnm-managed Node.js installation

#### Scenario: Python runtime resolution
- **WHEN** an MCP server command is "uvx", "python", or "python3"
- **THEN** the command path SHALL be resolved to Aether's uv-managed Python installation

#### Scenario: MCP server startup
- **WHEN** plugin MCP servers are configured
- **THEN** they SHALL be started via Aether's existing McpClient

---

### Requirement: Plugin State Management

The system SHALL persist and manage plugin enabled/disabled state.

#### Scenario: State persistence
- **WHEN** a plugin is enabled or disabled
- **THEN** the state SHALL be persisted to `plugins.json`

#### Scenario: State restoration
- **WHEN** the plugin manager initializes
- **THEN** it SHALL restore plugin states from `plugins.json`
- **AND** disabled plugins SHALL not be loaded

#### Scenario: Default state
- **WHEN** a new plugin is discovered
- **THEN** it SHALL default to enabled state

---

### Requirement: Skill Injection

The system SHALL inject enabled plugin skills into the LLM prompt system.

#### Scenario: Auto-invocable skill injection
- **WHEN** a skill has `disable-model-invocation: false` or unset
- **THEN** the skill content SHALL be included in the system prompt
- **AND** the LLM MAY invoke it automatically based on context

#### Scenario: Command-only skill
- **WHEN** a skill has `disable-model-invocation: true`
- **THEN** it SHALL NOT be included in the system prompt
- **AND** it SHALL only be triggered by explicit user command

#### Scenario: Skill execution
- **WHEN** a user invokes `/plugin-name:skill-name arguments`
- **THEN** the skill content SHALL be processed with `$ARGUMENTS` substituted
- **AND** sent to the LLM for execution

---

### Requirement: Plugin Manager API

The system SHALL provide a PluginManager API for plugin lifecycle management.

#### Scenario: Load all plugins
- **WHEN** `load_all()` is called
- **THEN** all plugins in the plugins directory SHALL be discovered and loaded

#### Scenario: Load single plugin
- **WHEN** `load_plugin(path)` is called with a valid plugin path
- **THEN** that specific plugin SHALL be loaded (useful for development)

#### Scenario: Unload plugin
- **WHEN** `unload_plugin(name)` is called
- **THEN** the plugin's components SHALL be unregistered from all systems

#### Scenario: List plugins
- **WHEN** `list_plugins()` is called
- **THEN** information about all discovered plugins SHALL be returned
- **AND** each entry SHALL include name, version, enabled state, and component counts

---

### Requirement: FFI Exports

The system SHALL export plugin management functions via FFI for UI integration.

#### Scenario: List plugins FFI
- **WHEN** `plugin_list()` is called via FFI
- **THEN** a JSON array of plugin info SHALL be returned

#### Scenario: Enable/disable FFI
- **WHEN** `plugin_set_enabled(name, enabled)` is called via FFI
- **THEN** the plugin state SHALL be updated and persisted

#### Scenario: Execute skill FFI
- **WHEN** `plugin_execute_skill(plugin, skill, args)` is called via FFI
- **THEN** the skill SHALL be executed and the result returned

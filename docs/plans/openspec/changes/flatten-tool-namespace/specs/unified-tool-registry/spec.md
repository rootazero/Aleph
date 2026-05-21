# Spec Delta: Unified Tool Registry - Flat Namespace

## ADDED Requirements

### Requirement: Flat Tool Registration

All tools from different sources (MCP, Skill, Custom) SHALL be registered as root-level commands without namespace prefixes.

**Rationale:** Users should invoke tools directly by name without remembering which namespace contains the tool.

#### Scenario: MCP tool registered as root command
Given an MCP server "git-server" provides a tool "git"
When ToolRegistry.register_mcp_tools() is called
Then the tool is registered with name "git" (not "mcp git")
And the tool routing regex is "^/git\s+"
And the tool is accessible via "/git status" command

#### Scenario: Skill registered as root command
Given a skill "refine-text" is installed
When ToolRegistry.register_skills() is called
Then the skill is registered with name "refine-text"
And the skill routing regex is "^/refine-text\s*"
And the skill is accessible via "/refine-text" command

---

### Requirement: Conflict Resolution Priority

When multiple tools have the same name, the system SHALL resolve conflicts using a defined priority order: Builtin > Native > Custom > MCP > Skill.

**Rationale:** Ensures consistent behavior and prevents external tools from overriding system functionality.

#### Scenario: MCP tool conflicts with Builtin
Given system has builtin "/search" command
And MCP server provides a tool named "search"
When ToolRegistry registers the MCP tool
Then the builtin "/search" remains unchanged
And the MCP tool is renamed to "search-mcp"
And a warning is logged about the conflict

#### Scenario: Skill conflicts with MCP tool
Given MCP server has a tool named "translate"
And a skill named "translate" is installed
When ToolRegistry registers the skill
Then the MCP tool "translate" remains unchanged
And the skill is renamed to "translate-skill"

#### Scenario: No conflict between different names
Given system has builtin "/search" command
And MCP server provides a tool named "git"
When ToolRegistry registers the MCP tool
Then the MCP tool is registered as "/git" without renaming
And no warning is logged

---

### Requirement: Source Badge Display

Command completion and Settings UI SHALL display each tool with a source badge indicating its origin (System, MCP, Skill, Custom).

**Rationale:** Users can see where each tool comes from without requiring namespace prefixes.

#### Scenario: Command completion shows source badges
Given the following tools are registered:
  | name    | source  |
  | search  | Builtin |
  | git     | MCP     |
  | refine  | Skill   |
  | en      | Custom  |
When user types "/" in command input
Then completion list shows:
  | name    | badge   |
  | search  | System  |
  | git     | MCP     |
  | refine  | Skill   |
  | en      | Custom  |

#### Scenario: Settings shows tools grouped by source
Given the following tools are registered from various sources
When user opens Settings > Routing > Preset Rules
Then tools are displayed in sections:
  | section | tools         |
  | System  | search, video |
  | MCP     | git, fs       |
  | Skill   | refine-text   |
  | Custom  | en, zh        |

---

## MODIFIED Requirements

### Requirement: Command Completion Navigation

The command completion system SHALL display all tools in a flat list without namespace navigation.

**Previous behavior:** Selecting "/mcp" navigated into a namespace showing MCP tools.
**New behavior:** All tools appear at root level; no namespace navigation needed.

#### Scenario: No namespace navigation
Given user types "/" in command input
When completion list is displayed
Then all tools appear in a single flat list
And there is no "/mcp" or "/skill" namespace to navigate into
And selecting any tool directly invokes it

#### Scenario: Filtering works across all sources
Given the following tools are registered:
  | name    | source |
  | search  | System |
  | search-mcp | MCP |
  | git     | MCP    |
When user types "/s"
Then completion shows:
  | name       | badge  |
  | search     | System |
  | search-mcp | MCP    |
And "git" is not shown (doesn't match filter)

---

## REMOVED Requirements

### Requirement: MCP Namespace Builtin

The "/mcp" builtin command with namespace navigation shall be removed.

**Rationale:** Flat namespace eliminates the need for a dedicated MCP namespace.

#### Scenario: /mcp not in builtin commands
Given app is started
When BUILTIN_COMMANDS is accessed
Then it does not contain an entry for "mcp"
And only "search", "video", "chat" are builtin commands

---

### Requirement: Skill Namespace Builtin

The "/skill" builtin command with namespace navigation shall be removed.

**Rationale:** Skills are accessible directly by name.

#### Scenario: /skill not in builtin commands
Given app is started
When BUILTIN_COMMANDS is accessed
Then it does not contain an entry for "skill"

---

## Cross-References

- Depends on: `unify-tool-registry` (ToolRegistry infrastructure)
- Related to: `ai-routing` (routing rules for flat commands)
- Related to: `command-completion` (UI for flat command list)

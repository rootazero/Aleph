# Plugins, Skills & MCP

## Plugins

### CLI Commands

```bash
# Marketplace
aleph plugin marketplace add <source>    # GitHub owner/repo or local path
aleph plugin marketplace list
aleph plugin marketplace update [name]
aleph plugin marketplace remove <name>

# Management
aleph plugin list
aleph plugin install <source>            # From marketplace or URL
aleph plugin install <name> --scope user # Scope: user|project|local

# Legacy
aleph plugins install <url>              # Git URL
aleph plugins uninstall <name>
aleph plugins enable <name>              # Remove .disabled marker
aleph plugins disable <name>             # Create .disabled marker
```

### Source Detection

| Format | Behavior |
|--------|----------|
| `github:owner/repo` | Fetch latest GitHub release .zip |
| `github:owner/repo/plugin-name` | Fetch release, extract specific plugin |
| `*.zip` | Install from zip directly |
| `https://...` URL | Clone git repository |
| Bare name | Search marketplace cache |

### Manifest Discovery (priority)

1. `aleph.plugin.toml`
2. `aleph.plugin.json`
3. `package.json` (DEPRECATED)
4. `.claude-plugin/plugin.toml`
5. `.claude-plugin/plugin.json`
6. Auto-discover (skills/, agents/, commands/, hooks/, .mcp.json)

### Plugin ID Rules

Lowercase, invalid chars → hyphens, no consecutive/leading/trailing hyphens.
Example: "My Plugin 123" → "my-plugin-123"

### Plugin Types

| Kind | Description |
|------|-------------|
| `Wasm` | WebAssembly (.wasm) |
| `Mcp` | MCP server (.mcp.json) |
| `Static` | Markdown skills, agents, commands |
| `NodeJs` | DEPRECATED |

### Common Errors

| Error | Fix |
|-------|-----|
| `InvalidManifest` | Check manifest file format |
| `MissingField` | Add required field |
| `InvalidPluginName` | Use a-z, 0-9, hyphens only |
| `PluginNotFound` | Check path exists |

---

## Skills

### CLI Commands

```bash
aleph skill list
aleph skill install <source>
aleph skill reload <name>
aleph skill delete <name>
```

### SKILL.md Format

```markdown
---
name: skill-name                   # REQUIRED
description: Short description     # REQUIRED
scope: system                      # Optional: system|tool|standalone|disabled
user_invocable: true               # Optional
disable_model_invocation: false    # Optional
bound_tool: tool_name              # Optional
eligibility:                       # Optional
  os: [macos, linux]
  required_bins: [python3]
---

Instructions go here.
```

### Directory Structure

Each skill: `~/.aleph/skills/my-skill/SKILL.md` or `~/.aleph/skills/my-skill.md`

Config:
```toml
[skills]
enabled = true
skills_dir = "skills"           # Relative to ~/.aleph/, or absolute
auto_match_enabled = false
```

### Common Errors

| Error | Fix |
|-------|-----|
| `NoFrontmatter` | Add `---` YAML block |
| YAML parse error | Fix YAML syntax |
| Invalid scope | Use: system, tool, standalone, disabled |

---

## MCP Servers

### .mcp.json (plugin root)

```json
{
  "mcpServers": {
    "server-name": {
      "command": "node",
      "args": ["${ALEPH_PLUGIN_ROOT}/src/server.js"],
      "env": {
        "NODE_ENV": "production"
      }
    }
  }
}
```

Variables: `${ALEPH_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_ROOT}` (alias) → absolute plugin path.

Server IDs namespaced: `plugin:{plugin-id}/{server-name}`

### MCP in config.toml

```toml
[mcp]
enabled = true

[[mcp.external_servers]]
name = "my-server"
command = "node"
args = ["path/to/server.js"]
env = {}
timeout_seconds = 30
```

### Common Errors

| Error | Fix |
|-------|-----|
| JSON parse error | Validate JSON syntax |
| Missing mcpServers | Wrap in `{"mcpServers": {...}}` |
| Server not starting | Check command path and args |

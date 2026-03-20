# Plugin System: Claude Code Compatibility Redesign

**Date**: 2026-03-20
**Status**: Draft
**Scope**: core/src/extension/, apps/cli/, Aleph-plugins repo

## Summary

Redesign Aleph's plugin system to achieve **single-direction compatibility with Claude Code** (Aleph loads Claude Code plugins natively) while preserving Aleph-unique capabilities (WASM runtime, channels, providers) as a **superset** via `[aleph]` extension fields. Migrate from custom IPC runtime to MCP Server model. Adopt Claude Code's namespace, CLI command structure, marketplace system, and scope management — all using **TOML as the primary format** with JSON read-compatibility.

**Core principle: Write TOML, Read TOML+JSON.**

## Decision Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Compatibility direction | Single-direction + superset | Aleph loads CC plugins; Aleph plugins are CC superset |
| Manifest format | Migrate to `.claude-plugin/plugin.toml`, read `.claude-plugin/plugin.json` | One manifest, CC compat, Rust ecosystem alignment |
| Marketplace sources | GitHub repo + local path (minimum viable) | 90% coverage, expand later |
| CLI command | `aleph plugin` (singular) | Align with CC `claude plugin` |
| Scope model | user / project / local + agent-level (Aleph-only) | CC-compatible + multi-agent support |
| Runtime model | MCP Server for Node.js; retain WASM native | Standardize on MCP, preserve WASM sandbox advantage |
| Config format | TOML for all Aleph config, JSON read-compat for CC plugins | Rust ecosystem consistency |

## Implementation Phases

- **P0**: Manifest compatibility layer
- **P1**: Namespace + CLI alignment
- **P2**: Marketplace system
- **P3**: Scope management
- **P4**: Runtime migration (Node.js IPC → MCP Server)
- **P5**: Cleanup (remove deprecated code)

---

## Section 1: Manifest Compatibility Layer (P0)

### Goal

Aleph loads both formats, outputs a unified `PluginManifest` struct.

### plugin.toml Superset Schema

```toml
# .claude-plugin/plugin.toml — Aleph recommended format
name = "my-plugin"
version = "1.0.0"
description = "Brief description"
repository = "https://github.com/..."
license = "MIT"
keywords = ["keyword1", "keyword2"]

# Component paths (supplement defaults, don't replace)
commands = ["./commands/"]
agents = "./agents/"
skills = "./skills/"
hooks = "./hooks/hooks.json"
mcp-servers = "./.mcp.json"
lsp-servers = "./.lsp.json"
output-styles = "./styles/"

[author]
name = "Author Name"
email = "author@example.com"
url = "https://github.com/author"

# Aleph-only extensions (ignored by Claude Code)
[aleph]
runtime = "mcp"                    # "mcp" | "wasm" | "static"

[[aleph.channels]]
id = "telegram"
handler = "handleTelegram"

[[aleph.providers]]
id = "custom-llm"
handler = "handleProvider"

[[aleph.services]]
id = "metrics-collector"
handler = "startMetrics"

[aleph.permissions]
network = true
filesystem = "read"
shell = false
```

### plugin.json Compatibility (read-only)

```json
{
  "name": "cc-community-plugin",
  "version": "1.0.0",
  "description": "A Claude Code community plugin",
  "skills": "./skills/",
  "agents": "./agents/",
  "hooks": "./hooks/hooks.json",
  "mcpServers": "./.mcp.json"
}
```

### Discovery Priority (per plugin directory)

1. `.claude-plugin/plugin.toml` — preferred (Aleph native)
2. `.claude-plugin/plugin.json` — fallback (Claude Code compat)
3. `aleph.plugin.toml` — deprecated, warn on load
4. `aleph.plugin.json` — deprecated, warn on load
5. `package.json` with `aleph` field — deprecated, warn on load
6. No manifest — auto-discover mode (scan `skills/`, `agents/`, `hooks/`, `.mcp.json`, `.lsp.json`)

### Auto-Discovery (No Manifest)

When no manifest is found, scan default locations (aligned with Claude Code behavior):

- `skills/*/SKILL.md` → skills
- `agents/*.md` → agents
- `commands/*.md` → commands (legacy)
- `hooks/hooks.json` → hooks
- `.mcp.json` → MCP servers
- `.lsp.json` → LSP servers

Plugin name derived from directory name.

### Environment Variables

Available in skill/agent content, hook commands, MCP/LSP configs:

- `${CLAUDE_PLUGIN_ROOT}` — absolute path to plugin installation directory
- `${CLAUDE_PLUGIN_DATA}` — persistent data directory (`~/.aleph/plugins/data/{id}/`)

### Code Changes

- New: `manifest/claude_plugin_toml.rs` — parse `.claude-plugin/plugin.toml`
- New: `manifest/claude_plugin_json.rs` — parse `.claude-plugin/plugin.json`
- Modify: `discovery/scanner.rs` — new scan order + auto-discover mode
- Modify: `manifest/types.rs` — add `aleph_extensions: Option<AlephExtensions>` to `PluginManifest`
- Deprecate: `manifest/aleph_plugin_toml/`, `manifest/package_json.rs`

---

## Section 2: Namespace System (P1)

### Goal

All plugin components use `plugin-name:component-name` namespace, matching Claude Code convention.

### Naming Rules

| Scope | Format | Example |
|-------|--------|---------|
| Plugin skill | `plugin-name:skill-name` | `cli-anything:run-command` |
| Plugin agent | `plugin-name:agent-name` | `diagnostics:health-checker` |
| Plugin tool (MCP) | `mcp__plugin_<plugin>_<server>__<tool>` | MCP standard naming |
| Built-in skill | short name | `/memory-search` |
| Built-in tool | short name | `web_search` |

### User Interaction

```
/diagnostics:health-check              # plugin skill
/cli-anything:run-command ls -la       # with arguments
/memory-search                         # built-in, no namespace
```

### Conflict Resolution

- Same plugin name across marketplaces: `name@marketplace` disambiguation
- Same-plugin component names: must be unique (manifest validation error)
- Built-in vs plugin name collision: built-in wins, log warning

### Code Changes

- New: `ComponentId` struct — `{ plugin: Option<String>, name: String }` for unified identification
- Modify: `PluginRegistry` — all registrations keyed by `plugin_name:name` composite key
- Modify: Gateway inbound router — parse `name:subname` command format
- Modify: tool/skill/agent invocation paths — adapt to `ComponentId`

---

## Section 3: CLI Command System (P1)

### Goal

`aleph plugin` command format fully aligned with Claude Code.

### Command Reference

```bash
# === Plugin Management ===
aleph plugin install <name>[@<marketplace>] [--scope user|project|local]
aleph plugin uninstall <name>[@<marketplace>] [--scope user|project|local] [--keep-data]
aleph plugin enable <name>[@<marketplace>] [--scope user|project|local]
aleph plugin disable <name>[@<marketplace>] [--scope user|project|local]
aleph plugin update <name>[@<marketplace>] [--scope user|project|local]
aleph plugin list
aleph plugin validate <path>

# === Marketplace Management ===
aleph plugin marketplace add <owner/repo>       # GitHub marketplace
aleph plugin marketplace add <local-path>        # local marketplace
aleph plugin marketplace list
aleph plugin marketplace update <name>
aleph plugin marketplace remove <name>

# === Development ===
aleph plugin init [--template <type>]
aleph --plugin-dir ./my-plugin                   # local dev loading

# === Deprecated ===
aleph plugins ...                                # alias → aleph plugin, print deprecation warning
```

### In-Session Commands (via LLM)

```
/plugin install cli-anything
/plugin marketplace add HKUDS/CLI-Anything
/plugin list
/reload-plugins
```

### @marketplace Resolution

- `cli-anything` — search all registered marketplaces, install if unique match
- `cli-anything@official` — target specific marketplace
- Ambiguous match (multiple marketplaces) — error, prompt user to specify

### Scope Defaults

- `install` → default `user`
- `uninstall/enable/disable` → auto-detect plugin's scope
- `update` → update in plugin's current scope

### Code Changes

- Rewrite: `apps/cli/src/commands/plugin_cmd.rs` — singular, new subcommand structure
- Deprecate: `apps/cli/src/commands/plugins_cmd.rs` — wrapper with deprecation warning
- New: Gateway RPC methods `plugin.install`, `plugin.uninstall`, `plugin.marketplace.add`, etc.
- New: `plugin.marketplace.*` RPC handlers

---

## Section 4: Marketplace System (P2)

### Goal

Replace `plugins-index.json` with Claude Code-compatible marketplace, TOML as primary format. Support GitHub repo and local path sources.

### Marketplace Registration Storage

```toml
# ~/.aleph/settings.toml
[plugin_marketplaces]

[plugin_marketplaces.aleph-official]
source = "rootazero/Aleph-plugins"
type = "github"

[plugin_marketplaces.cli-anything]
source = "HKUDS/CLI-Anything"
type = "github"
```

### Built-in Marketplace

`aleph-official` pointing to `rootazero/Aleph-plugins` is always available, no manual registration needed. Similar to Claude Code's `claude-plugins-official`.

### marketplace.toml Format

```toml
# .claude-plugin/marketplace.toml
name = "aleph-official"

[owner]
name = "Rootazero"
url = "https://github.com/rootazero"

[metadata]
description = "Aleph official plugin marketplace"
version = "1.0.0"
plugin-root = "./plugins"

[[plugins]]
name = "diagnostics"
source = "./plugins/diagnostics"
description = "System health & performance monitoring"
version = "0.1.0"

[[plugins]]
name = "media-office"
source = "./plugins/media-office"
description = "DOCX/XLSX/PPTX extraction"
version = "0.1.0"
```

Also reads `marketplace.json` for third-party CC-compatible marketplaces.

### Install Flow

```
aleph plugin install diagnostics
```

1. Iterate all registered marketplaces
2. Read `marketplace.toml` (or `.json`) from cache (`~/.aleph/plugins/cache/<marketplace>/`)
3. Match plugin name
4. Unique match → copy plugin directory to target scope path
5. Multiple matches → error, prompt `diagnostics@aleph-official`
6. Parse `.claude-plugin/plugin.toml` (or `.json`, or auto-discover) → register to `PluginRegistry`

### Cache Management

```bash
aleph plugin marketplace update aleph-official
```

- GitHub source: `git clone --depth 1` or `git pull` into `~/.aleph/plugins/cache/<name>/`
- Local source: read directly, no cache

### Code Changes

- New: `core/src/extension/marketplace/` module
  - `mod.rs` — MarketplaceManager trait + implementation
  - `types.rs` — MarketplaceConfig, MarketplaceManifest, PluginEntry
  - `github_source.rs` — GitHub clone/pull logic
  - `local_source.rs` — local path reader
- Modify: `discovery/mod.rs` — discover plugins from marketplace cache
- New: Gateway RPC `plugin.marketplace.add/list/update/remove`
- Remove: `plugins-index.json` loading logic

---

## Section 5: Scope Management (P3)

### Goal

Implement user/project/local three-tier scope for plugin installation and visibility.

### Scope Definitions

| Scope | Settings File | Plugin Storage | Purpose |
|-------|--------------|----------------|---------|
| `user` | `~/.aleph/settings.toml` | `~/.aleph/plugins/installed/` | Personal, all projects |
| `project` | `<project>/.aleph/settings.toml` | `<project>/.aleph/plugins/` | Team, in VCS |
| `local` | `<project>/.aleph/settings.local.toml` | `<project>/.aleph/plugins.local/` | Personal project-level, gitignored |

### Plugin Declaration in Settings

```toml
# ~/.aleph/settings.toml
[plugins]

[plugins."diagnostics@aleph-official"]
enabled = true
version = "0.1.0"

[plugins."cli-anything@cli-anything"]
enabled = false
```

### Priority (high → low)

`local` > `project` > `user` > `agent-level` (Aleph-only) > `bundled`

Same-name plugin in multiple scopes: higher scope wins, lower is shadowed (no error).

### Agent-Level Scope (Aleph-only, preserved)

`~/.aleph/agents/<id>/plugins/` — plugins scoped to a specific agent, only active in that agent's sessions. This supports Aleph's multi-agent architecture and has no Claude Code equivalent.

### Code Changes

- New: `core/src/extension/scope.rs` — `PluginScope` enum + path resolution
- Modify: `discovery/mod.rs` — scope-ordered scanning with shadow handling
- Modify: `core/src/config/` — read/write `plugins` and `plugin_marketplaces` from scope-specific settings files
- Install/uninstall operations write to the appropriate scope's settings file and directory

---

## Section 6: Runtime Migration (P4)

### Goal

Migrate Node.js plugins from custom IPC to MCP Server model. Retain WASM native runtime.

### Architecture Transition

```
Before:
  Core → PluginLoader → Node.js subprocess (custom JSON-RPC over stdio)
  Core → PluginLoader → WASM Extism (host function calls)

After:
  Core → MCP Client → MCP Server (standard MCP protocol over stdio/HTTP)
  Core → WASM Extism (retained for wasm plugins)
```

### Runtime Model

| `[aleph] runtime` | Loading | Use Case |
|--------------------|---------|----------|
| omitted / `"mcp"` | Read `.mcp.json`, launch MCP server | Most plugins (Node.js, Python, Go) |
| `"wasm"` | Extism direct load | High-perf sandbox plugins |
| `"static"` | Markdown only, no runtime | Skills/agents/commands only |

### Node.js Plugin Migration

Replace `plugin-host.js` (custom IPC) with MCP Server SDK. Each Node.js plugin becomes a standard MCP server declared in `.mcp.json`:

```json
{
  "mcpServers": {
    "diagnostics": {
      "command": "node",
      "args": ["${CLAUDE_PLUGIN_ROOT}/src/server.js"],
      "env": {}
    }
  }
}
```

### WASM Runtime (Retained)

WASM plugins retain Extism direct loading — the sandbox security and performance advantages are Aleph's differentiator. Not wrapped as MCP.

```toml
# .claude-plugin/plugin.toml
name = "diff-viewer"
version = "0.1.0"

[aleph]
runtime = "wasm"
entry = "target/wasm32-wasi/release/diff_viewer.wasm"

[aleph.permissions]
filesystem = "read"
```

### Code Changes

- Remove: `runtime/nodejs/ipc.rs`, `runtime/nodejs/process.rs`, `runtime/nodejs/plugin-host.js`
- Refactor: `PluginLoader` — from "manage Node.js/WASM subprocess" to "manage MCP server lifecycle + WASM runtime"
- Retain: `runtime/wasm/` (Aleph-only)
- Retain: reusable Node.js process management if needed by MCP server launcher

---

## Section 7: Aleph-plugins Repository Migration

### Target Structure

```
Aleph-plugins/
├── .claude-plugin/
│   └── marketplace.toml
├── plugins/
│   ├── diagnostics/
│   │   ├── .claude-plugin/
│   │   │   └── plugin.toml
│   │   ├── skills/
│   │   │   └── health-check/
│   │   │       └── SKILL.md
│   │   ├── .mcp.json
│   │   ├── hooks/
│   │   │   └── hooks.json
│   │   ├── src/
│   │   │   └── server.js          # MCP server (replaces index.js)
│   │   └── package.json
│   ├── diff-viewer/
│   │   ├── .claude-plugin/
│   │   │   └── plugin.toml        # runtime = "wasm"
│   │   ├── src/
│   │   └── Cargo.toml
│   ├── media-office/
│   ├── llm-task/
│   ├── memory-analytics/
│   ├── voice-call/
│   └── phone-control/
├── CLAUDE.md
└── README.md
```

### Migration Mapping (per plugin)

| Old | New | Notes |
|-----|-----|-------|
| `aleph.plugin.toml` (root) | `.claude-plugin/plugin.toml` | Format adjusted, moved to subdirectory |
| `src/index.js` (custom IPC) | `src/server.js` (MCP SDK) | Node.js plugins need entry rewrite |
| `plugins-index.json` (root) | `.claude-plugin/marketplace.toml` | Format conversion |
| Tools declared in manifest | Tools registered in MCP server | Tool definitions move from manifest to MCP |
| Hooks declared in manifest | `hooks/hooks.json` | Separate file, CC format |

### WASM Plugins

`diff-viewer` and `memory-analytics` keep WASM runtime, no MCP migration needed.

---

## Section 8: Deprecation & Cleanup (P5)

### Timeline

| Phase | Content | Behavior |
|-------|---------|----------|
| P0-P1 release | Old formats deprecated | Loading `aleph.plugin.toml` prints warning |
| P4 completion | Node.js custom IPC deprecated | Old IPC plugins still load with warning |
| P5 (cleanup) | Remove old code | Delete deprecated parsers, IPC runtime, plural CLI |

### Removal List (P5)

**Code:**
- `manifest/aleph_plugin_toml/` — entire directory
- `manifest/package_json.rs` — `package.json` with `aleph` field parser
- `runtime/nodejs/ipc.rs` — custom JSON-RPC protocol
- `runtime/nodejs/process.rs` — Node.js subprocess management
- `runtime/nodejs/plugin-host.js` — IPC host script
- `apps/cli/src/commands/plugins_cmd.rs` — deprecated plural entry

**Data:**
- `plugins-index.json` loading logic
- PluginRegistry legacy format compat branches

### Retained

- `runtime/wasm/` — Aleph-only capability
- `runtime/nodejs/mod.rs` — if reused by MCP server launcher
- `.mcp.json` / `hooks.json` — CC standard format, not subject to TOML migration

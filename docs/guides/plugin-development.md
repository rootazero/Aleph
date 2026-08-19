# Plugin Development Guide

This guide covers everything you need to build, test, and distribute Aleph plugins.

## Table of Contents

1. [Overview](#overview)
2. [Quick Start](#quick-start)
3. [Manifest Format](#manifest-format)
4. [MCP Plugin Development](#mcp-plugin-development-node--typescript--anything)
5. [WASM Plugin Development](#wasm-plugin-development)
6. [Static Plugin Development](#static-plugin-development)
7. [Tools](#tools)
8. [Hooks](#hooks)
9. [Services](#services)
10. [Permissions](#permissions)
11. [Configuration Schema](#configuration-schema)
12. [Testing](#testing)
13. [Packaging](#packaging)
14. [Installation & Discovery](#installation--discovery)
15. [Plugin Variables](#plugin-variables)

---

## Overview

Aleph plugins extend the AI assistant with custom **tools**, **hooks**,
**skills**, **commands**, **agents** and background **services**. Plugins are
defined by a `.claude-plugin/plugin.toml` manifest and use one of three
runtimes:

| Runtime | Language | Use Case | Sandboxing |
|---------|----------|----------|------------|
| **`mcp`** | Anything (Node/TS/Python/…) | Rich integrations, API clients | Process-level; the server is a separate process |
| **`wasm`** | Rust (via Extism) | High-performance, security-sensitive | Extism sandbox with capability kernel |
| **`static`** | Markdown | Skills/commands (prompt injection), no code | N/A (content only) |

> Channels, providers and HTTP routes were listed here as contribution points.
> They are not: the manifest has no field for any of them, so declaring one is
> silently ignored. Adding a channel or provider means changing core.

### What Plugins Can Do

- **Tools** — Functions the AI can call (e.g., `video_understand`, `web_search`)
- **Hooks** — Intercept or observe lifecycle events (e.g., `PreToolUse`, `SessionStart`)
- **Skills** — Markdown instructions injected into the AI's prompt
- **Commands** — User-triggered slash commands (e.g., `/status`, `/deploy`)
- **Services** — Long-running background processes managed by Aleph
- **Channels** — Messaging platform integrations (e.g., Slack, Telegram)
- **Providers** — Custom AI model providers
- **HTTP Routes** — REST API endpoints served by the plugin

---

## Quick Start

### Prerequisites

Run `aleph plugin doctor` to check your environment:

```bash
aleph plugin doctor
```

This checks for Node.js and npm (needed to run MCP-server plugins written in JS/TS), the WASM compilation target, and the global plugin directory.

### Scaffold a New Plugin

```bash
# MCP server plugin (Node/TypeScript — `nodejs`/`node`/`js`/`ts` are aliases)
aleph plugin init my-plugin --type mcp

# WASM plugin (Rust)
aleph plugin init my-wasm-plugin --type wasm

# Static plugin (Markdown skill)
aleph plugin init my-skill --type static
```

This creates a directory with `.claude-plugin/plugin.toml` and template source
files. `aleph plugin validate .` reads the same manifest the server will, and
rejects a runtime the host cannot load — so a green check means the plugin is
loadable, which it did not before 2026-08-19.

### Build and Validate

```bash
cd my-plugin

# For MCP plugins:
npm install

# For WASM plugins:
cargo build --target wasm32-wasi --release

# Validate the plugin
aleph plugin validate .
```

### Development Loop

```bash
# Start dev mode with hot-reload (watches for file changes)
aleph plugin dev .
```

---

## Manifest Format

A plugin's manifest is `.claude-plugin/plugin.toml`. That is Claude Code's
location with an Aleph superset bolted on: flat top-level fields Claude Code
understands, plus an optional `[aleph]` block it ignores by design. A manifest
written this way loads in both hosts.

> **This section said the opposite until 2026-08-19.** It told authors that
> "every plugin must have an `aleph.plugin.toml`" and called that "the
> preferred manifest format", while `PLUGIN_SYSTEM.md` called
> `.claude-plugin/plugin.toml` 首选 and the loader printed a deprecation
> warning for `aleph.plugin.toml` on every load. `aleph.plugin.toml` still
> parses; it is deprecated, and it no longer expresses anything the preferred
> format cannot.

### Minimal Manifest

```toml
name = "my-plugin"
version = "0.1.0"

[aleph]
runtime = "mcp"        # "mcp" | "wasm" | "static" — omit for "static"
```

`runtime` accepts exactly the three values the host can load. `kind = "nodejs"`
appeared throughout this guide and is **not** one of them: `PluginKind` rejects
it with `unknown variant`, so a manifest declaring it never loads. There is no
Node.js plugin runtime — a plugin written in Node runs as an MCP stdio server
(`runtime = "mcp"` + `.mcp.json`), which is what `aleph plugin init --type
nodejs` now scaffolds.

### Full Manifest Reference

> The block below is the **deprecated** `aleph.plugin.toml` dialect, kept
> because installed plugins still use it. In `.claude-plugin/plugin.toml` the
> identity fields are flat at the top level and everything under `[plugin]` /
> `[[tools]]` / `[[hooks]]` / `[[commands]]` / `[[services]]` / `[prompt]`
> moves inside `[aleph]` — `[[aleph.tools]]`, `[aleph.prompt]`,
> `[aleph.config_schema]`, and so on. The two dialects mean exactly the same
> thing; `cc_plugin_json::tests::both_cc_dialects_agree_on_the_superset` holds
> them equal.

```toml
[plugin]
id = "my-plugin"                    # Required. Lowercase, alphanumeric + hyphens
name = "My Plugin"                  # Display name (defaults to id)
version = "1.0.0"                   # Semver version
description = "What this plugin does"
kind = "mcp"                        # "mcp", "wasm", or "static"
entry = ".mcp.json"                 # Entry point relative to plugin root
homepage = "https://example.com"
repository = "https://github.com/user/repo"
license = "MIT"
keywords = ["productivity", "video"]

[plugin.author]
name = "Your Name"
email = "you@example.com"
url = "https://yoursite.com"

# --- Permissions ---
[permissions]
network = true                      # HTTP, WebSocket access
filesystem = "read"                 # "read", "write", or true (full)
env = true                          # Environment variable access
shell = false                       # Shell execution

# --- Tools ---
[[tools]]
name = "my_tool"
description = "Does something useful"
handler = "handleMyTool"            # Function name in plugin code
parameters = { type = "object", properties = { query = { type = "string" } } }

[[tools]]
name = "another_tool"
description = "Another tool"
handler = "handleAnother"
instruction_file = "tools/another.md"  # Markdown instructions for the tool

# --- Hooks ---
[[hooks]]
event = "PreToolUse"                # Hook event name
kind = "observer"                   # "observer" (read-only) or "interceptor" (can modify)
handler = "onPreToolUse"
priority = "high"                   # "low", "normal", "high"
filter = "Bash"                     # Regex filter (for tool-based events)

# --- Commands ---
[[commands]]
name = "deploy"
description = "Deploy to production"
handler = "handleDeploy"
prompt_file = "commands/deploy.md"  # Markdown with $ARGUMENTS placeholder

# --- Services ---
[[services]]
name = "watcher"
description = "File watcher service"
start_handler = "startWatcher"
stop_handler = "stopWatcher"

# --- System Prompt ---
[prompt]
file = "SYSTEM.md"                  # Prompt file injected into AI context
scope = "system"                    # "system" or "user"

# --- Advanced Capabilities ---
[capabilities]
dynamic_tools = true                # Plugin can register tools at runtime
dynamic_hooks = false

# WASM-only: Sandbox capabilities
[capabilities.workspace]
allowed_prefixes = ["docs/", "config/"]

[capabilities.http]
timeout_secs = 30

[[capabilities.http.allowlist]]
host = "api.example.com"
path_prefix = "/v1/"
methods = ["GET", "POST"]

[[capabilities.http.credentials]]
secret_name = "api_token"
host_patterns = ["api.example.com"]
[capabilities.http.credentials.inject]
type = "bearer"

[capabilities.secrets]
allowed_patterns = ["my_plugin_*"]

# --- Configuration Schema ---
[plugin.config_schema]
type = "object"
properties = { api_key = { type = "string" }, max_results = { type = "number" } }

[plugin.config_ui_hints.api_key]
label = "API Key"
help = "Your API key for the service"
sensitive = true
placeholder = "sk-..."

[plugin.config_ui_hints.max_results]
label = "Max Results"
help = "Maximum number of results to return"
advanced = true
```

### Default Entry Points

If `entry` is not specified, the default depends on the plugin kind:

| Runtime | Default Entry |
|---------|--------------|
| `wasm` | `plugin.wasm` |
| `mcp` | `.mcp.json` |
| `static` | `.` (the plugin directory itself) |

### Manifest Priority

When several manifests exist, `parse_manifest_from_dir_sync` picks the first
of these — the list is the code's order, not an aspiration:

1. `.claude-plugin/plugin.toml` — preferred
2. `.claude-plugin/plugin.json` — Claude Code's native format
3. `aleph.plugin.toml` — deprecated; warns on every load
4. no manifest at all — auto-discovery from `skills/`, `agents/`, `commands/`,
   `hooks/`, `.mcp.json`

The component fields (`skills`, `commands`, `agents`, `hooks`, `mcp-servers`)
each accept a path, an array of paths, or — for `hooks` and `mcp-servers` — the
configuration inlined, matching what Claude Code accepts.

---

## MCP Plugin Development (Node / TypeScript / anything)

A plugin written in a language that is not Rust runs as an **MCP stdio
server**. Aleph starts it from `.mcp.json`, speaks MCP to it, and every tool
the server registers appears in the agent's tool list namespaced
`<plugin>__<tool>`.

> **This section described a Node.js plugin runtime that does not exist.** It
> documented a JSON-RPC-over-stdio host process and an `api.registerTool(...)`
> entry point; `src/extension/runtime/` contains `wasm` and nothing else, and
> `registerTool` had exactly one occurrence in the whole tree — the scaffolder
> template that wrote it. `aleph plugin init --type nodejs` produced a plugin
> that could never load. It now scaffolds what is written below.

### Project Structure

```
my-plugin/
  .claude-plugin/
    plugin.toml
  .mcp.json
  package.json
  src/
    index.mjs         # the MCP server
```

### Manifest

```toml
name = "my-plugin"
version = "0.1.0"

[aleph]
runtime = "mcp"
entry = ".mcp.json"
```

### `.mcp.json`

```json
{
  "mcpServers": {
    "my-plugin": {
      "command": "node",
      "args": ["${CLAUDE_PLUGIN_ROOT}/src/index.mjs"]
    }
  }
}
```

`${CLAUDE_PLUGIN_ROOT}` is expanded by the host, so the server runs from the
install directory wherever the plugin ends up. For state that must survive
`plugin update` (which swaps the install directory atomically), use
`${CLAUDE_PLUGIN_DATA}` — see [Plugin Variables](#plugin-variables).

### Entry Point

```javascript
// src/index.mjs
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const server = new McpServer({ name: "my-plugin", version: "0.1.0" });

server.tool(
  "search",
  "Search something useful",
  { query: z.string().describe("Search query") },
  async ({ query }) => ({ content: [{ type: "text", text: `results for ${query}` }] }),
);

await server.connect(new StdioServerTransport());
```

### Configuration

Values the operator sets (see [Configuration Schema](#configuration-schema))
arrive as environment variables: `ALEPH_PLUGIN_CONFIG` holds the whole object
as JSON, and each scalar field is also exported as
`CLAUDE_PLUGIN_OPTION_<FIELD>` / `ALEPH_PLUGIN_OPTION_<FIELD>`.

```javascript
const apiKey = process.env.CLAUDE_PLUGIN_OPTION_API_KEY;
```

**Logging:** use `console.error()` (stderr). Anything on stdout that is not an
MCP message corrupts the protocol.

## WASM Plugin Development

WASM plugins use [Extism](https://extism.org/) and run in a sandboxed environment with a capability-based security model.

### Project Structure

```
my-wasm-plugin/
  aleph.plugin.toml
  Cargo.toml
  src/
    lib.rs            # Plugin implementation
  .gitignore
```

### Writing a WASM Plugin

Use the `extism-pdk` crate to define exported functions:

```rust
// src/lib.rs
use extism_pdk::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct SearchInput {
    query: String,
    max_results: Option<u32>,
}

#[derive(Serialize)]
struct SearchOutput {
    results: Vec<String>,
}

#[plugin_fn]
pub fn search(input: Json<SearchInput>) -> FnResult<Json<SearchOutput>> {
    let query = &input.0.query;
    let max = input.0.max_results.unwrap_or(10);

    // Your search logic here
    let results = vec![format!("Result for '{}' (max {})", query, max)];

    Ok(Json(SearchOutput { results }))
}
```

### Building

```bash
# Add the WASM target (one-time setup)
rustup target add wasm32-wasip1

# Build the plugin
cargo build --target wasm32-wasi --release
```

The compiled `.wasm` file will be at `target/wasm32-wasi/release/<name>.wasm`.

### Cargo.toml

```toml
[package]
name = "my_wasm_plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
extism-pdk = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### WASM Capabilities

WASM plugins run in a sandbox. Access to host resources must be declared in the manifest:

```toml
[capabilities.workspace]
allowed_prefixes = ["docs/", "config/"]

[capabilities.http]
timeout_secs = 30
max_request_bytes = 1048576      # 1 MB
max_response_bytes = 10485760    # 10 MB

[[capabilities.http.allowlist]]
host = "api.example.com"
path_prefix = "/v1/"
methods = ["GET", "POST"]

[[capabilities.http.credentials]]
secret_name = "api_token"
host_patterns = ["api.example.com"]
[capabilities.http.credentials.inject]
type = "bearer"

[capabilities.http.rate_limit]
requests_per_minute = 60
requests_per_hour = 1000

[capabilities.secrets]
allowed_patterns = ["my_plugin_*"]
```

**Credential injection types:**
- `bearer` — `Authorization: Bearer <secret>`
- `basic` — Basic auth with `{ username: "..." }`
- `header` — Custom header with `{ name: "X-Api-Key", prefix: "Token " }`
- `query` — Query parameter with `{ param_name: "api_key" }`
- `url_path` — URL path substitution with `{ placeholder: "{API_KEY}" }`

---

## Static Plugin Development

Static plugins are the simplest type. They consist of Markdown files that provide skills (AI prompt injections) or commands (user-triggered actions) without any executable code.

### Project Structure

```
my-skill/
  aleph.plugin.toml
  SKILL.md            # Main skill file
```

### SKILL.md Format

Skills use YAML frontmatter followed by Markdown content:

```markdown
---
name: code-reviewer
description: Review code for best practices and potential issues
---

# Code Reviewer

You are a code reviewer. When the user asks you to review code, follow
these guidelines:

## Process

1. Read the code carefully
2. Check for common issues:
   - Security vulnerabilities
   - Performance problems
   - Code style violations
3. Provide specific, actionable feedback

## Arguments

The user's request: $ARGUMENTS
```

### Frontmatter Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Skill name (defaults to directory name) |
| `description` | string | Short description for skill listing |
| `disable-model-invocation` | bool | If true, skill is not auto-invocable by AI |
| `scope` | string | Prompt injection scope: `"system"`, `"user"`, or `"tool"` |
| `bound-tool` | string | Tool name this skill is bound to (for `"tool"` scope) |

### The `$ARGUMENTS` Placeholder

Use `$ARGUMENTS` in your skill content to insert user-provided arguments:

```markdown
---
name: translate
description: Translate text to a target language
---

Translate the following text. Maintain the original formatting and tone.

$ARGUMENTS
```

### Commands vs Skills

- **Skills** (in `skills/` or `SKILL.md`) — Can be auto-invoked by the AI
- **Commands** (in `commands/` or `COMMAND.md`) — Triggered explicitly by user via `/command`

---

## Tools

Tools are functions the AI can call. They are the primary extension point for plugins.

### Declaring Tools in the Manifest

```toml
[[tools]]
name = "web_search"
description = "Search the web for information"
handler = "handleWebSearch"
parameters = { type = "object", properties = { query = { type = "string" } }, required = ["query"] }
```

### Tool Parameters

Tool parameters use [JSON Schema](https://json-schema.org/) format. Declare them either in the manifest or dynamically in code:

```toml
[[tools]]
name = "create_issue"
description = "Create a GitHub issue"
handler = "createIssue"

[tools.parameters]
type = "object"
required = ["title"]

[tools.parameters.properties.title]
type = "string"
description = "Issue title"

[tools.parameters.properties.body]
type = "string"
description = "Issue description"

[tools.parameters.properties.labels]
type = "array"
description = "Issue labels"
items = { type = "string" }
```

### Tools that are not known ahead of time

An MCP server decides its own tool list when it starts, so a plugin whose tools
depend on discovery simply registers them in the server — nothing needs to be
declared in the manifest.

> The `api.registerTool(...)` example that stood here belonged to the Node.js
> host process that never existed; `capabilities.dynamic_tools` is parsed and
> has no consumer. Manifest-declared `[[aleph.tools]]` are for WASM plugins,
> where `handler` names an exported guest function.

---

## Hooks

Hooks let plugins observe or intercept events in the Aleph lifecycle.

### Hook Events

| Event | Description | Can Intercept? |
|-------|-------------|---------------|
| `PreToolUse` | Before a tool is executed | Yes (can block) |
| `PostToolUse` | After successful tool execution | No |
| `PostToolUseFailure` | After failed tool execution | No |
| `SessionStart` | When a session begins | No |
| `SessionEnd` | When a session ends | No |
| `ChatMessage` | When a message is received | Yes |
| `ChatParams` | Before LLM call parameters are sent | Yes |

### Hook Kinds

- **Observer** (default) — Read-only. Cannot modify or block the event.
- **Interceptor** — Can modify event data or block execution.

### Hook Priority

Hooks execute in priority order: `high` > `normal` > `low`. Within the same priority, execution order is undefined.

### Manifest Declaration

```toml
[[hooks]]
event = "PreToolUse"
kind = "interceptor"
handler = "onPreToolUse"
priority = "high"
filter = "Bash|Write"    # Regex: only trigger for Bash or Write tools
```

### Hook Context

Hooks receive a context object with event-specific data:

```typescript
// PreToolUse context
{
  session_id: "sess-123",
  tool_name: "Bash",
  arguments: '{"command": "rm -rf /"}',
  tool_input: "...",
}
```

---

## Services

Services are long-running background processes managed by Aleph.

```toml
[[services]]
name = "file-watcher"
description = "Watches project files for changes"
start_handler = "startWatcher"
stop_handler = "stopWatcher"
```

Services are started and stopped via the Aleph API:

```bash
# From the CLI
aleph plugins call <plugin-id> service.start --args '{"service_id": "file-watcher"}'
```

---

## Permissions

Plugins must declare the permissions they need. Users are informed about required permissions when installing a plugin.

```toml
[permissions]
network = true           # HTTP/WebSocket access
filesystem = "read"      # "read", "write", or true (full access)
env = true              # Read environment variables
shell = true            # Execute shell commands
```

### Permission Levels

| Permission | Values | Description |
|-----------|--------|-------------|
| `network` | `true`/`false` | Network access |
| `filesystem` | `false`, `"read"`, `"write"`, `true` | Filesystem access level |
| `env` | `true`/`false` | Environment variable access |
| `shell` | `true`/`false` | Shell command execution |

### WASM Granular Permissions

WASM plugins have additional fine-grained capabilities declared in the `[capabilities]` section (see [WASM Capabilities](#wasm-capabilities) above).

---

## Configuration Schema

Plugins can declare a configuration schema so users can configure them through the Aleph UI:

```toml
[plugin.config_schema]
type = "object"
required = ["api_key"]

[plugin.config_schema.properties.api_key]
type = "string"
description = "API key for the service"

[plugin.config_schema.properties.max_results]
type = "number"
description = "Maximum results to return"
default = 10

# UI hints for better configuration experience
[plugin.config_ui_hints.api_key]
label = "API Key"
help = "Get your API key from https://example.com/settings"
sensitive = true
placeholder = "sk-..."

[plugin.config_ui_hints.max_results]
label = "Max Results"
advanced = true
```

### UI Hint Fields

| Field | Type | Description |
|-------|------|-------------|
| `label` | string | Human-readable label |
| `help` | string | Help text explaining the field |
| `sensitive` | bool | Mask input (for passwords, tokens) |
| `advanced` | bool | Hide under "Advanced" section |
| `placeholder` | string | Placeholder text for input |

In `.claude-plugin/plugin.toml` these live under `[aleph.config_schema]` and
`[aleph.config_ui_hints.<field>]`; the `[plugin.*]` spelling above is the
deprecated `aleph.plugin.toml` dialect.

### Where the values live, and how they reach the plugin

Values are stored per plugin in `<data_dir>/plugins.toml` and validated against
`config_schema` on write — every violation is reported, not just the first. A
plugin that declares no schema accepts anything.

| Runtime | How configuration arrives |
|---------|---------------------------|
| `wasm` | Extism config keys — `config::get("api_key")` in `extism-pdk` |
| `mcp` | environment: `ALEPH_PLUGIN_CONFIG` (whole object as JSON) plus `CLAUDE_PLUGIN_OPTION_<FIELD>` / `ALEPH_PLUGIN_OPTION_<FIELD>` per scalar |
| hooks | the same environment variables |

An explicit `env` entry in `.mcp.json` wins over an injected one: the author's
own value beats a convention.

Read and write it over JSON-RPC (`plugin.config.get` / `plugin.config.set`) or
conversationally with the `plugin_manage` tool
(`action='config_get'` / `action='config_set'`). `config_set` **replaces** the
whole object, so read it first and send the merged result. Changes take effect
on the next `action='reload'` — reloading tears down the plugin's MCP servers
and background services, so it is never implicit.

> `config_schema` and `config_ui_hints` existed on the manifest type long
> before any of this, and until 2026-08-19 their only consumer was the
> authoring-time linter. A declared schema described a control that was not
> there: no store, no RPC, no tool, and the runtime received nothing. They also
> could not be declared in the *preferred* manifest at all — only in the
> deprecated one.

---

## Testing

### Validate Your Plugin

```bash
# Validate manifest and structure
aleph plugin validate .

# JSON output for CI
aleph plugin validate . --json
```

Validation checks:
- `aleph.plugin.toml` exists and is valid TOML
- Required fields (`id`, `name`, `kind`, `entry`) are present
- Entry file exists (warning if missing, since it may need building)
- No duplicate tool names
- No duplicate hook events

### Check Environment

```bash
aleph plugin doctor

# JSON output
aleph plugin doctor --json
```

Doctor checks:
- Node.js runtime availability
- npm package manager availability
- WASM compilation target (`wasm32-wasi` / `wasm32-wasip1`)
- Global plugin directory existence

### Manual Testing with Dev Mode

```bash
# Start dev mode with hot-reload
aleph plugin dev .

# In another terminal, test tool calls
aleph plugins call <plugin-id> <tool-name> --args '{"key": "value"}'
```

### Testing Node.js Plugins

You can test your plugin's logic independently:

```typescript
// test/index.test.ts
import { describe, it, expect } from 'vitest';

// Test your tool handlers directly
describe('my_tool', () => {
  it('returns correct result', async () => {
    const result = await handleMyTool('call-1', { query: 'test' });
    expect(result.result).toBeDefined();
  });
});
```

### Testing WASM Plugins

```bash
# Run Rust unit tests
cargo test

# Build and validate
cargo build --target wasm32-wasi --release
aleph plugin validate .
```

---

## Packaging

### Pack for Distribution

```bash
# Create a distributable archive
aleph plugin pack .

# Specify output path
aleph plugin pack . --output ./dist/my-plugin.aleph-plugin.zip
```

The `pack` command:
1. Validates the plugin first (fails if validation errors exist)
2. Creates a `.aleph-plugin.zip` archive
3. Automatically excludes: `node_modules/`, `.git/`, `target/`, `.DS_Store`, `__pycache__/`, `.mypy_cache/`

### Archive Contents

The zip archive contains all plugin files needed for installation, excluding build artifacts and dependencies. Users install the archive and Aleph handles dependency installation.

---

## Installation & Discovery

### Plugin Discovery Paths

Aleph discovers plugins from four locations, in priority order:

| Priority | Location | Description |
|----------|----------|-------------|
| 1 (highest) | Config-specified paths | Paths in `aleph.jsonc` configuration |
| 2 | `~/.aleph/projects/<id>/extensions/` | Project-level plugins |
| 3 | `~/.aleph/extensions/` and `~/.claude/extensions/` | Global user-level plugins |
| 4 (lowest) | Bundled directory | Plugins shipped with Aleph |

When the same plugin ID exists at multiple levels, the higher-priority version wins.

### Installing Plugins

```bash
# Install from a local directory
aleph plugins install /path/to/my-plugin

# Install from a zip archive
aleph plugins install ./my-plugin.aleph-plugin.zip
```

### Managing Plugins

```bash
# List installed plugins
aleph plugins list

# Enable/disable a plugin
aleph plugins enable <plugin-id>
aleph plugins disable <plugin-id>

# Uninstall a plugin
aleph plugins uninstall <plugin-id>

# Call a tool directly
aleph plugins call <plugin-id> <tool-name> --args '{"key": "value"}'
```

### Plugin Directory Layout

For manual installation, place your plugin in one of the discovery paths:

```
~/.aleph/extensions/
  my-plugin/
    aleph.plugin.toml
    dist/
      index.js
    package.json
```

Each subdirectory in the extensions folder is treated as a separate plugin. The directory name is used as the plugin ID if the manifest doesn't specify one.

---

## Plugin Variables

Four variables are expanded wherever a plugin contributes text or configuration
— `.mcp.json`, hook commands and hook environment, and the body of every skill,
command and agent:

| Variable | Points at |
|----------|-----------|
| `${CLAUDE_PLUGIN_ROOT}` / `${ALEPH_PLUGIN_ROOT}` | the plugin's install directory |
| `${CLAUDE_PLUGIN_DATA}` / `${ALEPH_PLUGIN_DATA}` | `<plugins_root>/data/<plugin-id>/` |

Use `_DATA` for anything that must survive an upgrade: `plugin update` swaps the
install directory atomically, so everything under `_ROOT` is replaced. The data
directory is created the first time a plugin names it.

Hooks additionally receive `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PLUGIN_DATA` in their
environment.

> Until 2026-08-19 this table described four surfaces and the code covered one
> and a half: `_DATA` worked only in `.mcp.json`, and skill / command / agent
> bodies expanded nothing at all — so `Run ${CLAUDE_PLUGIN_ROOT}/scripts/x.py`
> in a `SKILL.md` reached the model as a literal and it ran `bash` against a
> path containing `${CLAUDE_PLUGIN_ROOT}`.

Identifiers are deliberately **not** expanded. A variable in a skill's `name`
stays a variable, so a manifest cannot smuggle an absolute path into a registry
key.

---

---

## Examples

### Minimal MCP Tool Plugin

```toml
# .claude-plugin/plugin.toml
name = "hello-world"
version = "0.1.0"

[aleph]
runtime = "mcp"
```

```json
// .mcp.json
{
  "mcpServers": {
    "hello-world": {
      "command": "node",
      "args": ["${CLAUDE_PLUGIN_ROOT}/src/index.mjs"]
    }
  }
}
```

```javascript
// src/index.mjs
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const server = new McpServer({ name: "hello-world", version: "0.1.0" });

server.tool(
  "hello",
  "Say hello",
  { name: z.string().optional().describe("Name to greet") },
  async ({ name }) => ({ content: [{ type: "text", text: `Hello, ${name ?? "world"}!` }] }),
);

await server.connect(new StdioServerTransport());
```

### Minimal WASM Tool Plugin

```toml
# aleph.plugin.toml
[plugin]
id = "hello-wasm"
name = "Hello WASM"
version = "0.1.0"
kind = "wasm"
entry = "target/wasm32-wasi/release/hello_wasm.wasm"

[[tools]]
name = "hello"
description = "Say hello"
handler = "hello"
```

```rust
// src/lib.rs
use extism_pdk::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct HelloInput {
    name: Option<String>,
}

#[derive(Serialize)]
struct HelloOutput {
    result: String,
}

#[plugin_fn]
pub fn hello(input: Json<HelloInput>) -> FnResult<Json<HelloOutput>> {
    let name = input.0.name.unwrap_or_else(|| "world".to_string());
    Ok(Json(HelloOutput {
        result: format!("Hello, {}!", name),
    }))
}
```

### Minimal Static Skill

```toml
# aleph.plugin.toml
[plugin]
id = "code-review"
name = "Code Review"
version = "0.1.0"
kind = "static"
entry = "SKILL.md"
```

```markdown
---
name: code-review
description: Review code for quality and best practices
---

# Code Review Skill

Review the provided code for:
1. Correctness
2. Performance
3. Security
4. Readability

Provide specific, actionable feedback with code examples.

$ARGUMENTS
```

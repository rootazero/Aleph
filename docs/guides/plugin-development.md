# Plugin Development Guide

This guide covers everything you need to build, test, and distribute Aleph plugins.

## Table of Contents

1. [Overview](#overview)
2. [Quick Start](#quick-start)
3. [Manifest Format](#manifest-format)
4. [Node.js Plugin Development](#nodejs-plugin-development)
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

---

## Overview

Aleph plugins extend the AI assistant with custom **tools**, **hooks**, **skills**, **commands**, **services**, **channels**, **providers**, and **HTTP routes**. Plugins are defined by an `aleph.plugin.toml` manifest and can be implemented using one of three runtimes:

| Runtime | Language | Use Case | Sandboxing |
|---------|----------|----------|------------|
| **Node.js** | TypeScript/JavaScript | Rich integrations, API clients, dynamic tools | Process-level |
| **WASM** | Rust (via Extism) | High-performance, security-sensitive, sandboxed | Extism sandbox with capability kernel |
| **Static** | Markdown | Skills/commands (prompt injection), no code needed | N/A (content only) |

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

This checks for Node.js, npm, WASM compilation targets, and plugin directories.

### Scaffold a New Plugin

```bash
# Node.js plugin (TypeScript)
aleph plugin init my-plugin --type nodejs

# WASM plugin (Rust)
aleph plugin init my-wasm-plugin --type wasm

# Static plugin (Markdown skill)
aleph plugin init my-skill --type static
```

This creates a directory with `aleph.plugin.toml`, a sample tool definition, and template source files.

### Build and Validate

```bash
cd my-plugin

# For Node.js plugins:
npm install
npm run build

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

Every plugin must have an `aleph.plugin.toml` file at its root. This is the preferred manifest format (Aleph also supports `aleph.plugin.json` and `package.json` with an `aleph` field, but TOML is recommended).

### Minimal Manifest

```toml
[plugin]
id = "my-plugin"
name = "My Plugin"
version = "0.1.0"
kind = "nodejs"
entry = "dist/index.js"
```

### Full Manifest Reference

```toml
[plugin]
id = "my-plugin"                    # Required. Lowercase, alphanumeric + hyphens
name = "My Plugin"                  # Display name (defaults to id)
version = "1.0.0"                   # Semver version
description = "What this plugin does"
kind = "nodejs"                     # "nodejs", "wasm", or "static"
entry = "dist/index.js"             # Entry point relative to plugin root
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

# --- Channels ---
[[channels]]
id = "slack"
label = "Slack Integration"
handler = "handleSlackMessage"

# --- Providers ---
[[providers]]
id = "custom-llm"
name = "Custom LLM Provider"
models = ["custom-7b", "custom-13b"]
handler = "handleCompletion"

# --- HTTP Routes ---
[[http_routes]]
path = "/api/v1/data"
methods = ["GET", "POST"]
handler = "handleDataRoute"

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

[capabilities.tool_invoke]
max_per_execution = 10
[capabilities.tool_invoke.aliases]
search = "brave_search"

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

| Kind | Default Entry |
|------|--------------|
| `wasm` | `plugin.wasm` |
| `nodejs` | `index.js` |
| `static` | `.` (the plugin directory itself) |

### Manifest Priority

When multiple manifest formats exist, Aleph uses this priority:

1. `aleph.plugin.toml` (highest)
2. `aleph.plugin.json`
3. `package.json` with `aleph` field
4. `.claude-plugin/plugin.json` (legacy)

---

## Node.js Plugin Development

Node.js plugins run as a subprocess communicating over **JSON-RPC 2.0 via stdio**. Aleph spawns a host process that loads your plugin's entry file and manages the IPC protocol.

### Project Structure

```
my-plugin/
  aleph.plugin.toml
  package.json
  tsconfig.json
  src/
    index.ts          # Entry point
  dist/
    index.js          # Built output (entry point)
  .gitignore
```

### Entry Point

Your plugin exports a default async function that receives the plugin API:

```typescript
// src/index.ts
export default async (api: any) => {
  // Register tools
  api.registerTool({
    name: 'my_tool',
    description: 'Does something useful',
    parameters: {
      type: 'object',
      properties: {
        query: { type: 'string', description: 'Search query' },
      },
      required: ['query'],
    },
    execute: async (toolCallId: string, params: { query: string }) => {
      // Your tool logic here
      const result = await doSomething(params.query);
      return { result };
    },
  });

  // Register hooks
  api.on('before_tool_call', async (event: any) => {
    console.error(`Tool being called: ${event.tool_name}`);
    // Return modified event or nothing
  });
};
```

### JSON-RPC Protocol

Under the hood, Aleph communicates with Node.js plugins using JSON-RPC 2.0 over stdin/stdout:

**Request (Aleph to Plugin):**
```json
{
  "jsonrpc": "2.0",
  "id": "call-1",
  "method": "plugin.call",
  "params": {
    "handler": "handleMyTool",
    "arguments": { "query": "hello" }
  }
}
```

**Response (Plugin to Aleph):**
```json
{
  "jsonrpc": "2.0",
  "id": "call-1",
  "result": { "result": "Hello, world!" }
}
```

**Registration (Plugin to Aleph, on startup):**
```json
{
  "jsonrpc": "2.0",
  "method": "plugin.register",
  "params": {
    "plugin_id": "my-plugin",
    "tools": [
      {
        "name": "my_tool",
        "description": "Does something",
        "parameters": { "type": "object" },
        "handler": "handleMyTool"
      }
    ],
    "hooks": [],
    "channels": [],
    "providers": [],
    "gateway_methods": []
  }
}
```

**Important:** Use `console.error()` for logging (stderr). Do **not** write to stdout unless sending JSON-RPC messages, as this will corrupt the protocol.

### Hook Handling

Hooks allow your plugin to observe or intercept lifecycle events:

```typescript
export default async (api: any) => {
  // Observer hook: read-only, cannot modify the event
  api.on('session_start', async (event: any) => {
    console.error('Session started:', event.session_id);
  });

  // Tool lifecycle hooks
  api.on('before_tool_call', async (event: any) => {
    if (event.tool_name === 'Bash') {
      console.error('Bash command:', event.arguments);
    }
  });

  api.on('after_tool_call', async (event: any) => {
    console.error('Tool result:', event.tool_name);
  });
};
```

---

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

[capabilities.tool_invoke]
max_per_execution = 10
[capabilities.tool_invoke.aliases]
search = "brave_search"

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

### Dynamic Tool Registration (Node.js)

Plugins with `capabilities.dynamic_tools = true` can register tools at runtime:

```typescript
export default async (api: any) => {
  // Discover available tools dynamically
  const tools = await discoverAvailableTools();

  for (const tool of tools) {
    api.registerTool({
      name: tool.name,
      description: tool.description,
      parameters: tool.schema,
      execute: async (_id: string, params: any) => {
        return await executeTool(tool, params);
      },
    });
  }
};
```

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

## Examples

### Minimal Node.js Tool Plugin

```toml
# aleph.plugin.toml
[plugin]
id = "hello-world"
name = "Hello World"
version = "0.1.0"
kind = "nodejs"
entry = "dist/index.js"

[[tools]]
name = "hello"
description = "Say hello"
handler = "hello"
```

```typescript
// src/index.ts
export default async (api: any) => {
  api.registerTool({
    name: 'hello',
    description: 'Say hello',
    parameters: {
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Name to greet' },
      },
    },
    execute: async (_id: string, params: { name?: string }) => {
      return { result: `Hello, ${params.name ?? 'world'}!` };
    },
  });
};
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

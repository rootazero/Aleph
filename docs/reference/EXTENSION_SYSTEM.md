# Extension System

> Plugin architecture: WASM runtime, MCP-kind external servers, static (Markdown) plugins

---

## Overview

Aleph's extension system allows third-party tools via:
- **WASM Plugins**: Fast, sandboxed WebAssembly modules
- **MCP-kind Plugins**: any-language external servers (Node.js, Python, …) reached over MCP stdio / HTTP — see "Node plugins run as MCP stdio servers" below
- **Manifest-driven**: Declarative plugin definitions

**Location**: `src/extension/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Extension Manager                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Loader     │     │   Registry   │     │   Watcher    │    │
│  │              │     │              │     │              │    │
│  │ • Discovery  │     │ • Register   │     │ • Hot reload │    │
│  │ • Manifest   │     │ • Lookup     │     │ • Events     │    │
│  │ • Validate   │     │ • Unregister │     │              │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                     Plugin Runtimes                       │   │
│  │  ┌────────────────────┐  ┌────────────────────┐         │   │
│  │  │    WASM Runtime    │  │  MCP-kind (extern) │         │   │
│  │  │    (Extism)        │  │  stdio / http srv  │         │   │
│  │  │                    │  │                    │         │   │
│  │  │ • Sandboxed        │  │ • Any language     │         │   │
│  │  │ • Fast startup     │  │ • Tools via bridge │         │   │
│  │  │ • Limited I/O      │  │ • No hook channel  │         │   │
│  │  └────────────────────┘  └────────────────────┘         │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Plugin Structure

### Directory Layout

```
~/.aleph/plugins/
├── my-plugin/
│   ├── aleph_plugin.toml    # Plugin manifest
│   ├── .mcp.json             # (MCP-kind: declares the external server) or
│   ├── plugin.wasm           # (WASM)
│   └── src/
│       └── index.ts
└── another-plugin/
    └── ...
```

### Manifest (aleph_plugin.toml)

```toml
[plugin]
name = "my-plugin"
version = "1.0.0"
description = "My awesome plugin"
author = "Your Name"

[runtime]
type = "wasm"    # wasm | mcp | static — the real key is [aleph] runtime, see PLUGIN_SYSTEM.md「Runtime 模型」
entry = "plugin.wasm"

[[tools]]
name = "my_tool"
description = "Does something useful"

[tools.args]
input = { type = "string", required = true }
options = { type = "object", required = false }
```

---

## WASM Runtime

**Location**: `src/extension/runtime/wasm/`

Feature-gated: `plugin-wasm`

### Architecture

```rust
pub struct WasmRuntime {
    plugins: HashMap<String, ExtismPlugin>,
}

impl WasmRuntime {
    pub fn load(&mut self, path: &Path) -> Result<()> {
        let plugin = Plugin::new(path, [], true)?;
        self.plugins.insert(name, plugin);
    }

    pub fn call(
        &self,
        plugin: &str,
        function: &str,
        input: &[u8],
    ) -> Result<Vec<u8>> {
        self.plugins[plugin].call(function, input)
    }
}
```

### Plugin Interface

WASM plugins export functions:

```rust
// Plugin side (Rust → WASM)
#[extism_pdk::plugin_fn]
pub fn my_tool(input: String) -> FnResult<String> {
    let args: MyToolArgs = serde_json::from_str(&input)?;
    let result = do_something(args);
    Ok(serde_json::to_string(&result)?)
}
```

### Limitations

- No filesystem access (sandboxed)
- No network access (sandboxed)
- Memory limited (configurable)
- CPU time limited

---

## Node plugins run as MCP stdio servers

There is no Node.js runtime in Aleph and there never was one on disk (`ls src/extension/runtime/` →
`mod.rs` and `wasm/` only; `PluginKind` is `Wasm | Mcp | Static`). A plugin written in Node.js — or
Python, Go, anything — is an **MCP-kind plugin**: `[aleph] runtime = "mcp"` plus a `.mcp.json` naming
the command to spawn (a Claude Code `plugin.json` may instead declare `mcpServers` inline or by path,
and is then MCP-kind without an `aleph` block). At mount, `register_transient_servers`
(`src/extension/registrar/mcp_registrar.rs`) hands each server to
`McpManagerHandle::add_transient_server_detached`, the tool bridge (`src/mcp/tool_bridge.rs`)
registers its tools, and `unmount` stops it (EffectScope step `"mcp_server"`). Use the official MCP
SDK for your language; do not speak a private JSON-RPC-over-stdio dialect — nothing on the host side
answers it.

MCP has no hook channel: an MCP-kind plugin contributes **tools** (and skills / agents / commands as
static files), not `PreToolUse` / `PostToolUse` handlers. A plugin's hooks are `hooks.json` `command`
actions (its `prompt` / `http` / `agent` actions are dropped with a warning,
`src/extension/manifest/parsers.rs`; only user-settings hooks run all four) or WASM exports.

> Until 2026-09-20 this section described a `NodejsRuntime` at `src/extension/runtime/nodejs/` and an
> `@aleph/plugin-sdk` npm package (`packages/plugin-sdk/`, 906 lines of TypeScript with no host).
> Both were doc-only; both are gone. The sibling repo still carries the phantom dialect:
> `plugins/media-office/src/index.js:349` (`method === "plugin.call"`) and `:264` (`onPostToolUse`) —
> follow-up in Aleph-plugins, not here.

---

## Effects and `EffectScope` (temporal composability)

**Location**: `src/extension/effects/{mod.rs, scope.rs, disposer.rs}` (2026-09-20)

```rust
/// One reversible side effect a plugin made on the running process.
/// Dispose is async (MCP server removal, service stop) and reports failure
/// instead of panicking; the scope records the Err and keeps going.
pub type DisposeOutcome = Result<(), String>;
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, DisposeOutcome> + Send>;
pub fn sync_disposer(f: impl FnOnce() -> DisposeOutcome + Send + 'static) -> Disposer;
pub fn async_disposer<F, Fut>(f: F) -> Disposer
where F: FnOnce() -> Fut + Send + 'static, Fut: Future<Output = DisposeOutcome> + Send + 'static;

pub type PluginId = String;            // bare String, no newtype
pub const STEP_LABELS: [&str; 6];      // the six step labels, in registration order
pub struct EffectScope { /* plugin_id, disposers: Vec<(&'static str, Disposer)>, skipped */ }
impl EffectScope {
    pub fn new(plugin_id: PluginId) -> Self;
    pub fn effect(&mut self, step: &'static str, d: Disposer);
    /// A step the plugin declares but this process cannot provide (e.g. no MCP
    /// handle): recorded, not an error; `Pending { waiting_on }` derives from it.
    pub fn skip(&mut self, step: &'static str, why: impl Into<String>);
    /// Reverse registration order. A failing/panicking disposer is recorded
    /// and does NOT stop the rest. Consumes self: a scope cannot be half-disposed.
    pub async fn dispose(self) -> DisposeReport;
}
pub struct DisposeReport { pub plugin_id: PluginId, pub steps: Vec<(&'static str, DisposeOutcome)> }  // all_ok() / failures()
```

**The one rule — has an inverse → effect; recomputable from the registry → view.** Every registrar
function that puts something into the running process (`registrar/`, `service_manager.rs`,
`src/extension/loader.rs`, `memory/extensions/`) returns `#[must_use] Disposer`; the caller
(`lifecycle.rs::mount`) pushes it into the plugin's `EffectScope` under one of six fixed step labels,
in this order: `registry_row`, `wasm_module`, `mcp_server`, `service`, `memory_extension`,
`slash_command`. `unmount` disposes in reverse, so the registry row is the last thing to go and every
view recomputed afterwards already sees the plugin gone. Anything with no inverse that can be
recomputed from `PluginRegistry` (tool-index snapshot, `PLUGIN_SKILL_DIRS`, `PLUGIN_SUBAGENTS`,
`HookExecutor`) is a **view**, derived by `projection.rs` from `after_transition()` only.

This is the ownership rule from DeepSeek Harness / Cordis (`scan-dsh-cordis.md` §1), expressed as
signatures. It is **not** a fiber runtime: no DI container, no Proxy context, no cascade restart, no
HMR (the three earlier rounds' rulings stand — HARNESS_PHILOSOPHY.md §8 第五课, narrowed 2026-09-20).
Guard: `effects::census::every_crate_visible_registration_returns_a_disposer` (source-level census, G1;
mutation: an extra `pub fn register_extra` in `registrar/api.rs` goes red by name) and the six-effect
round-trip (G2, `tests/plugin_lifecycle_roundtrip.rs` + the two fixtures in P1.13).

---

## `ScopeKey` and visibility (spatial composability)

**Location**: `src/extension/visibility.rs` (renamed from `scope.rs`, which served only hooks)

```rust
pub enum ScopeKey { Global, Project(PathBuf /* canonicalized root */) }
pub struct VisibilityCtx { pub project_root: Option<PathBuf> }
/// Global → always visible. Project(p) → visible iff ctx.project_root == Some(p).
/// A session with no project sees Global only (fail-closed).
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool;
```

Every registry row carries a `scope_key` derived at discovery: `Project(root)` only for
`<project>/.claude/` and `<project>/.aleph/plugins{,.local}`; every other origin — `Bundled`, `Config`,
`Global`, marketplace installs, and the new `ClaudeCache` — is `Global`. Project level is the only
level (no session / agent sub-scopes — user ruling U4). The five faces that present plugin capability
to a request (tool index, skills index, agent resolution, slash list, MCP tool bridge) all call
`visible_to` at request-build time; hooks' `project_scope_allows` calls the same predicate.
`VisibilityCtx.project_root` has exactly one derivation — the one hooks already used upstream of
`executor.rs:918` — extracted, not duplicated.

**Behaviour change (2026-09-20)**: a session with no project root sees `Global` plugins only. Before,
discovery was the union of every project and every session saw everything.

---

## Plugin Discovery

**Location**: `src/extension/discovery/`

```rust
pub struct PluginDiscovery {
    search_paths: Vec<PathBuf>,
}

impl PluginDiscovery {
    pub fn discover(&self) -> Result<Vec<PluginManifest>> {
        let mut manifests = vec![];

        for path in &self.search_paths {
            for entry in fs::read_dir(path)? {
                let manifest_path = entry.path().join("aleph_plugin.toml");
                if manifest_path.exists() {
                    manifests.push(parse_manifest(&manifest_path)?);
                }
            }
        }

        manifests
    }
}
```

### Search Paths

1. `~/.aleph/plugins/` (user plugins)
2. `/usr/local/share/aleph/plugins/` (system plugins)
3. `./plugins/` (project plugins)

---

## Plugin Registry

**Location**: `src/extension/registry/`

```rust
pub struct PluginRegistry {
    plugins: HashMap<String, RegisteredPlugin>,
    tools: HashMap<String, ToolRef>,
}

pub struct RegisteredPlugin {
    pub manifest: PluginManifest,
    pub runtime: RuntimeType,
    pub status: PluginStatus,
}

pub enum PluginStatus {
    Loaded,
    Running,
    Stopped,
    Error(String),
}
```

### Registration Flow

```
Plugin Directory Found
    │
    ▼
┌─────────────────────────────────────────┐
│ 1. Parse manifest                        │
│    aleph_plugin.toml or package.json   │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 2. Validate manifest                     │
│    • Required fields                     │
│    • Version compatibility               │
│    • Tool name conflicts                 │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 3. Select runtime                        │
│    WASM → WasmRuntime                   │
│    MCP  → McpManager (transient server) │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 4. Register tools                        │
│    Add to ToolServer registry           │
└─────────────────────────────────────────┘
    │
    ▼
Plugin Ready
```

---

## Hot Reload

**Location**: `src/extension/watcher.rs`

```rust
pub struct PluginWatcher {
    watcher: RecommendedWatcher,
    registry: Arc<RwLock<PluginRegistry>>,
}

impl PluginWatcher {
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        self.watcher.watch(path, RecursiveMode::Recursive)?;
    }

    async fn on_change(&self, event: Event) {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                self.reload_plugin(&event.paths[0]).await;
            }
            EventKind::Remove(_) => {
                self.unload_plugin(&event.paths[0]).await;
            }
            _ => {}
        }
    }
}
```

---

## Skill Integration

**Location**: `src/extension/skill_tool.rs`

Skills (from `~/.claude/skills/`) are also loaded as extensions:

```rust
pub struct SkillTool {
    name: String,
    definition: SkillDefinition,
}

impl AlephToolDyn for SkillTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn call(&self, args: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            // Execute skill via prompt injection
        })
    }
}
```

---

## Configuration

```json5
{
  "extensions": {
    "enabled": true,
    "searchPaths": [
      "~/.aleph/plugins",
      "./plugins"
    ],
    "runtimes": {
      "wasm": {
        "enabled": true,
        "memoryLimit": "256MB",
        "timeoutMs": 30000
      }
    },
    "hotReload": true
  }
}
```

---

## Plugin RPC Methods

| Method | Description |
|--------|-------------|
| `plugins.list` | List all plugins |
| `plugins.install` | Install from path/URL |
| `plugins.uninstall` | Remove plugin |
| `plugins.enable` | Enable plugin |
| `plugins.disable` | Disable plugin |
| `plugin.reload` | Reload one plugin (unmount + mount) |
| `plugins.callTool` | Call a tool on a loaded runtime plugin (CLI) |

---

## Extension SDK V2

The V2 SDK introduces enhanced manifest format, hook system, and prompt scopes for building powerful extensions.

### Manifest Format (aleph_plugin.toml)

V2 plugins use TOML format for better readability and Rust ecosystem alignment. The manifest priority order is:

1. `aleph_plugin.toml` (V2 TOML format) - **Preferred**
2. `aleph_plugin.json` (V2 JSON format)
3. `package.json` with `alephPlugin` section
4. Legacy manifest formats

#### Complete Example

```toml
[plugin]
id = "my-plugin"                    # Unique identifier
name = "My Plugin"                  # Display name
version = "1.0.0"                   # SemVer version
description = "Does something useful"
author = "Your Name"
kind = "wasm"                       # wasm | mcp | static
entry = "plugin.wasm"               # Entry point (wasm only; mcp uses .mcp.json)

[permissions]
network = ["connect:https://*"]     # Network permissions
filesystem = ["read:./data", "write:./output"]
env = ["API_KEY", "DEBUG"]          # Environment variables

[prompt]
file = "SKILL.md"                   # Prompt file path
scope = "system"                    # system | tool | standalone | disabled

[[tools]]
name = "my_tool"
description = "Performs a specific task"
handler = "handleMyTool"            # Function name in entry
instruction_file = "docs/INSTRUCTIONS.md"  # Tool-specific instructions

[[tools]]
name = "another_tool"
description = "Another useful tool"
handler = "handleAnotherTool"

[[hooks]]
event = "before_tool_call"
kind = "interceptor"                # interceptor | observer | resolver
priority = "normal"                 # system | high | normal | low
handler = "onBeforeTool"

[[hooks]]
event = "after_tool_call"
kind = "observer"
priority = "low"
handler = "onAfterTool"
```

### Hook Types

Hooks allow plugins to intercept and respond to system events.

| Type | Execution | Behavior |
|------|-----------|----------|
| **Interceptor** | Sequential | Can modify context or block execution. Each hook receives the result of the previous one. |
| **Observer** | Parallel | Fire-and-forget. Errors are logged but don't affect execution. Used for telemetry/logging. |

> A third `Resolver` kind (first-win competition) existed on paper but never
> gained a production fire-site and was removed under YAGNI. Configs that still
> say `"kind": "resolver"` parse to the `Observer` default rather than failing.

**What a `matcher` is tested against, and which events accept each kind.**
The single sources are `HookEvent::match_subject()` / `supports_interceptor()`
(`src/extension/types/hooks.rs`); both are surfaced per hook by
`hooks_manage(action="list")` and as a catalogue by
`hooks_manage(action="events")`. A matcher is tested against the event's
subject: the tool name on tool and permission events (the Aleph name and
every CC spelling — Claude Code's documented reading); the tool name on
`Notification` too (DEVIATION: Claude Code matches a notification *type*
there, so `permission_prompt` never matches here, and the inventory says so);
the session source on `SessionStart` (INFERRED from superpowers' real
`startup|clear|compact` matcher, not documented in the evidence). Aleph fires
SessionStart on an empty history only, always as `startup` — a session
emptied by `reset_session` fires as `startup` too; Aleph never sends
`resume` / `clear` / `compact`, so superpowers' matcher fires and a matcher
naming only those never does. On every other event Aleph has nothing to test
the matcher against: it is **ignored** and the hook fires on every occurrence
(a load-time warning says so). `"*"` and `""` match everything, like no
matcher (`hooks::matcher::compile_matcher`, the one compile for every hook
reader). Two shapes that can never fire:

- on an event that tests its matcher, one that is not a valid regex, or a
  `SessionStart` matcher that does not match `startup`;
- `"kind": "interceptor"` on an event whose fire-site dispatches observers
  only (message / provider / gateway / subagent seams).

Both are warned at load time — by `~/.aleph/hooks.json`, a plugin's
`hooks.json` and an `aleph.plugin.toml` `[[hooks]] filter` alike
(`hooks::matcher::warn_on_matcher`), and by both writing faces
(`hooks_manage add`, the `hooks.add` RPC's `matcher_warning`) — **and** reported per
hook as `reachable: false` with an `issue` string by the runtime inventory.

#### Available Hook Events

The exhaustive list is `HookEvent::ALL`. Frequently used:

| Event | Description |
|-------|-------------|
| `before_tool_call` | Before any tool is invoked |
| `after_tool_call` | After tool execution completes |
| `session_start` / `session_end` | Session lifecycle |
| `user_prompt_submit` | Before the first provider call of a run; may inject context or halt |
| `stop` | Gate on the loop's stop (veto = keep going, with feedback) |
| `subagent_start` | When a sub-agent is spawned (observer-only; env: `SUBAGENT_ID`, `SUBAGENT_TYPE`, `TASK`, `PARENT_AGENT_ID`, `CHAIN_DEPTH`) |
| `subagent_stop` | When a sub-agent completes (observer-only; env: `SUBAGENT_ID`, `SUBAGENT_TYPE`, `OUTCOME`, `ITERATIONS`, `DURATION_MS`, `TOKENS_USED`, `KEY_FINDINGS`) |
| `message_received` / `message_sending` / `message_sent` | Channel I/O (observer-only) |
| `before_compaction` / `after_compaction` | Around history compaction |
| `pre_api_request` / `post_api_request` | Around a provider call (observer-only) |

#### Limits enforced on every hook

| Limit | Value | Why |
|-------|-------|-----|
| `timeout_secs` ceiling | 300s | Interceptor seams **await** hooks; an unclamped override would wedge the tool gate. Clamped at `HookExecutor::effective_timeout`, covering every config source. |
| stdout / stderr / HTTP body read | 64KB | Truncation is a **hard error** (fail-closed): a `deny:` printed past the cap must never be silently dropped. |
| Injected context per block | ~2500 tokens | Over-budget `context:` text is spilled to `~/.aleph/data/hook_outputs/<session>/` and replaced by a head/tail preview naming the file, so the model can still read it in full on demand. |
| Consent key | owner + project + template | A `command` / `http` hook runs only after the operator approves it (`aleph-server hooks test <fingerprint>`). The approval is keyed by the hook's owner label, its command / URL template and — for a hook bound to one project (`.aleph/hooks{,.local}.json`, a plugin found under a project) — that project's canonical root: the `ScopeKey` the fire-time gate reads. Every project's files load under the one label `user:project`, so without the root an approval given in repo A also ran the same template in repo B, where `${CLAUDE_PLUGIN_ROOT}` is B's own directory. Approvals recorded before this binding (2026-09-24) carry no project: they stay on disk but are never consulted for a project hook, so each project's hooks go back to pending once. **An unapproved hook does not run, so every approved project guard (a `PreToolUse` hook that denies edits to `.env`, blocks `rm -rf` …) stops blocking from the upgrade on — the tool calls it stopped go through — until its project's new pending entry is approved.** `aleph doctor` reports the old approvals from project hook files ("Project-hook approvals no longer apply"); an old approval of a plugin installed inside a project is not counted — it shows as pending once the hook fires. Within its key, an approval is also bound to the root `aleph-server hooks test` ran it from, and to the content of ONE script file — the first word of the command with a script extension, else the first path — when that word is an absolute or `~/` path, a path variable (`${CLAUDE_PLUGIN_ROOT}` & co., a plugin's `_DATA` pair), `$CLAUDE_PROJECT_DIR` in a project hook, or a path relative to the hook's root. **Only the named file is hashed — what it sources, imports or reads is not.** `$CLAUDE_PROJECT_DIR` is bound only in a session bound to that project (a run with no project of its own can still fire the hooks of the project the daemon was started in, and there the variable names another directory, or none). Anything else is not content-bound and the approval is of the command string alone — among it a script reached through `PATH` (`npx`, `uvx`, a bare name) or another variable, command substitution or re-parsing (`$(…)`, backticks, `eval`, `sh -c "…"`), `python3 -m`, a quoted path containing a space, and a script over 1 MiB. So is a global hook's `$CLAUDE_PROJECT_DIR`: every project a session opens supplies its own script to that one approval, and it runs unreviewed. The same key from another directory, or an edited bound script, is refused — the next fire of an approval that bound no script, once its script can be hashed, withdraws it to pending; otherwise it stays approved-but-refused until `aleph-server hooks revoke`, after which the next fire records what it runs now, for review. **That withdrawal is a second migration:** every hook written the Claude Code way and approved before its script could be bound (`${CLAUDE_PLUGIN_ROOT}/…`, a relative script, a project hook's `$CLAUDE_PROJECT_DIR`), globally installed plugins included, does not run from its first fire after the upgrade until it is approved again — a guard among them stops blocking. The doctor does not count these; the hook shows as pending once it has fired. No approval is minted without a root. **Exception:** an approval of a hook that fires everywhere, recorded before roots were kept, is bound to no root and keeps working from any directory until it is revoked, fires once and is approved again. `aleph-server hooks list` shows each entry's project, `aleph-server hooks test` its project and root. Contract: `src/extension/hooks/consent.rs`. |
| Event data in a `command` | stdin JSON + env only | On unix nothing is substituted into the command text: the path variables (`${CLAUDE_PLUGIN_ROOT}` & co.) and the data (`$ARGUMENTS`, `$DENY_REASON`, …) are env vars the shell expands as data — write them double-quoted (or, for a path, unquoted); a single-quoted `'${CLAUDE_PLUGIN_ROOT}'` stays literal since 2026-09-24. Splicing ran a tool argument's `$(…)` as code, and a plugin root named `fmt$(…)` the same way — which until 2026-09-27 still happened for every plugin `hooks.json` command and every command body's `` !`cmd` ``, because the manifest adapter expanded the path variables into them at parse time; it now leaves both as written, so consent records the template (`${CLAUDE_PLUGIN_ROOT}/…`) and approvals recorded against the expanded text went back to pending once. On Windows (`cmd /C`, which cannot expand `${…}`) the path variables are substituted into the text and data is read from the stdin JSON. A settings hook's `_DATA` pair is unset, not inherited. Contract: `HookAction::Command` (`src/extension/types/hooks.rs`). |

#### Hook Example

```typescript
// Interceptor: Can modify or block
async function onBeforeTool(context: HookContext): Promise<HookContext> {
  if (context.toolName === 'dangerous_tool') {
    throw new Error('Tool blocked by security policy');
  }
  // Modify context
  context.args.timestamp = Date.now();
  return context;
}

// Observer: Fire-and-forget
async function onAfterTool(context: HookContext): Promise<void> {
  console.log(`Tool ${context.toolName} executed in ${context.duration}ms`);
}
```

### Hook Priorities

Priorities determine execution order for interceptors.

| Priority | Value | Use Case |
|----------|-------|----------|
| **System** | -1000 | Core system hooks, runs first |
| **High** | -100 | Security checks, validation |
| **Normal** | 0 | Default priority |
| **Low** | 100 | Logging, telemetry, cleanup |

Lower values execute first. Within the same priority, hooks execute in registration order.

### Prompt Scopes

Prompt scopes control when plugin prompts are injected into the agent context.

| Scope | Behavior |
|-------|----------|
| **system** | Always injected when the plugin is active. Use for core functionality. |
| **tool** | Injected when the bound tool is available in the current context. |
| **standalone** | User must explicitly invoke (e.g., `/my-plugin`). Not auto-injected. |
| **disabled** | Never injected. Useful for temporarily disabling prompts. |

#### Prompt File Example (SKILL.md)

```markdown
# My Plugin Instructions

You have access to the my_tool function which can...

## Usage Guidelines
- Always validate input before calling
- Handle errors gracefully

## Examples
User: Do something with X
Assistant: I'll use my_tool to process X...
```

### Static Plugins

Static plugins (`kind = "static"`) contain only prompts and configuration, with no executable code:

```toml
[plugin]
id = "coding-standards"
name = "Coding Standards"
version = "1.0.0"
kind = "static"               # No entry point needed

[prompt]
file = "STANDARDS.md"
scope = "system"
```

### Migration from V1

To migrate from V1 manifest format:

1. Rename `package.json` or `aleph_plugin.json` to `aleph_plugin.toml`
2. Convert JSON structure to TOML
3. Add `kind` field (`mcp`, `wasm`, or `static`)
4. Update `runtime.type` to `kind` and `runtime.entry` to `entry`
5. Add optional hook and prompt configurations

---

## Direct Commands —— ❌ 已删除（2026-09-20）

这里曾有一节（~95 行）描述 `[[commands]] handler = "handlePing"` 式的 manifest、TypeScript
`DirectCommandArgs` / `DirectCommandResult` 签名、以及 `plugins.executeCommand` RPC。

**三者都不再存在。** `CommandRegistration` 在 2026-07-17 折进 `SkillRegistration { skill_type: Command }`
（插件 `commands/*.md` 的 markdown 正文就是那个"handler"），之后 `plugins.executeCommand` 把一段
markdown 当 WASM 导出名去调，零客户端，2026-09-20 连同 `ExtensionManager::execute_plugin_command` 一起 CUT。

插件命令今天的形状：`commands/<name>.md` → 注册为 slash 条目 → 用户 `/name args` 经 `chat.send` →
正文经 `SkillTemplate` 展开（`$ARGUMENTS` / `$N` / `@file` / `` !`cmd` ``）后作为本轮用户内容注入模型
（见 PLUGIN_SYSTEM.md「commands / agents 正文」）。**没有绕过模型的直接命令**——要确定性执行，写一个
工具（WASM 导出或 MCP tool），不要写命令。

> 保留这一节的标题而不是删干净，理由同下方 Channel / Provider 那节。

---

## Background Services (P1)

Background services allow plugins to run long-lived processes that operate independently of the main request/response cycle.

### Service Lifecycle

```
┌─────────┐      start()      ┌──────────┐
│ Stopped │ ────────────────▶ │ Starting │
└─────────┘                   └──────────┘
     ▲                              │
     │                              │ ready
     │ stop()                       ▼
┌──────────┐                  ┌─────────┐
│ Stopping │ ◀──────────────── │ Running │
└──────────┘      stop()      └─────────┘
```

| State | Description |
|-------|-------------|
| **Stopped** | Service is not running |
| **Starting** | Service is initializing |
| **Running** | Service is active and processing |
| **Stopping** | Service is shutting down gracefully |

Lifecycle wiring (when services start/stop automatically):

| Event | Behavior |
|-------|----------|
| Daemon boot | `auto_start` services of active plugins are loaded and started |
| Hot-reload | Orphaned services (plugin removed/disabled on disk) are stopped; `auto_start` services of the new active set are started |
| `plugins.enable` | `auto_start` services are started |
| `plugins.disable` / `plugins.uninstall` | Plugin runtime is unloaded — its services (and transient MCP servers) are stopped first |
| Daemon shutdown | All running services are stopped (best-effort), alongside heartbeat/ACP/mDNS teardown |

### Manifest Format

> Declaring `[[services]]` requires the `background` permission
> (`[permissions] background = true` in `aleph.plugin.toml`, or
> `[aleph.permissions] background = true` in the CC-format manifest).
> Without it the services are skipped with a warning; the rest of the
> plugin still loads. Services must declare BOTH `start_handler` and
> `stop_handler` — entries missing either are skipped.

```toml
[[services]]
name = "file-watcher"
description = "Watches filesystem for changes"
start_handler = "startFileWatcher"
stop_handler = "stopFileWatcher"
auto_start = true              # Start when plugin loads (default: true)

[[services]]
name = "sync-daemon"
description = "Background sync service"
start_handler = "startSync"
stop_handler = "stopSync"
auto_start = false             # Manual start required
```

### Handler Signatures

```typescript
interface ServiceContext {
  serviceName: string;
  config: Record<string, unknown>;
  signal: AbortSignal;         // For graceful shutdown
}

// Start handler - called when service starts
async function startFileWatcher(ctx: ServiceContext): Promise<void> {
  const watcher = new FileWatcher(ctx.config.paths);

  // Listen for abort signal
  ctx.signal.addEventListener('abort', () => {
    watcher.close();
  });

  // Start watching
  await watcher.start();
}

// Stop handler - called when service stops
async function stopFileWatcher(ctx: ServiceContext): Promise<void> {
  // Cleanup resources, flush buffers, etc.
  console.log('File watcher stopped');
}
```

### ServiceManager API

The ServiceManager coordinates all background services:

```rust
pub struct ServiceManager {
    services: HashMap<String, ServiceHandle>,
}

impl ServiceManager {
    /// Start a service by name
    pub async fn start(&self, plugin: &str, service: &str) -> Result<()>;

    /// Stop a service gracefully
    pub async fn stop(&self, plugin: &str, service: &str) -> Result<()>;

    /// Get service status
    pub fn status(&self, plugin: &str, service: &str) -> Option<ServiceStatus>;

    /// List all services
    pub fn list(&self) -> Vec<ServiceInfo>;
}

pub struct ServiceInfo {
    pub plugin: String,
    pub name: String,
    pub status: ServiceStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_secs: Option<u64>,
}

pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}
```

### Gateway RPCs

| Method | Description |
|--------|-------------|
| `services.start` | Start a background service |
| `services.stop` | Stop a running service |
| `services.list` | List all services with status |
| `services.status` | Get status of a specific service |

#### Start Service

```json
{
  "jsonrpc": "2.0",
  "method": "services.start",
  "params": {
    "plugin": "my-plugin",
    "service": "file-watcher"
  },
  "id": 1
}
```

#### Stop Service

```json
{
  "jsonrpc": "2.0",
  "method": "services.stop",
  "params": {
    "plugin": "my-plugin",
    "service": "file-watcher"
  },
  "id": 2
}
```

#### List Services

```json
{
  "jsonrpc": "2.0",
  "method": "services.list",
  "params": {},
  "id": 3
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "result": [
    {
      "plugin": "my-plugin",
      "name": "file-watcher",
      "status": "running",
      "started_at": "2026-02-03T10:00:00Z",
      "uptime_secs": 3600
    },
    {
      "plugin": "my-plugin",
      "name": "sync-daemon",
      "status": "stopped",
      "started_at": null,
      "uptime_secs": null
    }
  ],
  "id": 3
}
```

---

## Channel / Provider / HTTP-Route Plugins — ❌ 不存在（2026-08-19 更正）

这里曾有三节共 ~490 行，描述插件如何贡献 **channel**、**provider** 和
**HTTP route**：manifest 格式、handler 命名约定、`ChannelManager` API、
`PluginProviderAdapter` API、路径参数语法。

**三者在代码里都不存在，一行也没有。**

- `[[channels]]` / `[[aleph.channels]]`：`grep -rn "ChannelDeclaration\|aleph.channels" src/`
  零命中。`AlephExtensionsToml` 只有 `runtime` / `entry` / `permissions` /
  `capabilities` / `services` / `tools` / `hooks` / `commands` / `prompt` /
  `config_schema` / `config_ui_hints` / `memory`。
- `[[providers]]`：同上，`ProviderDeclaration` 零命中。
- `[[http_routes]]`：`http_route` / `HttpRoute` 在 `src/` 下零命中；字段根本不解析。

`CapabilityDeclaration` 的**全部**变体是
`Tool | Hook | Service | Skill | Agent | McpServer` —— 那是一个插件今天能贡献的
完整集合。声明上面任何一段只会被 serde 静默丢弃（未知键），插件照样加载，
而作者会一直等一个永远不会被调用的 handler。

### 那要怎么加一个 channel / provider？

改 core：channel 落在 `src/gateway/channel*` + `interfaces/<channel>/`，
provider 落在 `src/providers/`。两者都要在
`aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES` 这类单一源上登记
（见 GATEWAY.md 关于「加了 adapter ≠ 用户能配」那条判据）。

对**外部服务**而言，`runtime = "mcp"` 的插件已经是可用的答案：MCP server 能提供
工具，而工具是模型真正会调的东西。

> 这一节保留而不是删干净，因为一个搜索 `[[channels]]` 的作者会先找到 git 历史里的
> 旧文档；让他在同一个位置读到「这从来没有过」比什么都读不到便宜。

---

## See Also

- [Aleph Hub](ALEPH_HUB.md) - Extension **distribution**: catalog contract, trust rails, install pipeline (this document covers the **runtime** that loads what the Hub installs)
- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How tools work
- [Gateway](GATEWAY.md) - Plugin RPC methods

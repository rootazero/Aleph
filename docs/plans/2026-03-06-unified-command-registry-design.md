# Unified Command Registry Design

> Aleph 统一命令注册表设计 — 学习 OpenClaw，超越 OpenClaw

Date: 2026-03-06

## 1. Background & Motivation

### 1.1 Current State

Aleph has two separate registry systems that partially overlap:

- **`dispatcher::registry::ToolRegistry`** — Unified tool aggregation with conflict resolution, smart discovery, state management. Aggregates Builtin, MCP, Skill, Custom tools into `UnifiedTool`.
- **`command::CommandRegistry`** — Static command list built from `config.toml` routing rules, with hint localization and prefix filtering. Outputs `CommandNode`.
- **`command::CommandParser`** — Slash command parser that maintains 4 independent lookup lists (builtin_commands, skills_registry, routing_rules, mcp_server_names).

These three components operate independently, causing:

1. **Duplicated registration** — Skills, MCP tools, and custom commands are registered in both `ToolRegistry` and `CommandRegistry`/`CommandParser` separately.
2. **Incomplete command discovery** — `commands.list()` RPC only returns rules from `CommandRegistry`, missing MCP tools and builtin tools registered in `ToolRegistry`.
3. **No channel differentiation** — All channels see the same commands with no visibility filtering.
4. **No dispatch mode distinction** — All commands go through Agent Loop, even deterministic ones like `/help` or `/status`.

### 1.2 OpenClaw Reference

OpenClaw (`~/Workspace/openclaw`) uses a centralized `CommandsRegistry` with:

- **Central declarative definitions** — All 40+ commands in `commands-registry.data.ts`
- **Multi-scope support** — `scope: "text" | "native" | "both"` for channel differentiation
- **Skill-as-first-class** — Skills auto-generate slash commands via `buildSkillCommandDefinitions()`
- **Plugin API** — `registerCommand()`, `registerTool()`, `registerHook()` unified registration
- **Mixed dispatch** — Some commands dispatch directly, others go through agent

### 1.3 Design Decisions (from brainstorming)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Unification scope | B: Discovery + Routing | A too shallow, C too risky |
| Command sources | Builtin + Tools + MCP + Skills + Plugins | Gateway RPC methods excluded |
| Naming | C: Flat-first + prefix on conflict | Already implemented in `ConflictResolver` |
| Dispatch mode | C: Mixed (Direct \| AgentLoop) | `/help` doesn't need LLM |
| Channel adaptation | B: Registry-side visibility | `visible_channels` + `list_for_channel()` |

## 2. Core Design: Extend ToolRegistry, Retire CommandRegistry

### 2.1 Key Insight

`ToolRegistry` already has 90% of what a unified command registry needs. Instead of building a new `UnifiedCommandRegistry`, we extend `ToolRegistry` with 2 fields and 2 methods, then retire `CommandRegistry`.

### 2.2 New Types

```rust
/// How a command is dispatched when invoked by user
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchMode {
    /// Execute directly, bypass Agent Loop (e.g., /help, /status)
    Direct,
    /// Inject into Agent Loop with context (e.g., /search, /translate)
    AgentLoop,
}

impl Default for DispatchMode {
    fn default() -> Self {
        DispatchMode::AgentLoop
    }
}

/// Channel types for visibility filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelType {
    Panel,      // WebSocket UI (Leptos/WASM)
    Telegram,
    Discord,
    IMessage,
    Cli,
}
```

### 2.3 UnifiedTool Extensions

Two new fields added to `UnifiedTool`:

```rust
pub struct UnifiedTool {
    // ... existing fields ...

    /// Dispatch mode: Direct (bypass LLM) or AgentLoop (inject into agent)
    #[serde(default)]
    pub dispatch_mode: DispatchMode,

    /// Channels that can see this command (empty = all channels)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_channels: Vec<ChannelType>,
}
```

### 2.4 ToolRegistry Extensions

Two new methods:

```rust
impl ToolRegistry {
    /// List active tools visible to a specific channel
    pub async fn list_for_channel(&self, channel: ChannelType) -> Vec<UnifiedTool>;

    /// Resolve a slash command input to a registered tool
    pub async fn resolve_command(&self, input: &str) -> Option<ResolvedCommand>;
}

/// Result of resolving a user slash command
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    pub tool: UnifiedTool,
    pub arguments: Option<String>,
    pub raw_input: String,
}
```

### 2.5 DispatchMode Inference Rules

Auto-inferred during registration, overridable via builder:

| Source | Default DispatchMode | Rationale |
|--------|---------------------|-----------|
| Builtin (generate_image, etc.) | AgentLoop | LLM needed to understand params |
| MCP | AgentLoop | LLM needed to assemble params |
| Skill | AgentLoop | LLM needed to inject skill instructions |
| Custom (routing rules) | AgentLoop | LLM needed with system_prompt |
| Special (/help, /status, /tools) | Direct | Deterministic, no LLM needed |

### 2.6 visible_channels Default Rules

| Condition | Default visible_channels | Rationale |
|-----------|------------------------|-----------|
| All tools | `[]` (all channels) | Default open |
| `safety_level = IrreversibleHighRisk` | Exclude Telegram/Discord/iMessage | Dangerous ops only via Panel/CLI |
| `requires_confirmation = true` | Exclude iMessage | iMessage has no confirmation UI |

Defaults auto-applied during `register_*` methods, overridable via config.

## 3. CommandRegistry Retirement

### 3.1 Migration Map

| CommandRegistry Method | Migration Target | Notes |
|-----------------------|-----------------|-------|
| `from_config()` | `ToolRegistry::register_custom_commands()` | Already exists, add hint/icon |
| `inject_skills()` | `ToolRegistry::register_skills()` | Already exists |
| `get_root_commands()` | `ToolRegistry::list_root_commands()` | Already exists |
| `filter_by_prefix()` | `ToolRegistry` new method | Simple migration |
| `execute_command()` | Delete — replaced by `resolve_command()` + dispatch | Current impl only returns strings |
| `get_builtin_hint()` | `UnifiedTool.localization_key` + hint field | Already has field |
| `set_language()` / `set_show_hints()` | `ToolRegistry` new UI config methods | Simple migration |

### 3.2 CommandParser Simplification

Before (4 independent lookup lists):

```rust
pub struct CommandParser {
    command_registry: Option<Arc<CommandRegistry>>,
    skills_registry: Option<Arc<SkillsRegistry>>,
    routing_rules: Vec<RoutingRuleConfig>,
    mcp_server_names: Vec<String>,
    builtin_commands: Vec<&'static str>,
}
```

After (single dependency):

```rust
pub struct CommandParser {
    tool_registry: Arc<ToolRegistry>,
}

impl CommandParser {
    pub async fn parse(&self, input: &str) -> Option<ResolvedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        self.tool_registry.resolve_command(trimmed).await
    }
}
```

`CommandContext` enum preserved but derived from `UnifiedTool` fields:

| UnifiedTool Fields | CommandContext Variant |
|-------------------|----------------------|
| `source: Builtin` + `routing_capabilities` | `Builtin { tool_name }` |
| `source: Mcp { server }` | `Mcp { server_name, tool_name }` |
| `source: Skill { id }` | `Skill { skill_id, instructions, ... }` |
| `source: Custom` + `routing_system_prompt` | `Custom { system_prompt, provider, pattern }` |

## 4. Dispatch Layer

### 4.1 Flow

```
User input "/search rust async"
    |
Channel layer (Telegram/Panel/CLI/...)
    |
CommandParser::parse(input) -> ToolRegistry::resolve_command(input)
    |
ResolvedCommand { tool, arguments, raw_input }
    |
+-- tool.dispatch_mode --+
|                         |
v Direct                  v AgentLoop
CommandDispatcher         Existing Agent Loop
::execute_direct()        (inject system_prompt + capabilities)
    |                         |
    v                         v
Immediate result          LLM think -> tool call -> result
```

### 4.2 CommandDispatcher (new)

```rust
/// Executes Direct-mode commands without going through Agent Loop
pub struct CommandDispatcher {
    tool_registry: Arc<ToolRegistry>,
    handlers: HashMap<String, Box<dyn DirectHandler>>,
}

#[async_trait]
pub trait DirectHandler: Send + Sync {
    async fn execute(&self, args: Option<&str>, ctx: &CommandContext) -> CommandExecutionResult;
}
```

Initial Direct commands:

| Command | DirectHandler | Behavior |
|---------|-------------|----------|
| `/help` | `HelpHandler` | Return formatted `list_for_channel()` |
| `/status` | `StatusHandler` | Return agent status, active sessions |
| `/tools` | `ToolsHandler` | Return available tools list |

### 4.3 Channel Integration

All channels follow the same pattern — no command parsing/routing logic in channel layer (R4 compliant):

```rust
async fn handle_message(input: &str, channel: ChannelType) {
    if let Some(resolved) = command_parser.parse(input).await {
        match resolved.tool.dispatch_mode {
            DispatchMode::Direct => command_dispatcher.execute_direct(&resolved).await,
            DispatchMode::AgentLoop => agent_loop.run_with_context(&resolved).await,
        }
    } else {
        agent_loop.run(input).await;
    }
}
```

## 5. File Changes

### 5.1 Modified Files

```
core/src/dispatcher/types/unified.rs     — Add DispatchMode, ChannelType, 2 fields + builders
core/src/dispatcher/registry/mod.rs      — Add list_for_channel(), resolve_command()
core/src/dispatcher/registry/query.rs    — Add list_for_channel() impl
core/src/dispatcher/registry/registration.rs — Infer dispatch_mode + visible_channels
core/src/command/mod.rs                  — Remove registry re-export
core/src/command/parser.rs               — Simplify to use ToolRegistry
core/src/gateway/handlers/commands.rs    — Use ToolRegistry for handle_list()
```

### 5.2 New Files

```
core/src/command/dispatcher.rs           — CommandDispatcher + DirectHandler (~100 lines)
```

### 5.3 Deleted Files

```
core/src/command/registry.rs             — Retired, logic migrated to ToolRegistry
```

### 5.4 Unchanged

- `dispatcher/registry/conflict.rs` — Conflict resolution works as-is
- `dispatcher/registry/state.rs` — State management unchanged
- `dispatcher/registry/discovery.rs` — LLM discovery unchanged
- `executor/builtin_registry/` — Tool execution engine unchanged
- `tools/` — AlephTool trait unchanged
- `mcp/` — MCP client unchanged
- `extension/` — Plugin system unchanged

### 5.5 Dependency Flow

```
command::CommandParser
    -> dispatcher::registry::ToolRegistry  (Arc reference)

command::CommandDispatcher
    -> dispatcher::registry::ToolRegistry  (Arc reference)
    -> DirectHandler trait impls

gateway::handlers::commands
    -> dispatcher::registry::ToolRegistry  (via GatewayContext)

Channels (Telegram/Discord/...)
    -> command::CommandParser  (parse)
    -> command::CommandDispatcher  (Direct execution)
    -> agent_loop  (AgentLoop execution)
```

Direction: `Interface -> command -> dispatcher` (P1 unidirectional).

## 6. OpenClaw Comparison

### 6.1 Learned From OpenClaw

| OpenClaw Design | Aleph Adoption | Difference |
|----------------|---------------|------------|
| Central declaration file | Reuse `ToolRegistry` as single source | Dynamic registry, not static file |
| `scope: text/native/both` | `visible_channels: Vec<ChannelType>` | Finer granularity, 5+ channels |
| `buildSkillCommandDefinitions()` | `register_skills()` already exists | Already have it |
| Plugin `registerCommand()` | `register_with_conflict_resolution()` | Already have it, with auto-conflict |
| Mixed dispatch | `DispatchMode::Direct \| AgentLoop` | Similar but simpler |

### 6.2 Where Aleph Exceeds OpenClaw

| Dimension | OpenClaw | Aleph |
|-----------|---------|-------|
| Conflict resolution | None (manual avoidance) | Priority-based auto-resolve + rename |
| LLM smart discovery | None | `generate_tool_index()` + `generate_smart_prompt()` |
| Safety classification | `ownerOnly` boolean | `ToolSafetyLevel` 4-tier + policy inference |
| Tool metadata | Simple description | `StructuredToolMeta` (capabilities, differentiation, use_when) |
| Type safety | TypeScript runtime checks | Rust compile-time guarantees |
| Architecture | Single-file declaration | 5-responsibility separation (Registrar/Conflict/Query/State/Discovery) |

### 6.3 Explicitly Not Doing (YAGNI)

- `nativeNameOverrides` (channel aliases) — No current need
- `argsMenu` / `argsParsing` (command parameter UI) — Over-complex
- `registerHook` / `registerChannel` / `registerProvider` (plugin full API) — Out of scope
- Channel alias adaptation (Discord `/voice` = Telegram `/tts`) — No requirement

## 7. Architectural Compliance

| Constraint | Satisfied |
|-----------|-----------|
| R1: Brain-Limb Separation | Yes — core layer has no platform API deps |
| R4: I/O-Only Interfaces | Yes — channels only do parse -> dispatch |
| P1: Low Coupling | Yes — unidirectional dependency flow |
| P2: High Cohesion | Yes — CommandRegistry responsibilities consolidated into ToolRegistry |
| P3: Extensibility | Yes — new command sources just implement registration |
| P4: Dependency Inversion | Yes — DirectHandler trait for dispatch |
| P6: Simplicity | Yes — extend existing system, not rebuild |

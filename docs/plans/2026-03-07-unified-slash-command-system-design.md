# Unified Slash Command System Design

> Date: 2026-03-07
> Status: Approved
> Scope: Connect existing dispatcher::ToolRegistry + CommandParser to channel message flow

## Problem

Aleph has a fully implemented unified tool registry (`dispatcher::ToolRegistry`) with four-source registration (Builtin, MCP, Skills, Custom), conflict resolution, channel visibility filtering, and `resolve_command()` slash command parsing. It also has a `CommandParser` that delegates to this registry.

**None of this is wired at startup.** The result:

| Component | Status |
|-----------|--------|
| `dispatcher::ToolRegistry` | Implemented, never instantiated at startup |
| `CommandParser` | Implemented, never injected into router |
| `BuiltinToolRegistry` (executor) | Only registry actually used — 19 builtin tools only |
| `ExecutionIntentDecider` | Hardcoded 6 slash commands, partially wired to router |
| Skills / MCP / Custom commands | Invisible to channel message routing |

Slash commands from any communication channel (Telegram, Discord, etc.) cannot resolve skills, MCP tools, or custom commands.

## Design Decisions

1. **dispatcher::ToolRegistry is the Single Source of Truth** — no new registry concepts
2. **Composition over unification** — dispatcher (discovery/query) and executor (execution) stay separate, connected via CommandParser
3. **Preserve existing InboundRouter framework** — keep `starts_with('/')` detection + metadata passing + engine fast path; upgrade parse source from static to dynamic
4. **Channel native command discovery deferred** — `list_for_channel()` already exists; channel-side registration (Telegram BotFather, Discord Application Commands) is future work
5. **Lane concurrency deferred** — not in scope

## Architecture

```
Channel Message (Telegram/Discord/...)
    │
    ▼
InboundMessageRouter::handle_message()
    │ starts_with('/')
    ▼
CommandParser::parse_async()           ← delegates to dispatcher::ToolRegistry
    │ ParsedCommand { source_type, context, args }
    ▼
serialize ExecutionMode → RunRequest.metadata
    │
    ▼
ExecutionEngine::execute()
    │ check SLASH_COMMAND_MODE_KEY
    ▼
execute_slash_command_fast_path()
    ├─ DirectTool  → executor.execute_tool()       [<1ms, no LLM]
    ├─ Skill       → inject instructions → agent   [needs LLM]
    ├─ MCP         → executor.execute_tool()       [<1ms, no LLM]
    └─ Custom      → inject system_prompt → agent  [needs LLM]
    │ (fallback on failure)
    ▼
Normal Agent Loop
```

## Four Connection Points

### C1: Startup — Create dispatcher::ToolRegistry and populate all sources

**File**: `core/src/bin/aleph/commands/start/mod.rs`

```rust
let dispatch_registry = Arc::new(dispatcher::ToolRegistry::new());

// Builtin tools
dispatch_registry.register_builtin_tools(&builtin_tool_defs).await;

// Skills (from SkillSystem snapshot)
if let Some(skill_system) = &skill_system {
    let skill_tools = skill_system.to_unified_tools();
    dispatch_registry.register_skills(&skill_tools).await;
}

// MCP tools (from McpManager)
if let Some(mcp_manager) = &mcp_manager {
    let mcp_tools = mcp_manager.aggregate_tools().await;
    dispatch_registry.register_mcp_tools(&mcp_tools).await;
}

// Custom commands (from config routing rules)
dispatch_registry.register_custom_commands(&app_config.routing.rules).await;
```

### C2: Startup — Create CommandParser, inject into InboundMessageRouter

**File**: `core/src/bin/aleph/commands/start/mod.rs`

```rust
let command_parser = Arc::new(CommandParser::new(dispatch_registry.clone()));
inbound_router = inbound_router.with_command_parser(command_parser);
```

### C3: InboundMessageRouter — Use CommandParser instead of ExecutionIntentDecider for L0

**File**: `core/src/gateway/inbound_router.rs`

Replace `ExecutionIntentDecider.decide()` with `CommandParser.parse_async()` for `/` prefixed messages. Keep `ExecutionIntentDecider` for L1-L4 intent analysis on non-slash inputs.

```rust
if ctx.message.text.trim().starts_with('/') {
    if let Some(ref parser) = self.command_parser {
        if let Some(parsed) = parser.parse_async(input).await {
            let mode = parsed_command_to_execution_mode(parsed);
            if let Some(mode_json) = serialize_execution_mode(&mode) {
                self.execute_for_context_with_metadata(&ctx, mode_json).await?;
                return Ok(());
            }
        }
    }
}
```

### C4: ExecutionEngine fast path — Already implemented, no changes needed

The `execute_slash_command_fast_path()` method reads `SLASH_COMMAND_MODE_KEY` from metadata and dispatches by type (direct_tool, skill, mcp, custom). This stays as-is.

## Responsibility Matrix

| Component | Role | Changes |
|-----------|------|---------|
| `dispatcher::ToolRegistry` | Register, query, conflict resolve, channel visibility | **Instantiate at startup** (C1) |
| `CommandParser` | `/input` → `ParsedCommand` | **Inject into router** (C2) |
| `executor::ToolRegistry` (trait) | Execute tools | None |
| `BuiltinToolRegistry` | Implement executor trait for builtin tools | None |
| `ExecutionIntentDecider` | L1-L4 intent (non-slash inputs) | Demote from L0 slash handling (C3) |
| `InboundMessageRouter` | Channel message entry, command interception | Use `CommandParser` for `/` (C3) |
| `ExecutionEngine` | Fast path execution | None (C4 already done) |

## Data Flow (Before → After)

**Before (disconnected)**:
```
Builtin 6 → ExecutionIntentDecider.SLASH_COMMANDS (static LazyLock)
Skills    → SkillSystem (isolated)
MCP       → McpManager (isolated)
Custom    → config (isolated)
```

**After (unified)**:
```
Builtin ──→ dispatcher::ToolRegistry ──┐
Skills  ──→ dispatcher::ToolRegistry   ├→ CommandParser → InboundRouter → Engine
MCP     ──→ dispatcher::ToolRegistry   │
Custom  ──→ dispatcher::ToolRegistry ──┘
```

## Reserved Interfaces (Not Implemented)

- `dispatch_registry.list_for_channel("telegram")` — method exists, data will be ready after C1. Channel adapters can use this to register native slash commands with platform APIs.
- Lane-based concurrency isolation — future work for separating slash command execution from normal conversation.

## Reference: OpenClaw Comparison

| Aspect | OpenClaw | Aleph (After) |
|--------|----------|---------------|
| Registry | `ChatCommandDefinition` + plugin tool resolution | `dispatcher::ToolRegistry` (4-source, conflict-resolved) |
| Command parsing | `getTextAliasMap()` + `findCommandByNativeName()` | `CommandParser.parse_async()` → `resolve_command()` |
| Channel adaptation | `ChannelDock` + `ChannelCommandAdapter` | `list_for_channel()` (data ready, channel impl deferred) |
| Concurrency | Lane-based async task queue | Tokio task spawning (lane pattern deferred) |
| Execution | PI Agent Framework | AgentLoop + executor::ToolRegistry |

Key insight: Aleph's `dispatcher::ToolRegistry` is more sophisticated than OpenClaw's command registry (conflict resolution, priority-based naming, channel visibility filtering). The gap was purely in wiring, not design.

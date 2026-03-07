# Unified Slash Command System — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing `dispatcher::ToolRegistry` and `CommandParser` into the channel message flow so all slash commands (builtin, skills, MCP, custom) work from any communication channel.

**Architecture:** Four connection points at startup and routing layers. No new abstractions — only connect existing components. Also unify `/switch` and `/groupchat` into the command registry instead of special-casing them.

**Tech Stack:** Rust, Tokio, alephcore dispatcher/executor/command modules

**Design doc:** `docs/plans/2026-03-07-unified-slash-command-system-design.md`

---

### Task 1: Create and populate dispatcher::ToolRegistry at startup (C1)

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs:335-348`

**Step 1: Add dispatcher::ToolRegistry creation after BuiltinToolRegistry**

In `register_agent_handlers()`, after `let tool_registry = Arc::new(tool_registry);` (line 336), add:

```rust
// Create unified dispatch registry (command discovery + resolution)
use alephcore::dispatcher::ToolRegistry as DispatchRegistry;
let dispatch_registry = Arc::new(DispatchRegistry::new());

// Register builtin tools
dispatch_registry.register_builtin_tools().await;

// Register custom commands from config routing rules
let routing_rules = &app_config.routing.rules;
dispatch_registry.register_custom_commands(routing_rules).await;

// Register skills from ExtensionManager (if initialized)
{
    use alephcore::gateway::handlers::plugins::get_extension_manager;
    if let Ok(ext_manager) = get_extension_manager() {
        if let Some(skill_sys) = ext_manager.skill_system() {
            let skills = skill_sys.list_skills().await;
            let skill_infos: Vec<alephcore::skills::SkillInfo> = skills
                .iter()
                .filter(|s| s.is_user_invocable())
                .map(|s| alephcore::skills::SkillInfo {
                    id: s.id().as_str().to_string(),
                    name: s.name().to_string(),
                    description: s.description().to_string(),
                    triggers: Vec::new(),
                    allowed_tools: Vec::new(),
                    ecosystem: "aleph".to_string(),
                })
                .collect();
            dispatch_registry.register_skills(&skill_infos).await;
            if !daemon {
                println!("  Dispatch registry: {} skills registered", skill_infos.len());
            }
        }
    }
}

if !daemon {
    println!("  Dispatch registry initialized");
}
```

**Step 2: Thread dispatch_registry through to `setup_inbound_routing`**

Add `dispatch_registry: Arc<DispatchRegistry>` to the `AgentHandlersResult` struct and return it. Then pass it into `setup_inbound_routing()`.

In `AgentHandlersResult` struct (line 264):
```rust
struct AgentHandlersResult {
    _run_manager: Arc<AgentRunManager>,
    execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,  // NEW
}
```

Set `dispatch_registry: Some(dispatch_registry)` in the provider-available path, and `dispatch_registry: None` in the fallback path.

**Step 3: Verify compilation**

Run: `cargo check -p alephcore && cargo check -p aleph`
Expected: No errors

**Step 4: Commit**

```
startup: create and populate dispatcher::ToolRegistry at boot
```

---

### Task 2: Create CommandParser and inject into InboundMessageRouter (C2)

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs` (setup_inbound_routing function, ~line 1015)

**Step 1: Accept dispatch_registry in `setup_inbound_routing`**

Add parameter `dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>` to the function signature (line ~1015).

**Step 2: Create CommandParser and inject**

After the `inbound_router` is created but before `Arc::new(inbound_router)` (around line 1085), add:

```rust
// Wire command parser for unified slash command resolution
if let Some(reg) = dispatch_registry {
    let command_parser = Arc::new(alephcore::command::CommandParser::new(reg));
    inbound_router = inbound_router.with_command_parser(command_parser.clone());
    if !daemon {
        println!("  Inbound router: slash command resolution enabled (unified registry)");
    }
}
```

**Step 3: Update the caller to pass dispatch_registry**

In `start_server()`, where `setup_inbound_routing()` is called, pass `agent_result.dispatch_registry`.

**Step 4: Verify compilation**

Run: `cargo check -p alephcore && cargo check -p aleph`

**Step 5: Commit**

```
startup: wire CommandParser into InboundMessageRouter
```

---

### Task 3: InboundMessageRouter uses CommandParser for slash commands (C3)

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`

**Step 1: Replace ExecutionIntentDecider L0 with CommandParser for `/` commands**

Replace the current slash command interception block (around line 594):

```rust
// OLD:
if ctx.message.text.trim().starts_with('/') {
    let decision = self.intent_decider.decide(ctx.message.text.trim(), None);
    if let Some(mode_json) = serialize_execution_mode(&decision.mode) {
        ...
    }
}
```

With:

```rust
// Slash command interception (unified registry)
// Resolves builtin, skill, MCP, and custom commands via dispatcher::ToolRegistry
if ctx.message.text.trim().starts_with('/') {
    if let Some(ref parser) = self.command_parser {
        if let Some(parsed) = parser.parse_async(ctx.message.text.trim()).await {
            let mode = self.parsed_command_to_mode(parsed);
            if let Some(mode_json) = serialize_execution_mode(&mode) {
                info!(
                    "[Router] Slash command resolved: source={:?}, name={}",
                    mode, ctx.message.text.trim().split_whitespace().next().unwrap_or("")
                );
                self.execute_for_context_with_metadata(&ctx, mode_json).await?;
                return Ok(());
            }
        }
    }
    // Fallback: if no CommandParser, try ExecutionIntentDecider (builtin-only)
    let decision = self.intent_decider.decide(ctx.message.text.trim(), None);
    if let Some(mode_json) = serialize_execution_mode(&decision.mode) {
        info!(
            "[Router] Slash command (builtin fallback): layer={:?}",
            decision.metadata.layer
        );
        self.execute_for_context_with_metadata(&ctx, mode_json).await?;
        return Ok(());
    }
}
```

**Step 2: Add `parsed_command_to_mode` helper method**

This method already exists in `ExecutionIntentDecider` — extract the same logic as a method on `InboundMessageRouter`:

```rust
/// Convert a ParsedCommand to ExecutionMode
fn parsed_command_to_mode(&self, cmd: ParsedCommand) -> ExecutionMode {
    use crate::command::CommandContext;
    use crate::intent::{
        CustomInvocation, McpInvocation, SkillInvocation, ToolInvocation,
    };

    let args = cmd.arguments.clone().unwrap_or_default();

    match cmd.context {
        CommandContext::Builtin { tool_name } => {
            ExecutionMode::DirectTool(ToolInvocation {
                tool_id: tool_name,
                args,
            })
        }
        CommandContext::Skill {
            skill_id,
            instructions,
            display_name,
            allowed_tools,
        } => ExecutionMode::Skill(SkillInvocation {
            skill_id,
            display_name,
            instructions,
            args,
            allowed_tools,
        }),
        CommandContext::Mcp {
            server_name,
            tool_name,
        } => ExecutionMode::Mcp(McpInvocation {
            server_name,
            tool_name,
            args,
        }),
        CommandContext::Custom {
            system_prompt,
            provider,
            ..
        } => ExecutionMode::Custom(CustomInvocation {
            command_name: cmd.command_name,
            system_prompt,
            provider,
            args,
        }),
        CommandContext::None => ExecutionMode::DirectTool(ToolInvocation {
            tool_id: cmd.command_name,
            args,
        }),
    }
}
```

**Step 3: Store `command_parser` field**

Change the `command_parser` field type from implicit (via `intent_decider.set_command_parser`) to explicit:

```rust
/// Command parser for unified slash command resolution (optional)
command_parser: Option<Arc<CommandParser>>,
```

Initialize as `None` in all three constructors and set via `with_command_parser`:

```rust
pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
    self.command_parser = Some(parser.clone());
    // Also wire into intent_decider for backward compatibility
    self.intent_decider.set_command_parser(parser);
    self
}
```

**Step 4: Verify compilation**

Run: `cargo check -p alephcore`

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib inbound_router execution_decider command`

**Step 6: Commit**

```
inbound_router: use CommandParser for unified slash command resolution
```

---

### Task 4: Unify /switch and /groupchat into command registry

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`
- Modify: `core/src/dispatcher/registry/registration.rs`

**Step 1: Register /switch and /groupchat as builtin commands**

In `registration.rs`, inside `register_builtin_tools()`, add:

```rust
// Agent switching command
let switch_cmd = UnifiedTool::new(
    "builtin:switch",
    "switch",
    "Switch to a different AI agent",
    ToolSource::Builtin,
)
.with_icon("arrow.triangle.swap")
.with_usage("/switch <agent_id>")
.with_slash_command(true);

conflict_resolver
    .register_with_conflict_resolution(switch_cmd, &self.tools)
    .await;

// Group chat command
let groupchat_cmd = UnifiedTool::new(
    "builtin:groupchat",
    "groupchat",
    "Start, end, or manage a multi-persona group chat",
    ToolSource::Builtin,
)
.with_icon("person.3")
.with_usage("/groupchat start <personas> [topic]")
.with_slash_command(true);

conflict_resolver
    .register_with_conflict_resolution(groupchat_cmd, &self.tools)
    .await;
```

**Step 2: Move /switch and /groupchat handling into the unified slash command path**

In `inbound_router.rs`, the `/switch` interception at line 463 and `/groupchat` at line 509 are currently handled BEFORE the unified slash command check. Refactor so:

1. The unified slash command check happens FIRST
2. When `CommandParser` resolves `/switch` or `/groupchat`, the router handles them internally (no need to send to ExecutionEngine)
3. Add a match in the slash command block:

```rust
if ctx.message.text.trim().starts_with('/') {
    // Try unified command resolution first
    if let Some(ref parser) = self.command_parser {
        if let Some(parsed) = parser.parse_async(ctx.message.text.trim()).await {
            // Handle /switch internally
            if parsed.command_name == "switch" {
                if let Some(args) = &parsed.arguments {
                    return self.handle_switch_command(args.trim(), &msg, &ctx).await;
                }
            }
            // Handle /groupchat internally
            if parsed.command_name == "groupchat" {
                return self.handle_groupchat_command(&msg).await;
            }
            // All other commands → execution engine
            let mode = self.parsed_command_to_mode(parsed);
            if let Some(mode_json) = serialize_execution_mode(&mode) {
                self.execute_for_context_with_metadata(&ctx, mode_json).await?;
                return Ok(());
            }
        }
    }
    // Fallback path for when no CommandParser is available
    // (keeps existing /switch and /groupchat inline handling)
}
```

**Step 3: Extract /switch handling into a method**

Extract the existing `/switch` logic (lines 463-501) into:

```rust
async fn handle_switch_command(
    &self,
    agent_name: &str,
    msg: &InboundMessage,
    ctx: &InboundContext,
) -> Result<(), RoutingError> {
    // ... existing /switch logic moved here ...
}
```

**Step 4: Extract /groupchat handling into a method**

Extract `/groupchat` logic into:

```rust
async fn handle_groupchat_command(
    &self,
    msg: &InboundMessage,
) -> Result<(), RoutingError> {
    // ... existing /groupchat dispatch logic moved here ...
}
```

**Step 5: Verify compilation and run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib inbound_router`

**Step 6: Commit**

```
inbound_router: unify /switch and /groupchat into command registry
```

---

### Task 5: Integration test — end-to-end slash command resolution

**Files:**
- Modify: `core/tests/steps/gateway_steps.rs` (or create new test)

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_slash_command_resolution_via_dispatch_registry() {
    use alephcore::dispatcher::ToolRegistry as DispatchRegistry;
    use alephcore::command::CommandParser;

    // Create and populate registry
    let registry = Arc::new(DispatchRegistry::new());
    registry.register_builtin_tools().await;

    // Register a custom command
    let rules = vec![alephcore::config::RoutingRuleConfig {
        regex: "^/translate".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Translate text".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // Create parser
    let parser = CommandParser::new(registry);

    // Verify builtin resolves
    let result = parser.parse_async("/screenshot").await;
    assert!(result.is_some(), "builtin /screenshot should resolve");

    // Verify custom resolves
    let result = parser.parse_async("/translate hello").await;
    assert!(result.is_some(), "custom /translate should resolve");
    let cmd = result.unwrap();
    assert_eq!(cmd.arguments, Some("hello".to_string()));

    // Verify unknown returns None
    let result = parser.parse_async("/nonexistent").await;
    assert!(result.is_none());
}
```

**Step 2: Run the test**

Run: `cargo test -p alephcore --lib test_slash_command_resolution_via_dispatch_registry`

**Step 3: Commit**

```
test: add integration test for unified slash command resolution
```

---

### Task 6: Clean up dead code from initial partial fix

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Step 1: Remove ExecutionIntentDecider field if fully replaced**

If `CommandParser` is always available (non-optional), the `intent_decider` field can be removed. If it's optional (fallback), keep it as-is.

Decision: Keep `intent_decider` as fallback for when no `CommandParser` is configured (e.g., tests, simple mode without provider). No cleanup needed.

**Step 2: Remove unused imports if any**

Run: `cargo check -p alephcore 2>&1 | grep "unused import"`

Fix any warnings related to our changes.

**Step 3: Final test run**

Run: `cargo test -p alephcore --lib -- gateway inbound_router execution_engine execution_decider command`

**Step 4: Commit**

```
cleanup: remove unused imports from slash command unification
```

---

## Summary

| Task | What | Risk |
|------|------|------|
| T1 | Populate dispatcher::ToolRegistry at startup | Low — additive, no existing behavior changed |
| T2 | Inject CommandParser into InboundMessageRouter | Low — additive wiring |
| T3 | Router uses CommandParser for `/` commands | Medium — changes message flow, has fallback |
| T4 | Unify /switch and /groupchat | Medium — refactors existing interception |
| T5 | Integration test | None — test only |
| T6 | Clean up | Low — cosmetic |

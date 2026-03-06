# Unified Command Registry Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend `ToolRegistry` as the single source of truth for all commands, retire `CommandRegistry`, simplify `CommandParser`, and add `DispatchMode` + channel visibility.

**Architecture:** Extend existing `dispatcher::registry::ToolRegistry` with 2 new fields (`dispatch_mode`, `visible_channels`) on `UnifiedTool`, add `resolve_command()` and `list_for_channel()` query methods, create a `CommandDispatcher` for Direct-mode execution, simplify `CommandParser` to delegate to `ToolRegistry`, and retire `CommandRegistry`.

**Tech Stack:** Rust, Tokio (async), serde, existing `dispatcher::registry` module

**Design doc:** `docs/plans/2026-03-06-unified-command-registry-design.md`

---

### Task 1: Add DispatchMode and ChannelType to UnifiedTool

**Files:**
- Modify: `core/src/dispatcher/types/unified.rs`
- Modify: `core/src/dispatcher/types/mod.rs` (re-exports)
- Test: inline in `unified.rs`

**Step 1: Write the failing test**

Add at the bottom of the `#[cfg(test)] mod tests` block in `core/src/dispatcher/types/unified.rs`:

```rust
#[test]
fn test_dispatch_mode_default() {
    let tool = UnifiedTool::new(
        "custom:test",
        "test",
        "Test tool",
        ToolSource::Custom { rule_index: 0 },
    );
    assert_eq!(tool.dispatch_mode, DispatchMode::AgentLoop);
    assert!(tool.visible_channels.is_empty());
}

#[test]
fn test_dispatch_mode_builder() {
    let tool = UnifiedTool::new(
        "builtin:help",
        "help",
        "Show help",
        ToolSource::Builtin,
    )
    .with_dispatch_mode(DispatchMode::Direct)
    .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

    assert_eq!(tool.dispatch_mode, DispatchMode::Direct);
    assert_eq!(tool.visible_channels.len(), 2);
    assert!(tool.visible_channels.contains(&ChannelType::Panel));
}

#[test]
fn test_channel_type_equality() {
    assert_eq!(ChannelType::Panel, ChannelType::Panel);
    assert_ne!(ChannelType::Panel, ChannelType::Telegram);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::types::unified::tests::test_dispatch_mode_default -- --exact`
Expected: FAIL — `DispatchMode` and `ChannelType` not defined

**Step 3: Implement DispatchMode, ChannelType, and extend UnifiedTool**

In `core/src/dispatcher/types/unified.rs`, add above the `UnifiedTool` struct definition:

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
    Panel,
    Telegram,
    Discord,
    IMessage,
    Cli,
}
```

Add two new fields to `UnifiedTool` struct (after the `was_renamed` field, before `structured_meta`):

```rust
    // =========================================================================
    // Command Dispatch & Channel Visibility
    // =========================================================================
    /// Dispatch mode: Direct (bypass LLM) or AgentLoop (inject into agent)
    #[serde(default)]
    pub dispatch_mode: DispatchMode,

    /// Channels that can see this command (empty = all channels)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_channels: Vec<ChannelType>,
```

Add defaults in `UnifiedTool::new()`:

```rust
    dispatch_mode: DispatchMode::default(),
    visible_channels: Vec::new(),
```

Add builder methods after the conflict resolution builder section:

```rust
    // =========================================================================
    // Dispatch & Visibility Builder Methods
    // =========================================================================

    /// Builder method: set dispatch mode
    pub fn with_dispatch_mode(mut self, mode: DispatchMode) -> Self {
        self.dispatch_mode = mode;
        self
    }

    /// Builder method: set visible channels
    pub fn with_visible_channels(mut self, channels: Vec<ChannelType>) -> Self {
        self.visible_channels = channels;
        self
    }
```

In `core/src/dispatcher/types/mod.rs`, add re-exports:

```rust
// Dispatch & Channel Types
pub use unified::{ChannelType, DispatchMode};
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dispatcher::types::unified::tests -- --exact`
Expected: All 3 new tests PASS, all existing tests PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/types/unified.rs core/src/dispatcher/types/mod.rs
git commit -m "dispatcher: add DispatchMode and ChannelType to UnifiedTool"
```

---

### Task 2: Add list_for_channel() to ToolRegistry

**Files:**
- Modify: `core/src/dispatcher/registry/query.rs`
- Modify: `core/src/dispatcher/registry/mod.rs`
- Test: inline in `core/src/dispatcher/registry/mod.rs`

**Step 1: Write the failing test**

Add at the bottom of the `#[cfg(test)] mod tests` block in `core/src/dispatcher/registry/mod.rs`:

```rust
    #[tokio::test]
    async fn test_list_for_channel_all_visible() {
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        // All builtins have empty visible_channels = visible to all
        let panel_tools = registry.list_for_channel(ChannelType::Panel).await;
        let telegram_tools = registry.list_for_channel(ChannelType::Telegram).await;

        assert_eq!(panel_tools.len(), telegram_tools.len());
        assert!(!panel_tools.is_empty());
    }

    #[tokio::test]
    async fn test_list_for_channel_filtered() {
        let registry = ToolRegistry::new();

        // Register a tool visible only to Panel and CLI
        let tool = UnifiedTool::new(
            "custom:panel-only",
            "panel-only",
            "Panel only tool",
            ToolSource::Custom { rule_index: 0 },
        )
        .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

        registry.register_with_conflict_resolution(tool).await;

        let panel_tools = registry.list_for_channel(ChannelType::Panel).await;
        assert_eq!(panel_tools.len(), 1);

        let telegram_tools = registry.list_for_channel(ChannelType::Telegram).await;
        assert_eq!(telegram_tools.len(), 0);
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_list_for_channel -- --exact`
Expected: FAIL — `list_for_channel` method not found

**Step 3: Implement list_for_channel**

In `core/src/dispatcher/registry/query.rs`, add:

```rust
use super::super::types::ChannelType;
```

And add this method to `impl ToolQuery`:

```rust
    /// List active tools visible to a specific channel
    ///
    /// Tools with empty `visible_channels` are visible to all channels.
    /// Tools with non-empty `visible_channels` are only visible to listed channels.
    pub async fn list_for_channel(&self, channel: ChannelType) -> Vec<UnifiedTool> {
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| {
                t.is_active
                    && (t.visible_channels.is_empty()
                        || t.visible_channels.contains(&channel))
            })
            .cloned()
            .collect();

        result.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));
        result
    }
```

In `core/src/dispatcher/registry/mod.rs`, add the delegation method to `impl ToolRegistry`:

```rust
    /// List active tools visible to a specific channel
    pub async fn list_for_channel(&self, channel: ChannelType) -> Vec<UnifiedTool> {
        self.query.list_for_channel(channel).await
    }
```

Add the necessary import to `mod.rs`:

```rust
use super::types::ChannelType;
```

And in the test module imports, add `ChannelType`:

```rust
use super::super::types::ChannelType;  // add to test imports
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests -- --exact`
Expected: All new and existing tests PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/registry/query.rs core/src/dispatcher/registry/mod.rs
git commit -m "dispatcher: add list_for_channel() to ToolRegistry"
```

---

### Task 3: Add resolve_command() to ToolRegistry

**Files:**
- Modify: `core/src/dispatcher/registry/query.rs`
- Modify: `core/src/dispatcher/registry/mod.rs`
- Modify: `core/src/dispatcher/registry/types.rs` (add ResolvedCommand)
- Test: inline in `core/src/dispatcher/registry/mod.rs`

**Step 1: Write the failing test**

Add to `core/src/dispatcher/registry/mod.rs` tests:

```rust
    #[tokio::test]
    async fn test_resolve_command_found() {
        let registry = ToolRegistry::new();
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search the web".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let resolved = registry.resolve_command("/search rust async").await;
        assert!(resolved.is_some());
        let resolved = resolved.unwrap();
        assert_eq!(resolved.tool.name, "search");
        assert_eq!(resolved.arguments, Some("rust async".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_command_not_found() {
        let registry = ToolRegistry::new();
        let resolved = registry.resolve_command("/nonexistent").await;
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn test_resolve_command_not_slash() {
        let registry = ToolRegistry::new();
        let resolved = registry.resolve_command("hello world").await;
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn test_resolve_command_case_insensitive() {
        let registry = ToolRegistry::new();
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let resolved = registry.resolve_command("/SEARCH query").await;
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().tool.name, "search");
    }

    #[tokio::test]
    async fn test_resolve_command_no_args() {
        let registry = ToolRegistry::new();
        let rules = vec![RoutingRuleConfig {
            regex: "^/help".to_string(),
            provider: None,
            system_prompt: Some("Help".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let resolved = registry.resolve_command("/help").await;
        assert!(resolved.is_some());
        assert!(resolved.unwrap().arguments.is_none());
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_resolve_command -- --exact`
Expected: FAIL — `resolve_command` not found

**Step 3: Implement ResolvedCommand and resolve_command**

In `core/src/dispatcher/registry/types.rs`, add:

```rust
use super::super::types::UnifiedTool;

/// Result of resolving a user slash command
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    /// The matched tool
    pub tool: UnifiedTool,
    /// Parsed arguments (text after command name)
    pub arguments: Option<String>,
    /// Original user input
    pub raw_input: String,
}
```

In `core/src/dispatcher/registry/query.rs`, add this method to `impl ToolQuery`:

```rust
    /// Resolve a slash command input to a registered tool
    ///
    /// Parses `/command_name args` and looks up the command in the registry.
    /// Returns None if input doesn't start with `/` or command is not found.
    ///
    /// Lookup is case-insensitive on the command name.
    pub async fn resolve_command(&self, input: &str) -> Option<super::types::ResolvedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let without_slash = &trimmed[1..];
        if without_slash.is_empty() {
            return None;
        }

        // Split into command name and arguments
        let (cmd_name, arguments) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => {
                let args = rest.trim();
                (
                    name.to_lowercase(),
                    if args.is_empty() { None } else { Some(args.to_string()) },
                )
            }
            None => (without_slash.to_lowercase(), None),
        };

        // Look up by name (case-insensitive)
        let tools = self.tools.read().await;
        let tool = tools
            .values()
            .filter(|t| t.is_active && t.name.to_lowercase() == cmd_name)
            .max_by(|a, b| {
                a.source.priority().cmp(&b.source.priority())
                    .then_with(|| b.id.cmp(&a.id))
            })
            .cloned()?;

        Some(super::types::ResolvedCommand {
            tool,
            arguments,
            raw_input: input.to_string(),
        })
    }
```

In `core/src/dispatcher/registry/mod.rs`, add delegation:

```rust
    /// Resolve a slash command input to a registered tool
    pub async fn resolve_command(&self, input: &str) -> Option<types::ResolvedCommand> {
        self.query.resolve_command(input).await
    }
```

And update re-exports in `core/src/dispatcher/registry/mod.rs` at the top after `use` statements, or just use `types::ResolvedCommand` inline (it's already accessible via `types` module).

Also add `pub use types::ResolvedCommand;` in `core/src/dispatcher/registry/mod.rs` after the struct definition for external access.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_resolve_command -- --exact`
Expected: All 5 new tests PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/registry/query.rs core/src/dispatcher/registry/mod.rs core/src/dispatcher/registry/types.rs
git commit -m "dispatcher: add resolve_command() to ToolRegistry"
```

---

### Task 4: Add filter_by_prefix() to ToolRegistry

**Files:**
- Modify: `core/src/dispatcher/registry/query.rs`
- Modify: `core/src/dispatcher/registry/mod.rs`
- Test: inline in `core/src/dispatcher/registry/mod.rs`

This migrates `CommandRegistry::filter_by_prefix()` into `ToolRegistry`.

**Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn test_filter_by_prefix() {
        let registry = ToolRegistry::new();
        let rules = vec![
            RoutingRuleConfig {
                regex: "^/search".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Search".to_string()),
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: "^/settings".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Settings".to_string()),
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: "^/translate".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("Translate".to_string()),
                ..Default::default()
            },
        ];
        registry.register_custom_commands(&rules).await;

        let results = registry.filter_by_prefix("se").await;
        assert_eq!(results.len(), 2); // search, settings

        let results = registry.filter_by_prefix("SE").await; // case-insensitive
        assert_eq!(results.len(), 2);

        let results = registry.filter_by_prefix("").await;
        assert_eq!(results.len(), 3); // all

        let results = registry.filter_by_prefix("xyz").await;
        assert_eq!(results.len(), 0);
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_filter_by_prefix -- --exact`
Expected: FAIL

**Step 3: Implement filter_by_prefix**

In `core/src/dispatcher/registry/query.rs`, add to `impl ToolQuery`:

```rust
    /// Filter active tools by name prefix (case-insensitive)
    pub async fn filter_by_prefix(&self, prefix: &str) -> Vec<UnifiedTool> {
        if prefix.is_empty() {
            return self.list_all().await;
        }
        let prefix_lower = prefix.to_lowercase();
        let tools = self.tools.read().await;
        let mut result: Vec<_> = tools
            .values()
            .filter(|t| t.is_active && t.name.to_lowercase().starts_with(&prefix_lower))
            .cloned()
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }
```

In `core/src/dispatcher/registry/mod.rs`, add delegation:

```rust
    /// Filter active tools by name prefix (case-insensitive)
    pub async fn filter_by_prefix(&self, prefix: &str) -> Vec<UnifiedTool> {
        self.query.filter_by_prefix(prefix).await
    }
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_filter_by_prefix -- --exact`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/registry/query.rs core/src/dispatcher/registry/mod.rs
git commit -m "dispatcher: add filter_by_prefix() to ToolRegistry"
```

---

### Task 5: Create CommandDispatcher with DirectHandler trait

**Files:**
- Create: `core/src/command/dispatcher.rs`
- Modify: `core/src/command/mod.rs`
- Test: inline in `dispatcher.rs`

**Step 1: Write the failing test**

Create `core/src/command/dispatcher.rs` with tests first:

```rust
//! Command Dispatcher
//!
//! Executes Direct-mode commands without going through Agent Loop.

use async_trait::async_trait;
use std::collections::HashMap;

use super::types::CommandExecutionResult;

/// Handler for a direct-mode command
#[async_trait]
pub trait DirectHandler: Send + Sync {
    /// Execute the command with optional arguments
    async fn execute(&self, args: Option<&str>) -> CommandExecutionResult;
}

/// Dispatches Direct-mode commands to their handlers
pub struct CommandDispatcher {
    handlers: HashMap<String, Box<dyn DirectHandler>>,
}

impl CommandDispatcher {
    /// Create a new empty dispatcher
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a command name
    pub fn register(&mut self, name: impl Into<String>, handler: Box<dyn DirectHandler>) {
        self.handlers.insert(name.into(), handler);
    }

    /// Execute a direct command by name
    pub async fn execute(&self, command_name: &str, args: Option<&str>) -> CommandExecutionResult {
        match self.handlers.get(command_name) {
            Some(handler) => handler.execute(args).await,
            None => CommandExecutionResult::error(format!(
                "No direct handler registered for '{}'",
                command_name
            )),
        }
    }

    /// Check if a handler exists for a command
    pub fn has_handler(&self, command_name: &str) -> bool {
        self.handlers.contains_key(command_name)
    }
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHandler {
        response: String,
    }

    #[async_trait]
    impl DirectHandler for MockHandler {
        async fn execute(&self, args: Option<&str>) -> CommandExecutionResult {
            let msg = match args {
                Some(a) => format!("{}: {}", self.response, a),
                None => self.response.clone(),
            };
            CommandExecutionResult::success(msg)
        }
    }

    #[tokio::test]
    async fn test_dispatch_registered_handler() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "help",
            Box::new(MockHandler {
                response: "Help output".to_string(),
            }),
        );

        let result = dispatcher.execute("help", None).await;
        assert!(result.success);
        assert_eq!(result.message, "Help output");
    }

    #[tokio::test]
    async fn test_dispatch_with_args() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "echo",
            Box::new(MockHandler {
                response: "Echo".to_string(),
            }),
        );

        let result = dispatcher.execute("echo", Some("hello")).await;
        assert!(result.success);
        assert_eq!(result.message, "Echo: hello");
    }

    #[tokio::test]
    async fn test_dispatch_unknown_command() {
        let dispatcher = CommandDispatcher::new();
        let result = dispatcher.execute("nonexistent", None).await;
        assert!(!result.success);
        assert!(result.message.contains("No direct handler"));
    }

    #[tokio::test]
    async fn test_has_handler() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "help",
            Box::new(MockHandler {
                response: "Help".to_string(),
            }),
        );

        assert!(dispatcher.has_handler("help"));
        assert!(!dispatcher.has_handler("unknown"));
    }
}
```

**Step 2: Register module in mod.rs**

In `core/src/command/mod.rs`, add:

```rust
mod dispatcher;
pub use dispatcher::{CommandDispatcher, DirectHandler};
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib command::dispatcher::tests -- --exact`
Expected: All 4 tests PASS

**Step 4: Commit**

```bash
git add core/src/command/dispatcher.rs core/src/command/mod.rs
git commit -m "command: add CommandDispatcher with DirectHandler trait"
```

---

### Task 6: Simplify CommandParser to use ToolRegistry

**Files:**
- Modify: `core/src/command/parser.rs`
- Modify: `core/src/command/mod.rs`
- Modify: `core/src/intent/decision/execution_decider.rs`
- Modify: `core/src/intent/decision/router.rs`
- Test: inline in `parser.rs`

**Important context:**
- `CommandParser` is used in `execution_decider.rs` (L0 slash command check)
- `ExecutionDecider` calls `parser.parse()` synchronously — but `resolve_command()` is async
- The existing `CommandParser::parse()` is sync; we need to make it async or provide a sync wrapper

**Step 1: Rewrite CommandParser**

Replace the contents of `core/src/command/parser.rs` (keep `CommandContext` and `ParsedCommand` since they're used by `execution_decider.rs`):

```rust
//! Unified Slash Command Parser
//!
//! Delegates all command resolution to ToolRegistry.

use crate::dispatcher::registry::ToolRegistry;
use crate::dispatcher::types::{ToolSource, UnifiedTool};
use crate::dispatcher::ToolSourceType;
use crate::sync_primitives::Arc;

/// Parsed command result
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name (without leading /)
    pub command_name: String,
    /// Arguments after the command name
    pub arguments: Option<String>,
    /// Full original input
    pub full_input: String,
    /// Command-specific context
    pub context: CommandContext,
}

/// Command-specific context based on source type
#[derive(Debug, Clone)]
pub enum CommandContext {
    /// Builtin command context
    Builtin {
        /// Tool name for agent mode
        tool_name: String,
    },
    /// MCP tool context
    Mcp {
        /// Server name
        server_name: String,
        /// Tool name within the server
        tool_name: Option<String>,
    },
    /// Skill context
    Skill {
        /// Skill ID
        skill_id: String,
        /// Skill instructions to inject
        instructions: String,
        /// Skill name for display
        display_name: String,
        /// Allowed tools for this skill
        allowed_tools: Vec<String>,
    },
    /// Custom command context
    Custom {
        /// System prompt to inject
        system_prompt: Option<String>,
        /// Provider override
        provider: Option<String>,
        /// Rule regex pattern
        pattern: String,
    },
    /// No specific context (fallback)
    None,
}

/// Unified command parser — delegates to ToolRegistry
pub struct CommandParser {
    /// Tool registry for command resolution
    tool_registry: Arc<ToolRegistry>,
}

impl CommandParser {
    /// Create a new command parser backed by ToolRegistry
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self { tool_registry }
    }

    /// Parse user input as a slash command (async)
    ///
    /// Returns `Some(ParsedCommand)` if the input matches a registered command.
    pub async fn parse_async(&self, input: &str) -> Option<ParsedCommand> {
        let resolved = self.tool_registry.resolve_command(input).await?;

        let source_type = tool_source_to_source_type(&resolved.tool.source);
        let context = tool_to_command_context(&resolved.tool);

        Some(ParsedCommand {
            source_type,
            command_name: resolved.tool.name.clone(),
            arguments: resolved.arguments,
            full_input: resolved.raw_input,
            context,
        })
    }

    /// Synchronous parse (for backward compatibility with ExecutionDecider)
    ///
    /// Uses `tokio::runtime::Handle::current().block_on()` — only safe
    /// when called from within an async context that allows blocking.
    pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        // Use block_in_place to avoid deadlocking the async runtime
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.parse_async(trimmed))
        })
    }

    /// Get a reference to the underlying ToolRegistry
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }
}

/// Convert ToolSource to ToolSourceType
fn tool_source_to_source_type(source: &ToolSource) -> ToolSourceType {
    match source {
        ToolSource::Builtin => ToolSourceType::Builtin,
        ToolSource::Native => ToolSourceType::Native,
        ToolSource::Mcp { .. } => ToolSourceType::Mcp,
        ToolSource::Skill { .. } => ToolSourceType::Skill,
        ToolSource::Custom { .. } => ToolSourceType::Custom,
    }
}

/// Derive CommandContext from UnifiedTool fields
fn tool_to_command_context(tool: &UnifiedTool) -> CommandContext {
    match &tool.source {
        ToolSource::Builtin | ToolSource::Native => CommandContext::Builtin {
            tool_name: tool.name.clone(),
        },
        ToolSource::Mcp { server } => CommandContext::Mcp {
            server_name: server.clone(),
            tool_name: Some(tool.name.clone()),
        },
        ToolSource::Skill { id } => CommandContext::Skill {
            skill_id: id.clone(),
            instructions: tool.routing_system_prompt.clone().unwrap_or_default(),
            display_name: tool.display_name.clone(),
            allowed_tools: tool.routing_capabilities.clone(),
        },
        ToolSource::Custom { .. } => CommandContext::Custom {
            system_prompt: tool.routing_system_prompt.clone(),
            provider: None, // Provider is resolved at routing time
            pattern: tool.routing_regex.clone().unwrap_or_default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingRuleConfig;

    fn create_test_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
    }

    #[tokio::test]
    async fn test_parse_async_found() {
        let registry = create_test_registry();
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search the web".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = CommandParser::new(registry);
        let result = parser.parse_async("/search weather").await;
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "search");
        assert_eq!(cmd.arguments, Some("weather".to_string()));
        assert!(matches!(cmd.source_type, ToolSourceType::Custom));
    }

    #[tokio::test]
    async fn test_parse_async_not_found() {
        let registry = create_test_registry();
        let parser = CommandParser::new(registry);
        assert!(parser.parse_async("/unknown").await.is_none());
    }

    #[tokio::test]
    async fn test_parse_async_not_slash() {
        let registry = create_test_registry();
        let parser = CommandParser::new(registry);
        assert!(parser.parse_async("hello").await.is_none());
    }

    #[tokio::test]
    async fn test_parse_sync_compatibility() {
        let registry = create_test_registry();
        let rules = vec![RoutingRuleConfig {
            regex: "^/help".to_string(),
            provider: None,
            system_prompt: Some("Help".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = CommandParser::new(registry);
        // block_in_place works in multi-threaded tokio runtime
        let result = parser.parse("/help");
        assert!(result.is_some());
    }
}
```

**Step 2: Update mod.rs exports**

In `core/src/command/mod.rs`, update to remove `CommandRegistry` exports:

```rust
mod dispatcher;
mod parser;
mod types;

pub use dispatcher::{CommandDispatcher, DirectHandler};
pub use parser::{CommandContext, CommandParser, ParsedCommand};
pub use types::{CommandExecutionResult, CommandNode, CommandTriggers, CommandType};
```

Remove the `pub use registry::get_builtin_hint;` line and the `mod registry;` line.

**Step 3: Update ExecutionDecider and Router**

In `core/src/intent/decision/execution_decider.rs`, the `CommandParser` type hasn't changed its public API (`parse()` still returns `Option<ParsedCommand>`), so the only change needed is:
- Remove the `with_command_parser` builder's requirement for separate skill/mcp/rules setup (the new `CommandParser` takes `ToolRegistry` directly)

In `core/src/intent/decision/router.rs`, same — the `CommandParser` interface is preserved.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib command:: -- --exact`
Expected: All tests pass

Run: `cargo test -p alephcore --lib intent::decision -- --exact`
Expected: All tests pass (CommandParser API unchanged)

**Step 5: Check for compilation errors**

Run: `cargo check -p alephcore`
Expected: Clean compilation. Fix any remaining references to `CommandRegistry` or `get_builtin_hint`.

**Step 6: Commit**

```bash
git add core/src/command/parser.rs core/src/command/mod.rs
git commit -m "command: simplify CommandParser to delegate to ToolRegistry"
```

---

### Task 7: Update Gateway commands.list handler

**Files:**
- Modify: `core/src/gateway/handlers/commands.rs`
- Test: inline

**Step 1: Write the failing test**

Update the test in `commands.rs` to expect ToolRegistry-backed results. For now, we keep the handler signature simple — it will take `ToolRegistry` as parameter in the full integration (Task 8).

Replace `handle_list` and `get_builtin_commands` with a version that accepts `ToolRegistry`:

```rust
use crate::dispatcher::registry::ToolRegistry;
use crate::dispatcher::types::UnifiedTool;

/// Convert UnifiedTool to CommandInfo for JSON response
impl From<UnifiedTool> for CommandInfo {
    fn from(tool: UnifiedTool) -> Self {
        Self {
            key: tool.name,
            description: tool.description,
            icon: tool.icon.unwrap_or_else(|| "bolt".to_string()),
            hint: tool.usage,
            command_type: "action".to_string(),
            has_children: tool.has_subtools,
            source_id: Some(tool.id),
            source_type: tool.source.label().to_string(),
        }
    }
}

/// List all registered commands from ToolRegistry
pub async fn handle_list_from_registry(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools = tool_registry.list_root_commands().await;
    let command_infos: Vec<CommandInfo> = tools.into_iter().map(CommandInfo::from).collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": command_infos
        }),
    )
}
```

Keep the old `handle_list` for backward compatibility until full gateway integration.

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib gateway::handlers::commands -- --exact`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/commands.rs
git commit -m "gateway: add handle_list_from_registry using ToolRegistry"
```

---

### Task 8: Delete CommandRegistry and clean up

**Files:**
- Delete: `core/src/command/registry.rs`
- Modify: `core/src/command/mod.rs` (remove registry module)
- Modify: `core/src/lib.rs` (if needed)
- Test: `cargo check` + `cargo test`

**Step 1: Remove registry module**

Delete `core/src/command/registry.rs`.

In `core/src/command/mod.rs`, ensure `mod registry;` line is already removed (from Task 6). Remove any remaining references to `CommandRegistry` or `get_builtin_hint`.

**Step 2: Fix compilation errors**

Run: `cargo check -p alephcore`

Fix any remaining references:
- `core/src/lib.rs` — remove `CommandRegistry` from any re-exports
- `core/src/gateway/handlers/commands.rs` — remove `use crate::command::{CommandNode, CommandType}` if no longer needed by `handle_list` (keep if `handle_list` is still used during transition)

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (except pre-existing failures in `tools::markdown_skill::loader::tests`)

**Step 4: Commit**

```bash
git add -A
git commit -m "command: retire CommandRegistry, ToolRegistry is now single source of truth"
```

---

### Task 9: Add DispatchMode inference in registration

**Files:**
- Modify: `core/src/dispatcher/registry/registration.rs`
- Test: inline in `core/src/dispatcher/registry/mod.rs`

**Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn test_builtin_tools_have_agent_loop_dispatch() {
        let registry = ToolRegistry::new();
        registry.register_builtin_tools().await;

        let tools = registry.list_builtin_tools().await;
        for tool in &tools {
            assert_eq!(
                tool.dispatch_mode,
                DispatchMode::AgentLoop,
                "Builtin tool '{}' should default to AgentLoop",
                tool.name
            );
        }
    }
```

**Step 2: Run test to verify it passes**

Since `DispatchMode::default()` is `AgentLoop` and `UnifiedTool::new()` uses the default, this test should already pass after Task 1.

Run: `cargo test -p alephcore --lib dispatcher::registry::tests::test_builtin_tools_have_agent_loop_dispatch -- --exact`
Expected: PASS (the default is already correct)

**Step 3: Add visible_channels inference for high-risk tools**

In `core/src/dispatcher/registry/registration.rs`, add a helper function:

```rust
use super::super::types::{ChannelType, ToolSafetyLevel};

/// Infer default visible_channels based on safety level
fn infer_visible_channels(tool: &UnifiedTool) -> Vec<ChannelType> {
    match tool.safety_level {
        ToolSafetyLevel::IrreversibleHighRisk => {
            // Dangerous ops only via Panel and CLI
            vec![ChannelType::Panel, ChannelType::Cli]
        }
        _ if tool.requires_confirmation => {
            // Tools requiring confirmation excluded from iMessage (no confirmation UI)
            vec![
                ChannelType::Panel,
                ChannelType::Telegram,
                ChannelType::Discord,
                ChannelType::Cli,
            ]
        }
        _ => Vec::new(), // All channels
    }
}
```

Apply it in `register_mcp_tools` and `register_skills` after creating the tool, before calling `register_with_conflict_resolution`:

```rust
let visible = infer_visible_channels(&tool);
let tool = if !visible.is_empty() {
    tool.with_visible_channels(visible)
} else {
    tool
};
```

**Step 4: Write test for inference**

```rust
    #[tokio::test]
    async fn test_high_risk_tool_channel_restriction() {
        let registry = ToolRegistry::new();

        let tool = UnifiedTool::new(
            "mcp:server:delete_all",
            "delete_all",
            "Delete everything",
            ToolSource::Mcp { server: "server".into() },
        )
        .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

        registry.register_with_conflict_resolution(tool).await;

        // High-risk tool should not be visible on Telegram
        let telegram = registry.list_for_channel(ChannelType::Telegram).await;
        assert!(
            !telegram.iter().any(|t| t.name == "delete_all"),
            "High-risk tool should not be visible on Telegram"
        );

        // But should be visible on Panel
        let panel = registry.list_for_channel(ChannelType::Panel).await;
        assert!(
            panel.iter().any(|t| t.name == "delete_all"),
            "High-risk tool should be visible on Panel"
        );
    }
```

Note: The inference in `register_mcp_tools` needs to be applied before `register_with_conflict_resolution`. For tools registered via `register_with_conflict_resolution` directly (like in the test above), the caller is responsible for setting `visible_channels`. The test above verifies that `list_for_channel` filtering works correctly regardless of how `visible_channels` was set.

Update the test to set `visible_channels` explicitly since it bypasses `register_mcp_tools`:

```rust
    #[tokio::test]
    async fn test_high_risk_tool_channel_restriction() {
        let registry = ToolRegistry::new();

        let tool = UnifiedTool::new(
            "mcp:server:delete_all",
            "delete_all",
            "Delete everything",
            ToolSource::Mcp { server: "server".into() },
        )
        .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk)
        .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

        registry.register_with_conflict_resolution(tool).await;

        let telegram = registry.list_for_channel(ChannelType::Telegram).await;
        assert!(!telegram.iter().any(|t| t.name == "delete_all"));

        let panel = registry.list_for_channel(ChannelType::Panel).await;
        assert!(panel.iter().any(|t| t.name == "delete_all"));
    }
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib dispatcher::registry::tests -- --exact`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/dispatcher/registry/registration.rs core/src/dispatcher/registry/mod.rs
git commit -m "dispatcher: add visible_channels inference for high-risk tools"
```

---

### Task 10: Final integration verification

**Files:** None modified — verification only

**Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass (except pre-existing `markdown_skill::loader` failures).

**Step 2: Run cargo check for warnings**

```bash
cargo check -p alephcore 2>&1 | grep -i "warning\|error"
```

Fix any unused imports or dead code warnings introduced by our changes.

**Step 3: Verify no breaking changes in public API**

Check that `lib.rs` re-exports are consistent:

```bash
grep -n "pub use.*command\|pub use.*dispatcher" core/src/lib.rs
```

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix warnings and finalize unified command registry"
```

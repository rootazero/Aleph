# Tool Permission System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two-layer tool execution permission system (Global + Agent) with Allow/Ask/Deny levels, configurable via Panel UI.

**Architecture:** New `ToolPermissionsConfig` struct feeds into `SafetyGuard` via `from_permissions()` constructor. Global config in `PoliciesConfig`, agent config in `AgentDefinition`. RPC handlers expose get/update. Panel ToolsTab becomes three-state, PoliciesView gets tool permissions section.

**Tech Stack:** Rust (alephcore), Leptos (Panel WASM), JSON-RPC

**Spec:** `docs/superpowers/specs/2026-03-17-tool-permission-system-design.md`

---

## Chunk 1: Config & SafetyGuard (Backend Core)

### Task 1: Create ToolPermissionsConfig struct

**Files:**
- Create: `src/config/types/policies/tool_permissions.rs`
- Modify: `src/config/types/policies/mod.rs`

- [ ] **Step 1: Create tool_permissions.rs with ToolPermissionsConfig**

```rust
// src/config/types/policies/tool_permissions.rs
//! Tool execution permission configuration.
//!
//! Defines per-tool Allow/Ask/Deny permissions used by both
//! global policies and per-agent overrides.

use crate::extension::PermissionAction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool execution permission configuration.
///
/// Used at two levels:
/// - Global (`[policies.tool_permissions]`): ceiling for all agents
/// - Agent (`[[agents.list]] tool_permissions`): per-agent overrides
///
/// Merge logic: `effective(tool) = min(global, agent)` where Allow > Ask > Deny.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolPermissionsConfig {
    /// Default permission for tools not listed in overrides.
    #[serde(default = "default_allow")]
    pub default: PermissionAction,

    /// Per-tool permission overrides. Keys are exact `UnifiedTool.name` matches.
    #[serde(default)]
    pub overrides: HashMap<String, PermissionAction>,
}

impl Default for ToolPermissionsConfig {
    fn default() -> Self {
        Self {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        }
    }
}

fn default_allow() -> PermissionAction {
    PermissionAction::Allow
}

impl ToolPermissionsConfig {
    /// Resolve the permission for a specific tool.
    pub fn resolve(&self, tool_name: &str) -> PermissionAction {
        self.overrides
            .get(tool_name)
            .copied()
            .unwrap_or(self.default)
    }

    /// Merge global and agent configs: effective = min(global, agent).
    ///
    /// `min` means the more restrictive level wins.
    /// Allow (2) > Ask (1) > Deny (0).
    pub fn merge(global: &Self, agent: &Self) -> Self {
        let default = min_permission(global.default, agent.default);

        let mut overrides = HashMap::new();
        // Collect all keys from both
        let all_keys: std::collections::HashSet<&String> = global
            .overrides
            .keys()
            .chain(agent.overrides.keys())
            .collect();

        for key in all_keys {
            let g = global.resolve(key);
            let a = agent.resolve(key);
            let effective = min_permission(g, a);
            // Only store if different from merged default
            if effective != default {
                overrides.insert(key.clone(), effective);
            }
        }

        Self { default, overrides }
    }
}

/// Return the more restrictive of two permission actions.
/// Allow > Ask > Deny, so min = most restrictive.
fn min_permission(a: PermissionAction, b: PermissionAction) -> PermissionAction {
    use PermissionAction::*;
    match (a, b) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        _ => Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PermissionAction::*;

    #[test]
    fn test_default_is_all_allow() {
        let config = ToolPermissionsConfig::default();
        assert_eq!(config.default, Allow);
        assert!(config.overrides.is_empty());
        assert_eq!(config.resolve("shell"), Allow);
    }

    #[test]
    fn test_resolve_override() {
        let mut config = ToolPermissionsConfig::default();
        config.overrides.insert("shell".into(), Deny);
        assert_eq!(config.resolve("shell"), Deny);
        assert_eq!(config.resolve("file_read"), Allow);
    }

    #[test]
    fn test_min_permission() {
        assert_eq!(min_permission(Allow, Allow), Allow);
        assert_eq!(min_permission(Allow, Ask), Ask);
        assert_eq!(min_permission(Allow, Deny), Deny);
        assert_eq!(min_permission(Ask, Allow), Ask);
        assert_eq!(min_permission(Ask, Ask), Ask);
        assert_eq!(min_permission(Ask, Deny), Deny);
        assert_eq!(min_permission(Deny, Allow), Deny);
        assert_eq!(min_permission(Deny, Ask), Deny);
        assert_eq!(min_permission(Deny, Deny), Deny);
    }

    #[test]
    fn test_merge_global_deny_wins() {
        let mut global = ToolPermissionsConfig::default();
        global.overrides.insert("shell".into(), Deny);

        let mut agent = ToolPermissionsConfig::default();
        agent.overrides.insert("shell".into(), Allow);

        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("shell"), Deny);
    }

    #[test]
    fn test_merge_agent_deny_wins_over_global_allow() {
        let global = ToolPermissionsConfig::default(); // all Allow

        let mut agent = ToolPermissionsConfig::default();
        agent.overrides.insert("file_write".into(), Deny);

        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("file_write"), Deny);
        assert_eq!(merged.resolve("shell"), Allow);
    }

    #[test]
    fn test_merge_defaults() {
        let mut global = ToolPermissionsConfig::default();
        global.default = Ask;

        let agent = ToolPermissionsConfig::default(); // Allow

        let merged = ToolPermissionsConfig::merge(&global, &agent);
        // min(Ask, Allow) = Ask
        assert_eq!(merged.default, Ask);
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
            default = "allow"
            [overrides]
            shell = "deny"
            file_delete = "ask"
        "#;
        let config: ToolPermissionsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default, Allow);
        assert_eq!(config.resolve("shell"), Deny);
        assert_eq!(config.resolve("file_delete"), Ask);
        assert_eq!(config.resolve("file_read"), Allow);
    }

    #[test]
    fn test_empty_toml_uses_defaults() {
        let config: ToolPermissionsConfig = toml::from_str("").unwrap();
        assert_eq!(config.default, Allow);
        assert!(config.overrides.is_empty());
    }
}
```

- [ ] **Step 2: Wire into policies/mod.rs**

In `src/config/types/policies/mod.rs`:
- Add `pub mod tool_permissions;` after the existing `pub mod tool_safety;` line
- Add `pub use tool_permissions::ToolPermissionsConfig;` after the existing `pub use tool_safety::ToolSafetyPolicy;` line
- Add field to `PoliciesConfig` struct:

```rust
    /// Tool execution permissions (Allow/Ask/Deny per tool)
    #[serde(default)]
    pub tool_permissions: ToolPermissionsConfig,
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib tool_permissions`
Expected: All tests pass

- [ ] **Step 4: Commit**

```
git add src/config/types/policies/tool_permissions.rs src/config/types/policies/mod.rs
git commit -m "config: add ToolPermissionsConfig with merge logic"
```

---

### Task 2: Add PermissionAction traits needed by SafetyGuard

**Files:**
- Modify: `src/extension/types/agents.rs`

`PermissionAction` needs `Copy`, `PartialEq`, `Eq`, and `JsonSchema` for use in `ToolPermissionsConfig` and `SafetyGuard`. Check if it already has them; add what's missing.

- [ ] **Step 1: Check current derives on PermissionAction**

Read `src/extension/types/agents.rs:42-46`. Currently:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}
```

If `JsonSchema` is missing, add it. If `Copy`/`PartialEq`/`Eq` are missing, add them.

- [ ] **Step 2: Add JsonSchema derive if missing**

Add `JsonSchema` to the derive list and `use schemars::JsonSchema;` if not already imported.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles with no new errors

- [ ] **Step 4: Commit (if changes were needed)**

```
git add src/extension/types/agents.rs
git commit -m "types: add JsonSchema derive to PermissionAction"
```

---

### Task 3: Add tool_permissions to AgentDefinition

**Files:**
- Modify: `src/config/types/agents_def.rs`

- [ ] **Step 1: Add field to AgentDefinition**

After the `allowed_links` field (around line 244), add:

```rust
    /// Per-agent tool execution permissions (Allow/Ask/Deny).
    /// None = inherit global permissions entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles (existing Default derive handles Option::None)

- [ ] **Step 3: Commit**

```
git add src/config/types/agents_def.rs
git commit -m "config: add tool_permissions field to AgentDefinition"
```

---

### Task 4: Refactor SafetyGuard to use ToolPermissionsConfig

**Files:**
- Modify: `src/agent_loop/safety.rs`

- [ ] **Step 1: Add PolicyDenied variant and update struct**

Replace the entire `safety.rs` file content. Key changes:
- Add `PolicyDenied { tool: String }` to `SafetyError`
- Change struct to hold `tool_permissions: HashMap<String, PermissionAction>` and `default_permission: PermissionAction`
- Add `from_permissions()` constructor
- Keep `default_guard()` for backward compat
- Keep `new()` for direct construction in tests
- Update `check()` logic

```rust
//! Single-layer safety guard for tool calls.
//!
//! Two-check approach:
//! 1. Pattern matching against blocked commands (hard block, highest priority)
//! 2. Permission lookup from merged global + agent config (Allow/Ask/Deny)

use crate::config::types::policies::ToolPermissionsConfig;
use crate::extension::PermissionAction;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

// =============================================================================
// ToolCall
// =============================================================================

/// A tool invocation to be safety-checked before execution.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
}

// =============================================================================
// SafetyError
// =============================================================================

/// Safety check outcome when a tool call is not unconditionally allowed.
#[derive(Debug)]
pub enum SafetyError {
    /// The tool call matched a blocked pattern and must not execute.
    Blocked { tool: String, pattern: String },
    /// The tool requires user confirmation (Ask level, currently = Deny).
    NeedsConfirmation { tool: String },
    /// The tool is denied by permission policy.
    PolicyDenied { tool: String },
}

impl fmt::Display for SafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SafetyError::Blocked { tool, pattern } => {
                write!(f, "tool '{}' blocked by pattern '{}'", tool, pattern)
            }
            SafetyError::NeedsConfirmation { tool } => {
                write!(f, "tool '{}' requires user confirmation", tool)
            }
            SafetyError::PolicyDenied { tool } => {
                write!(f, "tool '{}' denied by permission policy", tool)
            }
        }
    }
}

impl std::error::Error for SafetyError {}

// =============================================================================
// SafetyGuard
// =============================================================================

/// Safety guard using blocked patterns + permission-based tool access control.
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    tool_permissions: HashMap<String, PermissionAction>,
    default_permission: PermissionAction,
}

impl SafetyGuard {
    /// Create from raw blocked patterns and per-tool permissions.
    pub fn new(
        blocked: Vec<String>,
        tool_permissions: HashMap<String, PermissionAction>,
        default_permission: PermissionAction,
    ) -> Self {
        let blocked_patterns = blocked
            .into_iter()
            .filter_map(|p| Regex::new(&p).ok())
            .collect();
        Self {
            blocked_patterns,
            tool_permissions,
            default_permission,
        }
    }

    /// Build from merged global + agent permission configs.
    ///
    /// Retains default blocked patterns (rm -rf /, DROP DATABASE, etc.)
    /// which are ALWAYS enforced regardless of permission settings.
    pub fn from_permissions(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> Self {
        let merged = ToolPermissionsConfig::merge(global, agent);
        Self::new(
            default_blocked_patterns(),
            merged.overrides,
            merged.default,
        )
    }

    /// Create a guard with default blocked patterns and all tools allowed.
    ///
    /// Used as fallback when no permission config is available.
    pub fn default_guard() -> Self {
        Self::new(
            default_blocked_patterns(),
            HashMap::new(),
            PermissionAction::Allow,
        )
    }

    /// Check whether a tool call is safe to execute.
    ///
    /// Priority:
    /// 1. Blocked patterns (hard block, highest priority)
    /// 2. Permission lookup (Allow → Ok, Ask → NeedsConfirmation, Deny → PolicyDenied)
    pub fn check(&self, call: &ToolCall) -> Result<(), SafetyError> {
        // 1. Blocked patterns take highest priority
        let input_json = call.input.to_string();
        let haystack = format!("{} {}", call.name, input_json);

        for pattern in &self.blocked_patterns {
            if pattern.is_match(&haystack) {
                return Err(SafetyError::Blocked {
                    tool: call.name.clone(),
                    pattern: pattern.to_string(),
                });
            }
        }

        // 2. Permission lookup
        let permission = self
            .tool_permissions
            .get(&call.name)
            .copied()
            .unwrap_or(self.default_permission);

        match permission {
            PermissionAction::Allow => Ok(()),
            PermissionAction::Ask => Err(SafetyError::NeedsConfirmation {
                tool: call.name.clone(),
            }),
            PermissionAction::Deny => Err(SafetyError::PolicyDenied {
                tool: call.name.clone(),
            }),
        }
    }
}

/// Default blocked patterns for truly dangerous commands.
fn default_blocked_patterns() -> Vec<String> {
    vec![
        r"rm\s+-rf\s+/".to_string(),
        r"(?i)drop\s+database".to_string(),
        r"mkfs\.".to_string(),
        r"dd\s+if=.*of=/dev/".to_string(),
        r">\s*/dev/sd".to_string(),
    ]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_blocked_pattern() {
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            HashMap::new(),
            PermissionAction::Allow,
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::Blocked { .. }));
    }

    #[test]
    fn test_allowed_tool() {
        let guard = SafetyGuard::default_guard();
        let call = ToolCall {
            name: "read_file".to_string(),
            input: json!({ "path": "/tmp/foo.txt" }),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_policy_denied() {
        let mut perms = HashMap::new();
        perms.insert("shell".to_string(), PermissionAction::Deny);
        let guard = SafetyGuard::new(vec![], perms, PermissionAction::Allow);

        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "echo hello" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::PolicyDenied { .. }));
        assert!(err.to_string().contains("denied by permission policy"));
    }

    #[test]
    fn test_needs_confirmation() {
        let mut perms = HashMap::new();
        perms.insert("shell".to_string(), PermissionAction::Ask);
        let guard = SafetyGuard::new(vec![], perms, PermissionAction::Allow);

        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "echo hello" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
    }

    #[test]
    fn test_blocked_takes_priority_over_permission() {
        let mut perms = HashMap::new();
        perms.insert("shell".to_string(), PermissionAction::Allow);
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            perms,
            PermissionAction::Allow,
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::Blocked { .. }));
    }

    #[test]
    fn test_default_guard_all_allow() {
        let guard = SafetyGuard::default_guard();

        // Dangerous commands still blocked
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::Blocked { .. })
        ));

        // Normal tools allowed
        for name in &["shell", "file_write", "file_delete"] {
            let call = ToolCall {
                name: name.to_string(),
                input: json!({ "safe": true }),
            };
            assert!(guard.check(&call).is_ok(), "expected Ok for {}", name);
        }
    }

    #[test]
    fn test_from_permissions_merge() {
        let mut global = ToolPermissionsConfig::default();
        global.overrides.insert("shell".to_string(), PermissionAction::Deny);

        let mut agent = ToolPermissionsConfig::default();
        agent.overrides.insert("shell".to_string(), PermissionAction::Allow);

        let guard = SafetyGuard::from_permissions(&global, &agent);

        // Global Deny wins over agent Allow
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "safe": true }),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::PolicyDenied { .. })
        ));
    }

    #[test]
    fn test_default_permission_deny() {
        let guard = SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Deny);
        let call = ToolCall {
            name: "anything".to_string(),
            input: json!({}),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::PolicyDenied { .. })
        ));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::safety`
Expected: All tests pass

- [ ] **Step 3: Commit**

```
git add src/agent_loop/safety.rs
git commit -m "safety: refactor SafetyGuard to use ToolPermissionsConfig"
```

---

### Task 5: Handle PolicyDenied in agent loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Add PolicyDenied arm to the safety check match**

In `loop_core.rs`, find the match on `self.safety_guard.check(&safety_call)` (around line 226). Add a new arm after `NeedsConfirmation`:

```rust
                    Err(SafetyError::PolicyDenied { ref tool }) => {
                        let err = SafetyError::PolicyDenied {
                            tool: tool.clone(),
                        };
                        callback.on_safety_block(&err);
                        messages.push(UnifiedMessage::tool_result(
                            tc.id.clone(),
                            tc.name.clone(),
                            format!(
                                "DENIED: tool '{}' is not allowed by permission policy",
                                tool
                            ),
                            true,
                        ));
                        // Do NOT increment consecutive_errors — policy denials are not tool errors
                    }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

- [ ] **Step 3: Commit**

```
git add src/agent_loop/loop_core.rs
git commit -m "agent_loop: handle PolicyDenied in think-act loop"
```

---

### Task 6: Wire SafetyGuard::from_permissions into ExecutionEngine

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`
- Modify: `src/gateway/execution_engine/engine.rs`
- Modify: `src/gateway/agent_instance.rs`

- [ ] **Step 1: Add tool_permissions() to AgentInstance**

In `src/gateway/agent_instance.rs`, add a method to `AgentInstance` that returns the agent's `ToolPermissionsConfig`. The agent instance already has access to agent config. We need to pass the agent's definition's tool_permissions through `AgentInstanceConfig`.

Add to `AgentInstanceConfig`:

```rust
    /// Per-agent tool execution permissions
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
```

Add method to `AgentInstance`:

```rust
    /// Get the agent's tool permissions config (or default if unset).
    pub fn tool_permissions(&self) -> crate::config::types::policies::ToolPermissionsConfig {
        self.config
            .tool_permissions
            .clone()
            .unwrap_or_default()
    }
```

- [ ] **Step 2: Add global_tool_permissions to ExecutionEngine**

In `src/gateway/execution_engine/engine.rs`, add field to `ExecutionEngine`:

```rust
    /// Global tool permissions from policies config
    pub(super) global_tool_permissions: crate::config::types::policies::ToolPermissionsConfig,
```

Add builder method:

```rust
    pub fn with_global_tool_permissions(
        mut self,
        perms: crate::config::types::policies::ToolPermissionsConfig,
    ) -> Self {
        self.global_tool_permissions = perms;
        self
    }
```

Initialize in `new()` as `global_tool_permissions: Default::default()`.

- [ ] **Step 3: Update run_loop.rs to use from_permissions**

In `src/gateway/execution_engine/run_loop.rs`, change line 78:

```rust
// Before:
let safety = SafetyGuard::default_guard();

// After:
let agent_perms = agent.tool_permissions();
let safety = SafetyGuard::from_permissions(
    &self.global_tool_permissions,
    &agent_perms,
);
```

- [ ] **Step 4: Wire global_tool_permissions at server startup**

Find where `ExecutionEngine` is constructed in the server startup code (likely in `src/bin/aleph/commands/start/`). Pass `config.policies.tool_permissions.clone()` via the builder.

Search for `ExecutionEngine::new` in `src/bin/aleph/commands/start/` to find the exact location.

- [ ] **Step 5: Wire tool_permissions into AgentInstanceConfig at agent creation**

Find where `AgentInstanceConfig` is created from `AgentDefinition` (search for `AgentInstanceConfig` construction). Pass `definition.tool_permissions.clone()`.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

- [ ] **Step 7: Commit**

```
git add src/gateway/execution_engine/run_loop.rs src/gateway/execution_engine/engine.rs src/gateway/agent_instance.rs
git commit -m "gateway: wire SafetyGuard::from_permissions into agent loop"
```

---

## Chunk 2: RPC Handlers

### Task 7: Global tool permissions RPC handlers

**Files:**
- Modify: `src/gateway/handlers/config.rs`
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs`

- [ ] **Step 1: Add handler functions**

Add to `src/gateway/handlers/config.rs`:

```rust
/// Handle config.get_tool_permissions
pub async fn handle_get_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling config.get_tool_permissions");
    let cfg = config.read().await;
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&cfg.policies.tool_permissions).unwrap_or_default(),
    )
}

/// Handle config.update_tool_permissions
pub async fn handle_update_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling config.update_tool_permissions");

    #[derive(Deserialize)]
    struct Params {
        default: Option<crate::extension::PermissionAction>,
        overrides: Option<std::collections::HashMap<String, crate::extension::PermissionAction>>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    {
        let mut cfg = config.write().await;
        if let Some(default) = params.default {
            cfg.policies.tool_permissions.default = default;
        }
        if let Some(overrides) = params.overrides {
            cfg.policies.tool_permissions.overrides = overrides;
        }
        if let Err(e) = cfg.save_incremental(&["policies"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save: {}", e),
            );
        }
    }

    let _ = event_bus.publish(GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: "policies.tool_permissions".to_string(),
    })).await;

    info!("Tool permissions updated via RPC");
    let cfg = config.read().await;
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(&cfg.policies.tool_permissions).unwrap_or_default(),
    )
}
```

- [ ] **Step 2: Register handlers**

In `src/bin/aleph/commands/start/builder/handlers.rs`, find the config handler section and add:

```rust
    register_handler!(server, "config.get_tool_permissions", config_handlers::handle_get_tool_permissions, config);
    register_handler!(server, "config.update_tool_permissions", config_handlers::handle_update_tool_permissions, config, event_bus);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

- [ ] **Step 4: Commit**

```
git add src/gateway/handlers/config.rs src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "handlers: add config.get/update_tool_permissions RPC"
```

---

### Task 8: Agent tool permissions RPC handlers

**Files:**
- Modify: `src/gateway/handlers/agent_config.rs`
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs`

- [ ] **Step 1: Add handler functions**

Add to `src/gateway/handlers/agent_config.rs`:

```rust
/// Handle agent_config.get_tool_permissions
pub async fn handle_get_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.get_tool_permissions");

    #[derive(Deserialize)]
    struct Params {
        agent_id: String,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let cfg = config.read().await;

    // Find agent definition
    let agent_def = cfg.agents.list.iter().find(|a| a.id == params.agent_id);
    let agent_perms = agent_def
        .and_then(|a| a.tool_permissions.clone())
        .unwrap_or_default();

    let global_perms = &cfg.policies.tool_permissions;
    let merged = crate::config::types::policies::ToolPermissionsConfig::merge(global_perms, &agent_perms);

    // Build effective map: keys from agent.overrides ∪ global.overrides
    let effective: std::collections::HashMap<String, crate::extension::PermissionAction> =
        merged.overrides.clone();

    JsonRpcResponse::success(
        request.id,
        json!({
            "default": agent_perms.default,
            "overrides": agent_perms.overrides,
            "effective_default": merged.default,
            "effective": effective,
            "global_overrides": global_perms.overrides,
        }),
    )
}

/// Handle agent_config.update_tool_permissions
pub async fn handle_update_tool_permissions(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<super::super::event_bus::GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agent_config.update_tool_permissions");

    #[derive(Deserialize)]
    struct Params {
        agent_id: String,
        default: Option<crate::extension::PermissionAction>,
        overrides: Option<std::collections::HashMap<String, crate::extension::PermissionAction>>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    {
        let mut cfg = config.write().await;
        let agent = cfg.agents.list.iter_mut().find(|a| a.id == params.agent_id);
        let Some(agent) = agent else {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Agent '{}' not found", params.agent_id),
            );
        };

        let perms = agent.tool_permissions.get_or_insert_with(Default::default);
        if let Some(default) = params.default {
            perms.default = default;
        }
        if let Some(overrides) = params.overrides {
            perms.overrides = overrides;
        }

        if let Err(e) = cfg.save_incremental(&["agents"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save: {}", e),
            );
        }
    }

    let _ = event_bus.publish(super::super::event_bus::GatewayEvent::ConfigChanged(
        super::super::event_bus::ConfigChangedEvent {
            section: format!("agents.{}.tool_permissions", params.agent_id),
        },
    )).await;

    info!("Agent '{}' tool permissions updated via RPC", params.agent_id);
    // Return updated state by calling get
    handle_get_tool_permissions(
        JsonRpcRequest::with_id(
            "agent_config.get_tool_permissions",
            Some(json!({ "agent_id": params.agent_id })),
            request.id,
        ),
        config,
    ).await
}
```

- [ ] **Step 2: Register handlers**

In `handlers.rs` builder, add:

```rust
    register_handler!(server, "agent_config.get_tool_permissions", agent_config::handle_get_tool_permissions, config);
    register_handler!(server, "agent_config.update_tool_permissions", agent_config::handle_update_tool_permissions, config, event_bus);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles

- [ ] **Step 4: Commit**

```
git add src/gateway/handlers/agent_config.rs src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "handlers: add agent_config.get/update_tool_permissions RPC"
```

---

## Chunk 3: Panel UI

### Task 9: Panel API for tool permissions

**Files:**
- Modify: `apps/panel/src/api/agent.rs` (or create `apps/panel/src/api/tool_permissions.rs`)

- [ ] **Step 1: Add API types and methods**

Add to the panel API module:

```rust
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionsResponse {
    pub default: String,
    pub overrides: HashMap<String, String>,
    pub effective_default: Option<String>,
    pub effective: Option<HashMap<String, String>>,
    pub global_overrides: Option<HashMap<String, String>>,
}

pub struct ToolPermissionsApi;

impl ToolPermissionsApi {
    /// Get global tool permissions
    pub async fn get_global(state: &DashboardState) -> Result<ToolPermissionsResponse, String> {
        let result = state.rpc_call("config.get_tool_permissions", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Update global tool permissions
    pub async fn update_global(state: &DashboardState, default: &str, overrides: HashMap<String, String>) -> Result<(), String> {
        let params = serde_json::json!({ "default": default, "overrides": overrides });
        state.rpc_call("config.update_tool_permissions", params).await?;
        Ok(())
    }

    /// Get agent tool permissions (includes effective + global_overrides)
    pub async fn get_agent(state: &DashboardState, agent_id: &str) -> Result<ToolPermissionsResponse, String> {
        let params = serde_json::json!({ "agent_id": agent_id });
        let result = state.rpc_call("agent_config.get_tool_permissions", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Update agent tool permissions
    pub async fn update_agent(state: &DashboardState, agent_id: &str, default: &str, overrides: HashMap<String, String>) -> Result<(), String> {
        let params = serde_json::json!({ "agent_id": agent_id, "default": default, "overrides": overrides });
        state.rpc_call("agent_config.update_tool_permissions", params).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Commit**

```
git add apps/panel/src/api/
git commit -m "panel: add ToolPermissionsApi for RPC calls"
```

---

### Task 10: Redesign ToolsTab to three-state permissions

**Files:**
- Modify: `apps/panel/src/views/agents/tools.rs`

- [ ] **Step 1: Replace toggle with three-state segmented control**

Redesign the ToolsTab to show Allow/Ask/Deny for each tool:
- Load tool permissions via `ToolPermissionsApi::get_agent()`
- Load tool schema via `AgentsApi::tools_schema()` (existing, for group structure)
- Each tool shows a segmented control with Allow (green) / Ask (yellow) / Deny (red)
- Tools whose global permission is Deny: greyed out row, locked at Deny, tooltip
- Tools whose global permission is Ask: cannot select Allow
- Group header: batch set to Allow or Deny for all non-greyed tools
- Save sends only overrides (tools different from default)

This is a large UI change. The existing ToolsTab structure should be preserved (group/tool hierarchy), but the toggle mechanism changes from boolean to three-state.

Key Leptos patterns to follow from existing code:
- `RwSignal` for reactive state
- `spawn_local` for async RPC calls
- Segmented control: three `<button>` elements with conditional classes

- [ ] **Step 2: Test manually**

Build WASM: `just build` or the WASM build command.
Open Panel, navigate to an agent's Tools tab, verify three-state controls render.

- [ ] **Step 3: Commit**

```
git add apps/panel/src/views/agents/tools.rs
git commit -m "panel: redesign ToolsTab with Allow/Ask/Deny three-state permissions"
```

---

### Task 11: Add tool permissions to PoliciesView

**Files:**
- Modify: `apps/panel/src/views/settings/policies.rs`

- [ ] **Step 1: Add Tool Permissions section**

Add a "Tool Permissions" section above the existing "Content Safety" section:
- Load global tool permissions via `ToolPermissionsApi::get_global()`
- Load tool schema via `AgentsApi::tools_schema()` for group structure
- Default dropdown at top (Allow/Ask/Deny)
- Same group/tool layout as ToolsTab but no grey restrictions
- Save sends `config.update_tool_permissions` RPC
- Add hint text: "Changes will take effect on next agent run"

- [ ] **Step 2: Test manually**

Build WASM, open Panel → Settings → Policies, verify Tool Permissions section renders and saves.

- [ ] **Step 3: Commit**

```
git add apps/panel/src/views/settings/policies.rs
git commit -m "panel: add Tool Permissions section to PoliciesView"
```

---

## Chunk 4: Integration Test & Cleanup

### Task 12: End-to-end verification

- [ ] **Step 1: Start server and verify default behavior**

Run: `cargo run --bin aleph`
Send a message via Telegram that requires shell/file_write.
Expected: Tools execute without `NEEDS_CONFIRMATION` (default all Allow).

- [ ] **Step 2: Set a global Deny via Panel**

In Panel → Policies → Tool Permissions, set `shell` to Deny.
Send a message requiring shell.
Expected: Agent receives `DENIED: tool 'shell' is not allowed by permission policy`.

- [ ] **Step 3: Set agent-level override**

In Panel → Agents → [agent] → Tools, set `file_write` to Deny.
Expected: file_write calls are denied for that agent but not others.

- [ ] **Step 4: Verify global ceiling**

Set global `shell` = Deny. Try to set agent `shell` = Allow in Panel.
Expected: UI shows shell greyed out, cannot change.

- [ ] **Step 5: Commit safety.rs fix + rerank fix together**

Stage the earlier `safety.rs` default_guard change and `rerank_config.rs` VAULT_KEY fix:

```
git add src/agent_loop/safety.rs src/gateway/handlers/rerank_config.rs
git commit -m "fix: default SafetyGuard to all-allow, fix rerank VAULT_KEY test"
```

(Note: these were already committed earlier in the session — skip if already committed.)

- [ ] **Step 6: Final commit**

```
git commit -m "feat: tool permission system — two-layer Allow/Ask/Deny with Panel UI"
```

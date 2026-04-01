# Tool Permission System Design

## Summary

Two-layer tool execution permission system: **Global** (Policies) sets the ceiling, **Agent-level** sets per-agent restrictions within that ceiling. Default: all tools allowed. Users configure via Panel UI.

## Requirements

- Three permission levels per tool: **Allow** (execute freely), **Ask** (needs confirmation, currently = Deny), **Deny** (blocked)
- Global permissions in Panel Policies view — applies to all agents
- Per-agent permissions in Panel Agents ToolsTab — cannot exceed global level
- Default: all Allow
- Granularity: per individual tool, with group toggle for batch operations
- Separate from tool visibility (existing skills whitelist/blacklist)

## Data Model

### PermissionAction

Reuse existing `PermissionAction` enum (`src/extension/types/agents.rs`): `Allow` / `Ask` / `Deny`. Serializes to `"allow"` / `"ask"` / `"deny"`.

### ToolPermissionsConfig

New config struct, defined in `src/config/types/policies/tool_permissions.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolPermissionsConfig {
    #[serde(default = "default_allow")]
    pub default: PermissionAction,
    #[serde(default)]
    pub overrides: HashMap<String, PermissionAction>,
}
```

Override keys are exact `UnifiedTool.name` matches (e.g., `shell`, `file_write`, `mcp__server_name__tool_name` for MCP tools).

### Global Config — `config.toml`

```toml
[policies.tool_permissions]
default = "allow"

[policies.tool_permissions.overrides]
# shell = "deny"
# file_delete = "ask"
```

Add `tool_permissions` field to `PoliciesConfig` (`src/config/types/policies/mod.rs`):

```rust
pub struct PoliciesConfig {
    // ... existing fields ...
    #[serde(default)]
    pub tool_permissions: ToolPermissionsConfig,
}
```

### Agent Config — agent definition

New optional field on `AgentDefinition` (`src/config/types/agents_def.rs`):

```rust
pub struct AgentDefinition {
    // ... existing fields ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_permissions: Option<ToolPermissionsConfig>,
}
```

TOML path (within `[[agents.list]]`):

```toml
[[agents.list]]
id = "coder"

[agents.list.tool_permissions]
default = "allow"

[agents.list.tool_permissions.overrides]
# shell = "deny"
```

When `None`, inherits global permissions entirely.

### Merge Logic

```
effective(tool) = min(global(tool), agent(tool))
```

Where Allow > Ask > Deny, `min` takes the more restrictive:
- Global Deny → always Deny regardless of agent setting
- Global Allow + Agent Deny → Deny
- Global Allow + Agent Allow → Allow
- Global Ask + Agent Allow → Ask

## Relationship to Existing ToolSafetyPolicy

`ToolSafetyPolicy` (`src/config/types/policies/tool_safety.rs`) infers safety levels from tool name keywords but is **never used by SafetyGuard** — `SafetyGuard::default_guard()` is hardcoded. `ToolPermissionsConfig` replaces `ToolSafetyPolicy` as the authoritative permission mechanism:

- `ToolSafetyPolicy` remains for backward compatibility but is no longer on the execution path
- `ToolPermissionsConfig` is the single source of truth for tool execution permissions
- No interaction between the two — `ToolSafetyPolicy` may be deprecated in a future release

## Relationship to PermissionManager

`PermissionManager` (`src/permission/`) has a complete rule evaluation + EventBus confirmation system but is not used by the agent loop. This design deliberately keeps SafetyGuard as the agent loop's permission check because:

1. **Simplicity**: SafetyGuard is synchronous, no EventBus dependency needed
2. **Ask = Deny for now**: No interactive confirmation needed yet
3. **Migration path**: When Ask becomes interactive, replace SafetyGuard with PermissionManager; `ToolPermissionsConfig` maps directly to `PermissionConfigMap` (both use `PermissionAction`)

## SafetyGuard Changes

### Current Structure

```rust
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    confirmation_required: HashSet<String>,
}
```

### New Structure

```rust
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    tool_permissions: HashMap<String, PermissionAction>,
    default_permission: PermissionAction,
}
```

### New Constructor

```rust
impl SafetyGuard {
    /// Build from merged global + agent permission config.
    /// Retains default blocked patterns (rm -rf /, DROP DATABASE, etc.)
    /// which are ALWAYS enforced regardless of permission settings.
    pub fn from_permissions(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> Self {
        // 1. Start with default blocked patterns (rm -rf /, DROP DATABASE, etc.)
        // 2. Collect all keys from global.overrides ∪ agent.overrides
        // 3. For each key: effective = min(global, agent)
        // 4. effective_default = min(global.default, agent.default)
    }

    /// Existing default_guard() kept for backward compatibility / tests.
    pub fn default_guard() -> Self { ... }
}
```

### New SafetyError Variant

```rust
pub enum SafetyError {
    Blocked { tool: String, pattern: String },   // Dangerous pattern match
    NeedsConfirmation { tool: String },           // Ask level (= Deny for now)
    PolicyDenied { tool: String },                // NEW: Denied by permission policy
}
```

Named `PolicyDenied` (not `Denied`) to avoid confusion with `PermissionError::Denied`.

### check() Logic

```
1. blocked_patterns match → Blocked (unchanged, highest priority)
2. Lookup tool_permissions[tool_name], fallback to default_permission
   - Allow → Ok(())
   - Ask → NeedsConfirmation (currently = Deny)
   - Deny → PolicyDenied
```

`blocked_patterns` are NOT affected by permission settings — they always hard-block regardless of any Allow override.

### run_loop.rs Change

```rust
// Before:
let safety = SafetyGuard::default_guard();

// After:
let global_perms = &self.global_tool_permissions;   // from gateway config
let agent_perms = agent.tool_permissions();          // from agent definition
let safety = SafetyGuard::from_permissions(global_perms, agent_perms);
```

`ExecutionEngine` needs access to global `ToolPermissionsConfig`. Add it as a field read from `PoliciesConfig` at construction time.

## RPC Interface

### Global Tool Permissions

| Method | Params | Description |
|--------|--------|-------------|
| `config.get_tool_permissions` | none | Return global `ToolPermissionsConfig` |
| `config.update_tool_permissions` | `{ default?, overrides? }` | Partial update, `save_incremental` |

### Agent Tool Permissions

| Method | Params | Description |
|--------|--------|-------------|
| `agent_config.get_tool_permissions` | `{ agent_id }` | Return agent's config |
| `agent_config.update_tool_permissions` | `{ agent_id, default?, overrides? }` | Partial update |

Agent response includes `effective` and `global_overrides` for UI rendering:

```json
{
  "default": "allow",
  "overrides": { "shell": "allow" },
  "effective": { "shell": "deny" },
  "global_overrides": { "shell": "deny" }
}
```

- `effective`: merged permission for each key in `agent.overrides ∪ global.overrides` (not all tools — Panel uses `default_permission` for unlisted tools)
- `global_overrides`: the global overrides map, so Panel knows which tools are globally forced

Panel uses `global_overrides` to render globally-forced tools as greyed-out.

## Panel UI

### ToolsTab Redesign (Agent Level)

Transform existing on/off toggle into three-state segmented control per tool:

- **Allow** (green) / **Ask** (yellow) / **Deny** (red), current value highlighted
- Group header: dropdown to set entire group to Allow or Deny (overrides all non-greyed-out tools in group)
- Globally Denied tools: entire row greyed out, locked at Deny, tooltip "Globally denied in Policies"
- Globally Ask tools: cannot select Allow, only Ask or Deny
- Changes apply only to `tool_permissions` — existing skills whitelist/blacklist (tool visibility) remains on a separate tab or section

### PoliciesView — New "Tool Permissions" Section

Added above existing Content Safety section. Same layout as ToolsTab (reuse group structure) but labeled as global defaults. No grey restrictions (this IS the ceiling).

Includes a "Default" dropdown at the top for the global default permission level.

UI hint: "Changes will take effect on next agent run" to set expectations about hot reload behavior.

### Data Flow

```
PoliciesView                    ToolsTab (per agent)
    │                                │
    ▼                                ▼
config.update_tool_permissions   agent_config.update_tool_permissions
    │                                │
    ▼                                ▼
config.toml [policies]           agent definition
    │                                │
    └──────────┬─────────────────────┘
               ▼
    SafetyGuard::from_permissions(global, agent)
               ▼
         agent loop check()
```

## Error Handling

### Permission Denial in Agent Loop

```rust
Err(SafetyError::PolicyDenied { tool }) => {
    callback.on_safety_block(&err);
    messages.push(UnifiedMessage::tool_result(
        tc.id, tc.name,
        format!("DENIED: tool '{}' is not allowed by permission policy", tool),
        true,
    ));
    // Does NOT count toward consecutive_errors
}
```

Not counting toward `consecutive_errors` prevents permission denials from accidentally terminating the loop. The agent sees the denial and should adjust strategy.

### Hot Reload

- Global permission updates refresh via existing `hot_reload` mechanism
- Running agent loops are not affected (SafetyGuard constructed at loop start)
- New permissions take effect on next run

## Backward Compatibility

- Existing ToolsTab skills whitelist/blacklist preserved (controls tool visibility)
- New tool_permissions controls execution permission — independent layer
- Tool not in whitelist → agent cannot see it (won't call)
- Tool in whitelist but permission = Deny → agent sees it but call is rejected
- `SafetyGuard::default_guard()` preserved for tests and backward compatibility

## Defaults

- Global: `default = "allow"`, `overrides = {}`
- Agent: `tool_permissions = None` (inherits global entirely)
- Both unconfigured = all Allow, matching current behavior

## File Locations

| What | Where |
|------|-------|
| `ToolPermissionsConfig` struct | `src/config/types/policies/tool_permissions.rs` (new) |
| Global config field | `PoliciesConfig` in `src/config/types/policies/mod.rs` |
| Agent config field | `AgentDefinition` in `src/config/types/agents_def.rs` |
| SafetyGuard changes | `src/agent_loop/safety.rs` |
| run_loop integration | `src/gateway/execution_engine/run_loop.rs` |
| Global RPC handlers | `src/gateway/handlers/config.rs` |
| Agent RPC handlers | `src/gateway/handlers/agent_config.rs` |
| Panel ToolsTab | `apps/panel/src/views/agents/tools.rs` |
| Panel PoliciesView | `apps/panel/src/views/settings/policies.rs` |

## Future: Ask Confirmation Flow

Currently Ask = Deny (auto-denied). When implementing interactive confirmation:
1. Replace SafetyGuard with PermissionManager in agent loop
2. PermissionManager already has EventBus-based confirmation flow
3. Config format (ToolPermissionsConfig) maps to PermissionConfigMap
4. UI remains unchanged — Ask button already present

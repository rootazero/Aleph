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

Reuse existing `PermissionAction` enum: `Allow` / `Ask` / `Deny`. Serializes to `"allow"` / `"ask"` / `"deny"`.

### ToolPermissionsConfig

New config struct used by both global and agent-level:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolPermissionsConfig {
    #[serde(default = "default_allow")]
    pub default: PermissionAction,
    #[serde(default)]
    pub overrides: HashMap<String, PermissionAction>,
}
```

### Global Config — `config.toml`

```toml
[policies.tool_permissions]
default = "allow"

[policies.tool_permissions.overrides]
# shell = "deny"
# file_delete = "ask"
```

### Agent Config — agent definition

```toml
[tool_permissions]
default = "allow"

[tool_permissions.overrides]
# shell = "deny"
```

### Merge Logic

```
effective(tool) = min(global(tool), agent(tool))
```

Where Allow > Ask > Deny, `min` takes the more restrictive:
- Global Deny → always Deny regardless of agent setting
- Global Allow + Agent Deny → Deny
- Global Allow + Agent Allow → Allow
- Global Ask + Agent Allow → Ask

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
    pub fn from_permissions(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> Self {
        // 1. Start with default blocked patterns (rm -rf /, DROP DATABASE, etc.)
        // 2. Merge: effective = min(global, agent) for each tool
        // 3. effective_default = min(global.default, agent.default)
    }
}
```

### New SafetyError Variant

```rust
pub enum SafetyError {
    Blocked { tool: String, pattern: String },
    NeedsConfirmation { tool: String },
    Denied { tool: String },  // NEW
}
```

### check() Logic

```
1. blocked_patterns match → Blocked (unchanged)
2. Lookup tool_permissions[tool_name], fallback to default_permission
   - Allow → Ok(())
   - Ask → NeedsConfirmation (currently = Deny)
   - Deny → Denied
```

### run_loop.rs Change

```rust
// Before:
let safety = SafetyGuard::default_guard();

// After:
let global_perms = self.config.policies.tool_permissions();
let agent_perms = agent.tool_permissions();
let safety = SafetyGuard::from_permissions(&global_perms, &agent_perms);
```

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

Panel uses `global_overrides` to render globally-forced tools as greyed-out.

## Panel UI

### ToolsTab Redesign (Agent Level)

Transform existing on/off toggle into three-state segmented control per tool:

- **Allow** (green) / **Ask** (yellow) / **Deny** (red), current value highlighted
- Group header: dropdown to set entire group to Allow or Deny
- Globally Denied tools: entire row greyed out, locked at Deny, tooltip "Globally denied in Policies"
- Globally Ask tools: cannot select Allow, only Ask or Deny

### PoliciesView — New "Tool Permissions" Section

Added above existing Content Safety section. Same layout as ToolsTab (reuse group structure) but labeled as global defaults. No grey restrictions (this IS the ceiling).

Includes a "Default" dropdown at the top for the global default permission level.

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
Err(SafetyError::Denied { tool }) => {
    messages.push(UnifiedMessage::tool_result(
        tc.id, tc.name,
        "DENIED: tool '{}' is not allowed by permission policy",
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

## Defaults

- Global: `default = "allow"`, `overrides = {}`
- Agent: `default = "allow"`, `overrides = {}`
- Both unconfigured = all Allow, matching current behavior

## Future: Ask Confirmation Flow

Currently Ask = Deny (auto-denied). When implementing interactive confirmation:
1. Replace SafetyGuard with PermissionManager in agent loop
2. PermissionManager already has EventBus-based confirmation flow
3. Config format (ToolPermissionsConfig) remains unchanged
4. UI remains unchanged — Ask button already present

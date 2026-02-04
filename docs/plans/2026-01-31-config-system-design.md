# Config System Design: JSON Schema + UI Hints

> Date: 2026-01-31
> Status: Approved
> Goal: 实现 Moltbot 风格的配置系统，支持 JSON Schema 生成和 UI Hints

## Overview

为 Aleph 配置系统添加：
1. **全 TOML 格式** — 统一核心配置和扩展配置
2. **JSON Schema 生成** — 使用 schemars 自动生成
3. **UI Hints 系统** — 字段标签、帮助文本、分组
4. **RPC 暴露** — `config.schema` 方法供客户端渲染配置表单
5. **热重载集成** — 基于变更路径的分级 reload

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    配置系统架构                              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ~/.aleph/config.toml          workspace/aleph.toml       │
│         │                              │                    │
│         ▼                              ▼                    │
│  ┌─────────────┐                ┌─────────────┐            │
│  │ CoreConfig  │                │ ExtConfig   │            │
│  │ (schemars)  │                │ (schemars)  │            │
│  └──────┬──────┘                └──────┬──────┘            │
│         │                              │                    │
│         └──────────┬───────────────────┘                    │
│                    ▼                                        │
│           ┌───────────────┐                                 │
│           │ MergedConfig  │                                 │
│           └───────┬───────┘                                 │
│                   │                                         │
│         ┌─────────┼─────────┐                               │
│         ▼         ▼         ▼                               │
│   ┌──────────┐ ┌──────┐ ┌────────┐                         │
│   │ Validate │ │Schema│ │UiHints │                         │
│   └──────────┘ └──────┘ └────────┘                         │
│                   │         │                               │
│                   └────┬────┘                               │
│                        ▼                                    │
│              config.schema RPC                              │
│                        │                                    │
│         ┌──────────────┼──────────────┐                     │
│         ▼              ▼              ▼                     │
│    macOS App      Tauri App      Web UI                    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Part 1: Extension Config Migration (JSONC → TOML)

### Current State
- `aleph.jsonc` in workspace root

### Target State
- `aleph.toml` in workspace root

### File Format Comparison

**Old (aleph.jsonc):**
```jsonc
{
  "$schema": "https://aleph.ai/config.json",
  "model": "anthropic/claude-opus-4-5",
  "plugin": ["npm:@anthropic/mcp-server-filesystem"],
  "agent": {
    "main": { "model": "claude-opus-4-5", "temperature": 0.7 }
  }
}
```

**New (aleph.toml):**
```toml
# Aleph 项目配置
# schema: https://aleph.ai/config.json

model = "anthropic/claude-opus-4-5"
plugins = ["npm:@anthropic/mcp-server-filesystem"]

[agent.main]
model = "claude-opus-4-5"
temperature = 0.7
```

### Migration Strategy

| Step | Action |
|------|--------|
| 1 | New code supports both `aleph.toml` and `aleph.jsonc` |
| 2 | Priority: `aleph.toml` > `aleph.jsonc` > `aleph.json` |
| 3 | Provide `aleph config migrate` command for auto-conversion |
| 4 | Remove JSONC support after 1-2 versions |

### Code Changes

```
core/src/extension/config/
├── mod.rs          # Modify: add TOML loading logic
├── types.rs        # Keep: structs unchanged
├── loader.rs       # New: unified loader (TOML priority)
└── migrate.rs      # New: JSONC → TOML migration tool
```

## Part 2: JSON Schema Generation

Use `schemars` crate to auto-generate JSON Schema Draft-07 from Rust structs.

### Implementation

**1. Add derive to config structs:**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProviderConfig {
    /// AI provider type (openai, claude, gemini, ollama)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,

    /// API key for cloud providers
    #[schemars(skip)]  // Sensitive field excluded from schema
    pub api_key: Option<String>,

    /// Model identifier
    pub model: String,

    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 300))]
    pub timeout_seconds: u64,
}
```

**2. Schema generation function:**

```rust
use schemars::schema_for;

pub fn generate_config_schema() -> schemars::schema::RootSchema {
    schema_for!(Config)
}
```

**3. Generated output example:**

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Config",
  "type": "object",
  "properties": {
    "providers": {
      "type": "object",
      "additionalProperties": {
        "$ref": "#/definitions/ProviderConfig"
      }
    }
  },
  "definitions": {
    "ProviderConfig": {
      "type": "object",
      "properties": {
        "model": { "type": "string" },
        "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300 }
      },
      "required": ["model"]
    }
  }
}
```

### Structs Requiring JsonSchema Derive

| File | Structs |
|------|---------|
| `config/structs.rs` | `Config`, `FullConfig` |
| `config/types/provider.rs` | `ProviderConfig` |
| `config/types/general.rs` | `GeneralConfig`, `MemoryConfig` |
| `config/types/routing.rs` | `RoutingRuleConfig` |
| `extension/config/types.rs` | `AlephConfig`, `AgentConfigOverride` |

Approximately 20 core structs need `#[derive(JsonSchema)]`.

## Part 3: UI Hints System

### Type Definitions

```rust
// core/src/config/ui_hints.rs

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FieldHint {
    /// Field path, e.g., "providers.openai.api_key"
    pub path: String,
    /// Human-readable label
    pub label: Option<String>,
    /// Help text/tooltip
    pub help: Option<String>,
    /// Group name
    pub group: Option<String>,
    /// Order within group (lower = higher priority)
    pub order: Option<i32>,
    /// Advanced option (hidden by default)
    pub advanced: bool,
    /// Sensitive field (password, token)
    pub sensitive: bool,
    /// Input placeholder
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ConfigUiHints {
    /// Group definitions: id → display metadata
    pub groups: HashMap<String, GroupMeta>,
    /// Field hints: path → hint
    pub fields: HashMap<String, FieldHint>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GroupMeta {
    pub label: String,
    pub order: i32,
    pub icon: Option<String>,
}
```

### Declaration via Macros

```rust
// core/src/config/ui_hints/definitions.rs

define_groups! {
    "general"  => { label: "General",  order: 10, icon: "gear" },
    "providers"=> { label: "Providers",order: 20, icon: "cloud" },
    "agents"   => { label: "Agents",   order: 30, icon: "robot" },
    "channels" => { label: "Channels", order: 40, icon: "chat" },
    "tools"    => { label: "Tools",    order: 50, icon: "wrench" },
    "advanced" => { label: "Advanced", order: 100, icon: "sliders" },
}

define_hints! {
    // General
    "general.language" => {
        label: "Language",
        help: "UI display language",
        group: "general",
        order: 1,
    },
    "general.default_provider" => {
        label: "Default Provider",
        help: "AI provider used when no routing rule matches",
        group: "general",
        order: 2,
    },

    // Providers (wildcard matching)
    "providers.*.api_key" => {
        label: "API Key",
        sensitive: true,
        group: "providers",
    },
    "providers.*.model" => {
        label: "Model",
        help: "Model identifier (e.g., gpt-4o, claude-opus-4-5)",
        group: "providers",
    },
}
```

### Wildcard Matching

`providers.*.api_key` matches `providers.openai.api_key`, `providers.claude.api_key`, etc.

Runtime matching priority (longest match first):
1. `providers.openai.api_key` (exact)
2. `providers.*.api_key` (wildcard)
3. `providers.*.*` (more general)

## Part 4: RPC Method `config.schema`

### Request/Response Types

```rust
// core/src/gateway/handlers/config.rs

/// config.schema request params
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigSchemaRequest {
    /// Include plugin schemas (default: true)
    #[serde(default = "default_true")]
    pub include_plugins: bool,
}

/// config.schema response
#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigSchemaResponse {
    /// JSON Schema Draft-07
    pub schema: serde_json::Value,
    /// UI rendering hints
    pub ui_hints: ConfigUiHints,
    /// Aleph version
    pub version: String,
    /// Generation timestamp (ISO 8601)
    pub generated_at: String,
}
```

### Handler Implementation

```rust
pub async fn handle_config_schema(
    req: ConfigSchemaRequest,
    ctx: &HandlerContext,
) -> Result<ConfigSchemaResponse, RpcError> {
    // 1. Generate base schema
    let base_schema = generate_config_schema();

    // 2. Merge plugin schemas if requested
    let merged_schema = if req.include_plugins {
        let plugins = ctx.plugin_registry.list_plugins();
        merge_plugin_schemas(base_schema, &plugins)
    } else {
        serde_json::to_value(base_schema)?
    };

    // 3. Build UI hints
    let ui_hints = build_ui_hints();

    Ok(ConfigSchemaResponse {
        schema: merged_schema,
        ui_hints,
        version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}
```

### RPC Registration

```rust
// core/src/gateway/handlers/mod.rs

registry.register("config.schema", handle_config_schema);
```

### Client Usage Example

**Request:**
```json
{
  "type": "req",
  "id": "uuid-123",
  "method": "config.schema",
  "params": { "include_plugins": true }
}
```

**Response:**
```json
{
  "type": "res",
  "id": "uuid-123",
  "ok": true,
  "payload": {
    "schema": { "$schema": "http://json-schema.org/draft-07/schema#", ... },
    "ui_hints": {
      "groups": { "general": { "label": "General", "order": 10 } },
      "fields": { "general.language": { "label": "Language", ... } }
    },
    "version": "0.1.0",
    "generated_at": "2026-01-31T12:00:00Z"
  }
}
```

## Part 5: Hot Reload Integration

Aleph already has `ConfigWatcher` (336 lines) but not enabled. Need to integrate into Gateway with reload plan logic.

### Reload Plan Design

```rust
// core/src/config/reload.rs

#[derive(Debug, Clone, Default)]
pub struct ReloadPlan {
    /// Requires full Gateway restart
    pub restart_gateway: bool,
    /// Channels to restart
    pub restart_channels: HashSet<String>,
    /// Reload hooks
    pub reload_hooks: bool,
    /// Restart cron
    pub restart_cron: bool,
    /// Hot-updatable paths (no restart needed)
    pub hot_paths: Vec<String>,
}

/// Build reload plan from changed paths
pub fn build_reload_plan(changed_paths: &[String]) -> ReloadPlan {
    let mut plan = ReloadPlan::default();

    for path in changed_paths {
        match path.as_str() {
            // Requires Gateway restart
            p if p.starts_with("gateway.") => plan.restart_gateway = true,
            p if p.starts_with("plugins") => plan.restart_gateway = true,

            // Restart specific channel
            p if p.starts_with("channels.telegram") => {
                plan.restart_channels.insert("telegram".into());
            }
            p if p.starts_with("channels.discord") => {
                plan.restart_channels.insert("discord".into());
            }

            // Reload hooks
            p if p.starts_with("hooks") => plan.reload_hooks = true,

            // Restart cron
            p if p.starts_with("cron") => plan.restart_cron = true,

            // Hot update (no restart)
            _ => plan.hot_paths.push(path.clone()),
        }
    }

    plan
}
```

### Config Diff Detection

```rust
// core/src/config/diff.rs

/// Recursively compare two configs, return changed paths
pub fn diff_config(prev: &Config, next: &Config) -> Vec<String> {
    let prev_value = serde_json::to_value(prev).unwrap();
    let next_value = serde_json::to_value(next).unwrap();

    diff_json_values(&prev_value, &next_value, "")
}

fn diff_json_values(prev: &Value, next: &Value, prefix: &str) -> Vec<String> {
    // Recursive comparison, returns e.g., ["providers.openai.model", "general.language"]
}
```

### Gateway Integration

```rust
// core/src/gateway/server.rs

impl Gateway {
    pub async fn start_config_watcher(&self) -> Result<()> {
        let current_config = Arc::new(RwLock::new(Config::load()?));

        let watcher = ConfigWatcher::new({
            let current = current_config.clone();
            let gateway = self.clone();

            move |result| {
                if let Ok(new_config) = result {
                    let prev = current.read().unwrap();
                    let changed = diff_config(&prev, &new_config);
                    let plan = build_reload_plan(&changed);

                    // Execute reload plan
                    gateway.apply_reload_plan(plan, new_config);

                    // Broadcast event
                    gateway.event_bus.publish("config.reloaded", &changed);
                }
            }
        });

        watcher.start()?;
        Ok(())
    }
}
```

### Hot Update Classification

| Path Prefix | Action | Description |
|-------------|--------|-------------|
| `gateway.*` | Restart Gateway | Core settings like port, auth |
| `plugins` | Restart Gateway | Plugin loading requires restart |
| `channels.<name>` | Restart that channel | Only affects specific channel |
| `cron` | Restart cron | Scheduled tasks |
| `hooks` | Reload hooks | Event hooks |
| `agents`, `providers`, `rules` | Hot update | Effective on next request |

## File Changes Summary

```
core/src/config/
├── mod.rs              # Modify: export new modules
├── schema.rs           # New: JSON Schema generation
├── ui_hints/
│   ├── mod.rs          # New: UiHints types
│   ├── definitions.rs  # New: macro-defined hints
│   └── macros.rs       # New: define_groups! define_hints!
├── diff.rs             # New: config comparison
├── reload.rs           # New: ReloadPlan logic
├── structs.rs          # Modify: add #[derive(JsonSchema)]
└── types/*.rs          # Modify: add #[derive(JsonSchema)]

core/src/extension/config/
├── mod.rs              # Modify: TOML loading priority
├── loader.rs           # New: unified loader
└── migrate.rs          # New: JSONC → TOML migration

core/src/gateway/handlers/
└── config.rs           # Modify: add config.schema handler
```

## Dependencies

Add to `Cargo.toml`:
```toml
[dependencies]
schemars = { version = "0.8", features = ["chrono"] }
```

## Implementation Order

1. **Phase 1**: Add `#[derive(JsonSchema)]` to all config structs
2. **Phase 2**: Implement `schema.rs` with `generate_config_schema()`
3. **Phase 3**: Implement UI Hints macros and definitions
4. **Phase 4**: Add `config.schema` RPC handler
5. **Phase 5**: Implement config diff and reload plan
6. **Phase 6**: Integrate hot reload into Gateway
7. **Phase 7**: Extension config TOML migration

## Success Criteria

- [ ] `config.schema` RPC returns valid JSON Schema Draft-07
- [ ] UI Hints cover all user-facing config fields
- [ ] Hot reload works for non-gateway config changes
- [ ] `aleph config migrate` converts JSONC to TOML
- [ ] All existing tests pass

# Self-Configuration Design

> 通过自然语言交互，消除用户手动编辑 TOML 文件的痛苦，实现智能化配置体验。

**Date**: 2026-03-01
**Status**: Approved
**Approach**: Schema-Driven 通用引擎（方案 A）

---

## 1. 设计目标

让 Aleph 具备"理解自己、修改自己、验证自己"的能力：

- **全量覆盖**所有 config.toml section（providers, memory, dispatcher, tools, policies 等）
- **双路径接入**：BuiltinTool（LLM 调用）+ RPC（客户端调用），共享核心逻辑
- **安全优先**：密钥自动分流到 SecretVault，变更需用户确认，写入前备份
- **智能验证**：Provider 做连通性拨测，其他 section 做 Schema 校验 + `Config::validate()`

---

## 2. 架构概览

```
                    ┌─────────────────┐
                    │   User / LLM    │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
              ▼              ▼              ▼
     ┌────────────┐  ┌────────────┐  ┌────────────┐
     │ConfigRead  │  │ConfigUpdate│  │config.patch│
     │   Tool     │  │   Tool     │  │   RPC      │
     │(builtin)   │  │(builtin)   │  │(gateway)   │
     └─────┬──────┘  └─────┬──────┘  └─────┬──────┘
           │               │               │
           │               └───────┬───────┘
           │                       │
           ▼                       ▼
     ┌──────────┐         ┌──────────────┐
     │  Config   │         │ ConfigPatcher│  ← 核心引擎
     │  (read)   │         │              │
     └──────────┘         └──────┬───────┘
                                 │
                    ┌────────────┼────────────┐
                    │            │            │
                    ▼            ▼            ▼
             ┌──────────┐ ┌──────────┐ ┌──────────┐
             │  Schema   │ │ Vault    │ │ Backup   │
             │ Validator │ │(secrets) │ │ Manager  │
             └──────────┘ └──────────┘ └──────────┘
                    │            │            │
                    └────────────┼────────────┘
                                 │
                                 ▼
                    ┌──────────────────────┐
                    │  save_incremental()  │
                    │  + ConfigWatcher     │
                    │  (hot-reload)        │
                    └──────────────────────┘
```

---

## 3. 核心引擎 — ConfigPatcher

### 3.1 职责

接收 `PatchRequest`，执行完整的校验→备份→写入→重载→拨测流水线。

### 3.2 数据流

```
PatchRequest { path, patch, secret_fields }
  → Schema 校验 (jsonschema crate + generate_config_schema())
  → 冲突检测 (比对 config.toml mtime)
  → 密钥分流 (secret_fields → SecretVault, config 写 secret_name)
  → 备份快照 (~/.aleph/backups/config.toml.{timestamp})
  → 增量写入 (save_incremental([section]))
  → 热重载通知 (ConfigWatcher 自动感知 / 主动 reload)
  → 拨测验证 (Provider: list_models, 其他: validate())
  → PatchResult { applied_sections, diff, health_check, warnings }
```

### 3.3 核心类型

```rust
// core/src/config/patcher.rs

pub struct ConfigPatcher {
    config_path: PathBuf,
    vault: Arc<SecretVault>,
    backup: ConfigBackup,
}

pub struct PatchRequest {
    /// Config path, e.g. "providers.deepseek" or "memory"
    pub path: String,
    /// Values to merge (JSON)
    pub patch: serde_json::Value,
    /// Sensitive fields: key=field_name, value=plaintext (redirected to Vault)
    pub secret_fields: HashMap<String, String>,
    /// Whether to run health check after applying
    pub health_check: bool,
    /// Dry-run mode: validate only, don't write
    pub dry_run: bool,
}

pub struct PatchResult {
    /// Sections actually modified
    pub applied_sections: Vec<String>,
    /// Field-level diff (old → new)
    pub diff: Vec<FieldDiff>,
    /// Health check outcome
    pub health_check: Option<HealthCheckResult>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

pub struct FieldDiff {
    pub path: String,
    pub old_value: Option<serde_json::Value>,
    pub new_value: serde_json::Value,
}

pub enum HealthCheckResult {
    Passed,
    Failed { reason: String },
    Skipped,
}
```

### 3.4 密钥分流逻辑

```rust
for (field_name, secret_value) in &request.secret_fields {
    let vault_key = format!("{}_{}", provider_name, field_name);
    vault.set(&vault_key, &secret_value)?;
    patch["secret_name"] = json!(vault_key);
    patch.remove("api_key");
}
```

### 3.5 冲突检测

比对 config.toml 的文件 mtime 与上次 ConfigPatcher 加载时间。如果文件被外部修改（用户手动编辑），返回 `Conflict` 错误，附带 diff 信息让用户决定。

---

## 4. ConfigUpdateTool — LLM 工具层

### 4.1 Tool 定义

```rust
// core/src/builtin_tools/config_update.rs

pub struct ConfigUpdateTool {
    patcher: Arc<ConfigPatcher>,
}

impl AlephTool for ConfigUpdateTool {
    const NAME: &'static str = "config_update";
    const DESCRIPTION: &'static str =
        "Update Aleph configuration. Supports all config sections. \
         Sensitive fields (API keys, tokens) are automatically encrypted in SecretVault.";
    type Args = ConfigUpdateArgs;
    type Output = ConfigUpdateOutput;

    fn requires_confirmation(&self) -> bool { true }
}
```

### 4.2 Args (LLM 可见参数)

```rust
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigUpdateArgs {
    /// Target config path, e.g. "providers.deepseek", "memory", "dispatcher"
    pub path: String,
    /// Config values to set/update (JSON object, merged into existing config)
    pub values: serde_json::Value,
    /// Sensitive fields → SecretVault. Key = field name, Value = secret value.
    #[serde(default)]
    pub secrets: HashMap<String, String>,
    /// If true, only validate without applying changes
    #[serde(default)]
    pub dry_run: bool,
}
```

### 4.3 Output (返回给 LLM)

```rust
#[derive(Serialize)]
pub struct ConfigUpdateOutput {
    pub success: bool,
    /// Human-readable summary of changes
    pub summary: String,
    /// Changed field paths
    pub changed_fields: Vec<String>,
    /// Health check result (for provider configs)
    pub health_check: Option<String>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}
```

### 4.4 安全机制

- `requires_confirmation() = true` — Agent Loop 暂停等待用户确认
- 敏感值永不回显 — Output 只含 "API key stored in vault as 'deepseek_api_key'"
- dry_run 模式 — 返回将要发生的变更预览

### 4.5 LLM 调用示例

```json
// "帮我配置 DeepSeek，Key 是 sk-xxx"
{
  "tool": "config_update",
  "args": {
    "path": "providers.deepseek",
    "values": { "model": "deepseek-chat", "enabled": true },
    "secrets": { "api_key": "sk-xxx" }
  }
}

// "把记忆系统的检索数量调到 20"
{
  "tool": "config_update",
  "args": {
    "path": "memory",
    "values": { "search_limit": 20 }
  }
}
```

---

## 5. ConfigReadTool — LLM 读取配置

```rust
// core/src/builtin_tools/config_read.rs

pub struct ConfigReadTool;

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ConfigReadArgs {
    /// Config section path. Use "" or "all" for overview.
    pub path: String,
}

#[derive(Serialize)]
pub struct ConfigReadOutput {
    /// Config values (sensitive fields masked as "***")
    pub values: serde_json::Value,
    /// JSON Schema for this section
    pub schema: Option<serde_json::Value>,
}
```

敏感字段（`api_key`、`token` 等）在输出时自动替换为 `"***"`。

---

## 6. config.patch RPC 持久化

### 改造 `handle_patch_config()`

```rust
async fn handle_patch_config(params, ctx) -> Result<Value> {
    let request = PatchRequest::from_rpc_params(params)?;
    let result = ctx.config_patcher.apply(request).await?;
    ctx.broadcast(ConfigChangedEvent { ... });
    Ok(json!(result))
}
```

### RPC 参数

```json
{
  "method": "config.patch",
  "params": {
    "path": "providers.openai",
    "patch": { "model": "gpt-4o", "temperature": 0.8 },
    "secret_fields": { "api_key": "sk-xxx" },
    "dry_run": false,
    "health_check": true
  }
}
```

RPC 和 BuiltinTool 入参本质相同，都转换为 `PatchRequest` 交给 `ConfigPatcher`。

---

## 7. 备份系统

```rust
// core/src/config/backup.rs

pub struct ConfigBackup {
    backup_dir: PathBuf,  // ~/.aleph/backups/
    max_count: usize,     // default 10
}

impl ConfigBackup {
    /// Snapshot current config.toml to ~/.aleph/backups/config.toml.{timestamp}
    pub fn create_snapshot(&self) -> Result<PathBuf>;
    /// Remove oldest backups beyond max_count
    pub fn cleanup(&self) -> Result<()>;
    /// List all backup entries
    pub fn list(&self) -> Result<Vec<BackupEntry>>;
}
```

---

## 8. 安全设计

| 机制 | 实现 |
|------|------|
| **用户确认** | `requires_confirmation() = true`，Agent Loop 暂停等待确认 |
| **密钥零明文** | `secret_fields` 立即写入 Vault，不进 log，不回显 |
| **冲突检测** | 比对 config.toml mtime，防止覆盖手动编辑 |
| **Schema 强校验** | `jsonschema` crate 验证 patch 值合法性 |
| **原子写入** | 复用 `.tmp → fsync → rename` 路径 |
| **文件权限** | 复用 `chmod 0o600` |
| **备份快照** | 写入前自动备份，保留最近 10 份 |

---

## 9. 文件清单

### 新增文件

| 文件 | 职责 |
|------|------|
| `core/src/config/patcher.rs` | ConfigPatcher — 核心引擎 |
| `core/src/config/backup.rs` | ConfigBackup — 快照管理 |
| `core/src/builtin_tools/config_update.rs` | ConfigUpdateTool — LLM 写配置 |
| `core/src/builtin_tools/config_read.rs` | ConfigReadTool — LLM 读配置 |

### 改造文件

| 文件 | 变更 |
|------|------|
| `core/src/gateway/handlers/config.rs` | `handle_patch_config` 接入 ConfigPatcher |
| `core/src/builtin_tools/mod.rs` | 注册 ConfigUpdateTool + ConfigReadTool |
| `core/src/executor/builtin_registry/` | 注册新 Tool 到 registry |
| `core/src/config/mod.rs` | 导出 patcher 和 backup 模块 |

---

## 10. 不包含 (Out of Scope)

- **UI 集成** — Panel Chat 窗口已有，无需额外入口
- **Agent Workflow** — 后续可在此基础上构建引导式工作流
- **Auth Profiles 管理** — profiles.toml 的管理暂不纳入，聚焦 config.toml

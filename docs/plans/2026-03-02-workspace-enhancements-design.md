# Workspace Enhancements Design

> 日期: 2026-03-02
> 状态: 已批准
> 前置: [Workspace Wiring Design](2026-03-02-workspace-wiring-design.md)

## 背景

Workspace 核心接通已完成（8 commits, 23 files）。本文档设计 4 个增强功能，解决上轮留下的 TODO。

## Enhancement 1: Per-Request Temperature Override

### 问题

Provider trait 的 `process()` 方法不接受 per-request generation 参数。Temperature 在 provider 构造时固定，workspace profile 的 temperature 覆盖无法生效。

### 方案

新增 `GenerationParams` 结构体，扩展 AiProvider trait。

```rust
// core/src/providers/mod.rs
pub struct GenerationParams {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self { temperature: None, max_tokens: None, top_p: None }
    }
}
```

**AiProvider trait 扩展**（向后兼容）：

```rust
// 新增方法，有默认实现，不破坏现有 provider
fn process_with_generation_params(
    &self,
    input: &str,
    system: Option<&str>,
    params: GenerationParams,
) -> Pin<Box<dyn Future<Output = Result<...>> + Send + '_>> {
    // 默认：忽略 params，调用原 process()
    self.process(input, system)
}
```

**HttpProvider 覆写**：将 params 合并到请求 payload（覆盖 ProviderConfig 默认值）。

**Thinker 侧**：

```rust
fn resolve_generation_params(&self) -> GenerationParams {
    match &self.config.active_profile {
        Some(profile) => GenerationParams {
            temperature: profile.temperature,
            max_tokens: profile.max_tokens,
            top_p: None,
        },
        None => GenerationParams::default(),
    }
}
```

在 `call_llm_with_level()` 中调用 `process_with_generation_params()` 替代 `process()`。

### 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/providers/mod.rs` | 新增 GenerationParams，扩展 AiProvider trait |
| `core/src/providers/http_provider.rs` | 覆写 process_with_generation_params |
| `core/src/thinker/mod.rs` | 新增 resolve_generation_params()，修改 call_llm_with_level |

---

## Enhancement 2: Cross-Workspace Memory Query

### 问题

`WorkspaceFilter::Multiple` 已存在但未暴露给 AI。AI 无法跨 workspace 检索记忆（如"回忆健康 workspace 里提过的运动计划"）。

### 方案

扩展 `MemorySearchArgs`，支持多 workspace 查询。

```rust
pub struct MemorySearchArgs {
    pub query: String,
    pub max_results: usize,
    pub workspace: Option<String>,        // 保留，单 workspace 兼容
    pub workspaces: Option<Vec<String>>,   // 新增，多 workspace
    pub cross_workspace: Option<bool>,     // 新增，true = 搜索所有 workspace
}
```

**优先级规则**：
1. `cross_workspace: true` → `WorkspaceFilter::All`
2. `workspaces: ["crypto", "health"]` → `WorkspaceFilter::Multiple`
3. `workspace: "crypto"` → `WorkspaceFilter::Single`
4. 都没提供 → 当前活跃 workspace（默认隔离）

**FactRetrieval 扩展**：

```rust
pub async fn retrieve_with_filter(
    &self,
    query: &str,
    filter: WorkspaceFilter,
) -> Result<Vec<MemoryFact>>
```

### 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/builtin_tools/memory_search.rs` | 扩展 args，新增 workspace 解析逻辑 |
| `core/src/memory/fact_retrieval.rs` | 新增 retrieve_with_filter() |

---

## Enhancement 3: Channel → Workspace Auto-Routing

### 问题

当前 route binding 只绑定 channel → agent_id。Telegram/Slack 等渠道的消息无法自动路由到对应 workspace。

### 方案

扩展 `MatchRule`，新增 `workspace` 字段。

```rust
pub struct MatchRule {
    pub channel: Option<String>,
    pub account_id: Option<String>,
    pub peer: Option<PeerMatchConfig>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    pub workspace: Option<String>,  // 新增
}
```

**配置示例**：

```toml
[[route_bindings]]
agent_id = "main"
[route_bindings.match]
channel = "telegram"
workspace = "crypto"

[[route_bindings]]
agent_id = "main"
[route_bindings.match]
channel = "slack"
team_id = "T12345"
workspace = "work"
```

**ExecutionEngine 变更**：

```rust
// workspace 解析增加路由匹配
let active_workspace = match resolve_workspace_from_route(&session_key, &route_bindings) {
    Some(ws_id) => ActiveWorkspace::from_workspace_id(ws_manager, &ws_id).await,
    None => ActiveWorkspace::from_manager(ws_manager, "owner").await,  // 现有逻辑
};
```

**ActiveWorkspace 新增**：

```rust
pub async fn from_workspace_id(manager: &WorkspaceManager, ws_id: &str) -> Self {
    // 加载指定 workspace + profile，不走 user active binding
}
```

### 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/routing/config.rs` | MatchRule 新增 workspace 字段 |
| `core/src/gateway/execution_engine/engine.rs` | 路由匹配 → workspace 解析 |
| `core/src/gateway/workspace.rs` | 新增 from_workspace_id() |

---

## Enhancement 4: Config Hot Reload (RPC)

### 问题

配置修改后需要重启服务器才能生效。

### 方案

新增 `config.reload` RPC 方法，按安全顺序重载各子系统。

**RPC 接口**：

```json
{ "method": "config.reload", "params": {} }

// 响应
{
    "ok": true,
    "reloaded": ["profiles", "providers", "routing_rules"],
    "failed": []
}
```

**Reload 逻辑**：

```rust
pub struct ReloadResult {
    pub reloaded: Vec<String>,
    pub failed: Vec<(String, String)>,
}

pub async fn reload_config(state: &AppState) -> ReloadResult {
    let new_config = Config::load_from_file(&config_path)?;
    let mut result = ReloadResult::default();

    // 安全顺序：
    // 1. Profiles（影响 workspace → profile 绑定）
    if let Err(e) = reload_profiles(&new_config, state) { result.failed.push(("profiles", e)); }
    else { result.reloaded.push("profiles"); }

    // 2. Providers（影响模型选择）
    if let Err(e) = reload_providers(&new_config, state) { ... }

    // 3. Routing rules（影响 channel → workspace）
    if let Err(e) = reload_routing_rules(&new_config, state) { ... }

    result
}
```

**可重载子系统**：

| 子系统 | 可重载 | 机制 |
|--------|--------|------|
| Profiles | ✅ | `WorkspaceManager.load_profiles()` |
| Providers | ✅ | ProviderRegistry rebuild |
| Routing rules | ✅ | 替换 route binding 列表 |
| Memory config | ⚠️ | 部分（decay rate 可以，store 不能） |
| Gateway 端口 | ❌ | 需要重启 |

**UI**：Settings 页面新增 "Reload Config" 按钮。

### 修改文件

| 文件 | 变更 |
|------|------|
| `core/src/config/reload.rs` | 新增 reload 逻辑 |
| `core/src/gateway/handlers/config.rs` | 新增 config.reload handler |
| `core/src/bin/aleph_server/commands/start/builder/handlers.rs` | 注册 handler |
| `core/ui/control_plane/src/views/settings/` | 新增 reload 按钮 |

---

## 不做什么（YAGNI）

- ❌ 文件监听自动重载（后续可加，本次只做 RPC 触发）
- ❌ SIGHUP 信号重载（平台相关，后续考虑）
- ❌ Per-workspace Soul 替换（始终叠加，不替换）
- ❌ 跨 workspace 记忆自动关联（AI 手动决定何时跨 workspace 查询即可）
- ❌ Provider 热切换（reload providers 只更新配置，不中断进行中的请求）

## 依赖顺序

```
Enhancement 1 (Temperature) ← 独立
Enhancement 2 (Cross-WS Memory) ← 独立
Enhancement 3 (Channel Routing) ← 独立
Enhancement 4 (Config Reload) ← 依赖 Enhancement 3 的 routing rules 可重载
```

推荐执行顺序：1 → 2 → 3 → 4

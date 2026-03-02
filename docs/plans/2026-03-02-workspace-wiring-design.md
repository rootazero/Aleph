# Workspace 完整接通设计

> 日期: 2026-03-02
> 状态: 已批准

## 背景

Aleph 的 Workspace + Profile + Memory 过滤的"骨架"已搭好，但关键的"神经"未接通：

- WorkspaceManager（SQLite）完整实现了 CRUD、用户绑定、Profile 继承
- Memory 系统支持 WorkspaceFilter 过滤（SQL WHERE 级别）
- ProfileConfig 定义了工具白名单（glob 模式）、模型覆盖、system_prompt
- ThinkerToolFilter 已支持 `with_profile()`

**问题**：这些模块各自独立运行，没有在执行路径中串联起来。

## 设计目标

让 workspace 切换真正生效：切换后记忆隔离、人格变化、工具受限、模型切换全部自动完成。

## 对比分析：OpenClaw 借鉴

| 特性 | OpenClaw 做法 | Aleph 方案 | 为什么不照搬 |
|------|--------------|-----------|-------------|
| 角色隔离 | 每个 Agent 独立 workspace 目录 | 逻辑隔离（LanceDB 标签过滤） | 逻辑隔离更灵活，支持跨 workspace 关联 |
| 人格定义 | SOUL.md 文件 | SoulManifest 结构化 + ProfileLayer 叠加 | 结构化便于合并和验证 |
| 工具控制 | JSON5 allow/deny 列表 | ProfileConfig glob 模式 + 双层过滤 | glob 更灵活，双层更安全 |
| 路由 | Binding 规则自动路由 | UI 显式切换（本次） | 先做最可控的方式，Channel 路由后续扩展 |
| 记忆 | 文件系统物理隔离 | LanceDB 向量标签隔离 | 可跨 workspace 智能关联（未来优势） |

## 架构方案：Context 注入模式

### 核心数据结构

```rust
// core/src/gateway/workspace.rs 中新增
pub struct ActiveWorkspace {
    pub workspace_id: String,           // "crypto", "health", "novel"
    pub profile: ProfileConfig,         // 从 WorkspaceManager 加载
    pub memory_filter: WorkspaceFilter, // WorkspaceFilter::Single(workspace_id)
}
```

### 数据流

```
用户通过 UI 切换 workspace
    → workspace.switch RPC
    → WorkspaceManager.set_active(user_id, workspace_id)
    → 广播 workspace.changed 通知

下一次 Agent Loop 执行：
    → ExecutionEngine::execute()
    → 从 WorkspaceManager 加载 ActiveWorkspace
    → 注入到 ThinkerConfig / RunContext
    → Agent Loop 各子系统从 context 读取

记忆系统：
    → memory_store: fact.workspace = active_workspace.workspace_id
    → memory_search: filter.workspace = active_workspace.memory_filter
    → 隐式记忆（压缩/提取）: 同上

Thinker：
    → ProfileLayer (priority 75): 注入 profile.system_prompt 叠加到 Soul
    → ThinkerToolFilter: with_profile(active_workspace.profile)
    → 模型选择: 优先使用 profile.model
    → 温度/token: 优先使用 profile.temperature / profile.max_tokens

Executor：
    → ProfileFilter: 验证工具调用是否在白名单内
```

## 实现细节

### 1. ActiveWorkspace 构建（engine.rs）

在 `ExecutionEngine::execute()` 的 Identity/Profile 加载阶段（~L377）新增：

```rust
// 获取当前活跃 workspace
let active_workspace = {
    let ws = workspace_manager.get_active(&user_id)
        .unwrap_or_else(|| workspace_manager.get("global").unwrap());
    let profile = workspace_manager
        .get_profile(&ws.profile)
        .cloned()
        .unwrap_or_default();
    ActiveWorkspace {
        workspace_id: ws.id.clone(),
        profile,
        memory_filter: WorkspaceFilter::Single(ws.id.clone()),
    }
};
```

### 2. 记忆隔离接通

**存储侧**（写入打标签）：
- `memory_store` / `memory_add` 工具：`fact.workspace = context.active_workspace.workspace_id`
- 当前默认 `"default"` → 改为从 context 读取

**查询侧**（读取过滤）：
- `memory_search` 工具的 `workspace` 参数默认值：从 `"default"` → `context.active_workspace.workspace_id`
- `memory_browse` 同理
- 用户仍可通过显式参数覆盖（为跨 workspace 查询预留口子）

**隐式记忆操作**：
- 对话压缩、事实提取等 Agent Loop 内部的记忆操作，自动使用 `active_workspace.memory_filter`

### 3. Thinker Profile 注入

**新增 ProfileLayer**（core/src/thinker/prompt_builder/layers/）：

```rust
pub struct ProfileLayer;

impl PromptLayer for ProfileLayer {
    fn name(&self) -> &'static str { "profile" }
    fn priority(&self) -> u32 { 75 }  // Soul(50) 之后, Role(100) 之前
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(profile) = input.active_profile() {
            if let Some(prompt) = &profile.system_prompt {
                output.push_str("\n\n## Current Role Context\n");
                output.push_str(prompt);
            }
        }
    }
}
```

**模型/参数覆盖**：
- `Thinker::think()` 中模型选择：`profile.model.unwrap_or(default_model)`
- Provider 调用参数：`profile.temperature.unwrap_or(default_temp)`
- Token 限制：`profile.max_tokens.unwrap_or(default_max_tokens)`

**工具白名单**：
- `ThinkerToolFilter::new().with_profile(Some(active_workspace.profile.clone()))`
- Dispatcher `ProfileFilter` 同理

### 4. workspace.switch RPC

**新增 RPC 方法**（core/src/gateway/handlers/workspace.rs）：

```rust
// Method: workspace.switch
// Params: { "workspace_id": "crypto" }
// Response: { "ok": true, "workspace": { ... } }

async fn handle_workspace_switch(params, state) -> Result<Value> {
    let workspace_id = params.get("workspace_id").as_str()?;

    // 验证 workspace 存在
    let workspace = state.workspace_manager.get(workspace_id)?;

    // 设置活跃 workspace
    state.workspace_manager.set_active(&user_id, workspace_id)?;

    // 广播通知
    state.broadcast(json!({
        "method": "workspace.changed",
        "params": { "workspace_id": workspace_id, "workspace": workspace }
    }));

    Ok(json!({ "ok": true, "workspace": workspace }))
}
```

### 5. Control Plane UI

**TopBar workspace 选择器**：
- 显示当前 workspace 名称 + 图标
- 下拉列表展示所有可用 workspace
- 点击切换调用 `workspace.switch` RPC
- 订阅 `workspace.changed` 通知实时更新

**Workspace 管理页**：
- 创建新 workspace（选择 profile）
- 编辑 workspace（改名、换 profile、归档）
- 查看每个 workspace 的记忆统计

## 配置示例

```toml
# ~/.aleph/config.toml

[profiles.crypto_advisor]
description = "加密货币交易顾问"
model = "claude-sonnet"
tools = ["memory_*", "web_search", "calculator"]
system_prompt = """
你是一位专业的加密货币分析师和交易顾问。
注重风险管理，在给出任何交易建议时必须附带风险提示。
分析时使用技术分析和基本面分析相结合的方法。
"""
temperature = 0.3

[profiles.health_coach]
description = "健康管理顾问"
model = "claude-sonnet"
tools = ["memory_*", "web_search", "calendar"]
system_prompt = """
你是一位关注全面健康的生活顾问。
根据用户的健康档案提供个性化建议。
始终提醒：你不是医生，严重问题请咨询专业医生。
"""
temperature = 0.5

[profiles.novel_writer]
description = "小说创作助手"
model = "claude-opus"
tools = ["memory_*", "web_search", "fs_*"]
system_prompt = """
你是一位经验丰富的小说编辑和共创伙伴。
帮助用户构建世界观、塑造角色、推进情节。
保持一致的文风，记住所有已确定的设定。
"""
temperature = 0.8
max_tokens = 8192
```

## 不做什么（YAGNI）

- ❌ 跨 workspace 记忆检索（技术上支持，但本次不实现 API）
- ❌ Channel → Workspace 自动路由（后续扩展）
- ❌ 配置热重载（后续扩展）
- ❌ Workspace 间知识迁移
- ❌ Per-workspace Soul 替换（始终叠加，不替换）
- ❌ Docker 沙箱隔离（Aleph 不需要，这是 OpenClaw 的多用户场景）

## 修改文件清单

| 文件 | 修改类型 | 说明 |
|------|---------|------|
| `core/src/gateway/workspace.rs` | 修改 | 新增 ActiveWorkspace 结构体 |
| `core/src/gateway/execution_engine/engine.rs` | 修改 | 注入 ActiveWorkspace 到执行路径 |
| `core/src/gateway/handlers/workspace.rs` | 修改 | 新增 workspace.switch RPC |
| `core/src/thinker/prompt_builder/layers/profile.rs` | 新增 | ProfileLayer 实现 |
| `core/src/thinker/prompt_builder/prompt_pipeline.rs` | 修改 | 注册 ProfileLayer |
| `core/src/thinker/prompt_builder/mod.rs` | 修改 | LayerInput 增加 active_profile() |
| `core/src/thinker/mod.rs` | 修改 | 模型/参数覆盖逻辑 |
| `core/src/builtin_tools/memory_search.rs` | 修改 | workspace 默认值从 context 读取 |
| `core/src/builtin_tools/memory_browse.rs` | 修改 | 同上 |
| `core/src/builtin_tools/memory_store.rs` | 修改 | 存储时打 workspace 标签 |
| `core/src/agent_loop/bootstrap.rs` | 修改 | 隐式记忆操作带 workspace |
| `core/ui/control_plane/src/views/settings/` | 修改 | Workspace 管理界面 |
| `core/ui/control_plane/src/components/top_bar.rs` | 修改 | Workspace 选择器 |

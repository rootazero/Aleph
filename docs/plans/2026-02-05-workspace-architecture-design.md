# Workspace Architecture Design

> Anti-Gravity: 通过极致的上下文隔离和分层模型策略，克服长对话带来的 Token 膨胀和成本失控。

## Overview

引入 Workspace 系统，实现 OpenClaw 风格的"反重力"架构。核心思想：

- **Profile** (能力模板): 定义 Physics — 模型绑定、工具白名单、System Prompt
- **Workspace** (工作区实例): 定义 State — 会话历史、缓存状态、环境变量

这是一个 OOP 范式：Profile 是 Class，Workspace 是 Instance。

## Architecture Decisions

### 1. 混合模式 (Profile + Workspace)

**为什么不用纯静态模式？**
- 如果两个项目共用 `coding` Profile，上下文会互相干扰
- Context Cache 会因频繁切换项目而失效

**为什么不用纯动态模式？**
- 每次创建 Workspace 都要重新配置模型、工具，太繁琐

**混合模式的优势：**
- Profile 定义"能力边界"，一次配置，复用无限
- Workspace 定义"数据状态"，每个项目独立隔离

### 2. SessionKey 整合方式

**决策：Workspace 作为 SessionKey 的上层抽象**

利用现有 `SessionKey::Main` 的 `main_key` 字段实现物理隔离：

| Workspace | SessionKey | Storage Key |
|-----------|------------|-------------|
| Global | `Main { main_key: "main" }` | `agent:main:main` |
| coding | `Main { main_key: "ws-coding" }` | `agent:main:ws-coding` |
| project-x | `Main { main_key: "ws-project-x" }` | `agent:main:ws-project-x` |

**优势：**
- 零 Schema 变更
- 现有序列化逻辑完全兼容
- 天然物理隔离（不同 key = 不同数据库记录）
- 全渠道漫游（CLI/Telegram/Discord 共享 Workspace）

### 3. Context Caching 策略

**决策：Provider-Specific 适配器**

不同 Provider 的缓存机制差异巨大，必须分别适配：

```rust
pub trait ProviderCacheStrategy {
    fn should_cache(&self, messages: &[Message]) -> bool;
    fn inject_cache_markers(&self, request: &mut LlmRequest);
    async fn manage_cache_lifecycle(&self, context: &str) -> Option<String>;
}
```

| Provider | 模式 | 实现策略 |
|----------|------|----------|
| Anthropic | Ephemeral (Stateless) | Just-in-Time 注入 `cache_control` block |
| Gemini | Persistent (Stateful) | 显式创建 cache，存储 `cache_name` |
| OpenAI | Transparent | 依赖自动缓存，无需管理 |

```rust
pub enum CacheState {
    Ephemeral { cache_breakpoint_index: Option<usize> },
    Persistent { cache_name: String, content_hash: String, expires_at: DateTime<Utc> },
    Transparent,
}
```

### 4. 工具过滤策略

**决策：双层过滤 (Defense in Depth)**

- **Thinker 层 (The Lens)**: 过滤可见工具 schema → 减少 Token 消耗
- **Dispatcher 层 (The Gatekeeper)**: 执行时校验 → 安全兜底

```
User Input → Thinker (只看到允许的工具) → LLM → Dispatcher (二次校验) → Tool
```

## Data Structures

### Profile (静态配置)

```toml
[profiles.coding]
description = "Rust/Python 开发环境"
model = "claude-3-5-sonnet"
tools = ["git_*", "fs_*", "terminal"]
system_prompt = "You are a senior engineer..."
temperature = 0.2

[profiles.creative]
description = "小说创作与头脑风暴"
model = "gemini-1.5-pro"
tools = ["search", "fs_read"]
system_prompt = "You are a creative writer..."
temperature = 0.9
```

### Workspace (运行时状态)

```rust
pub struct Workspace {
    pub id: String,              // "project-aleph"
    pub profile: String,         // "coding"
    pub created_at: DateTime<Utc>,
    pub session_key: SessionKey, // Main { main_key: "ws-project-aleph" }
    pub cache_state: CacheState,
    pub env_vars: HashMap<String, String>,
}

pub struct UserActiveWorkspace {
    pub user_id: String,
    pub current_workspace: String,
}
```

## User Interaction

```
# 创建新工作区
Aleph > /new aleph-core type=coding
Created workspace 'aleph-core' using 'coding' profile.
• Model locked to Claude-3.5.
• Context is empty.
• Tools restricted to coding.

# 切换工作区
Aleph > /switch aleph-core
Switched to 'aleph-core'. Model: Claude-3.5-Sonnet.

# 日常使用 (Global)
Aleph > /weather 今天天气如何？
(使用 default profile)
```

## Implementation Phases

### Phase 1: Config System
- [ ] 定义 `ProfileConfig` 结构体
- [ ] 添加 `profiles: HashMap<String, ProfileConfig>` 到 Config
- [ ] 实现 glob 匹配工具名的逻辑
- [ ] 单元测试

### Phase 2: Workspace Manager
- [ ] 定义 `Workspace` 和 `WorkspaceManager`
- [ ] SQLite schema for workspaces
- [ ] 实现 `/new` 和 `/switch` 命令
- [ ] 路由逻辑变更

### Phase 3: Tool Filtering
- [ ] Thinker 层工具过滤
- [ ] Dispatcher 层安全校验
- [ ] 集成测试

### Phase 4: Context Caching
- [ ] `ProviderCacheStrategy` trait
- [ ] Anthropic adapter (ephemeral)
- [ ] Gemini adapter (persistent)
- [ ] Cache lifecycle management

## File Changes (Estimated)

| File | Change |
|------|--------|
| `core/src/config/types/profile.rs` | New: ProfileConfig |
| `core/src/config/structs.rs` | Add profiles field |
| `core/src/session/workspace.rs` | New: Workspace, WorkspaceManager |
| `core/src/session/mod.rs` | Export workspace module |
| `core/src/gateway/handlers/` | New: workspace handlers |
| `core/src/thinker/cache.rs` | New: ProviderCacheStrategy |
| `core/src/thinker/model_router.rs` | Integrate profile model binding |
| `core/src/dispatcher/tool_filter.rs` | Add workspace-aware filtering |

## Summary

Workspace 架构将 Aleph 从"聪明的聊天机器人"升级为"精密的多任务操作系统"：

1. **隔离**: 不同 Workspace = 不同 Memory Bucket
2. **绑定**: Profile 层级锁定 Model
3. **缓存**: Cache ID 绑定在 Workspace，复用率极高
4. **工具**: Profile 定义白名单，双层过滤

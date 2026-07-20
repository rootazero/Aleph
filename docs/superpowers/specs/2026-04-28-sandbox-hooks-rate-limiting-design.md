# SandboxHooks 限流设计

> 日期：2026-04-28
> 状态：已批准

## 概述

为 `SandboxHooks` 添加可选的限流功能，作为 sandbox 执行的前置过滤器。限流与工具策略（权限）是两个正交的维度：

- **工具策略**：权限维度 — 决定工具是否能被调用（yes/no/ask）
- **SandboxHooks 限流**：频率维度 — 决定工具能被调用的速度

两者独立判断，互不干扰。

## 设计目标

1. **安全优先**：防止失控的 agent 或外部恶意请求导致资源耗尽
2. **零侵入**：默认 `SandboxHooks::new()` 返回空 hooks，现有行为不变
3. **可观测**：限流事件输出到 tracing，提供可观测性
4. **可调节**：所有参数通过 panel 可视化配置

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     SandboxHooks                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐  │
│  │ RateLimitHook│  │ AuditHook   │  │ (future hooks)  │  │
│  └──────┬──────┘  └──────┬──────┘  └─────────────────┘  │
│         │                │                               │
│         ▼                ▼                               │
│  ┌──────────────────────────────────────────────────┐   │
│  │   before() → Allow / Deny                        │   │
│  │   after()  → Audit log                          │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 组件

| 组件 | 职责 |
|------|------|
| `RateLimitHook` | 实现 `SandboxBeforeHook`，在执行前检查速率 |
| `SandboxRateLimitConfig` | 限流配置结构，包含工具分类和参数 |
| `SandboxRateLimiter` | 复用现有滑动窗口算法，按 session_id + tool_name 计数 |

## 工具分类

限流按工具的危险等级分类：

| 类别 | 工具示例 | 默认限制 |
|------|----------|----------|
| `read` | search, memory retrieval | 60/min, burst 20 |
| `write` | file_write, bash_exec | 30/min, burst 10 |
| `dangerous` | code_exec, exec 类 | 10/min, burst 5 |
| `admin` | config.patch, plugins.install | 5/min, burst 2 |

## 配置结构

```rust
// src/sandbox/rate_limit.rs

/// 限流配置
#[derive(Clone, Debug)]
pub struct SandboxRateLimitConfig {
    /// 总开关（默认 true）
    pub enabled: bool,
    /// 豁免 loopback（默认 true）
    pub exempt_loopback: bool,
    /// 每类工具的速率配置
    pub per_tool_category: HashMap<ToolCategory, WindowConfig>,
    /// 全局 dangerous 工具上限（所有 session 共享）
    pub global_dangerous_limit: Option<WindowConfig>,
}

/// 单个滑动窗口配置
#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub max_requests: u32,    // 窗口内最大请求数
    pub window_secs: u64,     // 窗口时长（秒）
    pub burst_allow: u32,    // 突发配额（额外允许）
}

/// 工具分类
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum ToolCategory {
    Read,
    Write,
    Dangerous,
    Admin,
}
```

## Panel 配置项

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `sandbox.rate_limit.enabled` | bool | true | 总开关 |
| `sandbox.rate_limit.exempt_loopback` | bool | true | loopback 豁免 |
| `sandbox.rate_limit.categories.read.max_requests` | u32 | 60 | read 类每分钟上限 |
| `sandbox.rate_limit.categories.read.burst_allow` | u32 | 20 | read 类突发配额 |
| `sandbox.rate_limit.categories.write.max_requests` | u32 | 30 | write 类每分钟上限 |
| `sandbox.rate_limit.categories.write.burst_allow` | u32 | 10 | write 类突发配额 |
| `sandbox.rate_limit.categories.dangerous.max_requests` | u32 | 10 | dangerous 类每分钟上限 |
| `sandbox.rate_limit.categories.dangerous.burst_allow` | u32 | 5 | dangerous 类突发配额 |
| `sandbox.rate_limit.categories.admin.max_requests` | u32 | 5 | admin 类每分钟上限 |
| `sandbox.rate_limit.categories.admin.burst_allow` | u32 | 2 | admin 类突发配额 |

## 判断流程

```
before(ctx: SandboxHookContext) → SandboxHookResult
│
├─ 1. 检查 enabled == false？ → Allow（跳过）
├─ 2. 检查 exempt_loopback + loopback？ → Allow（跳过）
├─ 3. 查找工具分类（tool_name → ToolCategory）
├─ 4. 获取 WindowConfig
├─ 5. 计算 key = (session_id, tool_category)
├─ 6. 检查 sliding window 计数
│   ├─ 未超限 → Allow，记录 timestamp
│   └─ 超限 → Deny { reason: "rate limit exceeded for {tool_name}" }
└─ 7. 可选：全局 dangerous 计数（所有 session 共享）
```

## 与工具策略的关系

```
┌─────────────────────────────────────────────────────────┐
│                 ToolPolicyLayer (权限)                 │
│  classification = Allow / Ask / Deny                   │
└──────────────────────┬──────────────────────────────┘
                       │ Deny → 直接拒绝，不经过限流
                       │ Allow/Ask → 继续
                       ▼
┌─────────────────────────────────────────────────────────┐
│              SandboxHooks (频率)                       │
│  before(): RateLimitHook → Allow / Deny               │
└─────────────────────────────────────────────────────────┘
```

- 工具策略 Deny → 直接拒绝，限流不触发
- 工具策略 Allow/Ask → 过限流检查
- 两者独立，互不干扰

## 错误处理

限流触发时返回：

```rust
SandboxHookResult::Deny {
    reason: "rate limit exceeded for {tool_name}: {current}/{max} in {window}s window"
}
```

after hook 记录日志：

```rust
tracing::warn!(
    target: "sandbox_rate_limit",
    session_id = %ctx.session_id,
    tool_name = ctx.tool_name,
    category = ?category,
    "sandbox rate limit exceeded"
);
```

## 默认值（安全优先）

| 类别 | max_requests | window_secs | burst_allow |
|------|-------------|-------------|-------------|
| read | 60 | 60 | 20 |
| write | 30 | 60 | 10 |
| dangerous | 10 | 60 | 5 |
| admin | 5 | 60 | 2 |

## 实现计划

1. 新增 `src/sandbox/rate_limit.rs` — 配置结构和限流逻辑
2. 实现 `RateLimitHook` — 实现 `SandboxBeforeHook`
3. 修改 `SandboxHooks::new()` — 支持从配置构建
4. 修改 `build_sandbox` — 将限流 hook 注入
5. Panel 配置项 — 对应上述配置表
6. 测试 — 覆盖各分类、超限、豁免场景

## 参考

- 现有 `RateLimiter` 实现：`src/gateway/rate_limiter.rs`
- 现有 `SandboxHooks` 实现：`src/sandbox/hooks.rs`
- Sandbox 文档：`docs/reference/SANDBOX.md`

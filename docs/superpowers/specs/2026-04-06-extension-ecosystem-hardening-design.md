# Extension Ecosystem Hardening Design

**Date**: 2026-04-06  
**Status**: Draft  
**Scope**: MCP instructions pipeline, Hook permission semantics, Tool pipeline observability  

## Background

对比 Claude Code 源码中 Skills / Plugins / Hooks / MCP 生态的实现深度，识别出 Aleph 当前扩展生态中三个断裂/缺失点。Aleph 的扩展系统整体成熟（Plugin、Skill、Hook、MCP 均已 production-ready），但存在以下精准差距：

1. **MCP instructions 数据管道断裂** — Prompt 注入层已就绪，但协议层未提取 server instructions
2. **Hook 权限语义不完整** — 有 block/deny，缺 allow/ask，且未与 SafetyGuard 统一决策
3. **Tool pipeline 可观测性缺失** — duration_ms 始终为 0，缺少结构化耗时追踪

## Non-Goals

- 不重构 ToolPipeline 为 trait-based middleware chain（YAGNI，P6 简洁性原则）
- 不引入 metrics crate（tracing 已够用）
- 不删除旧 hook 字段（保持向后兼容）
- 不照搬 Claude Code 实现（充分融合 Aleph 架构思想）

---

## Section 1: MCP Instructions 数据管道补线

### Problem

`McpInstructionsLayer`（`src/thinker/layers/mcp_instructions.rs`）已能将 MCP server instructions 注入 system prompt。但数据源端断裂：

- `InitializeResult`（`src/mcp/protocol.rs:41`）不含 `instructions` 字段
- `McpServerConnection::initialize()`（`src/mcp/external/connection.rs:157`）不提取 instructions
- `McpClient` 无方法收集已连接 server 的 instructions

### Changes

#### 1.1 Protocol — `src/mcp/protocol.rs`

`InitializeResult` 增加 `instructions` 字段：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
    /// Server-provided instructions describing how to use its tools.
    /// Injected into system prompt via McpInstructionsLayer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}
```

#### 1.2 Connection — `src/mcp/external/connection.rs`

`McpServerConnection` 新增字段和方法：

```rust
/// Cached instructions from server initialize response.
cached_instructions: RwLock<Option<String>>,
```

在 `initialize()` 中，`init_result` 解析后提取 instructions：

```rust
{
    let mut inst = self.cached_instructions.write().await;
    *inst = init_result.instructions.clone();
}
```

暴露读取方法：

```rust
/// Get server-provided instructions (if any).
pub async fn instructions(&self) -> Option<String> {
    self.cached_instructions.read().await.clone()
}
```

#### 1.3 Client — `src/mcp/client.rs`

`McpClient` 新增收集方法：

```rust
/// Collect instructions from all connected MCP servers.
/// Returns (server_name, instructions) pairs for prompt injection.
pub async fn collect_instructions(&self) -> Vec<McpServerInstruction> {
    let mut result = Vec::new();
    for connection in self.active_connections() {
        if let Some(inst) = connection.instructions().await {
            result.push(McpServerInstruction {
                server_name: connection.name().to_string(),
                instructions: inst,
            });
        }
    }
    result
}
```

#### 1.4 Prompt Assembly — 调用侧

在 `src/thinker/prompt_builder/mod.rs` 的 `LayerInput` 构建处（如 `build_system_prompt()` 等方法），调用 `mcp_client.collect_instructions()` 并链式调用 `.with_mcp_instructions(&instructions)`。当前该方法仅在测试中使用，生产代码未接入。`McpInstructionsLayer` 已有完整的注入逻辑，无需修改。

需要确认 `PromptBuilder` 是否持有 `McpClient` 引用。如果没有，需要通过 `PromptConfig` 或方法参数传入 `Vec<McpServerInstruction>`。

### Cleanup

无旧代码需要删除，纯增量修改。

---

## Section 2: Hook Permission 三态语义

### Problem

当前 `HookResult`（`src/extension/hooks/mod.rs:144`）有：

- `blocked: bool` + `block_reason` — 临时拦截，retryable
- `denied: bool` + `deny_reason` — 策略拒绝，not retryable

缺少：

- `allow` — hook 声明工具调用安全，可跳过 SafetyGuard 的 blocked-pattern 检查
- `ask` — hook 要求强制用户确认，即使 SafetyGuard 认为安全

Claude Code 的 `resolveHookPermissionDecision()` 将 hook 权限决策严格嵌入整体权限模型，hook allow 不绕过 settings deny。

### Changes

#### 2.1 新增 `PermissionDecision` 枚举 — `src/extension/hooks/mod.rs`

```rust
/// Hook-emitted permission decision.
///
/// Follows the principle that hook `Allow` does NOT bypass settings-level
/// deny rules — it only skips SafetyGuard blocked-pattern checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Hook vouches for safety — skip SafetyGuard blocked-pattern check,
    /// but NOT settings-level deny rules.
    Allow,
    /// Force user confirmation before execution.
    Ask { reason: String },
    /// Temporary interception — retryable (maps to legacy `blocked`).
    Block { reason: String },
    /// Hard policy deny — not retryable (maps to legacy `denied`).
    Deny { reason: String },
}
```

#### 2.2 `HookResult` 演进

```rust
pub struct HookResult {
    // --- Legacy fields (preserved for backward compatibility) ---
    // Deprecated: use permission_decision instead
    pub blocked: bool,
    pub block_reason: Option<String>,
    pub denied: bool,
    pub deny_reason: Option<String>,

    // --- New unified field ---
    /// Hook-emitted permission decision. Last writer wins across interceptor chain.
    pub permission_decision: Option<PermissionDecision>,

    // ... other fields unchanged ...
}
```

#### 2.3 Command Output Protocol 扩展 — `parse_command_output()`

新增协议行，同时保持旧协议向后兼容：

```
allow                          → PermissionDecision::Allow
ask: <reason>                  → PermissionDecision::Ask { reason }
block: <reason>                → blocked=true + PermissionDecision::Block { reason }
deny: <reason>                 → denied=true + PermissionDecision::Deny { reason }
```

`block:` 和 `deny:` 同时设置旧字段和新字段，确保旧 hook 脚本不受影响。

#### 2.4 Tool Pipeline Stage 3-4 决策整合 — `tool_pipeline.rs`

Stage 3（pre-hooks）产出 `permission_decision` 后，Stage 4（safety check）使用统一决策逻辑：

```rust
// Resolve permission decision (new field takes precedence over legacy)
let decision = interceptor_result.permission_decision.clone()
    .or_else(|| {
        if interceptor_result.denied {
            Some(PermissionDecision::Deny {
                reason: interceptor_result.deny_reason.clone().unwrap_or_default(),
            })
        } else if interceptor_result.blocked {
            Some(PermissionDecision::Block {
                reason: interceptor_result.block_reason.clone().unwrap_or_default(),
            })
        } else {
            None
        }
    });

match decision {
    Some(PermissionDecision::Deny { reason }) => return denied_outcome(id, name, reason),
    Some(PermissionDecision::Block { reason }) => return blocked_outcome(id, name, reason),
    Some(PermissionDecision::Ask { reason }) => {
        needs_user_confirmation = true;
        confirmation_reason = Some(reason);
    }
    Some(PermissionDecision::Allow) => {
        skip_safety_patterns = true;
    }
    None => { /* proceed with full SafetyGuard check */ }
}

// Stage 4: Safety check
if !skip_safety_patterns {
    if let Err(e) = self.safety.check(&safety_call) { ... }
} else {
    // Allow skips blocked-pattern check, but settings-level deny is
    // enforced inside SafetyGuard separately (if implemented).
    // Currently SafetyGuard.check() is a single call — this is a
    // targeted bypass of the pattern-match portion only.
    if let Err(e) = self.safety.check_settings_only(&safety_call) { ... }
}
```

> **Note**: `SafetyGuard::check()` 已天然分为两段：blocked-pattern 匹配 → permission lookup（`src/agent_loop/safety.rs:139-172`）。新增 `check_permissions_only(&self, call: &ToolCall) -> Result<(), SafetyError>` 方法，只执行 permission lookup 部分（跳过 blocked patterns 循环），代码复用已有的 `tool_permissions` 查询逻辑。

#### 2.5 `PipelineOutcome` 扩展

```rust
pub struct PipelineOutcome {
    pub outcome: ToolOutcome,
    pub additional_contexts: Vec<String>,
    pub prevent_continuation: bool,
    pub hook_messages: Vec<String>,
    /// If true, execution was paused pending user confirmation.
    pub needs_user_confirmation: bool,
    /// Reason for requiring confirmation (from hook Ask decision).
    pub confirmation_reason: Option<String>,
}
```

### Cleanup

- 旧字段 `blocked`/`block_reason`/`denied`/`deny_reason` 标记 deprecated 注释，不删除
- Pipeline 内部决策统一为 `match permission_decision`，legacy fallback 作为兜底

---

## Section 3: Tool Pipeline 可观测性

### Problem

`ToolOutcome::duration_ms` 始终为 0。Pipeline 有 `tracing::instrument` 和 `debug_span`，但缺少实际计时。

### Changes

#### 3.1 Stage 5 执行计时 — `tool_pipeline.rs`

```rust
// Stage 5: Execute tool with cancellation
let exec_start = std::time::Instant::now();
let result = tokio::select! {
    r = registry.execute(name, effective_args.clone()) => r,
    _ = cancel.cancelled() => { ... }
};
let elapsed_ms = exec_start.elapsed().as_millis() as u64;

let mut outcome = Self::map_result(id, name, &result);
outcome.duration_ms = elapsed_ms;
```

#### 3.2 Hook 耗时 tracing

Stage 3（pre-hooks）和 Stage 6/7（post-hooks）添加 info 级别的耗时记录：

```rust
let hook_start = std::time::Instant::now();
let (ctx_after, interceptor_result) = self.hooks
    .execute_interceptors(HookEvent::BeforeToolCall, base_ctx.clone())
    .await?;
tracing::info!(
    tool = name,
    elapsed_ms = hook_start.elapsed().as_millis() as u64,
    "pre-hooks completed"
);
```

#### 3.3 Span 级别统一

将关键阶段的 `debug_span` 升级为 `info_span`：
- `pipeline_validate` → `info_span`
- `pipeline_safety` → `info_span`

调试阶段（observer 执行）保持 `debug_span`。

### Cleanup

- 移除多余的 `drop(_span2)` / `drop(_span4)` — 改用 block scope `{ }` 自然 drop，更 idiomatic

---

## Implementation Order

按依赖关系和风险排序：

| Phase | Scope | Risk | Files |
|-------|-------|------|-------|
| 1 | MCP instructions 补线 | Low | protocol.rs, connection.rs, client.rs, prompt 调用侧 |
| 2 | Pipeline 可观测性 | Low | tool_pipeline.rs |
| 3 | Hook PermissionDecision | Medium | hooks/mod.rs, tool_pipeline.rs, safety.rs |
| 4 | 清理与统一 | Low | tool_pipeline.rs (决策逻辑统一) |

Phase 1 和 2 可并行。Phase 3 依赖 Phase 2（在同一文件修改）。Phase 4 是 Phase 3 的收尾。

## Testing Strategy

- **Phase 1**: 单元测试 `InitializeResult` 反序列化含 `instructions` 字段；集成测试 `collect_instructions()` 返回正确数据
- **Phase 2**: 验证 `duration_ms` 非零；tracing 输出包含 elapsed_ms
- **Phase 3**: 
  - `parse_command_output()` 测试新协议行 `allow`、`ask:`
  - Pipeline 测试 `PermissionDecision::Allow` 跳过 pattern check
  - Pipeline 测试 `PermissionDecision::Ask` 设置 `needs_user_confirmation`
  - 向后兼容测试：旧 `block:`/`deny:` 协议行行为不变
- **Phase 4**: 确认所有现有测试通过

## Affected Files

| File | Change Type |
|------|-------------|
| `src/mcp/protocol.rs` | Add field |
| `src/mcp/external/connection.rs` | Add field + method |
| `src/mcp/client.rs` | Add method |
| `src/extension/hooks/mod.rs` | Add enum + extend HookResult + extend parse_command_output |
| `src/agent_loop/tool_pipeline.rs` | Add timing + permission decision logic + PipelineOutcome fields |
| `src/agent_loop/safety.rs` | Add `check_permissions_only()` method |
| `src/thinker/prompt_builder/mod.rs` | Wire `with_mcp_instructions()` into LayerInput construction |

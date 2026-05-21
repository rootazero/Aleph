# Tool Pipeline Enhancement Design

> 参考 Claude Code 工具执行链（权限、Hook、工具执行链），对 Aleph 的 ToolPipeline 进行针对性优化。
> 原则：不照搬，融合 Aleph 架构思想（R8 LLM 主权、P6 简洁性、P7 防御性设计）。

## 背景

Claude Code 的工具执行是一条 14-step runtime pipeline（schema 校验 → hooks → 权限 → 执行 → analytics → post-hooks）。
Aleph 当前有 6-stage pipeline（HookContext → Pre-hooks → Safety → Execute → Post-hooks → Failure hooks），已覆盖核心能力，但存在以下差距：

1. **输入校验时机错误** — schema 校验在 tool.call() 内部，hooks 可能处理无效输入
2. **Hook 缺少 deny 语义** — 只能 block（拦截），无法表达策略性拒绝（不可重试）
3. **Post-hook 无法修改输出** — 无法做 secret masking、输出规范化
4. **Pipeline 无结构化 tracing** — 调试时无法追踪各 stage 耗时和状态
5. **死代码** — `run_from_safety()` 和 `InterceptorResult` 未被使用

## 设计决策

### 不引入的能力（YAGNI）

| 能力 | 理由 |
|------|------|
| Hook allow/ask 权限语义 | 违反"权限只能收紧不能放松"原则（P7），引入 hook↔SafetyGuard 双向耦合 |
| OTel/analytics 集成 | 超出自托管 AI 助手需求（R3 核心轻量化） |
| Speculative classifier | ExecSecurityGate 已覆盖 shell 命令风险评估 |
| jsonschema crate | 过重；tool schema 普遍简单，轻量检查够用 |

## 改动 1: Input Schema Validation 前置

### 问题

LLM 生成无效参数时，hooks 先处理垃圾输入，浪费时间且可能误判。

### 方案

Pipeline 从 6-stage 扩展为 **7-stage**，在 pre-hooks 之前插入 schema fast-fail：

```
Stage 1: Build HookContext           (不变)
Stage 2: Input schema validation     (新增)
Stage 3: Pre-hooks (interceptors)    (原 stage 2)
Stage 4: Safety check                (原 stage 3)
Stage 5: Execute tool                (原 stage 4)
Stage 6: Post-hooks (observers)      (原 stage 5)
Stage 7: Failure hooks               (原 stage 6)
```

### 实现

在 `ToolPipeline::execute()` 中，stage 2 调用 `registry.get(name)` 获取 tool schema，做轻量校验：

```rust
fn validate_input_fast(schema: &Value, input: &Value) -> Result<(), String> {
    if !input.is_object() {
        return Err("expected JSON object".into());
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        let obj = input.as_object().unwrap();
        for field in required {
            if let Some(name) = field.as_str() {
                if !obj.contains_key(name) {
                    return Err(format!("missing required field: {name}"));
                }
            }
        }
    }
    Ok(())
}
```

- Schema 为 `{"type": "object"}` 且无 required → 跳过（零开销）
- 校验失败 → 直接返回 error outcome，不进入 hooks
- 真正的完整校验仍在 tool.call() 内 serde 反序列化时发生

### 涉及文件

- `src/agent_loop/tool_pipeline.rs` — 新增 `validate_input_fast()`, 修改 `execute()` 插入 stage 2

## 改动 2: Hook Deny 语义

### 问题

`block:` 语义模糊 — 不区分临时拦截（可重试）和策略拒绝（不可重试）。

### 方案

在 `parse_command_output()` 新增 `deny:` 前缀：

| 前缀 | 语义 | retryable |
|------|------|-----------|
| `block: <reason>` | 拦截（现有） | true |
| `deny: <reason>` | 策略拒绝（新增） | false |

### HookResult 扩展

```rust
pub struct HookResult {
    // ... 现有字段 ...
    pub denied: bool,
    pub deny_reason: Option<String>,
}
```

### Pipeline 变更

Stage 3 (pre-hooks) 检查顺序：
1. `denied == true` → 返回 `[HOOK_DENIED]` error, `retryable=false`
2. `blocked == true` → 返回 `[HOOK_BLOCKED]` error, `retryable=true`（现有行为）

### 涉及文件

- `src/extension/hooks/mod.rs` — `HookResult` 新增字段, `parse_command_output()` 新增 deny 解析
- `src/agent_loop/tool_pipeline.rs` — stage 3 新增 denied 检查

## 改动 3: Post-hook 输出修改

### 问题

Post-hooks 只能注入 additional_contexts，无法修改 tool output（无法做 secret masking 等后处理）。

### 方案

在 `parse_command_output()` 新增 `update_output:` 前缀：

```
update_output: <text>  → 替换 tool output_text
```

### HookResult 扩展

```rust
pub struct HookResult {
    // ... 现有字段 ...
    pub updated_output: Option<String>,
}
```

### Pipeline 变更

Stage 6 (post-hooks) 执行后：

```rust
if let Some(new_output) = post_result.updated_output {
    outcome.output_text = new_output;
}
```

- Last-writer-wins（与 `update_input` 一致）
- 仅 `AfterToolCall` / `AfterToolCallFailure` 事件生效
- `BeforeToolCall` 阶段返回此前缀会被忽略

### 涉及文件

- `src/extension/hooks/mod.rs` — `HookResult` 新增字段, `parse_command_output()` 新增解析
- `src/agent_loop/tool_pipeline.rs` — stage 6 应用 updated_output

## 改动 4: Pipeline Tracing Spans

### 问题

仅 hook 错误时有 `tracing::warn`，无结构化 stage 级 span。

### 方案

为 `execute()` 添加方法级 `#[instrument]`，为各 stage 添加 `debug_span!`：

```rust
#[tracing::instrument(
    name = "tool_pipeline",
    skip(self, registry, cancel),
    fields(tool_name = %name, tool_id = %id)
)]
pub async fn execute(...) -> PipelineOutcome {
    // 各 stage 内: debug_span! + debug! 事件
}
```

- `debug_span!` — 生产默认不输出，零噪音
- `skip(self, registry, cancel)` — 不序列化大对象
- 不记录 arguments（可能含敏感数据）
- Stage 结果通过 `tracing::debug!` 记录

### 涉及文件

- `src/agent_loop/tool_pipeline.rs` — 添加 instrument 和 span

## 改动 5: 清理死代码

### 删除项

| 代码 | 位置 | 原因 |
|------|------|------|
| `run_from_safety()` | `tool_pipeline.rs:349-445` | 重复 stages 3-6 逻辑，零调用 |
| `InterceptorResult` | `hooks/mod.rs:194-242` | pipeline 使用 `HookResult`，此 struct 零使用 |

### 涉及文件

- `src/agent_loop/tool_pipeline.rs` — 删除 `run_from_safety()`
- `src/extension/hooks/mod.rs` — 删除 `InterceptorResult` struct 及其 impl

## 变更汇总

| 文件 | 改动类型 |
|------|---------|
| `src/agent_loop/tool_pipeline.rs` | 新增 stage 2, deny 检查, output 修改, tracing, 删除 run_from_safety |
| `src/extension/hooks/mod.rs` | HookResult 扩展 (denied, deny_reason, updated_output), parse_command_output 扩展, 删除 InterceptorResult |

**零新依赖。两个文件改动。**

## 测试策略

现有测试（tool_pipeline.rs 和 hooks/mod.rs 中的 `#[cfg(test)]`）覆盖了 pipeline 各 stage 和 hook 解析。新增测试：

1. **schema validation** — 缺少 required 字段时 fast-fail, 无 required 时通过
2. **deny 语义** — hook 返回 `deny:` 时 pipeline 产生不可重试 error
3. **update_output** — post-hook 修改 output_text, BeforeToolCall 阶段忽略
4. **tracing** — 用 `tracing_test` 验证 span 被创建（可选）

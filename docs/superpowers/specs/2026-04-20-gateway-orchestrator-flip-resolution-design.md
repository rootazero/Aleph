# Gateway → Orchestrator 切换设计规范

**日期：** 2026-04-20
**状态：** 已审批（A/A/A 方案），待实施
**前置文档：**
- 缺口目录：`docs/superpowers/specs/2026-04-20-gateway-orchestrator-flip-design.md`
- Phase 6 清理设计：`docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
- Builder 审计结论：`docs/reference/PHASE_6B_BUILDER_AUDIT.md`
- 被取代的计划章节：Phase 6b plan Task 12/13/14

## 目的

本文档是上一份"缺口目录"三个设计问题（按请求工具范围、FlowStreamEvent 词汇表、ExecutionEngine 编排器字段）的正式决议。决议定稿后驱动一份新的实施计划，替换 Phase 6b plan 中被标记为 DEFERRED 的 Task 12–14。

Phase 6b Tasks 1–11 已落地（commit `3fedb1281`），库测试 9133 通过、2 个历史失败不动。本次工作在这一基线之上继续。

## 非目标

- 不删除 `src/agent_loop/loop_core.rs` 或其 sibling —— 那属于 Phase 6c。
- 不引入"v2 双路径 + feature flag"的切换模式 —— 决议即落地。
- 不扩张 `FlowOutcome` 字段超出本规范，除非能指向一个今日 `LoopRunResult` 消费方真实依赖的回归（例如 gateway 在 i18n `ErrLoopExhausted` 分支读 `hit_limit`）。
- 不重写 `src/agent_loop/loop_core.rs` 内的测试（那里还有大量 `AgentLoop::new` 引用，属于 Phase 6c 清理范围）。

## 设计决议总览

| 缺口 | 决议 |
|------|------|
| 1. 按请求工具范围 | `FlowRequest` 新增 `tool_service: Option<Arc<dyn ToolService>>`；由 Gateway 构建、Orchestrator 透传、`AgentHarnessRunner` 覆盖 `HarnessDeps.tools` |
| 2. FlowStreamEvent 词汇表 | 扩展到用户可见 UX 事件集；内部观测事件不进 stream，改走 `HarnessDeps` 内的 TraceSink |
| 3. `Arc<Orchestrator>` 归属 | `ExecutionEngine` 新增 `orchestrator: Arc<Orchestrator>` 字段，在 `start/mod.rs` 完成 session/tool/sandbox 装配后构造 |

## §1. FlowStreamEvent 词汇表（Gap 2）

### 现状

```rust
pub enum FlowStreamEvent {
    Delta(String),
    ToolCall { name: String },
    Complete,
}
```

Gateway 的 `StreamCallback` 实现 `LoopCallback`，消费 ~10 个事件。其中 Gateway emitter 真正转发到用户客户端的，是下面这组。

### 目标形态

```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FlowStreamEvent {
    /// 增量文本（保持）。
    Delta(String),

    /// 思考/推理片段。provider 返回 thinking 内容时发射。
    Reasoning(String),

    /// 工具调用开始。`id` 唯一标识一次调用，用于后续 Done/Summary 配对。
    ToolCallStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },

    /// 工具调用结束。`result` 与 `error` 互斥；result 为 Ok 时 error 为 None。
    ToolCallDone {
        id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
    },

    /// 工具调用的一行摘要（异步由 LLM 生成；失败静默跳过）。
    ToolSummary {
        id: String,
        text: String,
    },

    /// 安全闸拦截。`reason` 供 i18n 层格式化。
    SafetyBlock { reason: String },

    /// stop hook 阻断。harness 已强制再转一轮让模型响应。
    StopHookBlock { reason: String },

    /// 模型回退（provider 原生模型不可用、已切备用）。
    ModelFallback {
        reason: String,
        fallback_model: String,
    },

    /// 终止事件 —— 携带完整 FlowOutcome。Complete 永远是最后一个事件。
    Complete(FlowOutcome),
}
```

### 迁移

- `ToolCall { name }` → `ToolCallStart { id, name, args }` —— `id` 由 `AgentHarness` 分配（对应现有 `ToolCallStartEvent.id`）。
- `Complete` → `Complete(FlowOutcome)` —— 调用方（Gateway drain 任务）从这条消息取 `hit_limit` / `total_tokens` / `final_text`，不再需要单独 await `FlowHandle.completion`。`completion` oneshot 仍然保留，用于区分"流正常结束"与"harness panic/取消"。

### 观测事件（不进 stream）

`on_trace`、`on_text`（整段非增量）、`on_intermediate_text`、`on_confirmation_needed` 等内部事件不走 `FlowStreamEvent`。改为：

- `HarnessDeps` 新增 `pub trace_sink: Option<Arc<dyn TraceSink>>` 字段。
- 定义新 trait：

  ```rust
  pub trait TraceSink: Send + Sync {
      fn on_trace(&self, event: &LoopTraceEvent);
      fn on_confirmation_needed(&self, prompt: &str) -> ConfirmationVerdict;
      fn flush(&self);  // run 结束时调用，对应原 flush_trace_persistence
  }
  ```
- Gateway 在构建 `HarnessDeps` 时注入具体实现（`GatewayTraceSink`），包装原 `callback_state` + emitter 的 trace 持久化路径。

### FlowOutcome 保持

`FlowOutcome` 已有 `final_text / iterations / tool_calls_made / total_tokens / hit_limit`，与现有 `LoopRunResult` 字段一一对应。本次**不扩字段**；`total_tokens` 仍由 provider usage surfacing 后续单独解决，不阻塞切换。

## §2. 按请求工具范围（Gap 1）

### 现状

`run_loop.rs:465–508` 按请求构建 `LoopToolRegistry`，内容包括：
1. 从全局 `self.tool_registry` 过滤出 `agent.config().allowed_tools`；
2. 绑定 `default_working_dir` 与 `request.metadata`；
3. 注入 `SubagentTool`，携带 `run_chain / teammate_manager / message_router / inbox / background_tracker`；
4. 由 `ExtensionToolRefreshSource` 在 turn 之间动态追加 MCP 工具。

`AgentHarnessRunner.tool_service` 今天是全局单例，无法表达以上任何一点。`FlowSpec` 也没有工具字段。

### 目标形态

```rust
#[derive(Debug, Clone)]
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    pub channel: Option<String>,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub depth: u8,

    /// 每请求 ToolService 覆盖。None 时 AgentHarnessRunner 回退到其
    /// 构造时注入的默认 `tool_service`（适合无状态测试 / 内部 flow_run 调用）。
    /// Gateway 生产路径必须 Some，以携带 per-request 动态工具集。
    pub tool_service: Option<Arc<dyn ToolService>>,
}
```

`AgentHarnessRunner.run`：

```rust
let tools = req_tool_service  // 从 FlowRequest 透传
    .unwrap_or_else(|| self.tool_service.clone());

let deps = HarnessDeps {
    session: self.session_service.clone(),
    tools,  // 覆盖
    sandbox,
    llm,
    stop_hooks: self.stop_hooks.clone(),
    context_budget: self.context_budget.clone(),
    context_compactor: self.context_compactor.clone(),
    skill_prefetcher: self.skill_prefetcher.clone(),
    trace_sink: req_trace_sink,  // 同理从 FlowRequest 透传（见 §1）
};
```

### Orchestrator::dispatch 的透传

`Orchestrator::dispatch(req)` 内部把 `req.tool_service` 与 `req.trace_sink` 传给 `HarnessRunner::run`。这要求扩展 HarnessRunner trait：

```rust
#[async_trait]
pub trait HarnessRunner: Send + Sync {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        // 新增：
        tool_service_override: Option<Arc<dyn ToolService>>,
        trace_sink: Option<Arc<dyn TraceSink>>,
    ) -> Result<FlowOutcome, FlowError>;
}
```

### Gateway 侧的 per-request ToolService 构造

Gateway 在 `run_loop.rs` 新写一个辅助函数：

```rust
fn build_request_tool_service(
    global: Arc<dyn ToolService>,
    agent: &AgentDef,
    request: &ChatRequest,
    sub_deps: SubagentDeps,
) -> Arc<dyn ToolService> { ... }
```

内部逻辑等价于今天的 `build_registry_from_tools + subagent_tool 注入 + tool_refresh 绑定`。返回的 `Arc<dyn ToolService>` 装进 `FlowRequest.tool_service`。

这意味着 `ToolService` trait 本身**不变**，实现方可以按需组合 —— 今天的 `DispatcherToolService` 之外，Gateway 会有一个 `ScopedToolService`（或类似名字）的组合实现。

### `flow_run` LLM 工具的兼容性

`src/orchestrator/flow_run_tool.rs` 今天构造 `FlowRequest` 时不会带 `tool_service`（嵌套调用应沿用全局）。新字段默认 `None`，无需改动。

## §3. `Arc<Orchestrator>` 归属（Gap 3）

### 现状

`ExecutionEngine::dispatch_via_orchestrator(orchestrator: Arc<Orchestrator>, ...)` 在方法参数里接收 orchestrator；`ExecutionEngine` 结构体本身没有这个字段。`GatewayServer` 持有 orchestrator，但不把它注入 engine。

### 目标形态

`ExecutionEngine` 新增字段：

```rust
pub struct ExecutionEngine {
    // ... 现有字段 ...
    /// Phase 6b 切换后的主分发路径。None 表示未装配（例如某些测试上下文）；
    /// 生产路径下由 start/mod.rs 在 wire_gateway 之前装配完毕。
    orchestrator: Arc<Orchestrator>,
}
```

构造：
- `ExecutionEngine::new(...)` 签名新增 `orchestrator: Arc<Orchestrator>` 参数。
- `start/mod.rs` 调整装配顺序：
  1. `SessionService` / `ToolService` / `Sandbox` / `AgentRegistry`（现状）
  2. 构造 `AgentHarnessRunner`
  3. 构造 `Orchestrator`（依赖 AgentHarnessRunner + session + sandbox_factory + flow_registry）
  4. 构造 `ExecutionEngine(orchestrator, ...)`
  5. 构造 `GatewayServer(engine, orchestrator, ...)` —— GatewayServer 仍持有 orchestrator 引用用于 `/flow/reload` RPC；两处共享同一 `Arc`。

### 无环依赖证明

- `Orchestrator` → `HarnessRunner` → `SessionService / ToolService / Sandbox`（均早于 Orchestrator 构造）。
- `ExecutionEngine` → `Orchestrator`（单向，Orchestrator 不知道 ExecutionEngine）。
- `GatewayServer` → `ExecutionEngine`（单向）。
- `GatewayServer` → `Orchestrator`（单向，用于 reload）。
- 没有 `Orchestrator → ExecutionEngine` 反向依赖。

### `dispatch_via_orchestrator` 的命运

该方法今天接收 `orchestrator` 作参数，在切换后改为读 `self.orchestrator`。保留方法作为 dispatch 入口；原 `execute_chat_turn`（即 `run_loop.rs` 的 `run_agent_loop` 路径）在 §4 的切换中整体重写，内部只剩构造 `FlowRequest` + drain events + 映射 outcome 的逻辑。

## §4. 运行时切换（run_loop.rs:628）

### 替换范围

`run_loop.rs` 中 **行 623–731**（从 `let platform_name = ...` 到 `return Ok(response);` 结束）整体被替换为：

```rust
// 1. 构建 per-request ToolService
let tool_service = build_request_tool_service(
    self.tool_service.clone(),
    &agent,
    &request,
    SubagentDeps { run_chain, teammate_manager, message_router, inbox, .. },
);

// 2. 构建 TraceSink（包装原 callback_state 的持久化）
let trace_sink = Arc::new(GatewayTraceSink::new(
    callback_state.clone(),
    run_id.to_string(),
)) as Arc<dyn TraceSink>;

// 3. 构建 FlowRequest
let req = FlowRequest {
    flow_id: None,  // 走 agent_id → default_routing 解析
    agent_id: agent.id().to_string(),
    input: FlowInput::History { turns: history_to_turns(history), prompt: request.input.clone() },
    channel: request.metadata.get("platform").cloned(),
    session_hint: Some(request.session_key.to_key_string()),
    parent_session: None,
    depth: 0,
    tool_service: Some(tool_service),
    trace_sink: Some(trace_sink),  // 或挂在单独 channel，按实现就近
};

// 4. dispatch
let handle = self.orchestrator
    .dispatch(req)
    .await
    .map_err(|e| ExecutionError::Orchestrator(format!("dispatch: {e}")))?;

// 5. drain events → gateway emitter
let emitter_clone = emitter.clone();
let run_id_str = run_id.to_string();
let pending_media = request.pending_media.clone();
let has_emitted_text = Arc::new(AtomicBool::new(false));
let drain = tokio::spawn(async move {
    let mut rx = handle.events;
    let mut final_outcome: Option<FlowOutcome> = None;
    loop {
        match rx.recv().await {
            Ok(FlowStreamEvent::Delta(t)) => emit_text_delta(&emitter_clone, &run_id_str, t, &has_emitted_text).await,
            Ok(FlowStreamEvent::Reasoning(t)) => emit_reasoning(&emitter_clone, &run_id_str, t).await,
            Ok(FlowStreamEvent::ToolCallStart { id, name, args }) => emit_tool_start(&emitter_clone, &run_id_str, id, name, args).await,
            Ok(FlowStreamEvent::ToolCallDone { id, result, error }) => emit_tool_done(&emitter_clone, &run_id_str, id, result, error).await,
            Ok(FlowStreamEvent::ToolSummary { id, text }) => emit_tool_summary(&emitter_clone, &run_id_str, id, text).await,
            Ok(FlowStreamEvent::SafetyBlock { reason }) => emit_safety_block(&emitter_clone, &run_id_str, reason).await,
            Ok(FlowStreamEvent::StopHookBlock { reason }) => emit_stop_hook_block(&emitter_clone, &run_id_str, reason).await,
            Ok(FlowStreamEvent::ModelFallback { reason, fallback_model }) => emit_model_fallback(&emitter_clone, &run_id_str, reason, fallback_model).await,
            Ok(FlowStreamEvent::Complete(outcome)) => {
                final_outcome = Some(outcome);
                break;
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(n, "orchestrator stream lagged; dropping frames");
            }
        }
    }
    final_outcome
});

// 6. 等待 completion（兼容 stream drop / harness panic 路径）
let completion = handle.completion.await
    .map_err(|e| ExecutionError::Orchestrator(format!("completion dropped: {e}")))?
    .map_err(|e| ExecutionError::Orchestrator(format!("flow: {e}")))?;

// 7. 刷 TraceSink（持久化）
trace_sink.flush();

let drained_outcome = drain.await.ok().flatten();
let outcome = drained_outcome.unwrap_or(completion);

// 8. 映射 hit_limit → i18n ErrLoopExhausted（保留现有分支逻辑）
let response = if outcome.hit_limit && outcome.final_text.is_empty() {
    i18n::t(Msg::ErrLoopExhausted { iterations: outcome.iterations as usize, tool_calls: outcome.tool_calls_made as usize }, locale)
} else {
    outcome.final_text
};

// 9. 清理 media（保留）
if multimodal_messages.is_some() {
    if let Some(mp) = self.media_processor.as_ref() {
        mp.cleanup(&request.session_key.to_key_string());
    }
}

Ok(response)
```

### 清理 stale imports

切换完成后，`run_loop.rs` 头部删除：
```rust
use crate::agent_loop::{AgentLoop, LoopConfig, ...};
```

`PHASE-6-LEGACY` 注释块（行 625–627）一并删除。

### 提供方 fallback 重试路径

现存的 provider fallback 重试循环（在 `run_loop.rs:render_attempt_loop` 外层）保留：
- 它包装整个 "构建 per-request tool_service + dispatch + drain" 过程；
- fallback 发生时 `Orchestrator::dispatch` 会用新 provider 走一遍；
- `report_outcome` 调用点保持不变（Ok/Err 分类）。

## §5. 行为保留清单

切换后必须保持的用户可见行为：

| 行为 | 今日来源 | 切换后来源 |
|------|----------|-----------|
| 增量文本流 | `StreamCallback::on_text_delta` | `FlowStreamEvent::Delta` |
| 思考流 | `StreamCallback::on_reasoning`（通过 delta_sink） | `FlowStreamEvent::Reasoning` |
| 工具调用起止 | `on_tool_call_start / on_tool_call_done` | `ToolCallStart / ToolCallDone` |
| 工具摘要 | `on_tool_summary` | `ToolSummary` |
| 安全拦截提示 | `on_safety_block` → emitter | `SafetyBlock` → emitter |
| stop hook 阻断 | `on_stop_hook_block` | `StopHookBlock` |
| provider fallback 指示 | `ModelResolved` emitter（保留）+ `on_model_fallback` | `ModelResolved` + `ModelFallback` |
| 运行 trace 持久化 | `callback.flush_trace_persistence()` | `TraceSink::flush()` via `HarnessDeps.trace_sink` |
| i18n `ErrLoopExhausted` | gateway 读 `hit_limit` | gateway 读 `FlowOutcome.hit_limit`（保留） |
| 多模态 media cleanup | gateway 直接调 `media_processor.cleanup` | 同左（独立于 flow） |
| 取消（cancel token） | `cancel_token.clone()` 给 AgentLoop | `FlowHandle.cancel` → harness run cancel token |
| MCP 动态工具 | `ExtensionToolRefreshSource` + per-request registry | per-request `ToolService` 实现内聚合 |
| SubagentTool 注入 | `tool_registry.register(subagent_tool)` | per-request `ToolService` 实现内聚合 |
| 上下文压缩 | `with_context_compactor` | `HarnessDeps.context_compactor`（Task 10 已接入） |
| 预算拦截 | `with_context_budget` | `HarnessDeps.context_budget`（Task 10 已接入） |
| Skill prefetch | `with_skill_prefetcher` | `HarnessDeps.skill_prefetcher`（Task 10 已接入） |

**已文档化丢弃（PHASE_6B_BUILDER_AUDIT.md 审批过，本次不回退）：**
`with_chain / with_shared_snapshot / with_provider_name / with_platform_name / with_session_id / with_hook_executor / with_tool_refresh`。`with_hook_executor` 的 Gateway-side decorator 归入本切换 §2 Gap 1 的 `build_request_tool_service` 实现内（用户 tool hook 在 per-request ToolService 组合时装配）。

## §6. 测试覆盖

新增集成测试（切换任务必须同步落地）：

1. **`tests/gateway_chat_through_orchestrator.rs`** —— 构造最小 `ExecutionEngine` + 真实 `Orchestrator`（stub HarnessRunner 递增计数器）；断言 `execute_chat_turn`（或等效入口）触发一次 `HarnessRunner::run`。失败形态：未切换前测试编译失败（`ExecutionEngine::orchestrator` 不存在）或运行时计数 = 0。
2. **`tests/gateway_chat_preserves_hit_limit.rs`** —— stub harness 返回 `FlowOutcome { hit_limit: true, final_text: "", .. }`；断言 Gateway 响应包含 i18n `ErrLoopExhausted` 字符串。
3. **`tests/gateway_chat_streams_tool_events.rs`** —— stub harness 在 run 中发射 `ToolCallStart + ToolCallDone + ToolSummary`；断言 drain 任务按序调用 emitter 的 tool_start / tool_done / tool_summary（基于 mock emitter 记录调用顺序）。
4. **`tests/gateway_chat_dynamic_tools.rs`** —— Gateway 侧注入一个 per-request MCP 工具到 `tool_service`，dispatch 经过 harness 时工具可见；断言 harness 能 list/execute 它。
5. **`tests/gateway_chat_cancellation.rs`** —— 启动 dispatch 后 `handle.cancel.cancel()`；断言 200ms 内 completion resolve 到 `FlowError::Cancelled`。
6. **`tests/gateway_chat_trace_flush.rs`** —— stub `TraceSink` 记录 `on_trace` 与 `flush` 调用；断言 flush 在 run 结束后被 exactly-once 调用。

库测试（`cargo test -p alephcore --lib`）基线保持 ≥ 9133 通过 + 2 个既有失败不动。

## §7. 回滚策略

切换是单次 commit（集成测试 + 实现同 commit）。若上线后发现严重回归：

- `git revert` 该 commit —— 撤销切换；`AgentLoop::new` 路径回来；其他 Phase 6b Task 1–11 的搬迁 + 装配不受影响（它们在更早 commit）。
- revert 后 Phase 6c 依赖的"AgentLoop::new 仅剩测试使用"断言仍然成立（该断言由切换引入）—— revert 会让 Phase 6c 的 exit gate 回退，属正常。

不使用 feature flag —— 原因已在本规范 §"非目标"说明：flag 不替你回答设计问题，且此切换的设计问题已全部解决。

## §8. Exit gate 断言（供新计划收尾任务使用）

计划结束时 `scripts/check-phase6b-flip-exit.sh` 必须通过：

1. `src/gateway/execution_engine/run_loop.rs` 中不再存在 `AgentLoop::new`（grep 零匹配）。
2. `ExecutionEngine` 结构体包含 `orchestrator: Arc<Orchestrator>` 字段（grep 匹配）。
3. `FlowRequest` 包含 `tool_service: Option<Arc<dyn ToolService>>` 字段。
4. `FlowStreamEvent` 至少包含本规范 §1 目标形态中列出的 9 个变体。
5. `cargo test -p alephcore --lib` 在 baseline 9133 之上，且新增的 6 个集成测试全绿。
6. `cargo clippy -- -D warnings` 干净。
7. 仅留 `AgentLoop::new` 匹配的文件：`src/agent_loop/loop_core.rs`（测试块）、`src/agent_loop/factory.rs`、`src/agent_loop/integration_probe.rs`。新计划不清理这三处（Phase 6c 范围）。

## §9. 交接

新计划 `docs/superpowers/plans/2026-04-20-gateway-orchestrator-flip-plan.md` 根据本规范展开为 6 个 bite-sized 任务（见同日起草的计划）。

Phase 6c 与 Phase 6d 在本切换 landed 后解锁。

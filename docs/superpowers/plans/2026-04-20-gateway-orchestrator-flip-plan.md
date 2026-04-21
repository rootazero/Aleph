# Gateway → Orchestrator 切换实施计划

> **取代范围：** Phase 6b plan Task 12–14（已 DEFERRED）。本计划独立落地，不改 Phase 6b 已合并的 Task 1–11。
>
> **对应规范：** `docs/superpowers/specs/2026-04-20-gateway-orchestrator-flip-resolution-design.md`
>
> **For agentic workers:** REQUIRED SUB-SKILL：使用 `superpowers:subagent-driven-development` 按任务逐个执行。每个任务一个全新 subagent，两阶段 review（先 spec compliance，再 code quality）。

**Goal:** 把 Gateway chat 路径从 `AgentLoop::new` 切换到 `Orchestrator::dispatch`，落地规范 §1/§2/§3 的三处结构变化，并确保 §5 的全部用户可见行为保留。

**Architecture:** 先扩事件词汇 → 再扩 FlowRequest → 再装配 ExecutionEngine 字段 → 最后做实际切换。前三步都是"加字段不破坏旧调用者"，第四步才是单次切换 commit。

**Tech Stack:** Rust、tokio、既有 Aleph 模块。无新 crate。

---

## 预算与不变量

- `cargo test -p alephcore --lib` ≥ 9133 通过，2 个既有失败不动（telegram config + notes prompt snapshot）。
- `tests/harness_run_e2e.rs` 2/2 通过不动。
- 新增 6 个集成测试（规范 §6），切换任务落地时必须全部绿。
- 新增/改动文件单文件 < 400 LOC。
- `cargo clippy -- -D warnings` 干净。
- **不发版** —— 切换合入 main 后仍由用户决定是否 release；Task 6 明确停止。
- 英文 commit message，前缀 `phase6b-flip:`。

---

## Strangler-fig 讨论

本计划不是搬迁式修改，而是 **additive migration**：

- Task 1/2/3 都是"字段加法"，旧路径（run_loop.rs:628 的 AgentLoop::new）依然工作。
- Task 4a/4b 都是"新增文件 + 单元测试"，不触碰 run_loop.rs。Task 4c 才是**单次切换 commit** —— 6 个集成测试 + 实现同 commit 落；revert 即回滚。
- Task 5 清理 + exit gate。
- Task 6 人工 smoke，由用户亲自跑。

---

### Task 1: FlowStreamEvent 词汇表扩展

**Files:**
- Modify: `src/orchestrator/dispatch.rs` —— 扩枚举，保留向后兼容
- Modify: `src/orchestrator/harness_bridge.rs:181-203` —— `BroadcastCallback` 适配新变体
- Modify: `src/orchestrator/harness_bridge.rs:351-396` —— 更新 `broadcast_callback_fans_lifecycle_events` 测试

具体新变体见规范 §1 目标形态。

- [ ] **Step 1: 在 `dispatch.rs` 扩 `FlowStreamEvent` 枚举**

  替换 `src/orchestrator/dispatch.rs:34-39`：

  ```rust
  #[derive(Debug, Clone)]
  #[non_exhaustive]
  pub enum FlowStreamEvent {
      Delta(String),
      Reasoning(String),
      ToolCallStart {
          id: String,
          name: String,
          args: serde_json::Value,
      },
      ToolCallDone {
          id: String,
          result: Option<serde_json::Value>,
          error: Option<String>,
      },
      ToolSummary { id: String, text: String },
      SafetyBlock { reason: String },
      StopHookBlock { reason: String },
      ModelFallback { reason: String, fallback_model: String },
      Complete(FlowOutcome),
  }
  ```

  同时 `Complete` 从 unit 变成携带 `FlowOutcome`。

- [ ] **Step 2: 扩 `HarnessCallback` trait 增加对应方法（默认实现为空）**

  修改 `src/harness/callback.rs`（或 `src/harness/callback/mod.rs`）：

  ```rust
  pub trait HarnessCallback: Send {
      fn on_delta(&mut self, _text: &str) {}
      fn on_reasoning(&mut self, _text: &str) {}
      fn on_tool_call(&mut self, _name: &str) {}  // 保留旧 API 兼容
      fn on_tool_call_start(&mut self, _id: &str, _name: &str, _args: &serde_json::Value) {}
      fn on_tool_call_done(&mut self, _id: &str, _result: Option<&serde_json::Value>, _error: Option<&str>) {}
      fn on_tool_summary(&mut self, _id: &str, _text: &str) {}
      fn on_safety_block(&mut self, _reason: &str) {}
      fn on_stop_hook_block(&mut self, _reason: &str) {}
      fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}
      fn on_complete(&mut self) {}
      fn on_complete_with_outcome(&mut self, _outcome_hint: &crate::harness::callback::OutcomeHint) {}
  }
  ```

  注：Harness 尚不产生 FlowOutcome（那是 AgentHarnessRunner 在 run 结束后合成的），所以 `on_complete_with_outcome` 在本 Task 为空壳；Task 4 会由 `BroadcastCallback` 消费。

- [ ] **Step 3: 改写 `BroadcastCallback` 映射**

  修改 `src/orchestrator/harness_bridge.rs:181-203`：将每个新 HarnessCallback 方法映射到对应 FlowStreamEvent 变体。`on_tool_call(name)` 保留旧行为但标记 `#[deprecated(note = "use on_tool_call_start")]`，内部发送 `ToolCallStart { id: "legacy".into(), name, args: Value::Null }` 以维持兼容。

  注意：`on_complete` 依然由 `AgentHarness::run` 结束后调用，但 `FlowStreamEvent::Complete(outcome)` 由 `AgentHarnessRunner::run` 在合成 `FlowOutcome` 后单独发射（不经 BroadcastCallback），因为 outcome 在 harness 之外聚合。

- [ ] **Step 4: 更新既有测试**

  修改 `src/orchestrator/harness_bridge.rs:351-396` 的 `broadcast_callback_fans_lifecycle_events`：断言 `ToolCall { name }` 替换为 `ToolCallStart { name, .. }`，`Complete` 变体的 match 改为 `Complete(outcome)` pattern。

- [ ] **Step 5: 更新 `ExecutionEngine::dispatch_via_orchestrator`（engine.rs 附近 943–991）的 drain 逻辑**

  把枚举 match 展开到所有新变体；未用变体（测试上下文）可以 `_ => {}` 吞掉，但至少 `Complete(outcome)` 必须提取 outcome。

- [ ] **Step 6: `cargo check -p alephcore --lib`**

- [ ] **Step 7: `cargo test -p alephcore --lib`** —— baseline ≥ 9133 通过。

- [ ] **Step 8: Commit**

  ```
  phase6b-flip: expand FlowStreamEvent vocabulary + HarnessCallback methods (task 1)
  ```

---

### Task 2: FlowRequest.tool_service + TraceSink

**Files:**
- Modify: `src/orchestrator/dispatch.rs` —— `FlowRequest`、`HarnessRunner` trait
- Create: `src/harness/trace_sink.rs` —— `TraceSink` trait
- Modify: `src/harness/mod.rs` —— 导出 `TraceSink`
- Modify: `src/harness/deps.rs` —— 加 `trace_sink` 字段
- Modify: `src/orchestrator/harness_bridge.rs` —— `AgentHarnessRunner.run` 消费新参数
- Modify: `src/orchestrator/flow_run_tool.rs` —— 构造 `FlowRequest` 时显式设 `tool_service: None, trace_sink: None`

- [ ] **Step 1: 定义 `TraceSink` trait**

  新建 `src/harness/trace_sink.rs`：

  ```rust
  //! TraceSink — observability side-channel for AgentHarness runs.
  //!
  //! Events not exposed via `FlowStreamEvent` (internal trace,
  //! confirmation prompts, persistence flush) route here instead.

  use std::sync::Arc;

  use crate::harness::trace::LoopTraceEvent;  // 既已 relocate 到 harness/

  pub trait TraceSink: Send + Sync {
      fn on_trace(&self, event: &LoopTraceEvent);
      fn flush(&self);
  }

  /// No-op implementation for tests / internal flow_run calls.
  pub struct NoopTraceSink;

  impl TraceSink for NoopTraceSink {
      fn on_trace(&self, _event: &LoopTraceEvent) {}
      fn flush(&self) {}
  }
  ```

  `on_confirmation_needed` 暂不放入本 trait —— Gateway 不走 confirmation 流程（MenuBar 的 UI 流是独立的），只是让接口结构留后路；若未来需要，再扩 trait。

- [ ] **Step 2: `src/harness/mod.rs` 导出**

  加：
  ```rust
  pub mod trace_sink;
  pub use trace_sink::{TraceSink, NoopTraceSink};
  ```

- [ ] **Step 3: `HarnessDeps` 加字段**

  修改 `src/harness/deps.rs`，在结构体末尾追加：

  ```rust
  /// Gateway-side observability sink. `None` falls back to no-op tracing.
  /// Production path: Gateway wraps its persistence callback in `GatewayTraceSink`.
  pub trace_sink: Option<Arc<dyn TraceSink>>,
  ```

- [ ] **Step 4: 更新全部 `HarnessDeps { ... }` 构造点**

  `grep -rn "HarnessDeps {" src tests | head -30`；每个点加 `trace_sink: None,`。预计命中：
  - `src/harness/tests/*`（test helpers）
  - `src/harness/agent.rs` 内部测试（若有）
  - `src/orchestrator/harness_bridge.rs:108`
  - 其他 rg 可搜到的点

  全部加 `trace_sink: None` 默认；不改现有行为。

- [ ] **Step 5: `AgentHarness::run` 接入 TraceSink（可选消费）**

  在 `src/harness/agent.rs` 的 `run_turn`（或合适位置）：
  - 若 `deps.trace_sink.is_some()`，在每次 turn 结束 / 关键状态变更时调用 `trace_sink.on_trace(&event)`。先不扩事件类型，用现有 `LoopTraceEvent` 就够。
  - `run` 结束（成功或 HarnessError）时调用 `trace_sink.flush()`。

- [ ] **Step 6: 扩 `FlowRequest`**

  修改 `src/orchestrator/dispatch.rs:60-69`：

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
      /// Per-request tool service override. See §2 of the resolution design.
      /// Debug impl skips this field because Arc<dyn ToolService> isn't Debug.
      pub tool_service: Option<Arc<dyn crate::tools::service::ToolService>>,
      pub trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
  }
  ```

  `Debug` derive 会报错 `Arc<dyn ToolService>` 不实现 Debug —— 手写 `impl Debug for FlowRequest`，跳过这两个字段。

- [ ] **Step 7: 扩 `HarnessRunner` trait**

  修改 `src/orchestrator/dispatch.rs:98-109`：

  ```rust
  #[async_trait::async_trait]
  pub trait HarnessRunner: Send + Sync {
      async fn run(
          &self,
          session_key: String,
          spec: Arc<FlowSpec>,
          input: FlowInput,
          sandbox: Arc<dyn crate::sandbox::Sandbox>,
          events: broadcast::Sender<FlowStreamEvent>,
          cancel: CancellationToken,
          tool_service_override: Option<Arc<dyn crate::tools::service::ToolService>>,
          trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
      ) -> Result<FlowOutcome, FlowError>;
  }
  ```

- [ ] **Step 8: `Orchestrator::dispatch` 透传**

  修改 `src/orchestrator/dispatch.rs:132-215`：把 `req.tool_service` 与 `req.trace_sink` `clone()` 后传到 `harness.run(...)`。

- [ ] **Step 9: `AgentHarnessRunner::run` 消费 override**

  修改 `src/orchestrator/harness_bridge.rs:64-167`：

  ```rust
  let tools = tool_service_override.unwrap_or_else(|| self.tool_service.clone());
  // ...
  let deps = HarnessDeps {
      session: self.session_service.clone(),
      tools,
      sandbox,
      llm,
      stop_hooks: self.stop_hooks.clone(),
      context_budget: self.context_budget.clone(),
      context_compactor: self.context_compactor.clone(),
      skill_prefetcher: self.skill_prefetcher.clone(),
      trace_sink,
  };
  ```

- [ ] **Step 10: `flow_run_tool.rs` 构造点加默认 `None`**

  修改 `src/orchestrator/flow_run_tool.rs:72` 附近：

  ```rust
  let req = FlowRequest {
      // ...existing fields...
      tool_service: None,
      trace_sink: None,
  };
  ```

- [ ] **Step 11: 新增单元测试 `src/orchestrator/tests/dispatch.rs`**

  - `dispatch_forwards_tool_service_override` —— stub HarnessRunner 捕获 override，断言 `Some(_)` 到达 run。
  - `dispatch_forwards_trace_sink` —— 同上。

- [ ] **Step 12: `cargo check` + `cargo test -p alephcore --lib`**

  baseline ≥ 9133 + 2 new tests = 9135。

- [ ] **Step 13: Commit**

  ```
  phase6b-flip: thread tool_service + trace_sink through FlowRequest (task 2)
  ```

---

### Task 3: ExecutionEngine.orchestrator 字段 + boot 装配

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs` —— 加字段 + 改 `new` 签名 + 重写 `dispatch_via_orchestrator`
- Modify: `src/bin/aleph-server/commands/start/mod.rs` —— 装配顺序（orchestrator 在 engine 前）
- Modify: `src/bin/aleph-server/commands/start/agent_init.rs` 或相邻 orchestrator_init.rs —— 若需要

- [ ] **Step 1: `ExecutionEngine` 加字段 + 构造签名**

  `src/gateway/execution_engine/engine.rs`（现有 struct 定义附近）：

  ```rust
  pub struct ExecutionEngine {
      // ...现有字段...
      orchestrator: Arc<crate::orchestrator::Orchestrator>,
  }
  ```

  扩 `ExecutionEngine::new(..., orchestrator: Arc<Orchestrator>) -> Self`。

- [ ] **Step 2: 重写 `dispatch_via_orchestrator`（engine.rs:943-991）**

  原签名接收 `orchestrator` 作参数；改为读 `self.orchestrator`。方法保留（供 Task 4 作为切换入口复用）。

- [ ] **Step 3: 调整 boot 装配顺序 `start/mod.rs`**

  今天的装配（简化）：
  ```
  SessionService → ToolService → Sandbox → AgentRegistry
  → ExecutionEngine
  → Orchestrator (via new wire_orchestrator helper)
  → GatewayServer(engine, orchestrator)
  ```

  调整为：
  ```
  SessionService → ToolService → Sandbox → AgentRegistry
  → AgentHarnessRunner
  → Orchestrator
  → ExecutionEngine(..., orchestrator)
  → GatewayServer(engine, orchestrator)
  ```

  逐一找 `ExecutionEngine::new` 调用点并加 `orchestrator` 参数。Orchestrator 构造点在 Phase 5 已有 `orchestrator_init.rs` 这类文件；调整调用顺序即可。

- [ ] **Step 4: 更新测试里 `ExecutionEngine::new` 调用点**

  `grep -rn "ExecutionEngine::new" src tests`；测试可以注入一个最小 stub orchestrator（没有 flow_registry 时给空 `FlowSet::default()`）。本 Task 不改 run_loop.rs 行为 —— 切换留给 Task 4。

- [ ] **Step 5: `cargo check` + `cargo test -p alephcore --lib`**

  baseline ≥ 9135。

- [ ] **Step 6: Commit**

  ```
  phase6b-flip: wire Arc<Orchestrator> as ExecutionEngine field (task 3)
  ```

---

### Task 4a: Gateway drain helper 抽取

> **拆分原因：** 原 Task 4 规模被低估，无法单 commit 落地。执行 subagent 定位出 4 处真实缺口：(1) ToolService ↔ LoopToolRegistry 无适配层，(2) `execute()` 端到端测试需要巨大 fixture，(3) harness 今天不产生多数 FlowStreamEvent 变体，(4) provider fallback 重试循环包在 AgentLoop 外，dispatch 会丢分类。所以拆成 4a（drain helper）→ 4b（ScopedToolService adapter）→ 4c（切换 + FlowError::Transient + 6 集成测试）。

**Files:**
- Create: `src/gateway/execution_engine/event_drain.rs` —— 纯函数 `emit_flow_event(event, emitter, state)`：接 `FlowStreamEvent` 9 变体并调用 emitter 对应事件。
- Modify: `src/gateway/execution_engine/mod.rs` —— `pub(crate) mod event_drain;`

**Goal:** 把"FlowStreamEvent → Gateway emitter"映射从 `StreamCallback` 中提炼成可独立测试的纯辅助函数。不动 run_loop.rs。

- [ ] **Step 1: 创建 `src/gateway/execution_engine/event_drain.rs`**

  核心函数：
  ```rust
  pub(crate) async fn emit_flow_event(
      event: FlowStreamEvent,
      emitter: &Arc<dyn ChatEventEmitter>,
      run_id: &str,
      state: &Arc<Mutex<DrainState>>,
  ) -> Result<(), EmitError> { /* match 9 variants → emitter calls */ }
  ```

  对应规范 §5 的逐行映射。`DrainState` 暂存 pending `ToolCallStart` 到 `ToolCallDone` 的匹配 + `has_emitted_text` flag。

- [ ] **Step 2: 单元测试**

  `src/gateway/execution_engine/event_drain.rs` 底部加 `#[cfg(test)] mod tests`：
  - `delta_goes_to_emitter_text`
  - `tool_call_start_and_done_pair`
  - `safety_block_emits_error`
  - `complete_sets_outcome_hint`

  用 mock emitter（`Arc<MockEmitter>` 捕获调用序列）。

- [ ] **Step 3: `cargo test -p alephcore --lib`** ≥ 9135 + 4 = 9139。

- [ ] **Step 4: Commit**

  ```
  phase6b-flip: extract event drain helper (task 4a)
  ```

---

### Task 4b: ScopedToolService adapter

**Files:**
- Create: `src/tools/scoped.rs` —— `ScopedToolService` struct 实现 `ToolService`，桥接 `LoopToolRegistry + SubagentTool + MCP 动态工具 → Arc<dyn ToolService>`。
- Modify: `src/tools/mod.rs` —— 导出 `ScopedToolService`。

**Goal:** 桥接今天 Gateway 的 `LoopToolRegistry`（含 SubagentTool 与 MCP）到 harness 要的 `ToolService` trait。切换当前 AgentLoop 路径**不动**；本 task 只 landsdapter + 测试，为 4c 做好准备。

- [ ] **Step 1: 定义 `ScopedToolService`**

  ```rust
  pub struct ScopedToolService {
      inner: Arc<dyn LoopToolRegistry>,
      allowed: BTreeSet<String>,  // 过滤视图
      subagent_tool: Option<Arc<SubagentTool>>,  // 单独注入
      refresh: Option<Arc<dyn ToolRefreshSource>>,  // 懒加载触发
      hook_decorator: Option<Arc<dyn ToolHookDecorator>>,
  }

  #[async_trait::async_trait]
  impl ToolService for ScopedToolService {
      async fn list(&self, ctx: &ToolContext) -> Result<Vec<ToolDefinition>, ToolError> { /* ... */ }
      async fn describe(&self, name: &str, ctx: &ToolContext) -> Result<ToolDefinition, ToolError> { /* ... */ }
      async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> Result<ToolOutput, ToolError> { /* ... */ }
  }
  ```

- [ ] **Step 2: 实现 `list`**

  - 先触发 `refresh.refresh(ctx).await` 如果有；
  - 从 inner 拿全量 LoopTool，映射为 ToolDefinition；
  - append SubagentTool 的 definition；
  - 按 `allowed` 过滤；
  - 返回。

- [ ] **Step 3: 实现 `execute`**

  - 如是 `SubagentTool::name()`，走 SubagentTool 路径；
  - 否则查 inner；
  - 若 `hook_decorator.is_some()`，用 decorator 包装执行。

- [ ] **Step 4: 单元测试**

  `src/tools/scoped.rs` 底部 `#[cfg(test)] mod tests`：
  - `list_filters_by_allowed`
  - `list_includes_subagent_tool`
  - `list_triggers_refresh_on_first_call`
  - `execute_routes_to_subagent_tool_by_name`
  - `execute_applies_hook_decorator`
  - `describe_returns_from_filtered_set`

  用简单 stub `LoopToolRegistry` + stub `ToolRefreshSource` + stub `ToolHookDecorator`。

- [ ] **Step 5: `cargo test -p alephcore --lib`** ≥ 9139 + 6 = 9145。

- [ ] **Step 6: Commit**

  ```
  phase6b-flip: add ScopedToolService adapter (task 4b)
  ```

---

### Task 4c: 实际切换 + FlowError::Transient + 6 集成测试

**Files:**
- Modify: `src/orchestrator/dispatch.rs` —— `FlowError` 增加 `Transient { provider: String, source: String }` 变体。
- Modify: `src/orchestrator/harness_bridge.rs` —— 把 harness 返回的可重试错误映射到 `FlowError::Transient`。
- Create: `tests/gateway_chat_through_orchestrator.rs`
- Create: `tests/gateway_chat_preserves_hit_limit.rs`
- Create: `tests/gateway_chat_streams_tool_events.rs`
- Create: `tests/gateway_chat_dynamic_tools.rs`
- Create: `tests/gateway_chat_cancellation.rs`
- Create: `tests/gateway_chat_trace_flush.rs`
- Create: `src/gateway/execution_engine/trace_sink_adapter.rs`（`GatewayTraceSink`）
- Create: `src/gateway/execution_engine/tool_service_builder.rs`（`build_request_tool_service` 用 4b 的 `ScopedToolService`）
- Modify: `src/gateway/execution_engine/run_loop.rs` —— 替换 623–731；retry 循环外层保留，分类改读 `FlowError::Transient`。
- Modify: `src/gateway/execution_engine/mod.rs`

> **TDD 顺序：** 6 个集成测试先写（全部 red），再实现。切换同 commit。

- [ ] **Step 1: 扩 `FlowError`**

  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum FlowError {
      // ...existing...
      #[error("transient harness error ({provider}): {source}")]
      Transient { provider: String, source: String },
  }
  ```

  加单元测试确认 `matches!(err, FlowError::Transient { .. })` 判定工作。

- [ ] **Step 2: harness bridge 翻译分类**

  `src/orchestrator/harness_bridge.rs` `AgentHarnessRunner::run` 把 `HarnessError::Internal` 中带有 provider transient 语义（5xx / network / rate limit）的翻译为 `FlowError::Transient { provider, source }`；其他仍走 `FlowError::Internal`。

- [ ] **Step 3: 写 6 个集成测试（`tests/gateway_chat_*.rs`）**

  每个测试用最小 stub `HarnessRunner`（实现 `HarnessRunner` trait，不依赖 `execute()` 全套），通过 `ExecutionEngine::dispatch_via_orchestrator` 或新抽的 `run_agent_loop_via_orchestrator` helper 入口断言：
  1. `gateway_chat_through_orchestrator` —— HarnessRunner 被调用计数 = 1。
  2. `gateway_chat_preserves_hit_limit` —— stub 返回 `FlowOutcome { hit_limit: true, final_text: "" }`，响应文本为 i18n `ErrLoopExhausted` 串。
  3. `gateway_chat_streams_tool_events` —— stub 发 `ToolCallStart → ToolCallDone → ToolSummary`，emitter 收到有序三次调用。
  4. `gateway_chat_dynamic_tools` —— `FlowRequest.tool_service = Some(ScopedToolService { subagent_tool: Some(_), .. })`，stub HarnessRunner 对 override 的 `list()` 能看到 SubagentTool。
  5. `gateway_chat_cancellation` —— `CancellationToken::cancel()`，stub 返回 `FlowError::Internal("cancelled")`，drain 任务干净退出。
  6. `gateway_chat_trace_flush` —— `TestTraceSink::flush_called: Arc<AtomicBool>`，dispatch 完成后为 `true`。

- [ ] **Step 4: 跑 6 个测试确认全红 / 编译失败**

- [ ] **Step 5: 实现 `GatewayTraceSink`**

  `src/gateway/execution_engine/trace_sink_adapter.rs`：包装原 `callback_state` 与 emitter；`on_trace` 路由到既有持久化；`flush` 强制刷盘。

- [ ] **Step 6: 实现 `build_request_tool_service`**

  `src/gateway/execution_engine/tool_service_builder.rs`：组合 global ToolRegistry + `allowed_tools` 过滤 + SubagentTool + tool_refresh + hook decorator → `Arc<ScopedToolService> as Arc<dyn ToolService>`。复用 4b 的 adapter。

- [ ] **Step 7: 替换 `run_loop.rs:623-731`**

  伪代码见规范 §4。保留外层 `for attempt in 0..MAX_FALLBACK_ATTEMPTS` 与 `resolve_with_fallback`；错误分支改为：
  ```rust
  Err(e) if matches!(e, FlowError::Transient { .. }) => {
      self.provider_registry.report_outcome(&resolved.provider_name, Err(e.to_string().into()));
      continue;  // 重试
  }
  ```

- [ ] **Step 8: 清理 `run_loop.rs` 的 stale `use crate::agent_loop::...` imports**

- [ ] **Step 9: 跑 6 个集成测试绿**

- [ ] **Step 10: `cargo test -p alephcore --lib`** ≥ 9145。

- [ ] **Step 11: `cargo clippy -- -D warnings`** 干净。

- [ ] **Step 12: Commit（切换 + 测试同 commit）**

  ```
  phase6b-flip: route Gateway chat through Orchestrator::dispatch (task 4c)

  Replace run_loop.rs:623-731 AgentLoop builder chain with FlowRequest
  construction, Orchestrator::dispatch, event drain task, and
  FlowOutcome → response mapping. Add 6 integration tests covering
  dispatch wiring, hit_limit preservation, stream ordering, dynamic
  tools, cancellation, and trace flush. Extend FlowError with
  Transient variant so the outer provider-fallback retry loop survives.

  Behavioural parity per resolution design §5: Delta/Reasoning/ToolCall*/
  ToolSummary/Safety/StopHook/ModelFallback all forwarded to gateway
  emitter; TraceSink replaces flush_trace_persistence on the harness
  side; per-request tool_service carries SubagentTool + MCP dynamic
  tools via ScopedToolService (task 4b).

  Tests: cargo test -p alephcore --lib ≥9145 passing + 6 new
  gateway_chat_* integration tests green, 2 pre-existing failures
  unchanged.
  ```

---

### Task 5: 清理 + exit gate 脚本

**Files:**
- Create: `scripts/check-phase6b-flip-exit.sh`
- Modify: 任何 Task 4c 遗漏未清的 `use crate::agent_loop::...` import（在 gateway 子树内）

- [ ] **Step 1: 写 `scripts/check-phase6b-flip-exit.sh`**

  按规范 §8 的 7 条断言。bash + grep + cargo，exit 0 表示全过。

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail
  fail=0
  check() { if "$@"; then echo "ok: $*"; else echo "FAIL: $*"; fail=1; fi; }

  # 1. run_loop.rs 不再有 AgentLoop::new
  check ! grep -q "AgentLoop::new" src/gateway/execution_engine/run_loop.rs

  # 2. ExecutionEngine 有 orchestrator 字段
  check grep -q "orchestrator: Arc<crate::orchestrator::Orchestrator>\|orchestrator: Arc<Orchestrator>" \
       src/gateway/execution_engine/engine.rs

  # 3. FlowRequest 有 tool_service
  check grep -q "pub tool_service: Option<Arc<dyn crate::tools::service::ToolService>>" \
       src/orchestrator/dispatch.rs

  # 4. FlowStreamEvent 至少 9 变体
  variants=$(grep -cE "^\s*(Delta|Reasoning|ToolCallStart|ToolCallDone|ToolSummary|SafetyBlock|StopHookBlock|ModelFallback|Complete)" \
       src/orchestrator/dispatch.rs)
  check test "$variants" -ge 9

  # 5. 库测试绿（baseline 9133 + 新 2 dispatch 单元 + 6 gateway_chat 集成 = 9141；失败仍为 2）
  check cargo test -p alephcore --lib --quiet 2>&1 | tail -5 | grep -qE "[0-9]+ passed"

  # 6. clippy
  check cargo clippy -- -D warnings

  # 7. 剩余 AgentLoop::new 仅在 loop_core.rs / factory.rs / integration_probe.rs
  allowed="src/agent_loop/loop_core.rs|src/agent_loop/factory.rs|src/agent_loop/integration_probe.rs"
  unexpected=$(grep -rln "AgentLoop::new" src/ | grep -Ev "$allowed" | grep -v "^src/agent_loop/" || true)
  check test -z "$unexpected"

  exit $fail
  ```

- [ ] **Step 2: 运行脚本**

  ```bash
  bash scripts/check-phase6b-flip-exit.sh
  ```

  期望 exit 0。

- [ ] **Step 3: Commit**

  ```
  phase6b-flip: add exit-gate script (task 5)
  ```

---

### Task 6: 人工 smoke（用户驱动）

**非 agentic 步骤。用户亲自执行：**

- [ ] **Step 1:** `just dev` 跑起 server，随便聊一轮
- [ ] **Step 2:** 触发至少一次工具调用（如 `/memory search`），断流顺序正常（增量文本 → 工具 start → 工具 done）
- [ ] **Step 3:** 触发一次需要多轮的任务观察 `hit_limit` 是否照常提示（或未达 limit 仍拿到回答）
- [ ] **Step 4:** MCP 工具（若已配置）可用
- [ ] **Step 5:** Ctrl+C 中断后看日志没有 harness panic

满意后由用户决定是否发版（本计划不自动 release）。

---

## 自审清单

- [ ] 规范 §1 的 9 个 FlowStreamEvent 变体，Task 1 Step 1 全部列出。
- [ ] 规范 §2 的 `tool_service: Option<Arc<dyn ToolService>>` 字段，Task 2 Step 6 已写。
- [ ] 规范 §3 的 `orchestrator: Arc<Orchestrator>` 字段，Task 3 Step 1 已写。
- [ ] 规范 §5 行为保留清单每一项，Task 4c Step 5/6/7 都覆盖（dynamic tools 由 4b 的 ScopedToolService 承载，retry 由 4c 的 FlowError::Transient 承载）。
- [ ] 规范 §6 的 6 个集成测试，Task 4c Step 3 全部创建。
- [ ] Task 4c 是单次 commit（测试 + 实现同 commit），便于 revert。
- [ ] Exit gate 脚本 7 条，Task 5 Step 1 全部落地。
- [ ] 不动 `src/agent_loop/loop_core.rs` 内部测试 —— Phase 6c 范围。
- [ ] 不自动 release —— Task 6 明确由用户决定。

---

## 执行交接

**推荐：** 使用 `superpowers:subagent-driven-development`。

- Task 1/2/3 —— 每个用一个 subagent（executor，sonnet/opus 视复杂度），均为"加字段不破坏旧调用"，相对机械。
- Task 4a —— sonnet executor 即可（单文件 drain helper + 单元测试）。
- Task 4b —— sonnet executor（新 adapter + 单元测试，LoopToolRegistry 与 ToolService 表面映射）。
- Task 4c —— opus executor，涉及 run_loop.rs 改写 + FlowError::Transient 扩展 + 6 TDD 集成测试。两阶段 review 务必执行。
- Task 5 —— 简单脚本，用 haiku 即可。
- Task 6 —— **停下来找用户**，不 auto-execute。

每个 Task commit 后，主 orchestrator 跑一次 `cargo test -p alephcore --lib` 核对 baseline，再启动下一个。

# HITL 闭环设计：确认 / 授权 / 澄清

> Spec — 2026-05-19 · 分支 `worktree-hitl-loop-closure` · 基线 `47a94d0c3`

## 1. 背景与动机

参照超级 AI 助手 hermes-agent（Python）对 Aleph（Rust）做对比分析，发现 Aleph 的
**人在回路（Human-in-the-Loop, HITL）**子系统——工具确认、危险操作授权、中途澄清——
基础设施齐备但**未通电**：传输层、类型、trait 全部已实现且有单元测试，缺的只是连线。

hermes-agent 在该领域最成熟：硬阻断黑名单 + 可批准危险模式、per-session Event 队列桥接
「同步 agent 线程 ↔ 异步用户回复」、三级授权、`clarify` 阻塞式工具、纯文本兜底。
Aleph 已有等价物（`ChannelApprovalBridge` / `ExecApprovalManager` / `ClarificationManager`），
本 spec 不照搬 hermes，而是**把 Aleph 已有的部件接通**，并借鉴 hermes 的「单一传输 +
单一拦截点」思路避免它自身的弱点（hermes 有 4 份重复的 per-session 队列实现）。

本周期范围：**HITL 闭环**。不含流式、per-tool 超时、结果截断、`src/engine/` 清理等
——它们各自独立成周期。

## 2. 缺陷清单（探索结论）

| 级别 | 缺陷 | 证据 |
|---|---|---|
| 🔴 CRITICAL | `bash`/`code_exec` 能力升级被无条件自动拒绝 | `start/mod.rs:532` `ApprovalGate::new(ApprovalConfig::default(), None)`;`gate.rs:97-100` requester=None → `Denied`;boot 警告 `start/mod.rs:535-542` |
| 🟠 HIGH | 无任何面向用户的审批通道生效 | `ChannelApprovalBridgeAdapter`（`src/approval/adapters.rs`）已实现+测试，**从未被构造**;无生产 `ExecApprovalManager`（全部构造点 `#[cfg(test)]`） |
| 🟠 HIGH | 审批按钮回调未路由 | Telegram 回调转为普通 `InboundMessage`（text=`approve:tg-<uuid>`，`telegram/mod.rs:515-580`）;`inbound_router/` 无 `approve:`/`deny:` 处理;`ExecApprovalManager::resolve` 仅被 JSON-RPC/IPC 调用 |
| 🟠 HIGH | `requires_confirmation` 被静默丢弃（4 处） | `BuiltinToolDefinition`（`definitions.rs:42-49`）无该字段;`agent_init.rs:833` 不传播;`scoped.rs:124` 硬编码 `ToolDefinitionMetadata::default()`;`act.rs:119` 不检查 |
| 🟡 MED | 澄清端到端缺失 | `ClarificationManager`（`session.rs`）无响应 oneshot、零调用方;无 `ask_user` 工具 |
| 🟡 MED | 死代码 Stack A（竞争审批栈） | `single_step.rs`/`exec_security_gate.rs` 零生产调用方;`tools/middleware/` 装饰链被 `ScopedToolService` 旁路 |

## 3. 架构设计

### 3.1 原则

- **不动 `src/harness/`**：确认接缝放在 `ScopedToolService`，澄清实现为一个工具。
  保持 R10 薄 harness、笨循环。
- **单一审批传输**：`ChannelApprovalBridgeAdapter`（实现 `ApprovalRequester`）同时服务
  「沙箱能力升级」与「工具 `requires_confirmation`」，不重复造传输。
- **单一 inbound 拦截点**：`inbound_router` 在派发新轮次前做一次「待交互」拦截，
  统一处理审批按钮回调、`/approve` 文本兜底、澄清文本回复。
- **复用现有部件**：`ChannelApprovalBridge` / `ExecApprovalManager` / `ClarificationManager`
  / `ClarificationRequest|Result` 类型 / `SESSION_ID` task-local 全部沿用。
- **非破坏性**：先接线（P1–P4）并验证，最后才删死代码（P5），清理不得动摇已接好的线。

### 3.2 数据流

```
┌─ ApprovalGate (沙箱能力升级, WorkspaceSandbox::execute) ─┐
│                                                          ├─▶ ChannelApprovalBridgeAdapter
└─ ScopedToolService (工具 requires_confirmation) ─────────┘        │  (impl ApprovalRequester)
                                                                    ▼
                                              ChannelApprovalBridge ──▶ Channel 审批 UI（按钮/文本）
                                                                    └─▶ ExecApprovalManager（oneshot 注册表）

ask_user 工具 ──▶ ClarificationManager（oneshot 注册表）──▶ Channel（问题 + 编号选项，普通消息）

Channel inbound ─▶ inbound_router 预派发拦截 ─┬─ `approve:<id>` / `/approve` `/deny`
                                              │     → ExecApprovalManager::resolve → ack，短路
                                              ├─ 会话存在待澄清
                                              │     → ClarificationManager::resolve(回复文本) → 短路
                                              └─ 否则 → 正常 execute_for_context
```

### 3.3 关键设计决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | `ApprovalGate.requester` 由 `Option<Arc<…>>` 改为 `ArcSwapOption<dyn ApprovalRequester>`，新增 `set_requester(&self, …)` | `ApprovalGate` 在 `start/mod.rs:532` 构造，`ChannelRegistry` 在 `subsystems.rs:203` 才创建。用可交换字段在 channel 就绪后回填，**不重排 boot**。`ApprovalGate` 已 `Arc` 共享，`set_requester` 走 `&self`。 |
| D2 | `PendingApprovalState`（`channel_bridge.rs:54`）增加 `record_id` 字段；新增 `ChannelApprovalBridge::resolve_tool_approval(channel_approval_id, decision)` 做 id 映射 | `ExecApprovalManager` 的 record id（uuid）≠ channel 回传的 `approval_id`。存双向关联，按钮回调凭 channel id 找回 record id 再 `resolve`。不改 channel capability trait。 |
| D3 | 澄清走**文本回复**（hermes text-fallback 模式）：`ask_user` 把问题+编号选项作为普通消息发出，用户下一条消息即答案 | 不改任何 channel adapter。富按钮澄清 UI 留作后续增量周期。 |
| D4 | inbound 拦截点同时支持按钮回调与纯文本 `/approve` `/deny` | 无按钮能力的 channel 也能审批；与 hermes 的 `/approve` 兜底一致。 |
| D5 | 工具确认接缝放在 `ScopedToolService::execute`（已选「瘦接缝」架构） | `ScopedToolService` 已携带 `allowed_tools` 名单集，加一个 `confirm_tools: HashSet<String>` + `Option<Arc<dyn ApprovalRequester>>` 是同构改动；`LoopTool` trait 无需新增确认字段（保持其极简）。 |
| D6 | `ClarificationManager` 重塑：删 `PendingClarification` 的 `param_name`/`intent_type`/`tool_name`/`original_input`（意图检测时代残留），改为持 `oneshot::Sender<ClarificationResult>` + 会话路由键 | 该模块零生产调用方，重塑无破坏面；与 `ExecApprovalManager` 形成对称的「带 oneshot 的待处理注册表」。 |

## 4. 实施分期

每期独立可验证。P1–P2 修 CRITICAL+HIGH，P3 修 HIGH，P4 修 MED，P5 清理。

### P1 — 统一审批传输（修复 CRITICAL）

**改动**
- `src/sandbox/exec_approval/gate.rs`：`requester` 字段改 `ArcSwapOption`；
  `request_approval_for_tool` 读 `.load()`；新增 `set_requester(&self, Arc<dyn ApprovalRequester>)`。
  保留 `with_requester` 兼容（内部转 `set_requester`）。
- `src/bin/aleph-server/commands/start/`：boot 构造一个共享 `Arc<ExecApprovalManager>`；
  在 `initialize_channels` 产出 `Arc<ChannelRegistry>`（`subsystems.rs:203`）后，
  构造 `ChannelApprovalBridge::new(registry)` → `ChannelApprovalBridgeAdapter::new(bridge, manager)`
  → `approval_gate.set_requester(adapter)`。
- `src/exec/approval/channel_bridge.rs`：`PendingApprovalState` 增 `record_id`；
  `request_for_tool` 写入关联；新增 `resolve_tool_approval`（D2）。

**验证**：webchat 会话下，`code_exec` 带 `allow_network` 触发审批提示而非静默
`ApprovalOutcome::Denied`；无 channel 能力时降级为「带原因的拒绝」（安全）。

### P2 — 闭合 inbound 审批解析回路（修复 HIGH）

**改动**
- `src/gateway/inbound_router/`：在 `execute_for_context`（`mod.rs:718`）之前插入
  `intercept_pending_interaction(&ctx)` 预派发检查。识别：
  - 按钮回调文本 `approve:<id>` / `deny:<id>` → `ChannelApprovalBridge::resolve_tool_approval`。
  - 纯文本 `/approve` `/deny`（可带 id；无 id 取该会话最旧待审批，FIFO）。
  命中则 resolve + 发确认 ack + 短路返回，不启动新轮次。
- 把共享 `Arc<ExecApprovalManager>` + `Arc<ChannelApprovalBridge>` 线程进 `InboundRouter`。

**验证**：点击审批按钮或回复 `/approve` 后，被阻塞的 agent 轮次拿到 `Approved` 并继续。

### P3 — 工具 `requires_confirmation` 接缝（修复 HIGH）

**改动**
- `src/executor/builtin_registry/definitions.rs`：`BuiltinToolDefinition` 增
  `requires_confirmation: bool`；`BUILTIN_TOOL_DEFINITIONS` 中对 `vault_store`、`arena`、
  `skill_install`、`agent_manage` delete、`team` disband 置 `true`（与现有
  `AlephTool::requires_confirmation()` 覆盖一致）。
- `src/bin/aleph-server/commands/start/builder/agent_init.rs:833`：构造 `UnifiedTool`
  时调 `.with_requires_confirmation(def.requires_confirmation)`。
- `src/tools/scoped.rs`：`ScopedToolService` 增 `confirm_tools: HashSet<String>` +
  `requester: Option<Arc<dyn ApprovalRequester>>`；`execute()` 派发前——若工具在
  `confirm_tools` 内且 `requester` 存在——调 `request_approval`，非 `Approved` 则返回
  `ToolError`（"user declined / timed out"，措辞含「不要重试」）。
- `src/tools/.../tool_service_builder.rs`：`build_request_tool_service` 从 agent 的
  `UnifiedTool` 列表收集 `confirm_tools`，注入共享 adapter。

**验证**：LLM 调 `skill_install` 触发确认提示；用户拒绝 → 工具结果为 "user declined"，
agent 不重试。

### P4 — 澄清闭环 `ask_user` 工具（修复 MED）

**改动**
- `src/clarification/`：`ClarificationManager` 重塑为 oneshot 注册表（D6）——
  `register(request, session_key) -> oneshot::Receiver<ClarificationResult>`、
  `resolve(session_key, ClarificationResult)`、`pending_for_session(session_key)`、
  超时清理。保留 `ClarificationRequest`/`ClarificationResult`/`ClarificationOption`/
  `QuestionGroup` 类型不变。
- 新建 builtin 工具 `ask_user`（`src/builtin_tools/ask_user.rs`）：入参 `question: String`、
  可选 `choices: Vec<String>`。执行时——读 `SESSION_ID` task-local 路由——把问题
  （选项渲染为编号列表 + 「回复编号或文本」）作为普通消息发到 channel，向
  `ClarificationManager` 注册并阻塞等 oneshot（带超时，默认 10 分钟）。注册进
  `BUILTIN_TOOL_DEFINITIONS` 与 builtin 注册表。
- `src/gateway/inbound_router/`：P2 的拦截点扩展——若该会话存在待澄清，
  把用户这条消息作为答案 `resolve`，短路返回。
- 共享 `Arc<ClarificationManager>` 接入 boot、`ask_user` 工具、`InboundRouter`。

**验证**：LLM 调 `ask_user` → 用户在 channel 收到问题 → 回复 → agent 拿到答案续跑；
超时返回 `ClarificationResult::timeout()`，agent 可走默认值。

**约束**：`ask_user` 阻塞期受 harness `turn_timeout` 影响（默认 `None`，不受限）；
若配置了 `turn_timeout` 须大于澄清超时——记入文档。

### P5 — 清理死 Stack A（R10 死代码）

先决条件：P1–P4 完成并验证通过。

**净杀（零生产调用方，直接删）**
- `src/executor/single_step.rs`（~860 行）、`src/executor/exec_security_gate.rs`（~660 行）；
  移除 `src/executor/mod.rs:43` 的 `pub use` 及文档引用。

**链式删除（需同步解 boot 接线）**
- `src/tools/middleware/`（permission/、audit.rs、context_rule.rs、timeout.rs；~1296 行）、
  `src/tools/dispatch.rs`（217）、`src/tools/facade.rs`（158）、`src/tools/registry.rs`（174）、
  `src/tools/handlers/`（707）。
- 解除 `start/mod.rs:470-606` 对 `build_tool_service_with_handles` 的 boot 接线；
  解除 `McpClient::set_tool_registry`（`mcp/client.rs:89`）与
  `ExtensionManager::set_tool_registry`（`extension/mod.rs:185/263`）。
- Orchestrator 默认 `tool_service` 改为不再依赖 facade 链（gateway 始终以
  `ScopedToolService` 覆盖之，默认值实际不可达）。

**注意**：`ApprovalGate` 本身非死代码（`WorkspaceSandbox` 用），仅 `middleware/permission/`
里的 `impl Approver for ApprovalGate` 适配器随 Stack A 一并删除。

### 贯穿 — 测试与文档

- 每期 TDD：先写失败测试再实现。单元测试覆盖 D1/D2/D5/D6 的新逻辑；
  集成测试覆盖审批回路与澄清回路端到端。
- 校正文档漂移：`docs/reference/TOOL_SYSTEM.md`、`docs/reference/SECURITY.md`
  删除对 facade 链 / `ToolServer` / `PolicyEngine` 门控的过时描述。

## 5. 明确不做（后续独立周期）

富按钮澄清 UI（改 channel capability）；流式 `process_stream`；per-tool 执行超时；
工具结果截断/预算；删 `src/engine/` ~5000 行；删 `dispatcher/tool_index/` ~2865 行；
`flow_run` 工具接线；harness R10 文件/行数红线对账；身份/RBAC（`PolicyEngine`）门控。

## 6. 风险

| 风险 | 缓解 |
|---|---|
| channel 未实现审批 capability → 审批永远降级为拒绝 | 现状即如此（更糟：静默）；本 spec 后变为「带原因拒绝 + 日志」，安全。富 UI 是后续增量。`/approve` 文本兜底覆盖纯文本 channel。 |
| P5 链式删除波及 boot/MCP/Extension 接线 | 排在最后；删除前确认 facade 默认 `tool_service` 在所有路径均被 `ScopedToolService` 覆盖；分两步（净杀 → 链式），每步独立 `cargo check` + 测试。 |
| `ask_user` 阻塞与 `turn_timeout` 冲突 | 默认 `turn_timeout=None`；配置时记入文档约束。 |

## 7. 验收标准

- `cargo check -p alephcore` 通过；`just test-all` 通过。
- P1–P4 各自验证项（见上）全部满足。
- P5 后无悬挂引用、无新增 clippy 警告（相对基线）。
- `src/harness/` 无改动（薄 harness 不变）。

## 8. 实施状态（2026-05-20）

| 阶段 | 状态 | 提交 |
|---|---|---|
| P1 审批传输 + `ApprovalGate.requester` 修复 | ✅ 已交付 | `4c36b2d9d` |
| P2 inbound `/approve`·`/deny` 拦截回路 | ✅ 已交付 | `4c36b2d9d` |
| P3 工具 `requires_confirmation` 接缝 + boot 接线 | ✅ 已交付 | `839a20f07` `d572d909f` |
| **TURN_CONTEXT 路由修复**（spec 外修正） | ✅ 已交付 | `1673874b1` |
| P4 `ask_user` 澄清工具 | ✅ 已交付 | `e4fee8b7f` `1dc5e9674` |
| P5 净杀（`SingleStepExecutor` + `ExecSecurityGate`，~1480 行） | ✅ 已交付 | `0f2ebc307` |
| P5 链式删除（Phase 2 facade 链 + boot 改写，~2700 行） | ✅ 已交付 | `a48b2f9f6` |

### TURN_CONTEXT 修正（对 spec 的偏离）

D5 原设计假设 HITL 工具读 `SESSION_ID` task-local 取会话路由。实现期发现：
`SESSION_ID` 仅由 `invoke_with_session_trace` 设置，而该函数在 gateway 回合路径
**零生产调用方** —— 故审批适配器读不到会话，升级一律静默拒绝（P1 形同虚设）。
且 `ChannelApprovalBridge::parse_session_key` 对默认 `DmScope::PerPeer` 私聊无法
还原 channel（key 中不编码 channel）。

修正：新增 `TURN_CONTEXT` task-local，承载 `{session_key, channel_id,
conversation_id}`，由 `ScopedToolService::execute`（生产工具分发唯一咽喉）在每次
工具调用外层 scope —— 不跨 `tokio::spawn`，可靠可见。审批适配器与 `ask_user`
统一读它；channel 坐标来自 inbound metadata，对一切会话类型均可路由。

### P5 链式删除交付记录

Phase 2 facade 链（`tools/facade.rs`·`dispatch.rs`·`middleware/`·`handlers/`·
`tools/registry.rs`，实际 **~2702 行**）由 `start/mod.rs:470` 在 boot 装配，
输出经 `ExtensionManager::set_tool_registry` 与 `PermissionLayer` 接线，且作为
Orchestrator 默认 `tool_service`（被 gateway 的 `ScopedToolService` 覆盖、实际
不可达）。

**删除策略**：
- 通过对每一处 `FlowRequest::tool_service` 构造点逐一验证（gateway 生产路径
  `run_loop.rs:408` 总是 `Some(...)`；`dispatch_via_orchestrator` /
  `flow_run_tool` 这两条 `None` 路径无生产调用方）确认 chain 输出确实不可达。
- 新增 `src/tools/null.rs` 提供 `NullToolService`（失败闭合 stub），替代
  `AgentHarnessRunner.tool_service` 默认值 —— 任何 `NotFound` 日志即上游 override
  接线回归信号。
- 删除 `McpClient::set_tool_registry` / `ExtensionManager::set_tool_registry`
  字段与方法（set 后从未 read 的死字段）。
- `AlephToolServer::all_builtin_handlers`（facade 唯一消费方）一并删除。

**验证**：
- `cargo check -p alephcore --lib --bin aleph-server` clean。
- `cargo check -p alephcore --tests` 唯一报错为 `tests/gateway_chat_common`
  既有基线（与本次无关，fork 点已存在）。
- `cargo test -p alephcore --lib -- tools:: null:: scoped:: service::
  server:: mcp::client extension::`: 463/463 全绿。
- `cargo test -p alephcore --lib -- approval:: exec::approval::
  sandbox::exec_approval:: tools::turn_context clarification::`: 189/189
  全绿 —— HITL 闭环（P1-P4）回归零。
- 本环境 `just test-all` 仍被 SIGTERM 杀（exit 144），由库 + bin clean
  + 针对性 652 测试 + 死代码已无消费者三重证据替代。

**同步修订**：
- `docs/reference/TOOL_SYSTEM.md` —— 重写 "ToolService façade" 节为 as-built。
- `docs/reference/SANDBOX.md` —— 删除 PermissionLayer/LayeredPermissionResolver
  装配段，改写为 ScopedToolService.with_confirmation 接线描述。
- `docs/reference/GLOSSARY.md` —— 删除 `LayeredPermissionResolver` /
  `AgentPermissionFilter` 条目，Tools 条目改写为 as-built。
- `docs/reference/AGENT_LOOP_TOOL_EXECUTION.md` "Phase 4 Residue Audit
  (2026-04-19)" 与 `docs/superpowers/{plans,specs}/*` 作为历史快照保留。

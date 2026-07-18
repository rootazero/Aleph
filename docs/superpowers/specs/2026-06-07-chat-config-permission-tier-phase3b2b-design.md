# Chat/Config 权限分层 Phase 3b-2b 设计 — 发起侧「等待 server 授权」in-band 提示

> 续 Phase 2b（live operator sudo 审批后端）+ 3b-2a（operator 审批卡 Panel）。本期补**发起侧体验**：chat 档设备触发 config 工具被挂起等审批时，往发起者**自己的 run 输出流**注入一条 in-band 提示「⏳ 正在等待管理员授权运行 <工具>…」。**纯 backend（`src/`），零 Panel/WASM 改动、零 event_scope 改动。**

**Goal:** chat 档设备触发的 config 工具进入「等审批」挂起时，发起者在自己的对话流里看到一条「等待授权」提示（替代当前"工具卡永远转圈、无任何说明"的体验）；批准后工具照跑，拒绝/超时走现有 PermissionDenied。

**Architecture:** 提示复用**已存在的 `ResponseChunk` 流事件**（`is_intermediate:true`）——经 `event_bus.publish_frame` 直发到目标 run 的事件总线流，由 **event_bus 订阅端（chat 档设备经 `agent.run` RPC 订阅的 Panel/WS）** 渲染，故无需新事件变体、无需新渲染代码。（注：Telegram/Feishu 的 `ReplyEmitter` 只消费 run 自带 `EventEmitter` 的 `StreamEvent`、不订阅 event_bus 帧——但 channel-backed run 是本机 operator DM，`caller_role` 非 chat 档、不触发 config 门控、根本不走这条 notice 路径，故无影响。）提示在唯一的阻塞层 `OperatorApprovalRequester::request_approval` 内发出（该层已持 `event_bus`、已读 turn context），自然只在真正阻塞等审批的路径触发。因 run 输出流按 `run_id` 定向而挂起点当前没有 run_id，给 `TurnContext` 加一个 `run_id` 字段把它接进来。

**Tech Stack:** Rust（`src/tools/turn_context.rs` + `src/approval/operator_requester.rs`）。零前端。

---

## 背景与约束

- **R10 薄 harness**：提示不是 harness 推理，是审批层（已有 approval 语义的地方）发的一条 best-effort UI 信号。harness Think→Act 循环不变、不感知审批。
- **R7 LLM 主权**：纯基础设施信号，无推理判断。
- **R4**：通道是纯 I/O——它们本就渲染 ResponseChunk，本期不给通道加业务逻辑。
- **用户决策（brainstorm）**：发起侧 surface = **通道无关 in-band 文本提示**（否决了 Panel-专属工具卡 awaiting 态 + event_scope 改动方案，因目前不存在 chat 档 Panel 客户端、且 event_scope 改动安全敏感）。
- **fail-closed / best-effort**：提示失败（run_id 空、publish 出错）绝不拖垮审批主流程，静默跳过。

## 当前缺口（已核实）

- chat 档设备触发 config 工具：`act.rs:171` 先发 `ToolStart`（工具卡=运行中），随后 `execute → dispatch.rs:407 → request_approval` **阻塞在 oneshot 等审批**。harness 不知工具内部在等人。
- 结果：发起侧看到一个**永远转圈的工具卡 / 沉默的对话流**，无任何「在等管理员授权」提示，直到批/拒/超时才出 `ToolEnd`。本期修这个缺口。

## 关键已核实事实（决定设计）

1. **`TurnContext` 构造点 = 15 处**（1 生产 + 14 测试）。生产唯一点 `src/gateway/execution_engine/run_loop.rs:479`，此处 `request` 在手、`request.run_id` 可取。
2. **`is_intermediate:true` 的 ResponseChunk 不持久化**：只即时下发显示，从不进 `add_message_with_run_id`（`execute.rs:378` 只存最终 `response`）。不污染 transcript、不进记忆摄入。
3. **seq 非排序关键**：客户端（`interfaces/webchat/src/views/chat/events.rs`）按到达顺序 append，不按 seq 去重/排序。提示是先于工具输出的独立中间消息，`seq:0` 安全。
4. **`GatewayEventFrame::ResponseChunk`**（`frame.rs:60-70`）字段：`{run_id, seq, delta, full_text, content, chunk_index, is_final, is_intermediate}`，topic `agent.response.chunk` / stream method `stream.response_chunk`。挂起点持 `Arc<GatewayEventBus>` 可直接 `publish_frame` 构造之；唯一缺的是 run_id（本期接入）。
5. **挂起点唯一性**：`confirm_with_memory`（`dispatch.rs:341-450`）中，会话记忆命中（:351-358）/ 否决账本（:367-381）都**早返回不调 `request_approval`**；只有真正要阻塞等审批才走到 `request_approval`（:407）。故在 `request_approval` 内发提示 = 天然只在真阻塞路径发，无假提示。

## 数据流

```
chat 档设备消息 → 触发 config 工具 → execute → ScopedToolService::confirm_with_memory
  ├─ 会话记忆命中 → Ok(放行)                    ← 早返回, 不调 request_approval, 无提示
  ├─ 否决账本命中 → Err(ConfirmDenial)          ← 早返回, 无提示
  └─ 否则 → requester.request_approval(name, reason)   ← 唯一阻塞点
        ① publish_frame(ApprovalRequested{...})                    (现有)
        ② 【新】run_id 非空时:
           publish_frame(ResponseChunk{
             run_id, seq:0, is_intermediate:true, is_final:false,
             delta/full_text/content: "⏳ 正在等待管理员授权运行 `<name>`…",
             chunk_index:0,
           })
        ③ await oneshot 决策                                       (现有)
  → Approved/ApprovedForSession → Ok → 工具执行 → 输出续上
       (run_complete 用 summary.final_response 覆盖中间提示 = 提示短暂可见即消失)
  → Denied/Expired → Err(ConfirmDenial) → ToolError::PermissionDenied → ToolEnd error  (现有)
```

## 组件设计（全在 `src/`）

### 1. `src/tools/turn_context.rs` — `TurnContext` 加 `run_id`

- `TurnContext` struct 加字段 `pub run_id: String`（紧邻 `session_key`）。
- 生产构造点 `src/gateway/execution_engine/run_loop.rs:479` 填 `run_id: request.run_id.clone()`（若 `request` 的 run_id 字段名/类型不同，按实际取；run_id 在该处一定可得）。
- 14 个测试构造点机械补 `run_id: String::new()`（或测试值）。列表见"测试"节。
- 选 `String`（非 `Option`）：空串语义="无 gateway run / 不发提示"，与 best-effort 一致，调用处用 `!run_id.is_empty()` 守卫。

### 2. `src/approval/operator_requester.rs` — `request_approval` 内发提示

- `request_approval`（:52-117）已在 :53 经 `current_turn_context()` 取 `session_key/channel_id/conversation_id`；顺带取 `run_id`。
- 在 publish `ApprovalRequested`（:86-92）**之后**、`await` oneshot **之前**，加：若 `run_id` 非空，构造并 `event_bus.publish_frame(&GatewayEventFrame::ResponseChunk{...})`（字段见数据流 ②）。`publish_frame` 返回 `Result`，错误用 `let _ =` / `if let Err` 记 debug 后忽略（best-effort）。
- 提示文案：core 发固定字符串（不做 i18n，沿用 core 既有 notice 字符串约定）。文案示意：`"⏳ 正在等待管理员授权运行 `{name}`…"`（`name` = 工具名）。最终用词实现时定，保持简洁单行。
- `ScopedToolService` / `dispatch.rs` **零改动**（提示完全 co-located 在 requester 层）。

## 错误处理

- `run_id` 空（CLI/直调/测试无 gateway run）→ 跳过提示，审批照常。
- `publish_frame` 出错 → 记 debug 日志后忽略，不影响审批。
- 本机 daemon（`caller_role:None`）不过 config 门控 → 不调 `request_approval` → 无提示。
- 批准/拒绝/超时的最终态由**现有路径**呈现（工具输出 / PermissionDenied 错误），本期不加解决态提示。

## 不做（明确排除）

- 不发「✅ 已授权」/「❌ 被拒绝」解决态提示（YAGNI——工具输出/现有错误即信号）。
- 零 Panel/WASM 改动（复用已渲染的 ResponseChunk）。
- 零新 `StreamEvent`/`GatewayEventFrame` 变体。
- 零 `event_scope` 改动（提示走发起者本就授权接收的自有 run 流，不碰 `approval.*` 门控）。
- 不做 Panel-专属工具卡「awaiting」态（留给未来 browser 配对档位化 phase，那时才存在 chat 档 Panel 客户端）。
- 不为非 config 审批（另一 `approval_requester`）加提示——本期只针对 config 工具的 operator 审批。

## 测试

- **核心单测**（`cargo test -p alephcore`，新增于 `operator_requester` 测试或 `src/tools/scoped/tests.rs`）：
  - turn context 带非空 `run_id` 时调 `request_approval` → 断言 event_bus 收到一条 `ResponseChunk{is_intermediate:true, is_final:false, run_id 匹配}`（用测试 event_bus 订阅/typed_sender 捕获）。
  - turn context `run_id` 空时 → 断言**无** ResponseChunk 发出（仍发 ApprovalRequested）。
  - 现有审批测试（approve/deny/timeout、session-memory 命中不提示）仍绿。
- **编译**：改 `TurnContext` 后跑 `cargo check -p alephcore --all-targets`（`--all-targets` 才编译 `tests/` 集成测试，Phase 3a 教训）。
- **无 Panel 验证**：零前端改动，不需 `just wasm`。

## TurnContext 14 个测试构造点（补 `run_id: String::new()`）

`src/tools/turn_context.rs:70`、`src/tools/scoped/tests.rs:{1155,1316,1371,1390,1442,1468}`、`src/builtin_tools/select_model.rs:121`、`src/builtin_tools/ask_user.rs:{236,271}`、`src/builtin_tools/desktop/tests.rs:{421,440,467}`、`src/approval/adapters.rs:{135,202}`。（行号为探查快照，实现时以实际 grep 为准；任何 `TurnContext { ... }` 字面量都需补字段。）

## 部署说明

纯 backend：见效需重编 `aleph-server` + 热替换 daemon（无 `just wasm`）。可与 3b-1/3b-2a 的 Panel 部署合并，时机由用户定。

## Git 约束（继承本会话纪律）

- 共享单分支 main + 并发提交者：只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；仅用户要求才 push；提交信息英文、无 attribution footer；提交前 `git status` 确认不卷入他人 WIP（工作区有 dist 产物未暂存，勿 staged）。

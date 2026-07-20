# Chat/Config 权限分层 Phase 3b-2a 设计 — Operator 审批卡

> 续 Phase 2b（live operator sudo 审批后端：chat 档调 config 工具→挂起→operator 经 `exec.approval.resolve` 批/拒/超时）。本期把审批接到 Leptos Panel 通知中心：operator 看到挂起的授权请求并一键批准一次/本会话批准/拒绝。**纯 Panel UI，零后端改动**——Phase 2b 的事件、pending 查询、resolve RPC 全部就绪。

**Goal:** Operator 在 Panel 通知中心看到挂起的操作授权请求（含被请求的 config 工具名 + 请求方 agent + 过期倒计），一键 `批准一次 / 本会话批准 / 拒绝`。

**Architecture:** 复用 Phase 2b 全部后端。本地桌面 Panel 以 operator(`*`) 连接 → `event_scope` 放行 `approval.*` 事件 → Panel 订阅 `approval.**`。因 `ApprovalRequested` 事件稀疏（只 id/session_key/channel_id/conversation_id），卡片内容以 **`exec.approvals.pending` RPC 为唯一数据源（SSOT）**；事件仅作刷新触发器（收到任意 approval.* → 重拉 pending → 替换信号）。决策经 operator-gated `exec.approval.resolve` 下发。

**Tech Stack:** Leptos/WASM Panel（验证 = `just wasm` build，不 cargo-check）、leptos-i18n（en.json + zh.json 并行）。零 Rust 后端改动。

**Spec 前序:** Phase 3b-2 经用户拆分为 3b-2a（本期 operator 审批卡，后端就绪）+ 3b-2b（发起侧「等待 server 授权」态，推迟——需安全敏感的 event_scope 改动让 chat 档收到自己 session 的审批事件 + 聊天时间线新态，且眼下无真实 chat 档 remote Panel 触发）。

---

## 背景与约束

- **R4**：Panel 纯 I/O，不做业务逻辑——审批决策语义全在 Core，Panel 只渲染 pending + 发 resolve RPC。
- **零后端改动**（关键事实，已逐一核实）：
  - 事件 `GatewayEventFrame::{ApprovalRequested, ApprovalResolved, ApprovalExpired}`（`src/gateway/events/frame.rs:139-154`），topic `approval.requested/resolved/expired`（frame.rs:372-374）已由 `OperatorApprovalRequester`（`src/approval/operator_requester.rs:85-92`）发布。
  - `event_scope.rs:43` 把 `approval.` 放行给 `["admin","exec.approver"]` + 通配 `["*"]`；本地 shared-token 连接 = operator 持 `["*"]` → 收得到。
  - `exec.approvals.pending` RPC（注册 `src/gateway/handlers/exec_approvals.rs:139`，handler `handle_approvals_pending` :286-293）返回 `PendingListResponse { pending: Vec<PendingApproval> }`，`PendingApproval { record: ExecApprovalRecord, remaining_ms: u64 }`。**非 operator-gated**（reads 开放，method_authz 注释明示），本地 Panel 是 operator 不受影响。
  - `exec.approval.resolve {id, decision, resolved_by}` RPC（handler exec_approvals.rs:217-237）**operator-gated**（method_authz.rs:132 OPERATOR_METHODS）。
- **decision wire 值是 kebab-case**（`ApprovalDecisionType` `#[serde(rename_all = "kebab-case")]` `src/exec/socket.rs:91`）：`"allow-once"` / `"allow-session"` / `"allow-always"` / `"deny"`。本期用 `allow-once` / `allow-session` / `deny`（Phase 2b 中 allow-always 折叠为 session，故不暴露）。
- **fail-closed**：resolve 失败 console 记录；不静默假装成功。

## 关键数据结构（后端，只读引用）

`ExecApprovalRecord`（`src/exec/manager.rs`）字段：`id, command, cwd, host, agent_id, session_key, executable, resolved_path, created_at_ms, expires_at_ms, resolved_at_ms, decision, resolved_by`。对 config 工具审批，`OperatorApprovalRequester`（operator_requester.rs:64-71）设 `command = tool_name`、`agent_id = 请求 agent`、`session_key`、`cwd: None`。故卡片用 `command`(工具名) + `agent_id`(请求方) + `remaining_ms`(过期)。

## 数据流

### 列出 pending（卡片内容）
```
订阅建立 (app.rs:104 同址) → subscribe_topic("approval.**") + 立即拉一次 exec.approvals.pending
收到 approval.requested/resolved/expired 事件 → 重拉 exec.approvals.pending → 替换 pending_approvals 信号
notification_center 渲染每个 PendingApprovalView { id, command, agent_id, remaining_ms }
```

### 决策下发
```
卡片按钮 on:click → ExecApprovalApi::resolve(id, decision)
  → exec.approval.resolve {id, decision:"allow-once"|"allow-session"|"deny", resolved_by:"Operator (Panel)"}
  → (Phase 2b 已实现) manager.resolve 唤醒挂起的工具调用 → 放行/拒绝
  → 乐观移除该卡 + ApprovalResolved 事件触发重拉确认
```

## 组件设计（纯 Panel）

**1. `interfaces/webchat/src/api/`（exec approval API）**
- 新增 `ExecApprovalApi`，放新文件 `interfaces/webchat/src/api/exec_approval.rs`，并在 `interfaces/webchat/src/api.rs` 加 `pub mod exec_approval;`（同 security 模式，约 :33）+ `pub use exec_approval::*;`（约 :59）：
  - `list_pending(state) -> Result<Vec<PendingApprovalView>, String>`：调 `exec.approvals.pending`（无参），反序列化 `{pending: [{record: {id, command, agent_id, ...}, remaining_ms}]}` 取需要字段映射为 `PendingApprovalView`。
  - `resolve(state, id, decision) -> Result<(), String>`：调 `exec.approval.resolve {id, decision, resolved_by:"Operator (Panel)"}`（decision 为 kebab 字面量）。

**2. `interfaces/webchat/src/state/notifications.rs`**
- 新增 `PendingApprovalView { id: String, command: String, agent_id: String, remaining_ms: u64 }`（镜像 `IncomingPairing` 模式，display-only）。

**3. `interfaces/webchat/src/context.rs`**
- `DashboardState` 加字段 `pub pending_approvals: RwSignal<Vec<PendingApprovalView>>`（紧邻 `incoming_pairings` :115）+ `approval_subscription_id: StoredValue<Option<usize>>`（镜像 `pairing_subscription_id` :118）；构造默认 :170-171 同址初始化。
- 新增 `setup_approval_subscriptions(&self) -> Result<(), String>`（照搬 `setup_pairing_subscriptions` :774-832）：`subscribe_topic("approval.**")` → 立即 `ExecApprovalApi::list_pending` 填信号 → 注册 `subscribe_events` handler 对 `approval.requested|approval.resolved|approval.expired` 重拉 pending 并 set 信号。
- 在 `app.rs:104` `setup_pairing_subscriptions` 同址调用 `setup_approval_subscriptions`。

**4. `interfaces/webchat/src/components/notification_center.rs`**
- 读 `dashboard.pending_approvals`；在配对区之后、系统告警区之前插入「操作授权」卡区：每张卡显示标题(操作授权)/工具名(command)/请求方(agent_id)/约 Ns 过期 + 三钮（批准一次/本会话批准/拒绝）。
- 按钮 on:click 调 `ExecApprovalApi::resolve` + 乐观移除（`pending_approvals.update(|l| l.retain(|a| a.id != id))`）。
- `badge_count` Memo（:44）+= `pending_approvals.get().len()`。

**5. i18n（`en.json` + `zh.json` 并行）**
- `notifications` 段加：审批标题、请求方标签、过期标签、三按钮文案。

## 错误处理

- `list_pending` / `resolve` 失败：`web_sys::console::error`（与现有 pairing/revoke 一致）。Panel 无 toast/modal 基建，不为本期新建。
- 乐观移除后若 resolve 实际失败：下次任意 approval 事件重拉会纠正（pending 仍在则卡复现）。可接受。

## 不做（明确排除）

- 不改任何 Rust 后端（事件/pending/resolve 全就绪）。
- 不做发起侧「等待 server 授权」态（3b-2b 推迟）。
- 不暴露 allow-always（Phase 2b 折叠为 session）。
- 不做实时跳秒倒计时（用 remaining_ms 折算的静态「约 Ns」，避免引入 interval timer）。
- 不为 Panel 新增错误 toast/modal 基建。

## 测试

- **后端**：零改动，无新测试。
- **Panel（Leptos/WASM）**：组件无法 cargo-check，验证 = `just wasm`。逻辑薄（订阅+透传+刷新），靠 build + 部署人工点验。

## 部署说明

Panel 见效需 `just wasm` 重建 dist + 重编 `aleph-server`（rust_embed 烧 dist）+ 热替换 daemon（CLAUDE.md Panel↔Daemon 嵌入链）。3b-1 + 3b-2a 可统一部署，时机由用户定。

## Git 约束（继承本会话纪律）

- 共享单分支 main + 并发提交者：只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；仅用户要求才 push；提交信息英文、无 attribution footer。

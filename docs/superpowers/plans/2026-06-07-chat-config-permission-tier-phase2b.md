# chat/config 权限分层 Phase 2b — live operator sudo 审批 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 把 Phase 2 的「chat 档调 config 工具→硬拒」升级为「挂起→operator 现场审批→批准则放行/拒绝或超时则 PermissionDenied」，复用既有 `ExecApprovalManager` + `exec.approval.resolve` RPC + `ApprovalRequested` 事件。仅做 AllowOnce + AllowSession（复用 session_memory）；**永久设备提升留 Phase 3**。

**Architecture:** chat 档命中 config 工具时，dispatch 门控不再直接 `PermissionDenied`，而是经一个**operator 定向** `ApprovalRequester`（新 `OperatorApprovalRequester`）走既有 `confirm_with_memory`（自带 session_memory + denial_ledger + observer）。该 requester：构造极简 `ApprovalRequest{command=tool_name}` → `ExecApprovalManager::create` → 发布 `GatewayEventFrame::ApprovalRequested`（operator 端 Panel/桌面通知可见）→ `wait_for_decision`（2min 超时）挂起 → operator 经已 operator 门控的 `exec.approval.resolve` RPC 批准 → oneshot 唤醒 → 映射 decision→`ApprovalOutcome`。批准则门控放行（继续执行），拒绝/超时回 `ToolError::PermissionDenied`。

**Tech Stack:** Rust，复用 `src/exec/manager.rs` 的 register/resolve/oneshot，`GatewayEventBus::publish_frame`，既有 `ApprovalRequester` trait + `confirm_with_memory`。

**关键不变量：**
- operator / 本机无鉴权 / 非网关 run 不受影响（门控只在 chat 档触发，同 Phase 2）。
- 无 config approver 注入时（如测试、未 boot）→ **fail closed = 硬拒**（与 Phase 2 行为一致，绝不静默放行）。
- 审批请求事件**只投 operator**（chat 档收不到，不能批自己的请求）。

---

## 背景：已勘察的复用点（精确）

- **`ExecApprovalManager`**（`src/exec/manager.rs`）：`new()`；`create(&ApprovalRequest, timeout_ms) -> ExecApprovalRecord`（仅生成记录，含 `record.id`）；`wait_for_decision(record) -> Option<ApprovalDecisionType>`（注册 oneshot + `expires_at` 超时，超时/关闭返回 `None`）；`resolve(&id, decision, resolved_by) -> bool`。常量 `DEFAULT_APPROVAL_TIMEOUT_MS = 120_000`。
- **`ApprovalRequest`**（`src/exec/decision.rs`）：`{ id: String, command: String, cwd: Option<String>, analysis: CommandAnalysis, agent_id: String, session_key: String }`（无 `new`，字面量构造）。`from_request` 只从 `analysis.segments.first()` 取 executable/resolved_path（空 segments → executable=""，可接受）。
- **`CommandAnalysis`**（`src/exec/analysis.rs`）：字段全 pub `{ ok: bool, reason: Option<String>, segments: Vec<CommandSegment>, chains: Option<Vec<Vec<CommandSegment>>> }`。空构造：`CommandAnalysis { ok: true, reason: None, segments: vec![], chains: None }`（若有 `::empty()`/`Default` 优先用）。
- **`ApprovalDecisionType`**（`src/exec/socket.rs`）：`AllowOnce | AllowSession | AllowAlways | Deny`。
- **`exec.approval.resolve`** RPC（`src/gateway/handlers/exec_approvals.rs`）：`ApprovalResolveParams { id, decision, resolved_by }` → `manager.resolve(...)`。已在 `method_authz` OPERATOR 集——operator 才能批。**直接复用，本期不改。**
- **`GatewayEventFrame::ApprovalRequested { approval_id, session_key, channel_id, conversation_id }`**（`src/gateway/events/frame.rs`，topic `"approval.requested"`）+ `ApprovalResolved`/`ApprovalExpired`。**已定义，全代码无人发布**——本期首次发布。
- **`GatewayEventBus::publish_frame(&self, frame) -> Result<usize, serde_json::Error>`**（`src/gateway/event_bus.rs`）。
- **`ApprovalRequester` trait**（`src/sandbox/exec_approval/gate.rs`）：`async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome`。`ApprovalOutcome { Approved, ApprovedForSession, Denied, Timeout }`。
- **`ScopedToolService::confirm_with_memory(&self, requester: &Arc<dyn ApprovalRequester>, name, reason) -> Result<(), ConfirmDenial>`**（`src/tools/scoped/dispatch.rs`）：已封装 session_memory 短路 + denial_ledger 防盲目重试 + observer 触发 + `requester.request_approval`。**接收 requester 形参——可直接喂 config requester。**
- **Phase 2 门控**（`dispatch.rs::execute_inner`，`is_allowed`→**tier gate**→confirm）当前 chat 档直接 `PermissionDenied`。
- **boot 接线**（`src/bin/aleph-server/commands/start/mod.rs` ~2025-2067）：构造 `ChannelApprovalBridgeAdapter` + `set_confirmation_requester`。`exec_approval_manager` 在此 scope 内可用。

### 两个必修的既有 bug（否则 2b 不正确）

1. **topic 错配**：`event_scope::default_rules()` 用 `"exec.approval."` 前缀，但帧 topic 是 `"approval.requested"`（无 `exec.`）→ 当前 `approval.*` **不设防**，chat 档会收到审批请求事件（能看/批自己的请求）。修：加 `"approval."` 前缀规则。
2. **`["*"]` 通配失效**：operator 权限 = `["*"]`，但 `can_receive` 做字面 `required.contains(p)`，`"*"` 不匹配 `["admin",...]` → operator 收不到任何受控事件（pairing.requested 同样 latent 受影响）。修：`can_receive` 把 `"*"` 当超级通配。

---

## File Structure

- **Modify** `src/gateway/event_scope.rs` — `can_receive` 通配修复 + 加 `"approval."` 规则 + 测试。
- **Create** `src/approval/operator_requester.rs` — `OperatorApprovalRequester`（impl `ApprovalRequester`，operator 定向）+ 纯映射 fn + 测试。
- **Modify** `src/approval/mod.rs` — `pub mod operator_requester;` + 重导出。
- **Modify** `src/tools/scoped/mod.rs`（字段）+ `src/tools/scoped/builder.rs`（`with_config_approval`）— `ScopedToolService.config_approval_requester`。
- **Modify** `src/gateway/execution_engine/tool_service_builder.rs` — `CONFIG_APPROVAL_REQUESTER` OnceLock + `set_config_approval_requester` + `build_request_tool_service` 接线。
- **Modify** `src/gateway/execution_engine/mod.rs` — 重导出 `set_config_approval_requester`。
- **Modify** `src/tools/scoped/dispatch.rs` — 门控由硬拒改「经 config requester 求批，批准放行/否则 PermissionDenied」+ 更新/新增测试。
- **Modify** `src/bin/aleph-server/commands/start/mod.rs` — 构造 `OperatorApprovalRequester` + `set_config_approval_requester`。

---

## Task 1: event_scope 双 bug 修复（通配 + approval. 规则）

**Files:** Modify `src/gateway/event_scope.rs`

- [ ] **Step 1: 写失败测试** — 追加到该文件 `#[cfg(test)] mod tests`（无则新建）：

```rust
#[test]
fn wildcard_permission_satisfies_guarded_topics() {
    let g = EventScopeGuard::default_rules();
    let star = vec!["*".to_string()];
    assert!(g.can_receive("approval.requested", &star), "operator [*] must receive approval events");
    assert!(g.can_receive("pairing.requested", &star), "operator [*] must receive pairing events");
    assert!(g.can_receive("config.changed", &star));
}

#[test]
fn chat_tier_excluded_from_approval_events() {
    let g = EventScopeGuard::default_rules();
    let chat = vec!["chat".to_string(), "read".to_string()];
    assert!(!g.can_receive("approval.requested", &chat), "chat tier must NOT see approval requests");
    assert!(!g.can_receive("approval.resolved", &chat));
    assert!(g.can_receive("agent.run.started", &chat), "unguarded topics still flow");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib event_scope::tests::wildcard_permission_satisfies_guarded_topics`
Expected: FAIL（`["*"]` 当前不匹配 → `can_receive` 返回 false）。
Run: `cargo test -p alephcore --lib event_scope::tests::chat_tier_excluded_from_approval_events`
Expected: FAIL（`approval.` 当前无规则 → `can_receive` 返回 true，断言 `!` 失败）。

- [ ] **Step 3: 修复 `can_receive` 通配** — 把（约 60-69 行）：

```rust
            return permissions.iter().any(|p| required.contains(p));
```
改为：
```rust
            // A device holding the `"*"` wildcard (operator / local daemon) is a
            // superuser and satisfies every scope rule. Otherwise it needs at
            // least one of the topic's required permissions.
            return permissions.iter().any(|p| p == "*" || required.contains(p));
```

- [ ] **Step 4: 加 `approval.` 规则** — 在 `default_rules()` 的 `rules: vec![...]` 内（紧邻 `exec.approval.` 那条）追加：

```rust
            (
                "approval.".to_string(),
                vec!["admin".to_string(), "exec.approver".to_string()],
            ),
```

> 保留既有 `"exec.approval."` 规则不动（向后兼容）。新增 `"approval."` 覆盖 `GatewayEventFrame` 实际发布的 topic。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p alephcore --lib event_scope::tests`
Expected: 全 PASS（含既有 + 2 新）。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/event_scope.rs
git commit -m "gateway: treat [*] as wildcard + guard approval.* events to operators"
```

---

## Task 2: OperatorApprovalRequester（operator 定向求批）

**Files:** Create `src/approval/operator_requester.rs`；Modify `src/approval/mod.rs`

- [ ] **Step 1: 写失败测试 + 实现骨架** — 新建 `src/approval/operator_requester.rs`：

```rust
//! `OperatorApprovalRequester` — an [`ApprovalRequester`] that routes a config
//! tool approval to the SERVER OPERATOR (not the requesting chat-tier device).
//!
//! Unlike `ChannelApprovalBridgeAdapter` (which delivers back to the
//! requester's own channel), this requester registers a pending approval in the
//! shared [`ExecApprovalManager`] and publishes a `GatewayEventFrame::Approval*`
//! event that — after the event_scope `approval.` guard — only operator-tier
//! connections receive. The operator resolves it via the existing
//! `exec.approval.resolve` RPC, waking the oneshot. Used by the config-tier
//! gate in `ScopedToolService` (Phase 2b sudo).
//!
//! Scope (Phase 2b): AllowOnce + AllowSession only. `AllowAlways` is mapped to
//! session-grant — permanent device elevation is Phase 3 (Devices UI + RPC).

use async_trait::async_trait;

use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::frame::GatewayEventFrame;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::sync_primitives::Arc;

/// Maps an `ExecApprovalManager` decision into an `ApprovalOutcome`.
/// `None` = timed out / channel closed. `AllowAlways` collapses to a session
/// grant in Phase 2b (permanent device elevation deferred to Phase 3).
fn decision_to_outcome(decision: Option<ApprovalDecisionType>) -> ApprovalOutcome {
    match decision {
        Some(ApprovalDecisionType::AllowOnce) => ApprovalOutcome::Approved,
        Some(ApprovalDecisionType::AllowSession) => ApprovalOutcome::ApprovedForSession,
        Some(ApprovalDecisionType::AllowAlways) => ApprovalOutcome::ApprovedForSession,
        Some(ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
        None => ApprovalOutcome::Timeout,
    }
}

pub struct OperatorApprovalRequester {
    manager: Arc<ExecApprovalManager>,
    event_bus: Arc<GatewayEventBus>,
}

impl OperatorApprovalRequester {
    pub fn new(manager: Arc<ExecApprovalManager>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self { manager, event_bus }
    }
}

#[async_trait]
impl ApprovalRequester for OperatorApprovalRequester {
    async fn request_approval(&self, tool_name: &str, _reason: &str) -> ApprovalOutcome {
        // Originating session/channel for correlation + event routing. The gate
        // calls this inside the TURN_CONTEXT scope, so it is present for gateway
        // runs; absent for non-gateway runs (which never reach a chat-tier gate).
        let turn = crate::tools::turn_context::current_turn_context();
        let (session_key_str, agent_id, channel_id, conversation_id) = match &turn {
            Some(t) => (
                t.session_key.to_key_string(),
                t.session_key.agent_id().to_string(),
                t.channel_id.clone(),
                t.conversation_id.clone(),
            ),
            None => (String::new(), String::new(), String::new(), String::new()),
        };

        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: tool_name.to_string(),
            cwd: None,
            analysis: crate::exec::analysis::CommandAnalysis {
                ok: true,
                reason: None,
                segments: vec![],
                chains: None,
            },
            agent_id,
            session_key: session_key_str.clone(),
        };
        let record = self.manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
        let approval_id = record.id.clone();

        // Surface to operator-tier connections (event_scope `approval.` guard).
        if let Err(e) = self.event_bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
            approval_id: approval_id.clone(),
            session_key: session_key_str.clone(),
            channel_id,
            conversation_id,
        }) {
            tracing::warn!(error = %e, "failed to publish ApprovalRequested for config approval");
        }

        let decision = self.manager.wait_for_decision(record).await;

        // Best-effort resolution event for operator UIs.
        let frame = match decision {
            Some(d) => GatewayEventFrame::ApprovalResolved {
                approval_id,
                session_key: session_key_str,
                decision: d,
                resolved_by: None,
            },
            None => GatewayEventFrame::ApprovalExpired {
                approval_id,
                session_key: session_key_str,
            },
        };
        let _ = self.event_bus.publish_frame(&frame);

        decision_to_outcome(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_mapping() {
        assert_eq!(decision_to_outcome(Some(ApprovalDecisionType::AllowOnce)), ApprovalOutcome::Approved);
        assert_eq!(decision_to_outcome(Some(ApprovalDecisionType::AllowSession)), ApprovalOutcome::ApprovedForSession);
        assert_eq!(decision_to_outcome(Some(ApprovalDecisionType::AllowAlways)), ApprovalOutcome::ApprovedForSession);
        assert_eq!(decision_to_outcome(Some(ApprovalDecisionType::Deny)), ApprovalOutcome::Denied);
        assert_eq!(decision_to_outcome(None), ApprovalOutcome::Timeout);
    }
}
```

> **实现者注意**：上面的 `use` 路径与字段名是依据勘察写的，可能与真实模块路径有细微出入（如 `GatewayEventFrame` 的实际模块、`SessionKey::to_key_string()`/`agent_id()` 的确切名、`CommandAnalysis` 是否有 `empty()`/`Default`、`ApprovalRequest` 字段顺序）。**先 `cargo check` 跟随编译器修正路径/构造**，production 逻辑（map + create + publish + wait + resolve event）保持不变。若 `GatewayEventBus` 无 public test 构造器，Step 3 的 round-trip 测试可降级为仅 `decision_mapping`（已覆盖核心风险逻辑）。

- [ ] **Step 2: 注册模块** — `src/approval/mod.rs` 加：

```rust
pub mod operator_requester;
```
（如该 mod.rs 习惯重导出，加 `pub use operator_requester::OperatorApprovalRequester;`，与邻近风格一致。）

- [ ] **Step 3:（可选）round-trip 测试** — 若 `ExecApprovalManager::new()` 与 `GatewayEventBus`（找其构造器，如 `GatewayEventBus::new()`/`default()`）都可在测试构造，加一个 `#[tokio::test]`：在一个 task 起 `request_approval`，主 task 经 manager 列出 pending（用 `exec.approvals.pending` 背后的 manager 方法，grep `pub fn` in `manager.rs` 找列举/`resolve` 入口）取 id 后 `manager.resolve(&id, AllowOnce, None)`，断言返回 `Approved`。若构造或列举 API 不便，**跳过本 step 并在报告说明**——`decision_mapping` 已锁定映射逻辑，完整环路由 Task 4 门控测试（stub requester）+ 人工验证覆盖。

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test -p alephcore --lib approval::operator_requester`
Expected: PASS。
Run: `cargo check -p alephcore --all-targets`
Expected: 通过。

- [ ] **Step 5: 提交**

```bash
git add src/approval/operator_requester.rs src/approval/mod.rs
git commit -m "approval: operator-targeted approval requester for config sudo"
```

---

## Task 3: ScopedToolService.config_approval_requester + 接线

**Files:** Modify `src/tools/scoped/mod.rs`、`src/tools/scoped/builder.rs`、`src/gateway/execution_engine/tool_service_builder.rs`、`src/gateway/execution_engine/mod.rs`

- [ ] **Step 1: 加字段** — `src/tools/scoped/mod.rs` 的 `ScopedToolService` struct，在 `approval_requester` 字段旁加：

```rust
    /// Operator-targeted approval requester for config-tier tools (Phase 2b
    /// sudo). Distinct from `approval_requester` (which routes to the
    /// requester's OWN channel for `requires_confirmation`); this one routes to
    /// the server operator. `None` → config gate hard-rejects (fail closed).
    pub(super) config_approval_requester: Option<Arc<dyn crate::sandbox::exec_approval::gate::ApprovalRequester>>,
```

- [ ] **Step 2: builder 默认值 + setter** — `src/tools/scoped/builder.rs`：在 `ScopedToolService::new` 的字面量初始化处（与 `approval_requester: None` 同处）加 `config_approval_requester: None,`；并加方法（紧邻 `with_confirmation`）：

```rust
    /// Wire the operator-targeted config approval requester (Phase 2b sudo).
    pub fn with_config_approval(
        mut self,
        requester: Arc<dyn crate::sandbox::exec_approval::gate::ApprovalRequester>,
    ) -> Self {
        self.config_approval_requester = Some(requester);
        self
    }
```

- [ ] **Step 3: 全局 setter** — `src/gateway/execution_engine/tool_service_builder.rs`：镜像 `CONFIRMATION_REQUESTER`，在其下加：

```rust
/// Process-wide config-tier approval requester (Phase 2b sudo), installed by
/// boot once the gateway event bus + exec approval manager exist.
static CONFIG_APPROVAL_REQUESTER: OnceLock<Arc<dyn ApprovalRequester>> = OnceLock::new();

/// Install the process-wide config-tier approval requester. Called once at boot.
pub fn set_config_approval_requester(requester: Arc<dyn ApprovalRequester>) {
    let _ = CONFIG_APPROVAL_REQUESTER.set(requester);
}
```
（确认文件顶部已 `use` `ApprovalRequester` 与 `OnceLock`；`set_confirmation_requester` 已用，故应都在。）

- [ ] **Step 4: build_request_tool_service 接线** — 同文件，在现有 `if let Some(requester) = CONFIRMATION_REQUESTER.get() { svc = svc.with_confirmation(...); }` 之后加：

```rust
    // Phase 2b: operator-targeted approval for config-tier tools invoked by a
    // chat-tier connection. Inert until boot installs the requester (then the
    // config gate suspends for operator approval instead of hard-rejecting).
    if let Some(requester) = CONFIG_APPROVAL_REQUESTER.get() {
        svc = svc.with_config_approval(Arc::clone(requester));
    }
```

- [ ] **Step 5: 重导出** — `src/gateway/execution_engine/mod.rs`：在 `pub use tool_service_builder::set_confirmation_requester;` 旁加：

```rust
pub use tool_service_builder::set_config_approval_requester;
```

- [ ] **Step 6: 编译**

Run: `cargo check -p alephcore --all-targets`
Expected: 通过（字段加进 struct，所有 `ScopedToolService` 构造经 builder `new`，无字面量遗漏；若有直接字面量构造 ScopedToolService 的地方报缺字段，补 `config_approval_requester: None`）。

- [ ] **Step 7: 提交**

```bash
git add src/tools/scoped/mod.rs src/tools/scoped/builder.rs src/gateway/execution_engine/tool_service_builder.rs src/gateway/execution_engine/mod.rs
git commit -m "tools: add config_approval_requester seam to ScopedToolService"
```

---

## Task 4: 门控由硬拒改「求批→放行/拒绝」+ 测试

**Files:** Modify `src/tools/scoped/dispatch.rs`、`src/tools/scoped/tests.rs`

- [ ] **Step 1: 改测试** — `src/tools/scoped/tests.rs`：
  - 保留 Phase 2 的 `operator_tier_allowed_config_tool`、`no_turn_context_allows_config_tool` 不变（无 requester 时它们仍应通过：operator/no-context 根本不进求批分支）。
  - 把 `chat_tier_blocked_from_config_tool` 的语义明确为「**无 config requester 注入 → 仍硬拒**」（fail closed），不改其断言（仍 `PermissionDenied`）。
  - 新增两个测试：注入一个 stub requester（impl `ApprovalRequester`，返回固定 outcome）经 `with_config_approval`，验证 chat 档批准→执行成功、拒绝→PermissionDenied：

```rust
struct StubApprover(crate::sandbox::exec_approval::gate::ApprovalOutcome);
#[async_trait::async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for StubApprover {
    async fn request_approval(&self, _tool: &str, _reason: &str)
        -> crate::sandbox::exec_approval::gate::ApprovalOutcome { self.0 }
}

#[tokio::test]
async fn chat_tier_config_tool_approved_executes() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(StubTool { tool_name: "cron_manage" }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("a"),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
        })
        .with_config_approval(Arc::new(StubApprover(ApprovalOutcome::Approved)));
    assert!(svc.execute("cron_manage", json!({})).await.is_ok(),
        "operator-approved config tool must execute");
}

#[tokio::test]
async fn chat_tier_config_tool_denied_rejected() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(StubTool { tool_name: "cron_manage" }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("a"),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
        })
        .with_config_approval(Arc::new(StubApprover(ApprovalOutcome::Denied)));
    let err = svc.execute("cron_manage", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }),
        "operator-denied config tool must be PermissionDenied, got {err:?}");
}
```
> 用 Task 4(Phase 2) 已确认存在的 `StubTool { tool_name: &'static str }` + `make_registry` 风格；若该 stub 字段/构造不同，照文件实际调整。`async_trait` 若非 crate 既有依赖，按文件已用的 attribute 风格（`#[async_trait::async_trait]` 或 `use async_trait::async_trait`）。

- [ ] **Step 2: 跑测试确认（新测试）失败**

Run: `cargo test -p alephcore --lib scoped::tests::chat_tier_config_tool_approved_executes`
Expected: FAIL —— 当前门控对 chat 档无条件 `PermissionDenied`，不看 requester，故批准也不执行。

- [ ] **Step 3: 改门控** — `src/tools/scoped/dispatch.rs` 的配置档门控块（`if crate::gateway::method_authz::tool_requires_operator(name) { ... }`），把内层 `if !is_operator { return Err(PermissionDenied) }` 改为：

```rust
        if crate::gateway::method_authz::tool_requires_operator(name) {
            let is_operator = crate::tools::turn_context::current_turn_context()
                .map(|t| t.caller_is_operator())
                .unwrap_or(true);
            if !is_operator {
                // Phase 2b: suspend for live operator approval instead of an
                // outright reject. Routes through the operator-targeted requester
                // (publishes an operator-only `approval.requested`, waits on the
                // exec-approval oneshot resolved via `exec.approval.resolve`).
                // Reuses confirm_with_memory for session-grant memory + the
                // denial-ledger blind-retry guard. No requester wired (tests /
                // pre-boot) → fail closed (hard reject), never silent allow.
                match &self.config_approval_requester {
                    Some(req) => {
                        let reason = format!(
                            "A chat-tier device asked to run `{name}`, which changes Aleph's own \
                             configuration. Approve to allow this change."
                        );
                        if let Err(denial) = self.confirm_with_memory(req, name, &reason).await {
                            return Err(ToolError::PermissionDenied {
                                name: name.to_string(),
                                reason: format!(
                                    "config change via `{name}` was not authorized by the server \
                                     operator ({:?}). Do not retry until authorized.",
                                    denial.outcome
                                ),
                            });
                        }
                        // Approved → fall through to normal execution.
                    }
                    None => {
                        return Err(ToolError::PermissionDenied {
                            name: name.to_string(),
                            reason: format!(
                                "`{name}` changes Aleph's own configuration and requires operator \
                                 authorization, but no approval channel is available. This device \
                                 is paired at chat level. Do not retry."
                            ),
                        });
                    }
                }
            }
        }
```
> `ConfirmDenial` 已是 `confirm_with_memory` 的 Err 类型（含 `.outcome`），同文件内可见，无需新 import。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib scoped::tests`
Expected: 全 PASS（Phase 2 的 3 个 + 2 个新 + 既有 scoped）。`chat_tier_blocked_from_config_tool`（无 requester）仍 `PermissionDenied`（None 臂）。

- [ ] **Step 5: 提交**

```bash
git add src/tools/scoped/dispatch.rs src/tools/scoped/tests.rs
git commit -m "tools: config gate suspends for operator approval (Phase 2b sudo)"
```

---

## Task 5: boot 接线（构造 + 安装 config approver）

**Files:** Modify `src/bin/aleph-server/commands/start/mod.rs`

- [ ] **Step 1: 接线** — 在 HITL 审批接线块（~2025-2067，`set_confirmation_requester(approval_requester)` 之后、块结束前）加：

```rust
    // Phase 2b: operator-targeted approval for config-tier tools. A chat-tier
    // remote device calling a config tool suspends here until an operator
    // resolves it via `exec.approval.resolve`. Distinct from the channel-backed
    // requester above (which delivers to the requester's own channel).
    {
        use alephcore::approval::operator_requester::OperatorApprovalRequester;
        let config_approver: Arc<
            dyn alephcore::sandbox::exec_approval::gate::ApprovalRequester,
        > = Arc::new(OperatorApprovalRequester::new(
            exec_approval_manager.clone(),
            <EVENT_BUS_HANDLE>.clone(),
        ));
        alephcore::gateway::execution_engine::set_config_approval_requester(config_approver);
    }
```

> **实现者**：`<EVENT_BUS_HANDLE>` 换成该 scope 内可用的 `Arc<GatewayEventBus>`。grep 该 boot 文件里 event bus 怎么取（`event_bus`、`server.event_bus()`、`gateway_event_bus` 等局部/字段）。若 `exec_approval_manager` 在此块结束已被 move，改用其 `.clone()`（它在 `register_handlers` 与 adapter 处已 `.clone()`，应仍可用）。

- [ ] **Step 2: 编译**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: 通过（找对 event bus handle 后）。

- [ ] **Step 3: 提交**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "server: wire operator config-approval requester at boot"
```

---

## Task 6: 集成验证 + lint

**Files:** 无源码新增

- [ ] **Step 1: 全量编译**

Run: `cargo check -p alephcore --all-targets`
Expected: 通过。

- [ ] **Step 2: 相关测试**

Run（逐条）：`cargo test -p alephcore --lib event_scope::tests` / `approval::operator_requester` / `scoped::tests`
Expected: 全绿。

- [ ] **Step 3: fmt + clippy**

Run: `cargo fmt -p alephcore && cargo clippy -p alephcore --all-targets`
Expected: 改动文件零 clippy 警告，fmt 净。提交 fmt（若有改动）：

```bash
git add -u && git commit -m "chore: fmt + clippy for chat/config tier Phase 2b"
```

> 提交前 `git diff --stat` 确认 fmt 只动本期文件；若卷入他会话 WIP，仅 `git add` 本期显式路径。

---

## 红线对账

| 红线 | 落地 |
|---|---|
| R4 — Interface 无业务逻辑 | 审批判定/挂起全在 core（tools/gateway/exec），Panel 仅渲染（Phase 3） |
| R7/R9 — LLM 主权 | 门控+审批是确定性安全 infra（赋能层允许），不替 LLM 推理 |
| R8 — 工具即一切 | 在工具分发咽喉点拦截 + 求批 |
| R10 — 薄 harness | 改动在既有 dispatch/event/boot 接缝，不进 `src/harness/`；复用 ExecApprovalManager/confirm_with_memory，不造新审批通道 |

## 范围外（本期不做）

- **永久设备提升**（AllowAlways→config 档持久化）→ Phase 3（device-提升 RPC + Devices UI 一并做）。本期 AllowAlways 折为 session grant。
- **operator 审批 UI**（Panel 审批卡 + 发起端「等待授权」态）→ Phase 3。本期 operator 经 `exec.approval.resolve` RPC / 桌面通知批。
- **审批事件携带 reason/args 详情**：本期事件仅 approval_id+session（operator 可经 `exec.approvals.pending` 看 command=tool_name）；富化留 Phase 3。
- 同为 `requires_confirmation` 的 config 工具（如 `skill_install`）chat 档会先 operator 批 tier、再走 confirm 自身审批（双重）——已知小瑕，非阻塞。

## Self-Review

- **Spec 覆盖**：Spec B §3 缺口2「硬拒→sudo 现场审批」「只投 operator」「approve/deny/timeout 结构化错误」「session 记住」全覆盖（Task 1 投递门控/Task 2 operator 定向/Task 4 求批+映射+PermissionDenied/复用 session_memory）。永久提升按用户决策延 Phase 3。✔
- **Placeholder 扫描**：production 代码确切；两处显式标注实现者需跟编译器/grep 修正的点（Task 2 模块路径、Task 5 event bus handle）非占位，是已知的环境对齐步骤，附明确定位指令与回退。✔
- **类型一致**：`ApprovalRequester::request_approval`→`ApprovalOutcome`（Task2）↔ `confirm_with_memory(req,…)`（Task4 复用）↔ `with_config_approval`/`config_approval_requester: Option<Arc<dyn ApprovalRequester>>`（Task3）↔ `set_config_approval_requester`/`CONFIG_APPROVAL_REQUESTER`（Task3）一致；`ApprovalDecisionType` 4 变体 ↔ `decision_to_outcome` 穷尽（Task2）；`GatewayEventFrame::Approval{Requested,Resolved,Expired}` 字段与勘察一致。✔
- **Fail-closed**：无 requester→PermissionDenied（Task4 None 臂 + `chat_tier_blocked_from_config_tool` 测试锁定）。✔

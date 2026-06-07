# Chat/Config 权限分层 Phase 3b-2b Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** chat 档设备触发的 config 工具被挂起等 operator 审批时，往发起者自己的 run 输出流注入一条 in-band「⏳ 正在等待管理员授权运行工具 …」提示（替代当前"工具卡永远转圈、无说明"的体验）。

**Architecture:** 纯 backend。给 `TurnContext` 加一个 `run_id` 字段把 run 标识接到唯一阻塞层；在 `OperatorApprovalRequester::request_approval` 内（publish `ApprovalRequested` 之后、await 决策之前）经已持有的 `event_bus` 发一条 run-scoped、`is_intermediate:true` 的 `ResponseChunk`。复用已被所有通道+Panel 渲染的流事件，故零前端、零新事件变体、零 event_scope 改动。

**Tech Stack:** Rust（`src/tools/turn_context.rs`、`src/gateway/execution_engine/run_loop.rs`、`src/approval/operator_requester.rs` + 多处测试构造点）。

**Spec:** `docs/superpowers/specs/2026-06-07-chat-config-permission-tier-phase3b2b-design.md`

**Git 约束（全程）:** 共享单分支 main + 并发提交者——只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；提交信息英文、无 attribution footer；不 push；提交前 `git status` 确认不卷入他人 WIP（工作区有 `interfaces/webchat/dist/*` 产物未暂存，勿 staged）。

---

## File Structure

- `src/tools/turn_context.rs` — `TurnContext` struct 加 `pub run_id: String`；更新文件内测试 helper `ctx()`（:70）。
- `src/gateway/execution_engine/run_loop.rs` — 生产构造点（:479）填 `run_id: run_id.to_string()`（`run_id: &str` 是 `run_agent_loop_inner` 的入参，在 scope 内）。
- `src/approval/operator_requester.rs` — `request_approval` 内发提示 + 2 个新单测。
- 其余测试构造点补 `run_id: String::new()`：`src/tools/scoped/tests.rs`（:1155 helper、:1316、:1371、:1390、:1442、:1468）、`src/builtin_tools/select_model.rs`（:121）、`src/builtin_tools/ask_user.rs`（:236、:271）、`src/builtin_tools/desktop/tests.rs`（:421、:440、:467）、`src/approval/adapters.rs`（:135、:202）。

任务顺序：Task 1（加字段 + 全构造点，机械、编译通过、行为不变）→ Task 2（TDD：发提示 + 单测）。Task 1 必须先行，因 Task 2 的测试要给 `TurnContext` 设 `run_id`。

> **行号为快照**：实现时以 `rg "TurnContext\s*\{" --type rust` 实际结果为准——任何 `TurnContext { ... }` 字面量构造都必须补 `run_id` 字段，否则不编译。下面的 grep 步骤会列出全部站点。

---

### Task 1: `TurnContext` 加 `run_id` 字段 + 更新所有构造点

**Files:**
- Modify: `src/tools/turn_context.rs`（struct :21-34、测试 helper :70）
- Modify: `src/gateway/execution_engine/run_loop.rs:479`
- Modify: `src/tools/scoped/tests.rs`、`src/builtin_tools/select_model.rs`、`src/builtin_tools/ask_user.rs`、`src/builtin_tools/desktop/tests.rs`、`src/approval/adapters.rs`（各 TurnContext 构造点）

- [ ] **Step 1: 枚举所有构造点**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && rg -n "TurnContext\s*\{" --type rust
```
Expected: 列出全部字面量构造站点（生产 1 处 + 测试若干）。记下每一处，Step 4 逐一补字段。若出现快照之外的新站点，一并处理。

- [ ] **Step 2: 给 `TurnContext` 加字段**

在 `src/tools/turn_context.rs` 的 `TurnContext` struct（:21-34），在 `session_key` 之后加字段：

```rust
pub struct TurnContext {
    /// Session key of the running turn — the key HITL managers register under.
    pub session_key: SessionKey,
    /// Gateway run id of the running turn. Lets the config-approval gate emit a
    /// run-scoped "waiting for operator approval" notice on the requester's own
    /// output stream. Empty for non-gateway runs (cron, internal, tests) — the
    /// notice is then skipped (best-effort).
    pub run_id: String,
    /// Originating channel id (e.g. `telegram`). Empty for non-channel turns
    /// (cron, webhook, internal).
    pub channel_id: String,
    /// Originating conversation id. Empty for non-channel turns.
    pub conversation_id: String,
    /// Originating gateway connection's authorization role (`"operator"` /
    /// `"guest"`), stamped at run start from `CALLER_ROLE`. `None` for
    /// non-gateway runs (cron, internal) and for the local no-auth daemon —
    /// both treated as trusted by the config-tier gate.
    pub caller_role: Option<String>,
}
```

- [ ] **Step 3: 更新生产构造点 `run_loop.rs:479`**

在 `src/gateway/execution_engine/run_loop.rs` 的 `let turn_context = ... TurnContext {` 块（:479），在 `session_key` 行之后加 `run_id`（`run_id` 是 `run_agent_loop_inner` 的 `&str` 入参，已在 scope）：

```rust
        let turn_context = crate::tools::turn_context::TurnContext {
            session_key: request.session_key.clone(),
            run_id: run_id.to_string(),
            channel_id: request
                .metadata
                .get("channel_id")
                .cloned()
                .unwrap_or_default(),
            conversation_id: request
                .metadata
                .get("conversation_id")
                .cloned()
                .unwrap_or_default(),
            caller_role: request.metadata.get("caller_role").cloned(),
        };
```

- [ ] **Step 4: 更新所有测试构造点补 `run_id: String::new()`**

对 Step 1 列出的每个测试站点，在 `session_key: ...,` 之后加一行 `run_id: String::new(),`。已知站点：

- `src/tools/turn_context.rs:70`（helper `ctx()`）：
```rust
    fn ctx(role: Option<&str>) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("t"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: role.map(String::from),
        }
    }
```
- `src/tools/scoped/tests.rs`：helper `turn_ctx`（:1155）、内联构造（:1316、:1371、:1390）、`.with_turn_context(...)` 内联（:1442、:1468）。每处在 `session_key:` 之后加 `run_id: String::new(),`。
- `src/builtin_tools/select_model.rs:121`、`src/builtin_tools/ask_user.rs:236`、`src/builtin_tools/ask_user.rs:271`、`src/builtin_tools/desktop/tests.rs:421`、`src/builtin_tools/desktop/tests.rs:440`、`src/builtin_tools/desktop/tests.rs:467`、`src/approval/adapters.rs:135`、`src/approval/adapters.rs:202`：同样在 `session_key:` 之后加 `run_id: String::new(),`。

> 这些是 display-only 测试夹具，空 run_id 即"无 gateway run"，与生产语义一致。

- [ ] **Step 5: 编译验证（含测试目标）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --all-targets 2>&1 | tail -30
```
Expected: 编译通过，零错误。若报 `missing field run_id`，说明漏了某个构造点——回 Step 1 的列表补齐（`--all-targets` 才编译 `tests/` 集成测试，确保不漏）。

- [ ] **Step 6: 现有测试仍绿**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib turn_context 2>&1 | tail -15
```
Expected: `turn_context` 相关测试（`operator_and_local_are_operator`、`chat_tier_is_not_operator`）通过。

- [ ] **Step 7: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add src/tools/turn_context.rs src/gateway/execution_engine/run_loop.rs src/tools/scoped/tests.rs src/builtin_tools/select_model.rs src/builtin_tools/ask_user.rs src/builtin_tools/desktop/tests.rs src/approval/adapters.rs
git commit -m "harness: add run_id to TurnContext for run-scoped approval notice"
git show --stat HEAD
```
确认提交只含上述源文件（无 dist 产物）。若 Step 1 发现额外站点文件，一并 `git add` 显式路径。

---

### Task 2: `request_approval` 内发 run-scoped 等待提示 + 单测

**Files:**
- Modify: `src/approval/operator_requester.rs`（`request_approval` :85-95 之后插入；`mod tests` 加 2 个 async 测试）

- [ ] **Step 1: 写失败测试（提示发出）**

在 `src/approval/operator_requester.rs` 的 `mod tests`（:120）内，先补 imports（紧接 `use super::*;`）：

```rust
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use std::time::Duration;
```

然后加测试：

```rust
    fn guest_turn(run_id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("approval-test"),
            run_id: run_id.to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
        }
    }

    #[tokio::test]
    async fn emits_waiting_notice_when_run_id_present() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        // request_approval blocks on the decision oneshot; resolve it once the
        // approval is registered so the test terminates.
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn("run-123"), async move {
                    requester.request_approval("set_provider", "needs config").await
                })
                .await
        });

        let mut saw_notice = false;
        let mut approval_id: Option<String> = None;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            match frame {
                GatewayEventFrame::ApprovalRequested { approval_id: id, .. } => {
                    approval_id = Some(id);
                }
                GatewayEventFrame::ResponseChunk {
                    run_id,
                    is_intermediate,
                    is_final,
                    ..
                } => {
                    assert_eq!(run_id, "run-123", "notice must target the requester's run");
                    assert!(is_intermediate, "notice must be an intermediate (ephemeral) chunk");
                    assert!(!is_final, "notice must not be the final answer");
                    saw_notice = true;
                }
                _ => {}
            }
            if saw_notice {
                if let Some(id) = &approval_id {
                    mgr.resolve(id, ApprovalDecisionType::AllowOnce, None);
                    break;
                }
            }
        }

        assert!(saw_notice, "expected a run-scoped waiting-for-approval ResponseChunk");
        let outcome = handle.await.unwrap();
        assert_eq!(outcome, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn no_notice_when_run_id_empty() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::new(manager.clone(), event_bus.clone());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            TURN_CONTEXT
                .scope(guest_turn(""), async move {
                    requester.request_approval("set_provider", "needs config").await
                })
                .await
        });

        // Drain frames; resolve on ApprovalRequested; assert no ResponseChunk
        // appears before the approval resolves.
        let mut saw_chunk = false;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            match frame {
                GatewayEventFrame::ApprovalRequested { approval_id, .. } => {
                    mgr.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None);
                }
                GatewayEventFrame::ResponseChunk { .. } => {
                    saw_chunk = true;
                }
                GatewayEventFrame::ApprovalResolved { .. } => break,
                _ => {}
            }
        }

        assert!(!saw_chunk, "no notice must be emitted when run_id is empty");
        let outcome = handle.await.unwrap();
        assert_eq!(outcome, ApprovalOutcome::Approved);
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib operator_requester 2>&1 | tail -25
```
Expected: `emits_waiting_notice_when_run_id_present` **失败**（断言 `saw_notice` 为 false，因为提示尚未实现）；`no_notice_when_run_id_empty` 通过（本就不发）；`decision_mapping` 通过。

- [ ] **Step 3: 实现提示发送**

在 `src/approval/operator_requester.rs` 的 `request_approval` 中，紧接 publish `ApprovalRequested` 的 `if let Err(e) = ... { ... }` 块（:85-95）之后、`let decision = self.manager.await_registered(...)`（:97）之前，插入：

```rust
        // Phase 3b-2b: surface an in-band "waiting for operator approval" notice
        // on the requester's OWN run output stream, so a chat-tier device sees
        // why its config tool is suspended instead of a silently-spinning tool.
        // Reuses the existing intermediate ResponseChunk path (rendered by every
        // channel + the Panel, never persisted to the transcript). Best-effort:
        // only when we have a gateway run to target; publish failures are
        // non-fatal and must not derail the approval.
        if let Some(t) = &turn {
            if !t.run_id.is_empty() {
                let notice = format!("⏳ 正在等待管理员授权运行工具 `{}`…", tool_name);
                if let Err(e) = self.event_bus.publish_frame(&GatewayEventFrame::ResponseChunk {
                    run_id: t.run_id.clone(),
                    seq: 0,
                    delta: notice.clone(),
                    full_text: notice.clone(),
                    content: notice,
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: true,
                }) {
                    tracing::debug!(error = %e, "failed to publish waiting-for-approval notice");
                }
            }
        }
```

> `turn`（`Option<TurnContext>`）已在 :53 绑定且仍在 scope；直接读 `t.run_id`。`seq: 0` 安全——客户端不按 seq 去重/排序（spec FACT 3），且本 chunk 是先于工具输出的独立中间消息。

- [ ] **Step 4: 运行测试，确认通过**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib operator_requester 2>&1 | tail -25
```
Expected: 三个测试全部 PASS（`emits_waiting_notice_when_run_id_present`、`no_notice_when_run_id_empty`、`decision_mapping`）。

- [ ] **Step 5: fmt + clippy（本文件）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p alephcore && cargo clippy -p alephcore --lib 2>&1 | grep -A3 "operator_requester" | head -20
```
Expected: fmt 无改动残留；clippy 对 `operator_requester.rs` 无新警告。

- [ ] **Step 6: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add src/approval/operator_requester.rs
git commit -m "approval: in-band waiting-for-approval notice on requester run stream"
git show --stat HEAD
```
确认只含 `operator_requester.rs`（无 dist 产物）。

---

## 最终验证（全任务完成后）

- [ ] `cargo check -p alephcore --all-targets` 绿（TurnContext 新字段全站点补齐）
- [ ] `cargo test -p alephcore --lib operator_requester` 三测全绿
- [ ] 零前端/零 event_scope 改动确认：`git diff <base>..HEAD --stat` 只含 `src/` 下文件 + docs，无 `interfaces/` 改动
- [ ] 派 final code reviewer 审整体（spec 合规 + 代码质量 + 端到端：chat 档 config 工具挂起→发起者 run 流收到 is_intermediate 提示→批准后工具续跑/拒绝走 PermissionDenied；确认提示不入 transcript、本机 daemon 不误发）

## 部署（用户决定时机）

纯 backend：见效需重编 `aleph-server` + 热替换 daemon（无 `just wasm`）。可与 3b-1 / 3b-2a 的 Panel 部署合并统一上线。

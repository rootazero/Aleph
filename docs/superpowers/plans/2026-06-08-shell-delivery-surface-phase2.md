# 壳投递面 Phase 2 — 审批 banner 走投递面（桌面腿）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把桌面审批的 OS banner 出站腿从 `notify.rs` 的 bespoke `approval.requested` 订阅收口进 Phase 1 已建的投递面：核侧 `r5_router` 把 `ApprovalRequested` 映射为 `OutboundInteraction::ApprovalRequest` → `DesktopSurface` 发新帧 `surface.approval`（operator-gated + audience:[desktop]）→ `notify.rs` 用统一渲染出 banner、删重复 arm。

**Architecture:** 与 Phase 1 完全对称。新增独立 topic `surface.approval`（**不**复用 unguarded 的 `surface.notify`，否则丢 operator 闸 → 同机 guest 桌面看到审批 = 回归）。入站响应关联（`manager.resolve`）、Panel 卡片、in-band `ResponseChunk` 全部不动（守 R4 风险 #4）。`src/harness/` 零改（R10）。boot 不变（Phase 1 已 spawn `r5_router::run` + 注册 `DesktopSurface`）。

**Tech Stack:** Rust（`alephcore` crate + `desktop/shell` Tauri crate），`serde`，`tokio::sync::broadcast`，`thiserror`。

**Spec:** `docs/superpowers/specs/2026-06-08-shell-delivery-surface-phase2-design.md`

**约束（并发 main 纪律 + 安全路径）：**
- 单分支 main 开发；worktree 隔离；只追加提交、显式路径暂存（`git add <path>`，绝不 `-A`）。
- 英文 commit message，格式 `<scope>: <description>`；归属已全局禁用。
- `cargo test` 一次只接受一个 filter — 分开跑。
- Task 3（event_scope operator 闸）是**安全关键**：guest/chat 桌面绝不能收到 `surface.approval`。
- `src/harness/` 不得改动。

**File Structure（本计划触及的文件）：**
- Modify: `src/gateway/events/frame.rs` — 新 `SurfaceApproval` 帧变体 + `topic_name` 臂（Task 1）
- Modify: `src/gateway/surface/delivery.rs` — `SurfaceApproval` struct + `OutboundInteraction::ApprovalRequest` 变体（Task 2）
- Modify: `src/gateway/surface/desktop.rs` — `deliver` 改 `match`、加审批分支（Task 2）
- Modify: `src/gateway/surface/r5_router.rs` — 测试 `Capture` 改 `match`（Task 2）；`approval_for` + `run` 分支（Task 4）
- Modify: `src/gateway/event_scope.rs` — `surface.approval` operator 闸规则（Task 3）
- Modify: `desktop/shell/src/notify.rs` — 订阅 + 渲染收口、删孤儿 `extract_text`/`truncate`（Task 5）

**不动（spec §5）：** `OperatorApprovalRequester`、Panel `context.rs`、`exec.approval.resolve` RPC、`ExecApprovalManager`、`src/harness/`、boot。

---

### Task 1: `SurfaceApproval` 帧变体

**Files:**
- Modify: `src/gateway/events/frame.rs:184-191`（在 `SurfaceNotify` 变体后加 `SurfaceApproval`）
- Modify: `src/gateway/events/frame.rs:391`（`topic_name` 加臂）
- Test: `src/gateway/events/frame.rs`（`mod surface_notify_tests`）

**背景：** `GatewayEventFrame` 用 `#[serde(tag = "type", rename_all = "snake_case")]`。`topic_name` 是**穷尽 match（无 `_`）**——加变体会触发编译错误，强制补臂。`stream_method` 有 `_ => None`，新变体自动落非流式（TopicEvent 线形），无需改。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/events/frame.rs` 的 `mod surface_notify_tests`（约 line 440）内，紧接 `surface_notify_topic_and_wire_shape` 测试之后加：

```rust
    #[test]
    fn surface_approval_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceApproval {
            audience: vec!["desktop".to_string()],
            approval_id: "a1".to_string(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.approval");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_approval");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["approval_id"], "a1");
        assert_eq!(v["title"], "Aleph needs your approval");
        assert_eq!(v["body"], "A tool call is waiting for you.");
    }
```

- [ ] **Step 2: 运行测试，确认编译失败**

Run: `cargo test -p alephcore --lib gateway::events::frame::surface_notify_tests::surface_approval 2>&1 | tail -20`
Expected: 编译错误 `no variant named SurfaceApproval found for enum GatewayEventFrame`。

- [ ] **Step 3: 加变体 + topic_name 臂**

在 `src/gateway/events/frame.rs`，`SurfaceNotify { ... }` 变体（约 line 184-190）之后、enum 闭合 `}`（line 191）之前插入：

```rust
    /// Core-decided approval banner addressed to one or more delivery surfaces.
    /// The raw `approval.requested` frame stays operator-gated and drives the
    /// Panel card; this is the *banner* leg, routed by the R5 router so the
    /// shell renders it through the same unified path as `SurfaceNotify`. The
    /// payload is intentionally sparse — approval detail lives in the Panel
    /// card (via `exec.approvals.pending`); the banner only needs to get
    /// attention. Gated operator-only by `event_scope` (`surface.approval`).
    SurfaceApproval {
        audience: Vec<String>,
        approval_id: String,
        title: String,
        body: String,
    },
```

在 `topic_name` 的 `SurfaceNotify { .. } => "surface.notify",`（line 391）之后加：

```rust
            GatewayEventFrame::SurfaceApproval { .. } => "surface.approval",
```

- [ ] **Step 4: 全 crate 编译，修任何穷尽性错误**

Run: `cargo build -p alephcore 2>&1 | tail -30`
Expected: 编译通过。若出现其它 `non-exhaustive patterns` 错误（除已处理的 `topic_name` 外，Phase 1 的 `SurfaceNotify` 仅需 `topic_name`，故预期无），按编译器提示在对应 match 加 `GatewayEventFrame::SurfaceApproval { .. }` 臂（参照同位置 `SurfaceNotify` 的处理方式）。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::events::frame::surface_notify_tests 2>&1 | tail -20`
Expected: PASS（含 `surface_approval_topic_and_wire_shape` + 既有 `surface_notify_topic_and_wire_shape`）。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/events/frame.rs
git commit -m "gateway: add SurfaceApproval frame variant (surface.approval topic)"
```

---

### Task 2: `OutboundInteraction::ApprovalRequest` + `DesktopSurface::deliver` 审批分支

**Files:**
- Modify: `src/gateway/surface/delivery.rs:14-29`（加 `SurfaceApproval` struct + `OutboundInteraction::ApprovalRequest`）
- Modify: `src/gateway/surface/desktop.rs:30-41`（`deliver` 改 `match`）
- Modify: `src/gateway/surface/r5_router.rs`（测试 `Capture` 改 `match` — 编译修复）
- Test: `src/gateway/surface/desktop.rs`（`mod tests`）

**关键交叉编译依赖：** 给 `OutboundInteraction` 加第二变体后，所有 `let OutboundInteraction::Notify(n) = ...` 不可反驳 let 变为 **refutable → 编译失败**。本 crate 有两处：
1. `desktop.rs:31`（生产代码）
2. `r5_router.rs` 测试 `Capture::deliver`（Phase 1 测试）

本 Task 必须**一并修这两处**，否则 `cargo test -p alephcore --lib` 编译不过。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/surface/desktop.rs` 的 `mod tests`（约 line 44）内，紧接 `deliver_publishes_surface_notify_to_desktop_audience` 之后加：

```rust
    #[tokio::test]
    async fn deliver_publishes_surface_approval_to_desktop_audience() {
        use crate::gateway::surface::delivery::SurfaceApproval;

        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();
        let surface = DesktopSurface::new(bus.clone());

        surface
            .deliver(OutboundInteraction::ApprovalRequest(SurfaceApproval {
                approval_id: "a1".to_string(),
                title: "Aleph needs your approval".to_string(),
                body: "A tool call is waiting for you.".to_string(),
            }))
            .unwrap();

        match rx.recv().await.unwrap() {
            GatewayEventFrame::SurfaceApproval {
                audience,
                approval_id,
                title,
                body,
            } => {
                assert_eq!(audience, vec!["desktop".to_string()]);
                assert_eq!(approval_id, "a1");
                assert_eq!(title, "Aleph needs your approval");
                assert_eq!(body, "A tool call is waiting for you.");
            }
            other => panic!("expected SurfaceApproval, got {other:?}"),
        }
    }
```

- [ ] **Step 2: 运行测试，确认编译失败**

Run: `cargo test -p alephcore --lib gateway::surface::desktop 2>&1 | tail -20`
Expected: 编译错误 `no variant or associated item named ApprovalRequest`、`cannot find struct SurfaceApproval`。

- [ ] **Step 3a: delivery.rs — 加 `SurfaceApproval` struct + 变体**

在 `src/gateway/surface/delivery.rs`，`SurfaceNotification` struct（line 17-23）之后加：

```rust
/// A core-decided approval banner ready to render. Like `SurfaceNotification`
/// the interrupt-worthiness is already decided upstream (the R5 router). The
/// payload is intentionally sparse — detail lives in the Panel card; the
/// banner only carries `approval_id` (for diagnostics/correlation) + static
/// title/body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceApproval {
    pub approval_id: String,
    pub title: String,
    pub body: String,
}
```

把 `OutboundInteraction`（line 26-29）改为：

```rust
/// An outbound interaction the core routes to a delivery surface.
#[derive(Debug, Clone)]
pub enum OutboundInteraction {
    Notify(SurfaceNotification),
    ApprovalRequest(SurfaceApproval),
}
```

- [ ] **Step 3b: desktop.rs — `deliver` 改 `match`、加审批分支**

把 `src/gateway/surface/desktop.rs` 的 `deliver`（line 30-41）改为：

```rust
    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
        let frame = match outbound {
            OutboundInteraction::Notify(n) => GatewayEventFrame::SurfaceNotify {
                audience: vec![SurfaceKind::Desktop.as_str().to_string()],
                title: n.title,
                body: n.body,
                source_topic: n.source_topic,
            },
            OutboundInteraction::ApprovalRequest(a) => GatewayEventFrame::SurfaceApproval {
                audience: vec![SurfaceKind::Desktop.as_str().to_string()],
                approval_id: a.approval_id,
                title: a.title,
                body: a.body,
            },
        };
        self.event_bus
            .publish_frame(&frame)
            .map(|_| ())
            .map_err(|e| DeliveryError::Publish(e.to_string()))
    }
```

- [ ] **Step 3c: r5_router.rs — 测试 `Capture` 改 `match`（编译修复）**

在 `src/gateway/surface/r5_router.rs` 的 `mod tests` 内，`run_delivers_to_registered_surface` 测试的 `Capture` 实现，把：

```rust
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                let OutboundInteraction::Notify(n) = outbound;
                self.0.lock().unwrap().push(n);
                Ok(())
            }
```

改为：

```rust
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                match outbound {
                    OutboundInteraction::Notify(n) => self.0.lock().unwrap().push(n),
                    OutboundInteraction::ApprovalRequest(_) => {}
                }
                Ok(())
            }
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::surface 2>&1 | tail -25`
Expected: PASS（含新 `deliver_publishes_surface_approval_to_desktop_audience` + Phase 1 全部 surface 测试）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/surface/delivery.rs src/gateway/surface/desktop.rs src/gateway/surface/r5_router.rs
git commit -m "gateway: add OutboundInteraction::ApprovalRequest + DesktopSurface approval delivery"
```

---

### Task 3: `surface.approval` operator-gate（安全关键）

**Files:**
- Modify: `src/gateway/event_scope.rs:31-56`（`default_rules` 加规则 + 文档表）
- Test: `src/gateway/event_scope.rs`（`mod tests`）

**背景：** `EventScopeGuard::can_receive` 用 `starts_with`/精确匹配；**未匹配任何规则的 topic = unguarded（对所有人放行）**。`surface.approval` 今天未匹配任何规则 → 会广播给所有人 = **guest 桌面看到审批的回归**。必须加 operator 闸（与既有 `approval.` 规则同谓词：`["admin","exec.approver"]`，operator/local daemon 持 `*` 通配满足）。`surface.approval` 前缀**不撞** `surface.notify`（不同 topic 串），故 `surface.notify` 仍 unguarded（R5 投任意 desktop 不变）。

- [ ] **Step 1: 写失败的安全测试**

在 `src/gateway/event_scope.rs` 的 `mod tests` 内，紧接 `chat_tier_excluded_from_approval_events`（约 line 163）之后加：

```rust
    #[test]
    fn surface_approval_is_operator_gated() {
        let g = EventScopeGuard::default_rules();

        // chat / guest tier and no-perms must NOT receive approval banners.
        let chat = vec!["chat".to_string(), "read".to_string()];
        assert!(
            !g.can_receive("surface.approval", &chat),
            "chat tier must NOT see surface.approval banners"
        );
        assert!(!g.can_receive("surface.approval", &[]));
        assert!(!g.can_receive("surface.approval", &["viewer".to_string()]));

        // operator [*] / exec.approver / admin must.
        assert!(g.can_receive("surface.approval", &["*".to_string()]));
        assert!(g.can_receive("surface.approval", &["exec.approver".to_string()]));
        assert!(g.can_receive("surface.approval", &["admin".to_string()]));

        // surface.notify stays unguarded (R5 to any desktop, not approval).
        assert!(
            g.can_receive("surface.notify", &chat),
            "surface.notify must stay unguarded"
        );
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore --lib gateway::event_scope::tests::surface_approval 2>&1 | tail -20`
Expected: FAIL —`surface.approval` 现 unguarded，`can_receive("surface.approval", &chat)` 返回 `true`，断言 `!...` 失败（`chat tier must NOT see surface.approval banners`）。

- [ ] **Step 3: 加 operator-gate 规则**

在 `src/gateway/event_scope.rs` 的 `default_rules`（line 31-56），在 `approval.` 规则（line 42-45）之后加一条：

```rust
                (
                    "surface.approval".to_string(),
                    vec!["admin".to_string(), "exec.approver".to_string()],
                ),
```

并在该函数上方的文档表（line 25-30）追加一行，保持文档与规则同步：

```rust
    /// | `surface.approval` | admin, exec.approver |
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::event_scope 2>&1 | tail -20`
Expected: PASS（含新 `surface_approval_is_operator_gated` + 既有 7 个 event_scope 测试）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/event_scope.rs
git commit -m "gateway: operator-gate surface.approval in event_scope (no guest approval leak)"
```

---

### Task 4: `r5_router` 路由 `ApprovalRequested` → 投递面

**Files:**
- Modify: `src/gateway/surface/r5_router.rs:1-12`（模块文档更新）
- Modify: `src/gateway/surface/r5_router.rs:57-65`（`notification_for` 的 `_ => None` 注释更新）
- Modify: `src/gateway/surface/r5_router.rs`（加 `approval_for` 纯函数）
- Modify: `src/gateway/surface/r5_router.rs:80-99`（`run` 加审批 deliver 分支）
- Test: `src/gateway/surface/r5_router.rs`（`mod tests`）

**背景：** `approval_for` 不能复用 `notification_for`（后者只产 `SurfaceNotification`/`Notify`，审批要产 `SurfaceApproval`/`ApprovalRequest`，是另一变体）。Loop-safety：`SurfaceApproval` 是本路由自己的输出，`approval_for` 必须对它返回 `None`，否则 `DesktopSurface` 发回 bus 被本路由再处理 → 无限放大。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/surface/r5_router.rs` 的 `mod tests` 内，紧接 `approval_is_not_routed_here`（约 line 161）之后加：

```rust
    #[test]
    fn approval_for_surfaces_the_request() {
        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
        };
        let a = approval_for(&f).expect("approval is surfaced");
        assert_eq!(a.approval_id, "a1");
        assert_eq!(a.title, "Aleph needs your approval");
        assert_eq!(a.body, "A tool call is waiting for you.");
    }

    #[test]
    fn approval_for_ignores_its_own_surface_frame() {
        // LOOP-SAFETY: SurfaceApproval is this router's own output.
        let f = GatewayEventFrame::SurfaceApproval {
            audience: vec!["desktop".to_string()],
            approval_id: "a1".to_string(),
            title: "x".to_string(),
            body: "y".to_string(),
        };
        assert!(approval_for(&f).is_none());
    }

    #[test]
    fn approval_for_ignores_ask_user() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            question: "Proceed?".to_string(),
            options: vec![],
        };
        assert!(approval_for(&f).is_none());
    }

    #[tokio::test]
    async fn run_delivers_approval_to_registered_surface() {
        use crate::gateway::surface::delivery::{
            DeliveryError, DeliverySurface, SurfaceApproval,
        };
        use crate::gateway::surface::SurfaceKind;
        use std::sync::Mutex;

        struct ApprovalCapture(Arc<Mutex<Vec<SurfaceApproval>>>);
        impl DeliverySurface for ApprovalCapture {
            fn kind(&self) -> SurfaceKind {
                SurfaceKind::Desktop
            }
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                if let OutboundInteraction::ApprovalRequest(a) = outbound {
                    self.0.lock().unwrap().push(a);
                }
                Ok(())
            }
        }

        let bus = Arc::new(GatewayEventBus::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let surfaces: SurfaceRegistry = vec![Arc::new(ApprovalCapture(seen.clone()))];
        let task = tokio::spawn(run(bus.clone(), surfaces));

        tokio::task::yield_now().await;
        let _ = bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
        });

        for _ in 0..50 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        task.abort();

        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].approval_id, "a1");
        assert_eq!(got[0].title, "Aleph needs your approval");
    }
```

- [ ] **Step 2: 运行测试，确认编译失败**

Run: `cargo test -p alephcore --lib gateway::surface::r5_router 2>&1 | tail -20`
Expected: 编译错误 `cannot find function approval_for in this scope`。

- [ ] **Step 3a: 加 `approval_for` 纯函数**

在 `src/gateway/surface/r5_router.rs`，`notification_for` 函数（结束于 line 65 的 `}`）之后、`truncate` 函数（line 68）之前插入。先在文件顶部 `use super::delivery::{...}`（line 19）把 import 扩展为包含 `SurfaceApproval`：

```rust
use super::delivery::{OutboundInteraction, SurfaceApproval, SurfaceNotification, SurfaceRegistry};
```

然后插入函数：

```rust
/// Pure policy: map an `ApprovalRequested` frame to the approval banner the
/// operator should be interrupted with. The raw frame is sparse (approval_id
/// only) — detail lives in the Panel card; the banner carries static text.
///
/// Returns `None` for everything else, INCLUDING `SurfaceApproval` itself.
/// LOOP-SAFETY: `DesktopSurface::deliver` publishes `SurfaceApproval` back onto
/// the same bus this router subscribes to; it MUST fall through to `None` here
/// or we re-deliver our own output and amplify infinitely. The operator-only
/// gate is applied later, at the forward-filter (`event_scope` +
/// `audience_allows`), not here.
pub fn approval_for(frame: &GatewayEventFrame) -> Option<SurfaceApproval> {
    match frame {
        GatewayEventFrame::ApprovalRequested { approval_id, .. } => Some(SurfaceApproval {
            approval_id: approval_id.clone(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        }),
        _ => None,
    }
}
```

- [ ] **Step 3b: `run` 加审批 deliver 分支**

在 `src/gateway/surface/r5_router.rs` 的 `run`（line 80-99），把 `Ok(frame) => { ... }` 块改为（在既有 `notification_for` 块之后追加 `approval_for` 块）：

```rust
            Ok(frame) => {
                if let Some(note) = notification_for(&frame) {
                    for surface in &surfaces {
                        if let Err(e) =
                            surface.deliver(OutboundInteraction::Notify(note.clone()))
                        {
                            tracing::debug!(error = %e, "surface delivery failed");
                        }
                    }
                }
                if let Some(approval) = approval_for(&frame) {
                    for surface in &surfaces {
                        if let Err(e) =
                            surface.deliver(OutboundInteraction::ApprovalRequest(approval.clone()))
                        {
                            tracing::debug!(error = %e, "surface approval delivery failed");
                        }
                    }
                }
            }
```

- [ ] **Step 3c: 更新模块文档 + `notification_for` 的 `_ => None` 注释**

把模块文档（line 10-12）的 Scope 段改为：

```rust
//! Scope: Phase 1 routes `AskUser` + `RunComplete` (via `notification_for`).
//! Phase 2 additionally routes `ApprovalRequested` (via `approval_for`) to a
//! gated `surface.approval` frame; the raw `approval.requested` frame still
//! drives the Panel card and the inbound `manager.resolve` correlation.
```

把 `notification_for` 的 `_ => None` 注释（line 57-63）改为：

```rust
        // Everything else stays silent for *notifications* — including both
        // surface frames (loop-safety) and `ApprovalRequested` (handled by
        // `approval_for`, which delivers an `ApprovalRequest`, not a `Notify`).
        _ => None,
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::surface::r5_router 2>&1 | tail -25`
Expected: PASS（4 个新审批测试 + Phase 1 既有 r5_router 测试，含 `approval_is_not_routed_here`——该测试仍成立：`notification_for(ApprovalRequested)` 仍 `None`）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/surface/r5_router.rs
git commit -m "gateway: route ApprovalRequested through r5_router to surface.approval"
```

---

### Task 5: 壳侧 `notify.rs` 收口（订阅 + 渲染 + 删孤儿）

**Files:**
- Modify: `desktop/shell/src/notify.rs:31-44`（常量 + `NOTIFY_TOPICS`）
- Modify: `desktop/shell/src/notify.rs:226-256`（`decide_notification`）
- Modify: `desktop/shell/src/notify.rs:281-309`（删孤儿 `extract_text` + `truncate`）
- Test: `desktop/shell/src/notify.rs`（`mod tests`）

**背景：** banner 腿现由核侧经 `surface.approval` 投递。壳改订阅 `surface.approval` 取代 `approval.requested`（Panel 仍订阅 `approval.requested` 出卡片），渲染走与 `surface.notify` 同形的 title/body。`extract_text` 仅被旧审批 arm（line 251）调用、`truncate` 仅被 `extract_text` 调用——删 arm 后两者成孤儿，必须删（否则 `cargo clippy -D warnings` 失败）。`surface.approval` 帧带核侧静态文案，无需 `extract_text`。focus-gate 不变（`if focused { return None }` 在 match 前，对 `surface.approval` 同样生效）。

**零回归：** `ApprovalRequested` 帧稀疏 → 今天 `extract_text` 永远 fallback 到「A tool call is waiting for you.」；新 `surface.approval` 带同样静态文案 = banner 逐字等价。

- [ ] **Step 1: 改测试（先让其失败）**

在 `desktop/shell/src/notify.rs` 的 `mod tests` 内做如下修改：

(a) **删除** `approval_notifies_when_unfocused`（line 411-416）、`extract_text_prefers_message_then_falls_back`（line 439-444）、`truncate_respects_char_boundaries`（line 446-452）三个测试（它们测的是即将删除的孤儿/旧 arm）。

(b) **改** `subscribe_request_carries_notify_topics`（line 353-364）的两处断言：把 `assert!(names.contains(&TOPIC_APPROVAL));` 改为 `assert!(names.contains(&TOPIC_SURFACE_APPROVAL));`，并加一行确认旧审批 topic 不再订阅：

```rust
    #[test]
    fn subscribe_request_carries_notify_topics() {
        let v: Value = serde_json::from_str(&subscribe_request()).unwrap();
        assert_eq!(v["method"], "events.subscribe");
        let topics = v["params"]["topics"].as_array().unwrap();
        let names: Vec<&str> = topics.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&TOPIC_SURFACE_NOTIFY));
        assert!(names.contains(&TOPIC_SURFACE_APPROVAL));
        // Banner leg moved to the gated surface.approval; the shell no longer
        // subscribes the raw approval.requested (the Panel card still does).
        assert!(!names.contains(&"approval.requested"));
        // The raw stream topics moved to the core; the shell no longer subscribes.
        assert!(!names.contains(&"stream.run_complete"));
        assert!(!names.contains(&"stream.ask_user"));
    }
```

(c) **加** `surface.approval` 渲染测试，紧接 `surface_notify_falls_back_on_missing_fields`（line 432-437）之后：

```rust
    #[test]
    fn surface_approval_renders_core_supplied_title_body() {
        let data = json!({
            "type": "surface_approval",
            "audience": ["desktop"],
            "approval_id": "a1",
            "title": "Aleph needs your approval",
            "body": "A tool call is waiting for you."
        });
        let note = decide_notification(TOPIC_SURFACE_APPROVAL, &data, false)
            .expect("surface.approval fires when unfocused");
        assert_eq!(note.title, "Aleph needs your approval");
        assert_eq!(note.body, "A tool call is waiting for you.");
    }

    #[test]
    fn surface_approval_falls_back_on_missing_fields() {
        let note = decide_notification(TOPIC_SURFACE_APPROVAL, &json!({}), false).unwrap();
        assert_eq!(note.title, "Aleph needs your approval");
        assert_eq!(note.body, "A tool call is waiting for you.");
    }

    #[test]
    fn surface_approval_suppressed_when_focused() {
        let data = json!({ "title": "Aleph needs your approval", "body": "x" });
        assert!(decide_notification(TOPIC_SURFACE_APPROVAL, &data, true).is_none());
    }
```

（注：`focused_window_never_notifies` 测试遍历 `NOTIFY_TOPICS`，Step 3 把 `surface.approval` 纳入 `NOTIFY_TOPICS` 后它自动覆盖新 topic 的 focus-gate，无需改。）

- [ ] **Step 2: 运行测试，确认编译失败**

Run: `cargo test -p aleph-desktop-shell --lib notify 2>&1 | tail -20`
（壳 crate 名若不同，先 `grep '^name' desktop/shell/Cargo.toml` 确认，用实际包名替换 `aleph-desktop-shell`。）
Expected: 编译错误 `cannot find value TOPIC_SURFACE_APPROVAL in this scope`。

- [ ] **Step 3a: 常量 + 订阅 topics**

在 `desktop/shell/src/notify.rs`，把 `TOPIC_APPROVAL`（line 31-34）替换为 `TOPIC_SURFACE_APPROVAL`：

```rust
/// Core-decided approval banner, already gated operator-only by the gateway
/// (`event_scope` `surface.approval`) and addressed to this desktop surface.
/// The raw `approval.requested` frame still drives the Panel card; the shell
/// only focus-gates and renders the core-supplied title/body for the banner.
const TOPIC_SURFACE_APPROVAL: &str = "surface.approval";
```

把 `NOTIFY_TOPICS`（line 44）改为：

```rust
const NOTIFY_TOPICS: &[&str] = &[TOPIC_SURFACE_NOTIFY, TOPIC_SURFACE_APPROVAL];
```

- [ ] **Step 3b: `decide_notification` 收口**

把 `decide_notification` 的 `TOPIC_APPROVAL` 臂（line 244-253）替换为 `TOPIC_SURFACE_APPROVAL` 臂（读 core 供给的 title/body，静态 fallback；不再用 `extract_text`）：

```rust
        // Approval banner now arrives via the gated delivery surface
        // (surface.approval). The core supplies title/body; the shell renders
        // it through the same path as surface.notify (Phase 2). The Panel card
        // still listens on approval.requested for the inbound approve/deny UI.
        TOPIC_SURFACE_APPROVAL => Some(PreparedNotification {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Aleph needs your approval")
                .to_string(),
            body: data
                .get("body")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("A tool call is waiting for you.")
                .to_string(),
        }),
```

- [ ] **Step 3c: 删孤儿 `extract_text` + `truncate`**

删除 `extract_text`（line 281-299，含其 doc 注释）与 `truncate`（line 301-309，含其 doc 注释）两个函数——它们的唯一调用方是刚移除的旧审批 arm。

- [ ] **Step 4: 运行测试 + clippy，确认通过且无死代码**

Run: `cargo test -p aleph-desktop-shell --lib notify 2>&1 | tail -25`
Expected: PASS。

Run: `cargo clippy -p aleph-desktop-shell --all-targets 2>&1 | tail -15`
Expected: 无 `dead_code` / `unused` 警告（确认孤儿已彻底清除）。

- [ ] **Step 5: 提交**

```bash
git add desktop/shell/src/notify.rs
git commit -m "shell: render approval banner via surface.approval, drop bespoke approval arm"
```

---

## 最终验证（全部任务完成后）

- [ ] **核 crate 全测**：`cargo test -p alephcore --lib gateway 2>&1 | tail -15` → 全绿。
- [ ] **壳 crate 全测**：`cargo test -p aleph-desktop-shell --lib 2>&1 | tail -15` → 全绿。
- [ ] **clippy 全绿**：`cargo clippy -p alephcore --all-targets 2>&1 | tail -10`（`-D warnings` 由 CI 施加；本地确认无新警告）。
- [ ] **零回归人工核对**：
  - `OperatorApprovalRequester` 未改（仍发 `approval.requested` + in-band `ResponseChunk`）。
  - Panel `interfaces/webchat/src/context.rs` 未改（仍订阅 `approval.**` 出卡片）。
  - `src/harness/` 零改（`git diff --stat <base>..HEAD -- src/harness/` 为空）。
  - boot `src/bin/aleph-server/commands/start/mod.rs` 未改（Phase 1 已 spawn r5_router）。
- [ ] **最终代码审查**：dispatch final reviewer 审整个 Phase 2 实现，重点 7 个接缝：① `surface.approval` 线形非流式 ② `audience:[desktop]` 三方一致（desktop.rs / frame / notify 订阅）③ operator 闸（event_scope）不放宽 ④ loop-safety（`approval_for` 对 `SurfaceApproval` 返 None）⑤ banner 文案零回归 ⑥ 入站 `manager.resolve` / Panel 卡片不动 ⑦ 孤儿 `extract_text`/`truncate` 已删净。

## 部署（DEFERRED，按 Phase 1 模式，需用户确认）

- 后端：重编 `aleph-server`、热替换 binary、supervisor relaunch。
- 壳：`just shell-build` 验真 OS banner（纯逻辑已 `notify.rs` 单测覆盖）。
- 按仓库惯例 LANDED on main NOT pushed。

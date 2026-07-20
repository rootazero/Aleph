# 壳投递面 Phase 1 · R5 推送走投递面 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「值不值得打扰用户」的 R5 策略从桌面壳搬进核侧，核经一个具名的 `DeliverySurface` 把通知定向投给 `desktop` 投递面，壳内 `notify.rs` 退化为「focus-gate + 纯渲染」。

**Architecture:** 复用现有 `event_bus` / WS 单一传输（R3，不建第二通道）。新增非流式帧 `GatewayEventFrame::SurfaceNotify { audience, title, body, source_topic }`；转发过滤点按 `audience × ConnectionState.channel_kind`（Phase 0 已落字段）路由；核侧后台任务订阅 typed bus，对 `AskUser` / `RunComplete` 施加 interrupt 策略后交给已注册的 `DesktopSurface`。**审批（`approval.requested`）刻意留在既有 operator-gated 路径，归 Phase 2**，避免「guest 桌面突然看到审批」回归。

**Tech Stack:** Rust (`alephcore`)，tokio broadcast，serde_json，thiserror 2.0；桌面壳 `aleph-desktop-shell`（Tauri）。

**定向键 = surface-kind（本期决策）：** 本期只建 surface-kind 寻址层。多用户 owner-身份隔离（A 的 run_complete 不弹到 B 的桌面）**不在本计划**，另立 spec 作下一层——它需要把 owner 身份穿过 emit 路径（`RunComplete`/`AskUser` 今不携带），是安全敏感的独立子系统。**本期诚实代价：多个 desktop 面会各自收到每条 R5 通知，不按用户隔离。**

---

## 概念脊柱（实现者须先理解）

今天 R5：核 `publish_frame` 广播 `agent.ask.user` / `agent.run.complete` / `approval.requested` → 每个 WS 连接自过滤 → 桌面壳 `notify.rs` 自带 topic 成员 + 时长阈值策略 + focus-gate + 渲染。

Phase 1 之后：
1. 核后台 `r5_router` 订阅 typed bus，对 `AskUser`/`RunComplete` 跑 `notification_for`（topic 成员 + 时长阈值——**策略搬到核**），产出 `SurfaceNotification`。
2. 交给每个已注册 `DeliverySurface`；`DesktopSurface::deliver` = `publish_frame(SurfaceNotify{ audience:["desktop"], … })`。
3. 转发过滤点：帧 `data.audience` 存在时，仅 `channel_kind ∈ audience` 的连接放行（无 `audience` 字段的所有历史帧 = 不受限，零回归）。
4. 壳 `notify.rs`：只订阅 `surface.notify`（+ 暂留 `approval.requested`），对 `surface.notify` 仅 focus-gate + 渲染核给的 title/body。

**零回归不变量：** 所有既有帧序列化后**没有 `audience` 字段** → `audience_allows` 返回 `true` → 行为完全不变。`agent.ask.user` / `agent.run.complete` 今天就是 unguarded（`event_scope::can_receive` 对无规则 topic 返回 `true`），移到带 `audience` 的 `surface.notify` 只会**更窄**（仅 desktop），不放大。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `src/gateway/events/frame.rs` | 改 | 新增 `SurfaceNotify` 变体 + `topic_name` 映射 |
| `src/gateway/surface/delivery.rs` | 建 | `DeliverySurface` trait / `OutboundInteraction` / `SurfaceNotification` / `DeliveryError` / `SurfaceRegistry` / 纯函数 `audience_allows` |
| `src/gateway/surface/desktop.rs` | 建 | `DesktopSurface`：`deliver` → 发 `SurfaceNotify{audience:["desktop"]}` |
| `src/gateway/surface/r5_router.rs` | 建 | 纯策略 `notification_for` + `run()` 订阅 typed bus 定向投递 |
| `src/gateway/surface/mod.rs` | 改 | 注册 `pub mod delivery / desktop / r5_router` |
| `src/gateway/server/handler.rs` | 改 | 转发过滤点加 `audience` 门控（~1185–1200） |
| `src/bin/aleph-server/commands/start/mod.rs` | 改 | boot 处构造 `DesktopSurface` + spawn `r5_router::run`（~2095 后） |
| `desktop/shell/src/notify.rs` | 改 | 退化为 surface.notify 渲染 + focus-gate；声明 `channel_kind:"desktop"` |

---

### Task 1: `SurfaceNotify` 帧变体

**Files:**
- Modify: `src/gateway/events/frame.rs`（enum 体 ~178 行后；`topic_name` match ~378 行后；文件尾加 `#[cfg(test)]`）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/events/frame.rs` 文件末尾（`InboundMessagePayload` / `MessageSender` 定义之后）追加：

```rust
#[cfg(test)]
mod surface_notify_tests {
    use super::*;

    #[test]
    fn surface_notify_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceNotify {
            audience: vec!["desktop".to_string()],
            title: "Aleph finished".to_string(),
            body: "Your turn is complete.".to_string(),
            source_topic: "agent.run.complete".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.notify");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_notify");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["title"], "Aleph finished");
        assert_eq!(v["source_topic"], "agent.run.complete");
    }
}
```

- [ ] **Step 2: 运行测试确认失败（未编译）**

Run: `cargo test -p alephcore gateway::events::frame::surface_notify_tests`
Expected: 编译失败 —— `no variant named SurfaceNotify`。

- [ ] **Step 3: 加变体 + topic 映射**

在 enum `GatewayEventFrame` 内，`HeartbeatTaskChanged { … }`（~178 行）之后、enum 闭合 `}` 之前插入：

```rust
    /// Core-decided R5 interrupt addressed to one or more delivery surfaces.
    /// Unlike the raw agent-lifecycle frames, the "is this worth interrupting
    /// the user" policy has already been applied by the core R5 router; the
    /// shell only focus-gates and renders. `audience` lists the `SurfaceKind`
    /// wire strings (e.g. `["desktop"]`) the gateway forward-filter routes to.
    SurfaceNotify {
        audience: Vec<String>,
        title: String,
        body: String,
        /// Originating topic (e.g. `agent.run.complete`) — diagnostics only.
        source_topic: String,
    },
```

在 `topic_name(&self)` 的 match（exhaustive，无通配）内，`HeartbeatTaskChanged { .. } => "heartbeat.task.changed",`（~378 行）之后插入：

```rust
            GatewayEventFrame::SurfaceNotify { .. } => "surface.notify",
```

`stream_method` 无需改：它以 `_ => None` 收尾，新变体自动归非流式（正确——走 TopicEvent 线形）。`From<StreamEvent>` 无需改：`SurfaceNotify` 不是 `StreamEvent`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore gateway::events::frame::surface_notify_tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/events/frame.rs
git commit -m "gateway: add SurfaceNotify event frame for delivery-surface routing"
```

---

### Task 2: `DeliverySurface` 抽象 + `DesktopSurface`

**Files:**
- Create: `src/gateway/surface/delivery.rs`
- Create: `src/gateway/surface/desktop.rs`
- Modify: `src/gateway/surface/mod.rs`（注册子模块）

- [ ] **Step 1: 注册子模块**

在 `src/gateway/surface/mod.rs` 顶部文档注释之后、`pub enum SurfaceKind` 之前插入：

```rust
pub mod delivery;
pub mod desktop;
pub mod r5_router;
```

（`r5_router` 在 Task 4 创建；先声明会使 Task 2 暂时编译失败——故 Step 1 仅在 Task 4 文件就位后整体绿。实现者：本 Step 只写 `delivery` 与 `desktop` 两行，`r5_router` 一行留到 Task 4 Step 1 再加。）

即本 Task 实际只加：

```rust
pub mod delivery;
pub mod desktop;
```

- [ ] **Step 2: 写失败测试**

在 `src/gateway/surface/desktop.rs`（新建）末尾先放测试骨架（实现紧随其后）。完整文件见 Step 3/4；测试为：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::events::GatewayEventFrame;
    use crate::gateway::surface::delivery::{
        DeliverySurface, OutboundInteraction, SurfaceNotification,
    };
    use crate::gateway::surface::SurfaceKind;
    use std::sync::Arc;

    #[tokio::test]
    async fn deliver_publishes_surface_notify_to_desktop_audience() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();
        let surface = DesktopSurface::new(bus.clone());

        assert_eq!(surface.kind(), SurfaceKind::Desktop);

        surface
            .deliver(OutboundInteraction::Notify(SurfaceNotification {
                title: "Aleph finished".to_string(),
                body: "Your turn is complete.".to_string(),
                source_topic: "agent.run.complete".to_string(),
            }))
            .unwrap();

        match rx.recv().await.unwrap() {
            GatewayEventFrame::SurfaceNotify {
                audience, title, source_topic, ..
            } => {
                assert_eq!(audience, vec!["desktop".to_string()]);
                assert_eq!(title, "Aleph finished");
                assert_eq!(source_topic, "agent.run.complete");
            }
            other => panic!("expected SurfaceNotify, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: 写 `delivery.rs`**

`src/gateway/surface/delivery.rs`（完整内容）：

```rust
//! Outbound delivery to a named, addressable surface.
//!
//! A `DeliverySurface` is NOT a `MessagingChannel`: it only delivers outbound
//! interactions and names its identity. It never parses inbound text — that
//! line is the whole point of Approach A (see the Phase 0/1 spec). Phase 1
//! carries only R5 notifications; approval回投 joins in Phase 2.

use std::sync::Arc;

use serde_json::Value;

use super::SurfaceKind;

/// A core-decided notification ready to render. The "is this worth
/// interrupting the user" policy has already been applied upstream (the R5
/// router); a surface only marshals it onto its transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceNotification {
    pub title: String,
    pub body: String,
    /// Originating topic, for diagnostics only.
    pub source_topic: String,
}

/// An outbound interaction the core routes to a delivery surface.
#[derive(Debug, Clone)]
pub enum OutboundInteraction {
    Notify(SurfaceNotification),
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("failed to publish delivery frame: {0}")]
    Publish(String),
}

/// A named, addressable outbound surface (desktop shell, browser panel, …).
pub trait DeliverySurface: Send + Sync {
    /// This surface's identity. Used by the forward-filter's `audience` gate.
    fn kind(&self) -> SurfaceKind;
    /// Deliver one outbound interaction. Implementations marshal onto their
    /// transport (e.g. publish a `SurfaceNotify` frame); they do NOT render.
    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError>;
}

/// The set of surfaces the core can address. Phase 1 registers exactly one
/// (desktop); the Vec is the extension point for future surfaces.
pub type SurfaceRegistry = Vec<Arc<dyn DeliverySurface>>;

/// Forward-filter gate: may a connection of `channel_kind` receive an event
/// whose payload is `event_data`?
///
/// Events without an `audience` array are unrestricted — that is EVERY legacy
/// event, so this is byte-for-byte zero-regression. An addressed event reaches
/// only connections whose declared kind is listed; a connection that declared
/// no kind is excluded from any addressed event (fail-closed).
pub fn audience_allows(event_data: Option<&Value>, channel_kind: Option<SurfaceKind>) -> bool {
    let Some(audience) = event_data
        .and_then(|d| d.get("audience"))
        .and_then(|a| a.as_array())
    else {
        return true;
    };
    match channel_kind {
        Some(kind) => audience
            .iter()
            .filter_map(|v| v.as_str())
            .any(|k| k == kind.as_str()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_audience_field_is_unrestricted() {
        // Every legacy event lacks `audience` → always forwarded.
        let data = json!({ "run_id": "r1" });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(audience_allows(Some(&data), None));
        assert!(audience_allows(None, Some(SurfaceKind::Desktop)));
    }

    #[test]
    fn addressed_event_reaches_only_listed_kind() {
        let data = json!({ "audience": ["desktop"], "title": "x" });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Desktop)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Cli)));
    }

    #[test]
    fn addressed_event_excludes_undeclared_connection() {
        let data = json!({ "audience": ["desktop"] });
        assert!(!audience_allows(Some(&data), None));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Unknown)));
    }

    #[test]
    fn multi_kind_audience_matches_any() {
        let data = json!({ "audience": ["desktop", "browser"] });
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Desktop)));
        assert!(audience_allows(Some(&data), Some(SurfaceKind::Browser)));
        assert!(!audience_allows(Some(&data), Some(SurfaceKind::Cli)));
    }
}
```

- [ ] **Step 4: 写 `desktop.rs`**

`src/gateway/surface/desktop.rs`（完整内容；Step 2 的测试模块追加在末尾）：

```rust
//! The desktop shell as a first-class delivery surface.
//!
//! R1 boundary holds: this only publishes a `SurfaceNotify` frame addressed to
//! `desktop` connections; the shell's `notify.rs` renders the OS banner and
//! owns the focus-gate. No rendering happens in core.

use std::sync::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

use super::delivery::{DeliveryError, DeliverySurface, OutboundInteraction};
use super::SurfaceKind;

pub struct DesktopSurface {
    event_bus: Arc<GatewayEventBus>,
}

impl DesktopSurface {
    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self { event_bus }
    }
}

impl DeliverySurface for DesktopSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Desktop
    }

    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
        let OutboundInteraction::Notify(n) = outbound;
        self.event_bus
            .publish_frame(&GatewayEventFrame::SurfaceNotify {
                audience: vec![SurfaceKind::Desktop.as_str().to_string()],
                title: n.title,
                body: n.body,
                source_topic: n.source_topic,
            })
            .map(|_| ())
            .map_err(|e| DeliveryError::Publish(e.to_string()))
    }
}
```

- [ ] **Step 5: 运行测试**

Run（分两次，cargo test 单 filter）：
```
cargo test -p alephcore gateway::surface::delivery
cargo test -p alephcore gateway::surface::desktop
```
Expected: 全 PASS。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/surface/mod.rs src/gateway/surface/delivery.rs src/gateway/surface/desktop.rs
git commit -m "gateway: add DeliverySurface abstraction + DesktopSurface impl"
```

---

### Task 3: 转发过滤点 `audience` 门控

**Files:**
- Modify: `src/gateway/server/handler.rs`（事件转发分支 ~1185–1200）

纯函数 `audience_allows` 已在 Task 2 写好并单测。本 Task 仅把它接到转发过滤点，并在同一把读锁里取出 `channel_kind`。

- [ ] **Step 1: 改 `scope_allowed` 读锁块同时取 `channel_kind`**

在 `src/gateway/server/handler.rs`，把（~1186–1191）：

```rust
                            // Permission-based scope guard check
                            let scope_allowed = {
                                let conns = ctx.connections.read().await;
                                conns.get(&conn_id)
                                    .map(|s| ctx.event_scope_guard.can_receive(topic, &s.permissions))
                                    .unwrap_or(false)
                            };
```

替换为：

```rust
                            // Permission-based scope guard check + surface audience.
                            // Both read the same ConnectionState under one lock.
                            let (scope_allowed, channel_kind) = {
                                let conns = ctx.connections.read().await;
                                match conns.get(&conn_id) {
                                    Some(s) => (
                                        ctx.event_scope_guard.can_receive(topic, &s.permissions),
                                        s.channel_kind,
                                    ),
                                    None => (false, None),
                                }
                            };
```

（`channel_kind: Option<SurfaceKind>` 是 `Copy`，锁释放后仍可用。）

- [ ] **Step 2: 把 `audience_allows` 接进最终判定**

把（~1200）：

```rust
                            scope_allowed && ctx.subscription_manager.should_receive(&conn_id, topic, event_data).await
```

替换为：

```rust
                            scope_allowed
                                && crate::gateway::surface::delivery::audience_allows(
                                    event_data,
                                    channel_kind,
                                )
                                && ctx.subscription_manager.should_receive(&conn_id, topic, event_data).await
```

- [ ] **Step 3: 编译确认接线正确**

Run: `cargo check -p alephcore`
Expected: 编译通过（`audience_allows` 已存在；`channel_kind` 类型匹配；`&&` 短路顺序正确）。零回归由 Task 2 的 `no_audience_field_is_unrestricted` 测试 + 「既有帧无 `audience` 字段」保证。

- [ ] **Step 4: 提交**

```bash
git add src/gateway/server/handler.rs
git commit -m "gateway: gate event forwarding by surface audience"
```

---

### Task 4: 核侧 R5 路由任务 + boot 装配

**Files:**
- Create: `src/gateway/surface/r5_router.rs`
- Modify: `src/gateway/surface/mod.rs`（加 `pub mod r5_router;`）
- Modify: `src/bin/aleph-server/commands/start/mod.rs`（~2095 后 spawn）

- [ ] **Step 1: 注册模块**

在 `src/gateway/surface/mod.rs`（Task 2 已加 `delivery`/`desktop` 两行）补：

```rust
pub mod r5_router;
```

- [ ] **Step 2: 写失败测试 + `r5_router.rs`**

`src/gateway/surface/r5_router.rs`（完整内容）：

```rust
//! Core R5 router.
//!
//! Applies the "is this worth interrupting the user" policy ONCE, in the core,
//! and hands the result to every registered delivery surface. This is the
//! policy that used to live in the desktop shell's `notify.rs` (topic
//! membership + the long-run threshold). The shell now only focus-gates and
//! renders. The decisive focus-gate stays in the shell — the core cannot know
//! window focus.
//!
//! Scope: Phase 1 routes `AskUser` + `RunComplete`. `approval.requested` stays
//! on its existing operator-gated topic path until Phase 2 ("审批回投"), so a
//! guest surface never starts seeing approvals it could not see before.

use std::sync::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

use super::delivery::{OutboundInteraction, SurfaceNotification, SurfaceRegistry};

/// Minimum turn duration before a completed run is worth interrupting for.
/// Mirrors the shell's former `COMPLETION_NOTIFY_MIN_MS` — the policy moved here.
const COMPLETION_NOTIFY_MIN_MS: u64 = 15_000;

/// Max banner body length (char-boundary safe). Mirrors the shell's former cap.
const MAX_BODY_CHARS: usize = 180;

/// Pure policy: map an R5-relevant frame to the notification the user should be
/// interrupted with, or `None` to stay silent. Focus-gating is NOT here (only
/// the shell knows focus); this decides interrupt-worthiness + content only.
pub fn notification_for(frame: &GatewayEventFrame) -> Option<SurfaceNotification> {
    match frame {
        GatewayEventFrame::AskUser { question, .. } => {
            let q = question.trim();
            Some(SurfaceNotification {
                title: "Aleph has a question".to_string(),
                body: if q.is_empty() {
                    "Aleph is waiting for your reply.".to_string()
                } else {
                    truncate(q, MAX_BODY_CHARS)
                },
                source_topic: frame.topic_name(),
            })
        }
        GatewayEventFrame::RunComplete {
            total_duration_ms, ..
        } => {
            if *total_duration_ms < COMPLETION_NOTIFY_MIN_MS {
                return None;
            }
            Some(SurfaceNotification {
                title: "Aleph finished".to_string(),
                body: "Your turn is complete.".to_string(),
                source_topic: frame.topic_name(),
            })
        }
        _ => None,
    }
}

/// Truncate to `max` chars on a char boundary, appending an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Subscribe the typed bus and route interrupt-worthy R5 frames to every
/// registered surface. Runs until the bus closes. A `Lagged` receiver simply
/// skips dropped frames (R5 is best-effort; a missed banner is not fatal).
pub async fn run(event_bus: Arc<GatewayEventBus>, surfaces: SurfaceRegistry) {
    let mut rx = event_bus.subscribe_typed();
    loop {
        match rx.recv().await {
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
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;

    fn run_complete(ms: u64) -> GatewayEventFrame {
        GatewayEventFrame::RunComplete {
            run_id: "r1".to_string(),
            seq: 1,
            summary: RunSummary::default(),
            total_duration_ms: ms,
        }
    }

    #[test]
    fn ask_user_surfaces_the_question() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            question: "Delete it?".to_string(),
            options: vec![],
        };
        let n = notification_for(&f).expect("ask_user notifies");
        assert_eq!(n.title, "Aleph has a question");
        assert_eq!(n.body, "Delete it?");
        assert_eq!(n.source_topic, "agent.ask.user");
    }

    #[test]
    fn empty_question_falls_back() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            question: "   ".to_string(),
            options: vec![],
        };
        assert_eq!(
            notification_for(&f).unwrap().body,
            "Aleph is waiting for your reply."
        );
    }

    #[test]
    fn run_complete_is_gated_by_duration() {
        assert!(notification_for(&run_complete(COMPLETION_NOTIFY_MIN_MS - 1)).is_none());
        let n = notification_for(&run_complete(COMPLETION_NOTIFY_MIN_MS)).expect("long run notifies");
        assert_eq!(n.title, "Aleph finished");
        assert_eq!(n.body, "Your turn is complete.");
        assert_eq!(n.source_topic, "agent.run.complete");
    }

    #[test]
    fn approval_is_not_routed_here() {
        // Phase 1 leaves approval on its operator-gated path (Phase 2 folds it in).
        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
        };
        assert!(notification_for(&f).is_none());
    }

    #[tokio::test]
    async fn run_delivers_to_registered_surface() {
        use crate::gateway::surface::delivery::{DeliveryError, DeliverySurface};
        use crate::gateway::surface::SurfaceKind;
        use std::sync::Mutex;

        struct Capture(Arc<Mutex<Vec<SurfaceNotification>>>);
        impl DeliverySurface for Capture {
            fn kind(&self) -> SurfaceKind {
                SurfaceKind::Desktop
            }
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                let OutboundInteraction::Notify(n) = outbound;
                self.0.lock().unwrap().push(n);
                Ok(())
            }
        }

        let bus = Arc::new(GatewayEventBus::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let surfaces: SurfaceRegistry = vec![Arc::new(Capture(seen.clone()))];
        let task = tokio::spawn(run(bus.clone(), surfaces));

        // Give the subscriber a moment to register before publishing.
        tokio::task::yield_now().await;
        let _ = bus.publish_frame(&run_complete(COMPLETION_NOTIFY_MIN_MS));

        // Poll briefly for the delivery.
        for _ in 0..50 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        task.abort();

        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Aleph finished");
    }
}
```

> 实现者注：`RunSummary` 已 `#[derive(Default)]`（`src/gateway/event_emitter/types.rs:325`），`RunSummary::default()` 可直接用。

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p alephcore gateway::surface::r5_router`
Expected: 全 PASS（5 个测试）。

- [ ] **Step 4: boot 处装配并 spawn**

在 `src/bin/aleph-server/commands/start/mod.rs`，找到 `OperatorApprovalRequester` 装配块（~2082–2095，块内有 `event_bus.clone()`）。在其**闭合 `}` 之后**插入：

```rust
    // Phase 1 delivery surface: register the desktop shell as an addressable
    // outbound surface and spawn the core R5 router. The router applies the
    // "worth interrupting" policy (formerly in the shell) once and delivers to
    // every registered surface; the forward-filter routes by `audience`.
    {
        use alephcore::gateway::surface::delivery::SurfaceRegistry;
        use alephcore::gateway::surface::desktop::DesktopSurface;
        let surfaces: SurfaceRegistry =
            vec![std::sync::Arc::new(DesktopSurface::new(event_bus.clone()))];
        let router_bus = event_bus.clone();
        tokio::spawn(async move {
            alephcore::gateway::surface::r5_router::run(router_bus, surfaces).await;
        });
    }
```

- [ ] **Step 5: 编译确认 boot 接线**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: 通过（`event_bus` 在该作用域已存在，~324 行 `let event_bus = server.event_bus().clone();`）。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/surface/mod.rs src/gateway/surface/r5_router.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "gateway: spawn core R5 router delivering to desktop surface"
```

---

### Task 5: 壳 `notify.rs` 退化为 focus-gate + 纯渲染

**Files:**
- Modify: `desktop/shell/src/notify.rs`

桌面壳改为：订阅 `surface.notify`（核已决策 + 已渲染好 title/body）+ 暂留 `approval.requested`（Phase 2 再收口）；丢弃 topic 成员/时长策略（已搬核）；声明 `channel_kind:"desktop"`（远程桌面 client_ip 非 loopback，必须显式声明才落 Desktop，否则 Phase 0 回退给 Unknown → 收不到 `audience:["desktop"]`）。**focus-gate 与渲染保留在壳。**

- [ ] **Step 1: 改常量（topic 集合 + 删时长阈值）**

把（~30–57）三个 R5 topic 常量与阈值替换。删除 `TOPIC_ASK_USER`、`TOPIC_RUN_COMPLETE`、`COMPLETION_NOTIFY_MIN_MS` 及其文档注释；保留 `TOPIC_APPROVAL`；新增 `TOPIC_SURFACE_NOTIFY`。结果：

```rust
/// A tool call is waiting for approval. A topic event (`approval.requested`).
/// Phase 1 leaves approval on its existing operator-gated path; Phase 2 folds
/// it into the delivery surface.
const TOPIC_APPROVAL: &str = "approval.requested";

/// Core-decided R5 interrupt, already gated for interrupt-worthiness by the
/// gateway's R5 router and addressed to this desktop surface. The shell only
/// focus-gates and renders the core-supplied title/body.
const TOPIC_SURFACE_NOTIFY: &str = "surface.notify";

/// Topics this bridge subscribes to. The interrupt-worthiness policy for
/// run-completion / questions now lives in the core (it publishes
/// `surface.notify`); the shell no longer self-decides those.
const NOTIFY_TOPICS: &[&str] = &[TOPIC_SURFACE_NOTIFY, TOPIC_APPROVAL];
```

- [ ] **Step 2: `PreparedNotification.title` 改 `String`**

把（~225–228）：

```rust
struct PreparedNotification {
    title: &'static str,
    body: String,
}
```

改为：

```rust
struct PreparedNotification {
    title: String,
    body: String,
}
```

- [ ] **Step 3: 重写 `decide_notification`（focus-gate 保留，策略已搬核）**

把整个 `decide_notification`（~239–269）替换为：

```rust
/// Pure notification policy. The core already decided WHAT is worth
/// interrupting for (`surface.notify` is only published when worthy). The shell
/// keeps the one gate only it can apply: a focused Panel never produces an OS
/// banner (R5 — the in-Panel UI already shows the prompt).
fn decide_notification(topic: &str, data: &Value, focused: bool) -> Option<PreparedNotification> {
    if focused {
        return None;
    }
    match topic {
        TOPIC_SURFACE_NOTIFY => Some(PreparedNotification {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Aleph")
                .to_string(),
            body: data
                .get("body")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Aleph needs you.")
                .to_string(),
        }),
        // Approval stays shell-rendered until Phase 2 routes it through the
        // delivery surface too.
        TOPIC_APPROVAL => Some(PreparedNotification {
            title: "Aleph needs your approval".to_string(),
            body: extract_text(data)
                .unwrap_or_else(|| "A tool call is waiting for you.".to_string()),
        }),
        _ => None,
    }
}
```

`extract_text` / `truncate` / `panel_focused` / `emit_notification` / `resolve_event` 不变（`resolve_event` 已能处理 `method == "event"` 的 topic 帧——`surface.notify` 正是这种线形）。

- [ ] **Step 4: `connect_request` 声明 `channel_kind:"desktop"`**

在 `connect_request`（~133–145）的 `params` 字面量中，把：

```rust
    let mut params = json!({
        "device_name": "Aleph Desktop",
        "device_type": "desktop",
        "device_id": "aleph-desktop-shell",
    });
```

改为：

```rust
    let mut params = json!({
        "device_name": "Aleph Desktop",
        "device_type": "desktop",
        "device_id": "aleph-desktop-shell",
        // Declare the surface identity so the gateway routes `surface.notify`
        // (audience ["desktop"]) here even when the daemon is REMOTE — a remote
        // client_ip is not loopback, so the Phase 0 fallback would otherwise
        // label this connection Unknown and it would receive nothing.
        "channel_kind": "desktop",
    });
```

- [ ] **Step 5: 改测试**

`desktop/shell/src/notify.rs` 的 `#[cfg(test)] mod tests`：

a) `connect_request_is_well_formed` —— 增加一行断言：
```rust
        assert_eq!(v["params"]["channel_kind"], "desktop");
```

b) `subscribe_request_carries_notify_topics` —— 替换断言体为：
```rust
        let v: Value = serde_json::from_str(&subscribe_request()).unwrap();
        assert_eq!(v["method"], "events.subscribe");
        let topics = v["params"]["topics"].as_array().unwrap();
        let names: Vec<&str> = topics.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&TOPIC_SURFACE_NOTIFY));
        assert!(names.contains(&TOPIC_APPROVAL));
        // The raw stream topics moved to the core; the shell no longer subscribes.
        assert!(!names.contains(&"stream.run_complete"));
        assert!(!names.contains(&"stream.ask_user"));
```

c) **删除** 不再适用的测试：`ask_user_surfaces_the_question_text`、`run_complete_is_gated_by_duration`、`run_complete_without_duration_stays_silent`、`unknown_topic_is_ignored`（其断言的 `agent.tool.start` 仍返回 None——可保留改名，但更干净是删）。删除 `focused_window_never_notifies` 里对已删 topic 的迭代，改为：

```rust
    #[test]
    fn focused_window_never_notifies() {
        // R5: every subscribed topic stays silent while the Panel is focused.
        for topic in NOTIFY_TOPICS {
            let data = json!({ "title": "t", "body": "b", "message": "m" });
            assert!(
                decide_notification(topic, &data, true).is_none(),
                "topic {topic} should be suppressed when focused"
            );
        }
    }
```

d) **新增** surface.notify 渲染测试：

```rust
    #[test]
    fn surface_notify_renders_core_supplied_title_body() {
        let data = json!({
            "type": "surface_notify",
            "audience": ["desktop"],
            "title": "Aleph finished",
            "body": "Your turn is complete."
        });
        let note = decide_notification(TOPIC_SURFACE_NOTIFY, &data, false)
            .expect("surface.notify fires when unfocused");
        assert_eq!(note.title, "Aleph finished");
        assert_eq!(note.body, "Your turn is complete.");
    }

    #[test]
    fn surface_notify_falls_back_on_missing_fields() {
        let note = decide_notification(TOPIC_SURFACE_NOTIFY, &json!({}), false).unwrap();
        assert_eq!(note.title, "Aleph");
        assert_eq!(note.body, "Aleph needs you.");
    }
```

e) `approval_notifies_when_unfocused` —— 保留（approval 仍走壳渲染），但 `note.title` 现为 `String`，断言 `assert_eq!(note.title, "Aleph needs your approval");` 不变（`&str == String` 比较 OK）。

- [ ] **Step 6: 运行壳测试**

Run: `cargo test -p aleph-desktop-shell notify`
Expected: 全 PASS。

> 桌面壳 crate 名 = `aleph-desktop-shell`（`desktop/shell/Cargo.toml:2`）。

- [ ] **Step 7: 提交**

```bash
git add desktop/shell/src/notify.rs
git commit -m "desktop: degrade notify bridge to focus-gate + render surface.notify"
```

---

## 最终验证（全部任务后）

- [ ] **统一编译 + 测试 + lint**（注意 `cargo test` 单 filter，分次跑）

```
cargo check -p alephcore --all-targets
cargo test  -p alephcore gateway::events::frame::surface_notify_tests
cargo test  -p alephcore gateway::surface::delivery
cargo test  -p alephcore gateway::surface::desktop
cargo test  -p alephcore gateway::surface::r5_router
cargo clippy -p alephcore -- -D warnings
cargo test  -p aleph-desktop-shell notify
```
Expected: 全绿，clippy 净。

- [ ] **零回归人工对账**
  - 既有帧（run.accepted / tool.* / config.changed / approval.* …）序列化无 `audience` 字段 → `audience_allows` 恒 `true` → 转发行为不变。
  - `approval.requested` 仍由 `operator_requester` 原样发布、`event_scope` 仍 operator-gated、壳仍渲染 → 审批体验不变。
  - 桌面壳：长 run（≥15s）完成在非聚焦时弹 "Aleph finished"；短 run 不弹；提问弹问题文本；聚焦全静默 —— 与今天逐条等价（策略改在核执行）。

- [ ] **部署刷新链**（仅本地验证 R5 端到端，可选）
  - 后端改动需重编 `aleph-server` 并热替换正在跑的 daemon（panel dist 未动，无需 `just wasm`）。
  - 桌面壳改动需 `just shell-build` 重打 App 才能在真实 OS banner 上验证；纯逻辑已由 `notify` 单测覆盖。

---

## 范围外（YAGNI，本计划不做）

- **owner-身份隔离**（多用户：A 的通知不弹到 B 的桌面）—— 另立 spec，需把 owner 身份穿过 emit 路径（`RunComplete`/`AskUser` 今不携带）+ 安全审查。本期多 desktop 面各收全部 R5 通知。
- **审批回投走投递面** —— Phase 2。本期 approval 留既有 operator-gated 路径。
- **browser / 其它投递面注册** —— `SurfaceRegistry` 已是扩展点，但本期只注册 desktop。
- **per-surface 渲染细节**（卡片样式 / banner 文案微调）。

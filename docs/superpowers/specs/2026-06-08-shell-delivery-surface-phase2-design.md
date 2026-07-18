# Spec · 壳投递面 Phase 2 — 审批 banner 走投递面（桌面腿）

> 2026-06-08 · 状态：设计已批准，待 writing-plans
> 父 spec：[`2026-06-08-shell-delivery-surface-design.md`](./2026-06-08-shell-delivery-surface-design.md)（§6 Phase 2）。本文件是该 Phase 的精化设计，brainstorming 二次确认后**收窄/澄清**了父 spec 的粗粒度表述。
> 轨道归属：**轨道 1（壳核分离）**。续 Phase 0（命名身份）+ Phase 1（R5 推送走投递面）。

## 0. 一句话

把桌面审批的 **OS banner 出站腿**从 `notify.rs` 的 bespoke `approval.requested` 订阅，收口进 Phase 1 已建的投递面抽象：`r5_router` 把 `ApprovalRequested` 帧映射为 `OutboundInteraction::ApprovalRequest`，`DesktopSurface::deliver` 发**新帧 `surface.approval`**（operator-gated + audience:[desktop]），`notify.rs` 用与 `surface.notify` 同一渲染路径出 banner，删掉重复的审批 arm。**与 Phase 1 完全对称。**

## 1. 父 spec 表述的精化（brainstorming 二次确认）

父 spec §6 Phase 2 原文：「审批请求经 `deliver(ApprovalRequest)` 投到 operator 的投递面，与 channel 审批同一路；消化 Panel 的 in-band / event_bus 审批特例。验收：桌面审批与 Telegram 审批共用一条回投抽象。」

代码核查后发现该表述把三件**audience 与生命周期都不同**的事打包在一起。二次确认的收窄：

| 父 spec 粗表述 | 本期精化决策 | 理由 |
|---|---|---|
| 「与 Telegram 审批共用一条回投抽象」 | **桌面腿收口**：只把桌面 banner 出站腿走投递面；确立 `OutboundInteraction::ApprovalRequest` 为共享接缝。Telegram 留在既有 `ChannelApprovalBridgeAdapter`（它本就是 per-channel 出站投递器，概念已对齐，非字面同 trait）。 | 让 ~20 channel 字面实现 `DeliverySurface` 是大改造、触安全路径、回归面大，违 P6/R10。 |
| 「消化 Panel 的 in-band 文本特例」 | **保留** in-band `ResponseChunk`「⏳ 正在等待管理员授权」。 | 它投给**请求者自己的 run 流**（chat-tier 用户看「我的工具为何挂着」），与 operator banner 是不同 audience，**本质不是投递面重复**，是正交的请求者反馈。移除=减能力。 |
| 「消化 event_bus 帧特例」 | **只收口 banner 腿**：`notify.rs` 不再 bespoke 订阅 `approval.requested` 出 banner；改订阅 `surface.approval`。Panel 卡片仍订阅 `approval.requested`。 | 见 §3。`approval.requested` 帧服务两个不同消费者，只有 banner 腿是与 `surface.notify` 平行的特例。 |

## 2. 审批是「请求-响应」，投递面只统一「出站腿」（R4 守线）

审批有两条腿，**Phase 2 只动出站，绝不碰入站关联**（否则滑向父 spec 风险 #4 的 inbound 解析 = 已否决的 Approach B）：

```
出站腿（本期收口）：  「有个审批在等你」  → DeliverySurface::deliver(ApprovalRequest) → banner
入站腿（原样不动）：  operator 点 approve/deny → Panel RPC exec.approval.resolve
                     → manager.resolve(approval_id, decision) → oneshot 唤醒等待者
```

`DeliverySurface::deliver` 是 fire-and-forget（`Result<(), DeliveryError>`，无响应关联）。响应关联机制（`ExecApprovalManager::pending` 按 `approval_id` 的 HashMap + oneshot）**完全不变**。

## 3. `approval.requested` 帧的双消费者（关键拆分）

今天 `GatewayEventFrame::ApprovalRequested{approval_id, session_key, channel_id, conversation_id}`（由 `OperatorApprovalRequester` 发布）服务**两个不同需求的消费者**：

| 消费者 | 位置 | 用途 | Phase 2 |
|---|---|---|---|
| 桌面壳 `notify.rs` | `desktop/shell/src/notify.rs` 订阅 `approval.requested` → OS banner | **引起注意**（R5 式打扰） | **收口进投递面** → 改订阅 `surface.approval`；删 `approval.requested` arm |
| Panel `context.rs` | `interfaces/webchat/src/context.rs` 订阅 `approval.**` → refetch `exec.approvals.pending` → 渲染卡片 + approve/deny RPC | **入站审批 UI + 关联触发** | **原样不动**（是入站能力，非出站特例） |

「收口」= banner 腿与 R5 同构经投递面；卡片腿（入站关联）保持 `approval.requested` 不动。`OperatorApprovalRequester` 仍发 `approval.requested`（现同时喂 `r5_router` 的 banner 派生 + Panel 卡片）。

## 4. 接缝设计（与 Phase 1 完全对称）

```
ApprovalRequested 帧 (typed bus)
        │
        ▼
  r5_router::run  ── approval_for(frame) ──►  Some(SurfaceApproval) / None
        │                                       └ ApprovalRequested → Some(静态文案 + approval_id)
        │                                       └ SurfaceApproval / 其它 → None  (loop-safety)
        ▼
  for surface in registry:
     surface.deliver(OutboundInteraction::ApprovalRequest(SurfaceApproval))
        │
        ▼
  DesktopSurface::deliver ──► publish_frame(GatewayEventFrame::SurfaceApproval {
                                  audience: ["desktop"], approval_id, title, body })
        │
        ▼
  前向过滤点 (server/handler.rs, 不改)：
     event_scope.can_receive("surface.approval", perms)   ← operator gate（新规则）
     && audience_allows(data, channel_kind)               ← desktop 过滤（Phase 1 既有）
     && should_receive(...)
        │
        ▼
  桌面壳 notify.rs：订阅 surface.approval → decide_notification（focus-gate 保留）→ OS banner
```

`r5_router` 已在 Phase 1 订阅 typed bus 并注册 `DesktopSurface`；本期只在 `run()` 内加一条 deliver 分支 + 一个纯函数 `approval_for`。**boot 不变**。

### 4.1 新类型（净增量）

```rust
// src/gateway/surface/delivery.rs
pub struct SurfaceApproval {
    pub approval_id: String,
    pub title: String,
    pub body: String,
}

pub enum OutboundInteraction {
    Notify(SurfaceNotification),
    ApprovalRequest(SurfaceApproval),   // 新增
}
```

```rust
// src/gateway/events/frame.rs
SurfaceApproval {
    audience: Vec<String>,
    approval_id: String,
    title: String,
    body: String,
},
// topic_name 臂：=> "surface.approval"
// stream_method 不变（_ => None → 非流式 → TopicEvent 线形，与 SurfaceNotify 同）
```

```rust
// src/gateway/surface/r5_router.rs
pub fn approval_for(frame: &GatewayEventFrame) -> Option<SurfaceApproval> {
    match frame {
        GatewayEventFrame::ApprovalRequested { approval_id, .. } => Some(SurfaceApproval {
            approval_id: approval_id.clone(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        }),
        // LOOP-SAFETY: SurfaceApproval 是本路由自己的输出，必须落 None，
        // 否则 DesktopSurface 发回 bus 会被本路由再处理 → 无限放大。
        _ => None,
    }
}
```

`run()` 内每帧分别试 `notification_for`（→ `Notify`）与 `approval_for`（→ `ApprovalRequest`）；二者实践上互斥（一帧非 Ask/RunComplete 即 ApprovalRequested），并存无害。

### 4.2 operator-gate（安全核心，唯一 src/ 侧安全改动）

`src/gateway/event_scope.rs` 的 `default_rules()` 用 `starts_with` 前缀匹配，未匹配规则的 topic = unguarded（对所有人放行）。`surface.notify` 是 unguarded（Phase 1 设计如此，R5 投任意 desktop）。**若把审批丢上 `surface.notify` 会丢掉 operator 闸 → 同机 guest/chat-tier 桌面看到审批 = 回归。** 故用**独立 topic** `surface.approval`，在规则表新增：

```rust
("surface.approval".to_string(), vec!["admin".to_string(), "exec.approver".to_string()]),
```

与既有 `approval.` 规则**同一谓词**：operator/local daemon 持 `*` 通配满足；chat/guest 拒。`surface.approval` 不撞 `surface.notify` 前缀（不同 topic 串）。

## 5. 三处刻意不动（守 R4 / 风险 #4）

- **入站响应关联**：`manager.resolve(approval_id, decision)` via Panel RPC `exec.approval.resolve`。
- **`OperatorApprovalRequester`**：仍发 `approval.requested`（喂 r5_router banner + Panel 卡片）+ in-band `ResponseChunk`（保留，§1）。
- **Panel `approval.**` 订阅 + `exec.approvals.pending` refetch + 卡片 + approve/deny RPC**。
- **`src/harness/`**：零改（R10）。
- **boot**：Phase 1 已 spawn `r5_router::run` + 注册 `DesktopSurface`，不改。

## 6. 组件与物理落点

| 组件 | 位置 | 状态 | 职责 |
|---|---|---|---|
| `SurfaceApproval` frame 变体 | `src/gateway/events/frame.rs` | 净新增 | 审批 banner 的 addressed + gated 出站帧 |
| `SurfaceApproval` struct + `OutboundInteraction::ApprovalRequest` | `src/gateway/surface/delivery.rs` | 净新增 | 投递面审批载荷 |
| `DesktopSurface::deliver` 审批分支 | `src/gateway/surface/desktop.rs` | 净新增（match 加臂） | 把 ApprovalRequest 发为 `surface.approval` 帧 |
| `surface.approval` 规则 | `src/gateway/event_scope.rs`（`default_rules`） | 净新增（1 行 + 测试） | operator-gate（**安全关键**） |
| `approval_for` 纯函数 + `run` 分支 | `src/gateway/surface/r5_router.rs` | 净新增 | 核侧把 ApprovalRequested 路由进投递面 |
| `notify.rs` 审批 arm 替换 | `desktop/shell/src/notify.rs` | 改造 | 订阅 `surface.approval` 取代 `approval.requested`；删 bespoke arm；focus-gate 保留 |

## 7. 零回归论证

1. **banner 文案逐字等价**：`ApprovalRequested` 帧**稀疏**（只有 approval_id，无 tool/reason）→ 今天 `notify.rs` `extract_text` 永远 fallback 到「Aleph needs your approval / A tool call is waiting for you.」。`surface.approval` 带同样静态文案 = banner 逐字等价。审批详情仍在 Panel 卡片（refetch pending）。
2. **operator 闸等价或更窄**：今天 banner 唯一消费者是桌面壳（声明 `channel_kind:"desktop"`）；`audience:[desktop]` 恰命中它。`surface.approval` = operator 闸（同 `approval.` 谓词）+ audience:[desktop]。guest/chat 今天经 `approval.` 闸已拿不到，今后亦拿不到。operator 桌面今天有 banner，今后有。Panel（浏览器/webview）今天从 `approval.requested` 渲染**卡片**非 banner，不受 audience:[desktop] 影响（卡片走 `approval.requested`，不动）。
3. **focus-gate 不变**：`surface.approval` 走同一 `decide_notification`，Panel 聚焦时仍抑制 banner（`if focused { return None }` 在 match 前）。
4. **入站/关联零改**：approve/deny → RPC → `manager.resolve` 路径不动。

## 8. 测试策略（纯单元优先，延续 Phase 1 风格）

- `frame.rs`：`SurfaceApproval` topic_name = `"surface.approval"` + 线形（非流式 TopicEvent）。
- `delivery.rs` / `desktop.rs`：`deliver(ApprovalRequest(..))` 发 `SurfaceApproval{audience:["desktop"], approval_id, title, body}`。
- `event_scope.rs`（**安全测试**）：`surface.approval` + chat/guest perms → `can_receive==false`；`exec.approver` / `admin` / `*` → true。
- `r5_router.rs`：`approval_for(ApprovalRequested)` → Some（静态文案 + approval_id）；`approval_for(SurfaceApproval)` → None（loop-safety）；`approval_for(AskUser)` → None；`run` 在 ApprovalRequested 上 deliver `ApprovalRequest` 到注册面。
- `notify.rs`：`decide_notification("surface.approval", data, focused=false)` → banner；`focused=true` → None；订阅 topics = `[surface.notify, surface.approval]`；旧 `approval.requested` arm 已移除。
- **零回归**：banner 文案 = 今天 fallback 文案；operator 闸不放宽。

## 9. 红线对账

| 红线 | 落地 |
|---|---|
| R1 — 大脑/四肢分离 | banner 渲染最后一公里留壳内 `notify.rs`；核侧只做路由 + 身份 + 闸 |
| R4 — Interface 纯 I/O | 投递面只投出站审批 banner，不持久化、不关联响应、不解析入站 |
| R6 — 一核多端 | 桌面 banner 与 channel 出站投递概念对齐到 `OutboundInteraction` |
| R10 — 薄 harness | `src/harness/` 零改；投递面是 gateway 子系统 |
| 风险 #4（不滑向 inbound） | `ApprovalRequest` 只 carry 出站载荷；响应仍走既有 `manager.resolve` |

## 10. 范围外（YAGNI）

- 不让 channel（Telegram 等）字面实现 `DeliverySurface`（桌面腿收口；Telegram 留 `ChannelApprovalBridgeAdapter`）。
- 不移除 in-band `ResponseChunk`（请求者反馈，正交）。
- 不动 Panel 卡片 / `exec.approvals.pending` / approve-deny RPC（入站能力）。
- 不丰富 `ApprovalRequested` 帧载荷（详情在卡片，banner 静态文案足够且零回归）。
- 不动入站响应关联（`manager.resolve` + oneshot）。

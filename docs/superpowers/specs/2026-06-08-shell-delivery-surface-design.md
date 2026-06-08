# Spec · 壳作为一等投递面（Delivery Surface）

> 2026-06-08 · 状态：设计已批准，待 writing-plans
> 轨道归属：**轨道 1（壳核分离 / Aleph 专属 Channel）**。续 Spec A `2026-06-07-desktop-remote-gateway-design.md`。与轨道 2（Aleph 集群 `2026-06-08-aleph-cluster-design.md`）正交。

## 0. 一句话

把桌面壳从「直走 WS JSON-RPC 控制面、与 channel 抽象平行的二等特例」打磨成 **Aleph 自己的一等投递面（Delivery Surface）**：有具名身份、权限随身份走、R5 推送与审批回投经统一抽象路由——不再寄生其它通道、不再三条并行特例。

## 1. 背景与动机

「一核多端」(R6) 今天有个隐藏的不对称：

- `src/gateway/interfaces/` 注册了 ~20 个 channel（telegram / discord / slack / feishu …），它们是「外部平台 inbound bot」抽象，也是**审批投递**（`ChannelApprovalBridgeAdapter`，Telegram operator-DM 路）与 **R5 主动推送**赖以构建的基底。
- **桌面壳 + Panel 不是注册 channel**。它直走 WS JSON-RPC 控制面，并靠**三条并行特例**补齐 channel 本该免费提供的能力。

桌面壳今天的三条并行路：

| 出站交互 | channel（如 Telegram） | 桌面壳（今天） |
|---|---|---|
| R5 主动推送 | channel 出站发消息 | `notify.rs` 自行重订阅 event_bus WS + 自行过滤 topic + 原生 banner |
| 审批请求回投 | `ChannelApprovalBridgeAdapter`（operator DM） | event_bus 帧 + in-band 文本特例（Phase 3b-2a / 2b） |
| 普通回复渲染 | channel render text | Panel 富 UI 渲染（JSON-RPC） |

权限/身份也隐式：壳的档位由 device(operator/guest)+tier(chat/config) 推出，而「本机 desktop → operator / 远程 pairing → chat」这条规则**隐式埋在 loopback 检查里**，不是一个具名的 channel 身份。

「不再寄生、升为一等公民」= **给壳一个具名身份，并把推送 + 审批收口到一条统一投递抽象**。

## 2. 锁定决策（brainstorming 已确认）

| 维度 | 决策 |
|---|---|
| 抽象深度 | **轻量「投递面」身份层**（Approach A）。**否决**让桌面壳实现 Telegram 形状的 `MessagingChannel` trait（Approach B）——Panel 渲染富 WASM UI + 已说 JSON-RPC，硬塞进 parse-text/render-reply 契约是方枘圆凿（违反 P6/R3）。 |
| 权限引擎 | **不重建**。Spec B 的 device+tier 即引擎。本 spec 只让「权限源头」**显式化、具名化**。 |
| R1 切分 | **渲染最后一公里永远留壳内**（notify.rs / Panel）。本 spec 收口的是**核侧路由 + 具名身份**，不把壳渲染搬进核。 |
| 身份落点 | `ConnectionState` **新具名字段** `channel_kind`（与 `role`/`device_id` 同级），非塞进弱类型 `metadata` HashMap。权限源头值得一等字段。 |
| 互斥 | **不碰** Spec A 的本机/远程互斥。一壳一核不变。 |

## 3. 概念脊柱（the model）

壳核分离的规范模型，本 spec 形式化之：

```
壳 (shell / 肢体) = 纯 I/O：渲染 + 附着到唯一一个核
核 (core / 大脑)  = 业务推理
```

每个附着的壳连接由**三个正交 facet** 描述：

| facet | 含义 | 状态 |
|---|---|---|
| **附着目标** (attach target) | 这个壳连哪个核（本机/远程，互斥） | Spec A 已落地 |
| **投递面身份** (surface identity) | 这个壳是什么 channel kind（`desktop` / `browser` / `telegram` …）+ 实例 | **本 spec 新增（命名）** |
| **权限档位** (permission tier) | operator / chat，来自 device+tier 引擎 | Spec B 已落地 |

**核心主张**：**权限源头 = 核根据「投递面身份 × 附着模式」给附着的壳派发档位**。本机 desktop 壳 → operator；远程 browser → chat（until 升档）。这条规则今天隐式埋在 loopback 检查里；本 spec 把它命名、显式化。

**不变量**：一壳一核（互斥，Spec A）；壳只渲染不推理（R4）；核是唯一大脑（R6）。

## 4. 投递面抽象（the code seam）

核侧引入一个轻量抽象：**「把一个出站交互投递到一个具名可寻址的投递面」**。

```rust
// 核侧，addressable。不是 MessagingChannel——只管出站投递 + 身份。
trait DeliverySurface: Send + Sync {
    fn kind(&self) -> SurfaceKind;            // Desktop | Browser | Telegram | ...
    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError>;
}

enum OutboundInteraction {
    Notify(R5Event),          // R5 主动推送
    ApprovalRequest(..),      // 审批请求回投
    // 富回复仍走 Panel JSON-RPC 渲染，不进此枚举
}
```

**R1 边界（关键）**：桌面壳是独立进程，渲染最后一公里永远留壳内。`DesktopSurface::deliver` 的实现 = 经现有 event_bus / WS 把交互推给那条已连的壳连接；壳内 `notify.rs` / Panel 仍负责把它变成 banner / 卡片。本 spec **不**把壳渲染搬进核——只让「桌面投递面」成为与 channel 平级、可被核统一寻址的投递目标。

**消除的并行**：
- `notify.rs` 不再「自己决定订阅哪些 topic + 自己过滤」——核侧投递面决定「这个用户该被 R5 打扰」并定向投递；壳内 `notify.rs` 退化为纯渲染。
- 审批回投不再是 event_bus + in-band 文本特例，与 channel 审批走同一个 `deliver(ApprovalRequest)`。

**focus-gate 留壳内**：「Panel 是否聚焦」只有壳侧知道，所以 R5 的 focus-gate 必然仍在 `notify.rs`；核侧只做「是否值得打扰 + 投给哪个面」。这条接缝在 plan 阶段画清——是本 spec 最大的真实改动面。

## 5. 组件与物理落点（P2 高内聚 / R10 不污染 harness）

| 组件 | 位置 | 状态 | 职责 |
|---|---|---|---|
| `channel_kind` 字段 | `src/gateway/server/mod.rs`（`ConnectionState`） | 净新增（具名字段） | 连接身份带具名 surface kind；编译器强制全构造点更新 |
| `channel_kind` 握手解析 | `src/gateway/handlers/auth/connect.rs` | 净新增 | connect 时识别壳声明的 kind（desktop / browser），缺省按 transport 推断 |
| `default_tier(kind, is_loopback)` | tier SSOT（复用 Spec B `tier.rs`） | 净新增（具名函数） | 把隐式默认档位规则抽成具名函数；不重建引擎 |
| `DeliverySurface` trait + `SurfaceKind` | **新 `src/gateway/surface/`**（或既有 interfaces 旁） | 净新增 | 核侧出站投递抽象 + 具名身份 |
| `DesktopSurface` 实现 | `src/gateway/surface/desktop.rs` | 净新增 | 经 event_bus/WS 把交互投给壳连接；不渲染 |
| R5 推送定向投递 | 复用现有 R5 / event_bus 生产点 | 改造 | 核侧决策投给哪个面，替代 notify.rs 自订阅 |
| 审批回投统一 | 复用 Phase 2b approval infra + `ChannelApprovalBridgeAdapter` 旁 | 改造 | 桌面审批与 channel 审批共用 `deliver(ApprovalRequest)` |
| 壳侧 `notify.rs` 退化 | `desktop/shell/src/notify.rs` | 简化 | 退为纯渲染 + focus-gate；不再自定 topic 过滤 |

**R10 红线守住**：`src/harness/` 不增不改——投递面是 gateway 子系统，与 Think→Act 循环无关。harness 连「这交互投给谁」都不知道。

## 6. 分阶段（同一份 spec 定架构，每阶段独立 plan）

- **Phase 0 · 命名身份 + 权限源头（小、外科手术式）**
  - connect 握手让壳显式声明 `channel_kind`（`desktop` / `browser`），落进 `ConnectionState` 新具名字段；缺省按 transport（loopback / remote）推断。
  - 把隐式默认档位规则抽成具名函数 `default_tier(channel_kind, is_loopback)`：本机 desktop→operator，远程 browser→chat。与 Spec B device+tier 引擎对接。
  - **验收**：连接身份带具名 kind；默认档位由具名函数决定；本机零回归（不带 kind 的旧连接 = 今天行为）。

- **Phase 1 · R5 推送走投递面**
  - 引入 `DeliverySurface` + 桌面投递面注册为核侧可寻址目标；核侧 R5 决策定向投递；壳内 `notify.rs` 退化为纯渲染（保留 focus-gate）。
  - **验收**：核统一把 R5 投给桌面面，无并行 event_bus 重订阅逻辑；focus-gate 仍在壳内、行为不变。

- **Phase 2 · 审批 banner 走投递面（桌面腿）** — 精化设计见 [`2026-06-08-shell-delivery-surface-phase2-design.md`](./2026-06-08-shell-delivery-surface-phase2-design.md)
  - 桌面审批的 **OS banner 出站腿**经 `deliver(ApprovalRequest)` 走投递面：`r5_router` 把 `ApprovalRequested` 映射为 `OutboundInteraction::ApprovalRequest`，`DesktopSurface` 发**新帧 `surface.approval`**（operator-gated + audience:[desktop]），`notify.rs` 删 bespoke `approval.requested` arm、改订阅 `surface.approval`。确立 `OutboundInteraction::ApprovalRequest` 为共享接缝。
  - **brainstorming 二次确认的收窄**（父 spec 原表述过粗）：① Telegram 留在既有 `ChannelApprovalBridgeAdapter`（概念已对齐，**不**字面实现 `DeliverySurface`）；② **保留** in-band `ResponseChunk`「⏳ 等待管理员授权」（请求者反馈，正交，非投递面重复）；③ Panel 卡片 / `exec.approvals.pending` refetch / approve-deny RPC（入站能力）原样不动。
  - **验收**：桌面审批 banner 经投递面统一渲染；删除 `notify.rs` 重复审批 arm；operator 闸不放宽（guest/chat 桌面拿不到 `surface.approval`）；入站响应关联零改。

- **Phase 3 · owner-身份隔离（2026-06-08 brainstorming 后整个推迟，YAGNI）**
  - **动机**：Phase 2 收尾时记下「多 desktop 面各收全部审批/通知 banner，未按 owner 隔离」。本次 brainstorming 逐路径核查后判定：当前**没有值得建的东西**，整个 Phase 推迟，不建 per-install 身份基础设施。
  - **核查链（三条结论）**：
    1. **审批 banner 的 owner ≠ run 的 owner**。`OperatorApprovalRequester`（`src/approval/operator_requester.rs`）刻意把 config 工具审批路由给 **operator 角色**（Phase 2b sudo 特权升级闸：请求者故意不是审批者）。扇出到所有 operator 桌面是 **by-design 正确**，不是缺陷；按「发起 run 的设备」隔离反而会破坏模型（guest 发起的审批将无 operator 可落）。故审批 banner **保持 operator 扇出，不做 owner 隔离**。
    2. **跨渠道 R5 噪音不存在**。执行/输出二分：路径 1（Panel `gui:chat` / `agent.run` / CLI）走 `GatewayEventEmitter`→**event_bus**→r5_router；路径 2（外部 bot 渠道 telegram/slack/discord/feishu/whatsapp）走 `ReplyEmitter`**直接回渠道**。`ReplyEmitter` 无 `event_bus` 字段，inbound/reply 路径 grep `event_bus` 为空——**外部渠道的 `RunComplete`/`AskUser` 从不进 event_bus**，r5_router 永不为其产桌面 banner。外部渠道**早已被 emitter 二分隔离**。
    3. **单桌面零噪音**。路径 1 内，Panel 自己的长 run（`COMPLETION_NOTIFY_MIN_MS` 15s 闸）+ Panel 失焦（壳侧 focus-gate）才弹 banner = 正是 R5「你走开了，结果来了」的预期行为，非噪音。
  - **唯一残留真缺口**：路径 1 内部的**多 gateway surface 互投**（本地桌面 Panel + 远程 operator 浏览器 Panel，A 的 run 弹 B 的桌面）。修它需要新的 **per-install owner 身份**：唯一持久 `install_id`，被同一 App 的 Panel + `notify.rs` 两条连接**共同声明**，再从 dispatch→run→R5 帧→forward-filter 穿线匹配。横跨 core + shell + panel，触 run 热路径与两个客户端 crate。
  - **为何推迟**：① 该缺口仅在 **≥2 个 operator 桌面同时连接** 时显现，非当前个人部署的真实场景；② 现有身份不可用作 owner 键（`notify.rs` 硬编码 `device_id:"aleph-desktop-shell"` 每台机器相同；Panel 不发 device_id，核每次分配随机 UUID 且与本机 notify 连接无关联）；③ 即便多设备同属一人，「在自己任一设备看到完成提示」也可能是期望而非 bug。
  - **重启条件**：当多 operator 桌面成为真实部署需求时，另立 `Phase 3b · per-install owner 身份` spec（brainstorming→spec→plan→实施）。

## 7. 安全

- **凭证隔离不变**：沿用 Spec A 纪律——远程壳绝不拿本机 token；本机 token 只在 loopback 注入。
- **权限源头显式化即防御**：`default_tier` 具名函数比「散落 loopback 检查」更难误判——远程壳默认 chat 是一行可审计的规则，非隐式推断。
- **投递面不放大权限**：投递面只投递出站交互，不授予任何入站能力；入站仍受 method_authz + tier 门控（Spec B）。

## 8. 测试策略（延续现有风格，纯单元优先）

- `channel_kind`：connect 解析（显式声明 / transport 缺省推断）；`ConnectionState` 往返。
- `default_tier`：(desktop, loopback)→operator；(browser, remote)→chat；边界与缺省。
- `DeliverySurface` / `DesktopSurface`：投递路由到正确壳连接；连接不在时的错误面。
- R5：核侧定向投递断言；focus-gate 单测仍在壳侧 `decide_notification`（不回归）。
- 审批：桌面审批与 channel 审批走同一 `deliver(ApprovalRequest)`；in-band 特例移除后行为等价。
- 零回归：不带 channel_kind 的旧连接 = 今天权限/推送行为。

## 9. 红线对账

| 红线 | 落地 |
|---|---|
| R1 — 大脑/四肢分离 | 渲染最后一公里留壳内；核侧只做路由/身份；壳零 `src/` |
| R3/R10/P6 — 薄、不过度抽象 | 复用 event_bus/WS，不建第二传输；拒绝把富 Panel 塞进 `MessagingChannel` |
| R4 — Interface 纯 I/O | 投递面只投递 + 渲染，不持久化、不推理 |
| R6 — 一核多端 | 桌面壳成为与 channel 平级的具名一等公民 |
| R8 — 一切皆工具 | 档位切换沿用 Spec B 既有 `devices.set_level` 等工具 |
| R10 — 薄 harness | `src/harness/` 不增逻辑；投递面是 gateway 子系统 |

## 10. 风险与权衡

1. **R5 决策从壳搬到核的接缝**：focus 状态只有壳知道——核侧只能做「值不值得打扰 + 投给哪个面」，最终 gate 仍在壳。plan 阶段必须画清「核侧投递 vs 壳侧 focus-gate」的责任线，否则会出现「核以为投了、壳 focus-gate 吞了」的静默丢失。本 spec 最大真实改动面。
2. **`channel_kind` 全构造点更新**：`ConnectionState` 新具名非 Default 字段会强制全部构造点更新（编译器即检查），需确认测试构造点数量（参考 Spec B AuthContext 经验：Explore 可能漏报）。
3. **审批回投统一的等价性**：~~移除 in-band 文本特例后~~（2026-06-08 二次确认：**in-band `ResponseChunk` 保留**，它是请求者反馈非投递面重复）。Phase 2 只换 **banner 出站腿**的路由（`approval.requested` arm → `surface.approval`），必须验证桌面审批 banner 体验等价（稀疏帧→静态文案 fallback 逐字等价）+ operator 闸不放宽（`surface.approval` 与 `approval.` 同谓词）——不是删能力，是换路由。Panel 卡片（入站 UI）不动。
4. **投递面 vs channel 的边界拿捏**：`DeliverySurface` 只做出站投递，**绝不**滑向 inbound parse——一旦有人想给它加「解析壳消息」就是滑回 Approach B，需在 review 守住。
5. **loopback-bot channel_kind 误标（Phase 0 已落、~~Phase 1 前必须解决~~ — 2026-06-08 代码核查后判定伪命题）**：Phase 0 的身份推断对「未声明 kind + loopback」回退为 `Desktop`。担心本机 Telegram/Slack bot adapter 也走 loopback 被标成 `Desktop` 而在 Phase 1 投递路由时被误投。**核查结论：不成立。** `src/gateway/interfaces/` 的 ~20 个 channel 无一回连自己的 WS——telegram(teloxide HTTP) / discord / slack / feishu / whatsapp… 全是 in-process 跑 HTTP/原生协议客户端，从不发 loopback `connect`。能走 loopback `connect` 的只有桌面壳 / 浏览器 Panel / CLI，三者都在首帧显式声明 `channel_kind`，撞不上 Desktop 回退。Phase 1 plan 进一步令 `notify.rs` 显式声明 `channel_kind:"desktop"`（远程桌面 client_ip 非 loopback，必须显式声明），回退仅作安全网。**此风险关闭。**

## 11. 范围外（YAGNI）

- 不碰 Spec A 的本机/远程互斥（一壳一核不变）。
- 不重建 device+tier 权限引擎（Spec B 即引擎）。
- 不实现 `MessagingChannel` 的 inbound text 契约。
- 不做多投递面聚合 / 一壳多核（已否决的 Model A）。
- Phase 1/2 的壳侧渲染细节（卡片样式、banner 文案等）留各自 plan。
- 远程通知凭证派生（沿用 Spec A：远程 notify 只降级）。
- **owner-身份隔离（Phase 3）整个推迟**：审批 banner 的 operator 扇出 by-design 正确不动；跨渠道 R5 噪音经 emitter 二分早已隔离（不存在）；唯一残留的多 gateway surface 互投需 per-install 身份，待多 operator 桌面成真实需求再开（详见 §6 Phase 3 决策记录）。

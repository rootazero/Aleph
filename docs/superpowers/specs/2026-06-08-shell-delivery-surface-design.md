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

- **Phase 2 · 审批/交互回投走投递面**
  - 审批请求经 `deliver(ApprovalRequest)` 投到 operator 的投递面，与 channel 审批同一路；消化 Panel 的 in-band / event_bus 审批特例。
  - **验收**：桌面审批与 Telegram 审批共用一条回投抽象。

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
3. **审批回投统一的等价性**：移除 in-band 文本特例后，必须验证桌面审批体验等价（Phase 3b-2a 卡片仍在）——不是删能力，是换路由。
4. **投递面 vs channel 的边界拿捏**：`DeliverySurface` 只做出站投递，**绝不**滑向 inbound parse——一旦有人想给它加「解析壳消息」就是滑回 Approach B，需在 review 守住。
5. **loopback-bot channel_kind 误标（Phase 0 已落、Phase 1 前必须解决）**：Phase 0 的身份推断对「未声明 kind + loopback」回退为 `Desktop`。但本机 Telegram/Slack bot adapter 也走 loopback，会被标成 `Desktop`。Phase 0 无害（`channel_kind` 仅作 identity，不参与 tier——tier 只由 is_loopback 决定）。**但 Phase 1 一旦用 `channel_kind` 做投递路由（如「只给 Desktop 面推桌面通知」），这些 bot 连接会被误投。** Phase 1 落地前必须让本机 bot 显式声明自己的 kind，或在路由处加 metadata 标记区分真桌面壳与同机 bot。

## 11. 范围外（YAGNI）

- 不碰 Spec A 的本机/远程互斥（一壳一核不变）。
- 不重建 device+tier 权限引擎（Spec B 即引擎）。
- 不实现 `MessagingChannel` 的 inbound text 契约。
- 不做多投递面聚合 / 一壳多核（已否决的 Model A）。
- Phase 1/2 的壳侧渲染细节（卡片样式、banner 文案等）留各自 plan。
- 远程通知凭证派生（沿用 Spec A：远程 notify 只降级）。

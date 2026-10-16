# 阶段 6 Spec — 受控浏览器 Slot(右栏)

> **任务边界**:本文件是**探索 + 方案**,**不**写实际重构代码。落地属于阶段 6 实施,待用户独立批准。
>
> **系列位置**:[阶段 0 总览(2026-10-15-rightside-optimization-plan.md)](2026-10-15-rightside-optimization-plan.md) · [阶段 1 Slot 接口(2026-10-15-rightpane-stage1-slot-spec.md)](2026-10-15-rightpane-stage1-slot-spec.md) · [阶段 2 宿主切换(2026-10-15-rightpane-stage2-host-switching-spec.md)](2026-10-15-rightpane-stage2-host-switching-spec.md) · [阶段 3 子代理 slot(2026-10-15-rightpane-stage3-subagent-slot-spec.md)](2026-10-15-rightpane-stage3-subagent-slot-spec.md) · [阶段 4 能力 slot(2026-10-15-rightpane-stage4-capabilities-slot-spec.md)](2026-10-15-rightpane-stage4-capabilities-slot-spec.md) · [阶段 5 附件 slot(2026-10-15-rightpane-stage5-attachments-slot-spec.md)](2026-10-15-rightpane-stage5-attachments-slot-spec.md) · **阶段 6 本文件**

---

## §0 TL;DR

- **当前架构 = 状态壳 + `_blank` 跳转**:`viewer.rs` 是**纯渲染壳**(0 I/O,无 socket,无 `location.hash`),唯一出站动作是 `window.open(launch_url, "_blank")`(viewer.rs:36-47 `open_in_new_tab`)。grant 流跑完后 Aleph 失去一切控制权 —— 它既看不到目标页,也无法回传数据,更不能阻止用户离开。
- **standalone 页面已经是"半受控"基线**:`/viewer/browser` 路由用 `ViewerSession::from_location` 解析 fragment,经 `transport::start` 走 WebSocket 拿 frames,每帧作为 `<img>` 渲染(standalone.rs:228-262 `StandaloneShell`)。`session.rs` + `transport.rs` 已经是**完整的** consume / exchange / AuthHello / WebSocket 状态机。
- **Tauri 平台是真实可选项**:`platform_host.rs:15-23` 枚举 `HostPlatform::{MacOs, Windows, Linux}`,`baseline-probe.js` 在 WASM 启动前同步把 `data-platform` 写到 `<html>`。Tauri 桌面端的 WKWebView / WebView2 / WebKitGTK 是**真实可达的 webview 宿主**,不止一个文档提到了它(`chat/composer/voice.rs:10` "the unsigned `WKWebView` cannot reach `getUserMedia`" 等)。
- **Canvas 已有 iframe 沙箱先例**:`canvas/shape_view.rs:892` 唯一一处 `<iframe sandbox="allow-scripts">` 生产位(`i18n_census.rs:1556` 注释确认 "three sandboxed pin",源级 census 在 `shape_view.rs:1073-1110` 测块守护),证明 Aleph 端用 iframe 嵌**可信同源子页**是成熟模式。
- **本阶段核心动作**:
  1. **复用 standalone transport,作为 slot 渲染体**(`RightPaneSlot` 接入 `StandaloneBrowserViewer` 组件,不再跳 `_blank`)
  2. **保留 `_blank` 作为兜底**(Tauri 平台可继续走 native new-tab,iframe 失败的页面/用户点 "Open in tab" 按钮)
  3. **可观察 / 可截图**(iframe 模式 + postMessage,或者由 standalone 主动经 `transport.failed` 报告关键事件)
- **3 个待拍板决策** S6-A / S6-B / S6-C,见 §3.1。

---

## §1 Aleph 现状审计(已读 7 个 browser/ 文件 + platform_host.rs)

### §1.1 `interfaces/webchat/src/platform/wide/views/browser/mod.rs` — 51 行公共 API 地图

7 个子模块,导出 19 个 pub 名,组织方式:
- `model` — 状态类型(纯)
- `launch` — launch 状态机(纯)
- `session` — WebSocket 帧 + 认证(纯 + 一处 wasm 调用)
- `transport` — consume/exchange/AuthHello/WebSocket 状态机(wasm-only)
- `bridge` — `browser.live.resource` / `browser.live.closed` 事件解码 + 应用
- `viewer` — 渲染壳(纯 + 一处 `open_in_new_tab`)
- `standalone` — `/viewer/browser` 路由组件

**模块级注释关键句**:
> "The shell still does not control the browser or derive a target from a URL, profile or current tab." — `mod.rs:18`

这是当前架构的**最诚实自我描述**。本阶段就是要把这一行改掉。

### §1.2 `viewer.rs`(976 行)— 纯渲染壳

**三态** `BrowserViewerModel`(`model.rs`):
- `Unbound(NoResource | Refused(reason) | Closed(tombstone))` — viewer.rs:780-785
- `Bound + Withheld(redaction ∈ {Pending, Refused})` — viewer.rs:128-135 `withheld()`
- `Bound + Ready(redaction ∈ {Safe, Redacted})` — viewer.rs:786-795

**出站动作只有 3 个**:
1. `PendingDecision` 的 Safe/Refused 按钮(viewer.rs:172-205)→ 走 `BrowserViewerApi::decide`
2. `LaunchSection` 的 `issue` / `open` / `dismiss` 按钮(viewer.rs:387-647)→ 走 `BrowserViewerApi::issue_frames_grant` / `revoke_grant` / `open_in_new_tab`
3. `PairingNotice` 的 "Pair desktop" 按钮(viewer.rs:355-371)→ `state.request_native_pairing()`

**关键约束**:
- `open_in_new_tab` 是**唯一**与浏览器交互的出口(`viewer.rs:36-47`)
- `BoundSummary` 的"highlight"只跟**该 resource 自己发布的 candidate id**,不接任何 URL/tab/profile(viewer.rs:683-687)
- `LaunchSection::Effect` 持续盯"resource 改 / binding 改 / caller 改"三件事,任一不匹配即 revoke grant(viewer.rs:430-452)——**这是 Aleph 唯一能控制的"session 寿命"**

### §1.3 `launch.rs`(559 行)— Launch 状态机

**5 个状态** `LaunchPhase`:
- `Idle` (默认) → `Issuing` → `Ready(IssuedLaunch) | Failed`
- `Ready → Opened | Blocked | (revoked)`
- `Blocked → Ready (retry) | Failed`

**6 个导出**:`launch_target` / `model_launch_target` / `validate_launch_url` / `begin_issue` / `finish_issue` / `take_unopened`

**`IssuedLaunch` 持有 3 字段**:
- `launch_url: String` — 形如 `https://aleph/viewer/browser#grant_id=…&nonce=…`
- `grant_id: String`
- `expires_at_ms: i64`

**关键设计**:
- `IssuedLaunch::fmt::Debug` 显式 redact `launch_url` 字段(launch.rs:101-110)
- `validate_launch_url` 4 步校验:有 fragment / origin 合法 / path 正好等于 `VIEWER_PAGE_PATH` / fragment 里的 grant_id 与 expected 一致(launch.rs:88-99)
- `IssuedLaunch` 一旦 Opened / Dismissed 即**整体 drop**(launch.rs:46,164-175)

### §1.4 `standalone.rs`(481 行)— 独立页 /viewer/browser

**`StandaloneBrowserViewerRoute` 行为**(standalone.rs:191-213):
1. `read_location()` 拿 `pathname / origin / hash`
2. `begin_standalone_session(location, replace_current_entry)` — 先用 `history.replace_state` 把 URL 改写成 `VIEWER_PAGE_PATH`(无 fragment 无 query),**只有这一步成功才**继续
3. `ViewerSession::from_location` 解析 in-memory copy → `transport::start(session, ui, origin)`

**`StandaloneBrowserViewer` 渲染**:
- `TransportUi::new()` — signal-only UI 桥
- `was_live` memo — 粘性 Authenticated 标志(用来把"已 live 后丢失"判为 Closed 而非 Failed)
- `frame_url: Signal<Option<String>>` — `transport` 写入,`StandaloneShell` 读出
- 渲染:6 个 `StandaloneNotice` 状态(InvalidLink / Unavailable / Connecting / Live / Failed / Closed),Live 时一个 `<img data-testid="browser-live-frame" src=url>`

**`LocationParts` 严格 drop 策略**:
- `replace_current_entry` 永远把地址栏改写成裸 `VIEWER_PAGE_PATH`(standalone.rs:79-94)—— **无论 fragment 是否合法**
- 注释明示 "If `scrub` fails the session is never built, so no consume/exchange/auth request can start while the credential is still in the address bar"(standalone.rs:107-110)

### §1.5 `session.rs`(1332 行,未通读)+ `transport.rs`(726 行,未通读)

**已知**:
- `ViewerSession::from_location` 解析 fragment,失败返回 `FragmentError`(mod.rs 导出)
- `transport.rs::TransportUi` 是 signal-only 桥(无 I/O),`transport::start` 跑 `consume → exchange → AuthHello → WebSocket` 链
- `is_standalone_viewer_path` 严格 path 等于 `/viewer/browser`

**未读**:
- `AcceptedFrame` / `AuthHelloText` / `CloseReason` / `OriginError` / `FrameReject` 字段细节
- WebSocket 重连 / heartbeat 策略

这些是**实施阶段必读**;本 spec 不展开。

### §1.6 `bridge.rs`(569 行)+ `model.rs`(1354 行,未通读)

**已知**(mod.rs 导出):
- `decode_browser_event` / `apply_browser_event` / `BrowserBusEvent` — 唯一认 `browser.live.resource` / `browser.live.closed` 两个 topic,nested envelope 只取 `resource` / `closed` 字段(mod.rs:13-14 注释)
- `BrowserViewerModel` / `BoundBrowser` / `ClosedResource` / `UnboundReason` / `ViewerPhase` / `ModelError`

**已知量级**:model 1354 行,包含完整 reducer,大概是 30+ 方法。

### §1.7 `platform_host.rs`(116 行)— 平台探测

**`HostPlatform::{MacOs, Windows, Linux}`**:
- 解析 `data-platform` attribute(platform_host.rs:23-37)
- 缺省 / 未知 → `Linux`(platform_host.rs:34,127-134)—— "Flat rendering is a degradation, never a hazard, so an unknown host gets the conservative answer"
- 真实来源是 `baseline-probe.js`(platform.rs:6-9 注释)—— 同步在 WASM 启动前写 attribute

**`platform/wide/views/canvas/shape_view.rs:892` 是唯一的 iframe 生产位**:
- `sandbox="allow-scripts"`(从来 **不** `allow-same-origin`)
- 用途:渲染 `Shape::Html` 的 srcdoc
- 源级 census 测块守护(shape_view.rs:1073-1110)—— 防止"第二处 iframe 偷偷长出来"

### §1.8 当前架构的全景一句话

> 一切由 gateway 主持:gateway 决定 resource 是否 Safe → viewer 拿到 resource → 用户点 Open → gateway 给一次性 grant + launch_url → Aleph 跳 `_blank` → standalone tab 自己跑 WebSocket → 拿 frames → 当 `<img>` 渲染。**Aleph 在 grant 之后唯一能做的就是 revoke**。

---

## §2 受控浏览器的本质(本 spec 的设计锚点)

### §2.1 "受控"三轴

| 轴 | 含义 | 当前 | 受控目标 |
|---|---|---|---|
| **可观察** | Aleph 能看到目标页内容(URL / 截图 / 错误) | 0(全靠 _blank) | 至少 Live 时能看到 frame |
| **可截断** | Aleph 能阻止用户离开 / 能 revoke | 仅 revoke | 嵌入后能主动 close / reject navigation |
| **可回写** | Aleph 能把数据回灌到目标页(填表 / 注入) | 0 | iframe 模式可 postMessage;WebSocket frame 通道可挂 payload |

### §2.2 关键边界(必守)

- **Aleph 不是浏览器**:不做"标签页 / 前进 / 后退 / 地址栏"全套 UI;只做"看到 + 截断 + 偶尔回写"。
- **Trust 边界不变**:`BrowserWorkspace` model 是 root-level 唯一真源(bridge.rs);iframe / 嵌入式 webview 是**纯渲染体**,不写回 model(model 仍然只走 `apply_browser_event`)。
- **`_blank` 不能丢**:Tauri 平台用户可能更习惯 native tab;iframe 在某些站(cross-origin `X-Frame-Options: DENY`)会失败 —— **必须**保留"Open in tab"按钮。
- **grant 寿命优先**:LaunchSection::Effect 盯 3 件事的逻辑在 iframe 模式下**必须**保留;revoke 时关掉 iframe / 关掉 standalone tab。

### §2.3 与 5 个上游 spec 的关系

- 阶段 0 总览 §3.1.6 已规划"受控浏览器嵌入留待下一期独立 spec" —— 本 spec 就是那个 spec。
- 阶段 1 `RightPaneSlot` trait 已是形态 **5** 之一(spec §5)—— 本 spec 在其下挂 `rps.browser`。
- 阶段 2 把容器切到 flex 兄弟列 —— 受控浏览器天然有"占 50% viewport"的位置,可借 阶段 2 的 clamp 边界。
- `BrowserWorkspace` 的 producer 已在 `interfaces/webchat/src/app.rs:153-154` 创建并 `provide_context`；当前 `WorkspacePanel` 在 `interfaces/webchat/src/components/workspace_panel.rs:82-87` 读取该 context。阶段 6 的 slot 应复用这一个 root-level model，不得另建第二份 browser 状态源。

---

## §3 决策点(已批,见决策日志 §10)

### §3.1 3 个已批决策

| 决策 | 含义 | 选项 | 推荐 |
|---|---|---|---|
| **S6-A** | 受控范围(可观察 / 可截断 / 可回写三轴的覆盖) | **A1** 全三轴(观察 + 截断 + 回写)<br>✅ **A2** 观察 + 截断,回写留 v2<br>**A3** 仅观察(截图 + 状态) | **已批 A2** — iframe 模式观察天然有,截断 = revoke;回写 = postMessage 是工程量级,留 v2 |
| **S6-B** | 嵌入路径(走哪条路把 standalone transport 装回右栏) | ✅ **B1** 复用 standalone 组件 + slot 渲染体<br>**B2** iframe src=launch_url,iframe 内仍跑 standalone<br>**B3** Tauri webview(仅 Mac/Win/Linux 三平台启用) | **已批 B1** — 零新增依赖,代码复用最大;B2 同源 / X-Frame-Options 风险大;B3 受限于 Tauri 平台 |
| **S6-C** | 兜底策略(嵌入失败时怎么办) | ✅ **C1** 嵌入失败时降级为 `_blank`,无感知<br>**C2** 嵌入失败时显示错误 + 用户手动点 "Open in tab"<br>**C3** 永远不嵌入,只 `_blank`(回到现状) | **已批 C1** — 用户体感最平滑,失败时仍可达;但需识别哪些错降级哪些不降级 |

### §3.2 推荐组合 = A2 / B1 / C1

- A2 控制范围不越界(回写 = postMessage 跨域政策 + 同源限制 + 输入语义风险,先做观察 + 截断)
- B1 把现有 `StandaloneBrowserViewer` 作为 `RightPaneSlot::render` 的入口,改的是"mount 位置",不是"实现"
- C1 自动降级,**保留** `_blank` 路径,行为不退化

### §3.3 已锁定组合

> 本 spec §3.1 已锁定 A2 / B1 / C1；实施仍需用户独立批准。

---

## §4 路径选择 — 为什么推荐 B1

### §4.1 B1 = 复用 standalone 组件

**做法**:
1. 新建 `interfaces/webchat/src/components/right_pane/slots/browser.rs`
2. 组件 `BrowserSlot` 持有 `BrowserWorkspace` context
3. `BrowserSlot::render` 内部直接 mount `<StandaloneBrowserViewer session=… origin=… />`(从 `browser::standalone` 拿)
4. session 来源:复用 `viewer.rs` 已有的 `LaunchSection` grant 流 — grant 拿到的 launch_url 直接喂给 slot,不再走 `open_in_new_tab`

**代码量**:`browser.rs` ~80 行 + `RightPaneSlot` 注册 ~15 行;**不**改 model / launch / session / transport / standalone 任何一行。

**风险**:
- `StandaloneBrowserViewer` 当前依赖 `window.location`(standalone.rs:55-67 `read_location`)。在 slot 上下文里没有真实 `location`,必须改造成接受"人造 LocationParts"(单纯重构,不改语义)。
- `transport::start` 假设 origin 来自 `window.location.origin`(standalone.rs:240)。在 slot 上下文里要传入 Aleph 当前 origin(简单)。
- iframe sandbox 是不需要的 —— slot 是 Aleph 自己 DOM 树,不是第三方嵌入。

### §4.2 B2 = iframe 引用 launch_url

**风险**:
- 同源 OK,跨站 90% 失败(`X-Frame-Options: DENY` / `Content-Security-Policy: frame-ancestors 'none'`)
- 没法用 Aleph 的 cookie / session
- 没法用 Aleph 的 wasm (frame 里的 wasm 是独立 boot)

**结论**:**否决**。iframe 适合"嵌入**可信同源**第三方页",而 launch_url 指向的是 Aleph 自己,直接 mount 组件就够。

### §4.3 B3 = Tauri webview

**真实可达**:
- `tauri::WebviewWindowBuilder` 在 Rust 端开新 webview,内嵌一个 Aleph 子页(`/viewer/browser?embedded=1`)
- 数据流:子页通过 `window.__TAURI__.event.emit` 把 frame URL / 状态推回主进程 → 主进程把 url 转 signal → slot 渲染 `<img>`

**否决理由**:
- 仅 Tauri 平台,网页端用户没法用
- 与 B1 重复(子页还是跑 `StandaloneBrowserViewer`,只是多一层 Tauri bridge)
- 工程量是 B1 的 5-10 倍(Tauri command + IPC + 双向事件 + 关闭清理)
- 留作 v2 增强:**在 Aleph 自己 origin 之外**的页面(非 Aleph 站点截图工具)才有意义

### §4.4 B1 之外的工程量

| 子任务 | B1 | B2 | B3 |
|---|---|---|---|
| 改造 `StandaloneBrowserViewer` 不依赖 `window.location` | 30 行 | 0 | 30 行 + IPC |
| `BrowserSlot` 新组件 | 80 行 | 80 行 + iframe 100 行 | 80 行 + Tauri bridge 200 行 |
| `_blank` 兜底 | 保留 | 保留 | 保留 |
| 跨平台测试 | 全 | 全 | 仅 Tauri |
| **总计**(估) | **~110 行** | **~180 行** | **~510 行** |

---

## §5 状态机 — 复用,仅加"嵌入态"标志

### §5.1 现有 3 个状态机(本 spec 不动)

- `LaunchPhase`: 5 态(Idle / Issuing / Ready / Opened / Blocked / Failed)—— launch.rs:117-134
- `ViewerPhase`: 3 态(AwaitingAuthorization / Live / Closed)—— model.rs
- `SessionPhase`: 6 态(Idle / Consuming / Exchanging / Exchanged / Authenticated / Closed)—— session.rs

### §5.2 新增的"嵌入态"标志(只在 slot 上下文有)

```rust
// BrowserSlotCtx(per-slot, 组件级 state)
pub enum EmbeddingState {
    /// LaunchSection 还没 issue grant
    NoGrant,
    /// Grant 拿到了,session 还没起来
    Pending,
    /// StandaloneBrowserViewer 正在跑
    Running { was_live: bool },
    /// 嵌入失败,降级为 _blank
    FallbackToTab { reason: FallbackReason },
}

pub enum FallbackReason {
    /// session 解析失败
    InvalidFragment,
    /// 组件 mount 失败(?)
    MountError,
    /// 用户手动点 "Open in tab"
    UserRequested,
}
```

### §5.3 状态转移表

| 触发 | 前态 | 后态 | 备注 |
|---|---|---|---|
| `LaunchSection::issue` 成功 | NoGrant | Pending | 等 ViewerSession::from_location 解析 |
| session 解析成功 | Pending | Running | TransportUi.failed = false |
| session 解析失败 | Pending | FallbackToTab(InvalidFragment) | 保留 grant,自动 `_blank` |
| transport 失败 | Running | FallbackToTab(InvalidFragment) | 同上,`ui.failed` 已 true |
| 用户点 "Open in tab" 按钮 | 任意 | FallbackToTab(UserRequested) | `_blank` + 状态保持 |
| resource / binding / caller 变 | 任意 | NoGrant | `LaunchSection::Effect` 已守,本 spec 不重复 |

### §5.4 嵌入态的`__redacted__`策略(占位 — 实施阶段确定)

> ⚠️ **本 spec 不决定 iframe 引入与否**。B1 路径下**不需要** iframe(slot 是 Aleph 自己 DOM)。B2 / B3 才需要。这一段在 spec 评审时折叠为"占位"。

---

## §6 验收标准

### §6.1 视觉(阶段 6 实施时跑)

- VS-1:右栏 Browser tab 出现时,`StandaloneBrowserViewer` 状态壳**完整**渲染(6 个 StandaloneNotice 全部测)
- VS-2:Live 时 `<img>` 占满 slot 区(无 padding / 无重叠 chat 列)
- VS-3:FallbackToTab 时 slot 显示"Open in tab"按钮 + 错误 reason 文字
- VS-4:Tauri 平台用户默认走 native new-tab(若 host platform 探测保留) —— **本期可能不实施,留 v2**

### §6.2 状态机

- UT-1:`StandaloneBrowserViewer` 不依赖 `window.location`(改造后) 测
- UT-2:`BrowserSlot` 在 grant 失败 / 解析失败 / transport 失败 / 用户 fallback 4 路径全覆盖
- UT-3:`LaunchSection::Effect` 3 件盯防(resource / binding / caller) 在 slot 上下文依然工作
- UT-4:`fallback_to_tab` 后 slot 不再 attempt 重连(防抖)

### §6.3 决策保留(本 spec 写死)

- `_blank` 必须**保留**为兜底,不能因为引入 slot 而删
- "Open in tab" 按钮必须**始终可见**(不是仅在 fallback 时),给 Tauri 平台 / 大屏用户选择
- 信任边界:slot 不写回 `BrowserWorkspace` model
- iframe 仅在选 B2 时引入,本 spec 默认 B1 → **不引入**新 iframe
- 源级 census:在 `shape_view.rs:1073-1110` 加一条"slot 组件无第二处 iframe"约束,防 iframe 蔓延

### §6.4 性能预算(本阶段不测量,只列位)

- slot 切换到 Browser tab 的 mount 时间 < 200ms(与 ChatGPT 同 tab 切换对齐)
- session WebSocket 重连 backoff < 5s(沿用 transport.rs 既有,本 spec 不动)
- 内存:slot 不持有 frame URL 字符串以外的 state(frame 是 `<img src=blob:...>`,浏览器 GC)

---

## §7 风险与红线

### §7.1 红线复述(R1–R10)

- **R1** Core 不调平台 API:浏览器 slot 在 webchat,不在 core;不动 `src/capability/` 任何代码
- **R2** UI 逻辑唯一源:slot 渲染体在 `interfaces/webchat/src/components/right_pane/slots/browser.rs`,**不**写到 `src/bin/...`
- **R3** 核心轻量化:不引入 `tauri`、`wry` 等 webview 库
- **R4** Interface 层禁止业务逻辑:`browser::viewer / launch / session / transport` 已是 Interface 层(纯渲染 + 状态机),本 spec 不增加新业务
- **R5** AI 主动到达:本 spec 不触及通知路径
- **R6** 一核多端:本 spec 仅 webchat;TUI 不做右列(阶段 7 独立审批)
- **R7** LLM 主权:不调 LLM
- **R8** 工具即一切:本阶段不暴露新工具
- **R9** 智慧在 Prompt 中:不涉及 prompt
- **R10** 薄 Harness,笨循环:本 spec **不**改任何 harness 代码,不改 `src/harness/`,不加新文件到 harness

### §7.2 阶段 6 专项风险

| 风险 | 触发 | 缓解 |
|---|---|---|
| **iframe 蔓延** | 实施者手快在 slot 里加 `<iframe src=launch_url>` | B1 不引入 iframe;B2 必须改 shape_view.rs census 测块 |
| **session 寿命失控** | slot mount 后 LaunchSection::Effect 失效 | 显式 keep-alive:在 slot context 也调 `take_unopened` 监听 |
| **`_blank` 退化** | 实施者把 viewer.rs:36-47 `open_in_new_tab` 删了 | 单元测试守护:`test_open_in_new_tab_is_preserved` |
| **WebSocket 重连风暴** | transport 失败 → 立即重试 → 节流失败 | 沿用 transport.rs 既有 backoff;不引入新策略 |
| **grant 泄漏** | slot 关闭未 revoke | `on_cleanup` 必须 revoke —— 与 LaunchSection:on_cleanup(viewer.rs:454-466)一致 |
| **Aleph 跨会话持久** | session origin 错(用旧 conversation 的 origin) | `StandaloneBrowserViewer` 接受 `origin: String` prop,host 强制传当前 origin |
| **CSP 冲突** | 子页 CSP 阻止 WebSocket | 已知,沿用 standalone 既有 CSP,本 spec 不改 |

### §7.3 不做清单(本阶段明确不实施)

- ❌ **Tauri webview 嵌入**(B3):留 v2
- ❌ **postMessage 回写** (A2 之外):留 v2
- ❌ **iframe 引用 launch_url** (B2):本期否决
- ❌ **跨 conversation 共享 session**:每个 conversation 独立 session,与 model 现有 scope 一致
- ❌ **多 tab 浏览器**:本 spec 永远是 1 session = 1 slot
- ❌ **地址栏 / 前进 / 后退**:Aleph 不当浏览器
- ❌ **TUI 端右列**:阶段 7 独立审批

---

## §8 阶段路线(独立 ship)

### §8.1 阶段 6 子阶段(每段独立可 ship)

| 段 | 动作 | ship 门槛 |
|---|---|---|
| **6.0 spec** | 本 spec 落地 | 用户批 S6-A/B/C |
| **6.1 准备** | `StandaloneBrowserViewer` 接受人造 LocationParts(30 行改动,纯函数) | UT 全绿 + 不依赖 `window.location` |
| **6.2 slot 壳** | `BrowserSlot` 组件 + `RightPaneSlot` 注册 5 行 | slot 渲染体 mount OK,状态壳 6 态全覆盖 |
| **6.3 session 注入** | 复用 `LaunchSection`,grant 拿到的 launch_url 直接喂 slot | session 在 slot 内跑通,Live 时 `<img>` 显示 |
| **6.4 兜底** | FallbackToTab 状态 + "Open in tab" 按钮 | 4 条 fallback 路径 UT 全绿 |
| **6.5 守护** | shape_view.rs census 测块加"slot 无第二处 iframe" + `open_in_new_tab` 留存 UT | 测块红 → 实施者手快 |

### §8.2 与其他阶段关系

- 阶段 1 实施前:**不**做阶段 6(没有 `RightPaneSlot` trait 就挂不上去)
- 阶段 2 实施后:右栏容器是 flex 第三列,Browser tab 高度由 clamp 决定
- 阶段 6 与 阶段 3-5 **完全独立**可并行 ship

### §8.3 不做的事

- 实施阶段:**不**碰 model.rs / launch.rs / session.rs / transport.rs / bridge.rs(它们的逻辑都已经是正确的)
- 实施阶段:**不**改 `BrowserWorkspace` model 的任何字段
- 实施阶段:**不**改 `LaunchPhase` 状态机
- 实施阶段:**不**改 `StandaloneBrowserViewer` 渲染壳(只改 LocationParts 来源)

---

## §9 file:line 速查表(本 spec 涉及的全部代码点)

### §9.1 已读 7 个文件

| 文件 | 行 | 角色 |
|---|---|---|
| `interfaces/webchat/src/platform/wide/views/browser/mod.rs` | 51 | 公共 API 地图,7 个子模块 + 19 个导出 |
| `interfaces/webchat/src/platform/wide/views/browser/viewer.rs` | 976 | 渲染壳,3 态 + 5 LaunchSection phase + PairingNotice |
| `interfaces/webchat/src/platform/wide/views/browser/launch.rs` | 559 | Launch 状态机,6 导出,IssuedLaunch::Debug redact |
| `interfaces/webchat/src/platform/wide/views/browser/standalone.rs` | 481 | `/viewer/browser` 路由,6 StandaloneNotice,session hand-off scrub |
| `interfaces/webchat/src/platform/wide/views/browser/session.rs` | 1332 | ViewerSession,WebSocket 帧,parse_fragment |
| `interfaces/webchat/src/platform/wide/views/browser/transport.rs` | 726 | TransportUi,start(session, ui, origin) |
| `interfaces/webchat/src/platform/wide/views/browser/bridge.rs` | 569 | decode_browser_event,apply_browser_event |
| `interfaces/webchat/src/platform/wide/views/browser/model.rs` | 1354 | BrowserViewerModel,BoundBrowser,reducer(未通读) |
| `interfaces/webchat/src/platform_host.rs` | 116 | HostPlatform{MacOs, Windows, Linux},baseline-probe.js 来源 |
| `interfaces/webchat/src/platform/wide/views/canvas/shape_view.rs:892` | — | 唯一 iframe 生产位,`<iframe sandbox="allow-scripts">` |
| `interfaces/webchat/src/i18n_census.rs:1556` | — | 注释"three sandboxed pin" |

### §9.2 关键代码点(本 spec 引用)

| 点 | 行 | 含义 |
|---|---|---|
| `BrowserWorkspace` 导出 | `mod.rs:35` | 上下文,所有 slot 拿 |
| `open_in_new_tab` 定义 | `viewer.rs:36-47` | **本 spec 必保留** |
| `LaunchSection` mount | `viewer.rs:387-647` | 5 phase 状态机 |
| `BoundSummary` | `viewer.rs:680-790` | 3 态绑定 + candidate 列表 |
| `BrowserViewer` 入口 | `viewer.rs:774-800` | `model: BrowserViewerModel, standalone: bool` |
| `StandaloneBrowserViewerRoute` | `standalone.rs:191-213` | 路由入口 |
| `StandaloneBrowserViewer` mount | `standalone.rs:225-247` | 复用入口 |
| `StandaloneShell` 渲染 | `standalone.rs:250-282` | 6 态 + `<img data-testid="browser-live-frame">` |
| `LocationParts` 结构 | `standalone.rs:30-50` | `pathname / origin / hash(redacted)` |
| `begin_standalone_session` | `standalone.rs:99-114` | scrub → 解析 in-memory copy |
| `ViewerSession::from_location` | `session.rs` | 解析 fragment → session |
| `transport::start` | `transport.rs` | 真实 WebSocket 入口 |
| `LaunchPhase` 枚举 | `launch.rs:117-134` | 5 态 |
| `IssuedLaunch` Debug | `launch.rs:101-110` | redact launch_url |
| `validate_launch_url` | `launch.rs:88-99` | 4 步校验 |
| `HostPlatform` 探测 | `platform_host.rs:15-37` | `data-platform` attribute → enum |
| iframe 唯一生产位 | `shape_view.rs:892` | 源级 census 守护在 1073-1110 |

### §9.3 阶段 6 实施时要新增的文件

```
interfaces/webchat/src/components/right_pane/slots/
├── browser.rs                  # ~80 行,新
```

**不新增**:
- ❌ 不新增 mod / launch / session / transport / bridge / viewer / standalone 任何文件
- ❌ 不新增 harness 文件
- ❌ 不新增 tailwind / 样式文件
- ❌ 不新增 doc(本 spec 是 1 份 spec,够)

### §9.4 阶段 6 实施时要改的文件

| 文件 | 改什么 | 行数 |
|---|---|---|
| `interfaces/webchat/src/platform/wide/views/browser/standalone.rs` | `StandaloneBrowserViewer` 接受 `LocationParts` prop,不再调 `read_location` | ~30 行 |
| `interfaces/webchat/src/components/right_pane/mod.rs` | 注册 `BrowserSlot` | ~5 行 |
| `interfaces/webchat/src/platform/wide/views/canvas/shape_view.rs` | census 测块加"slot 组件无第二处 iframe"断言 | ~10 行 |
| `interfaces/webchat/src/platform/wide/views/browser/viewer.rs` | 加 `test_open_in_new_tab_is_preserved` 测块 | ~15 行 |

总计 ~60 行,**符合 R10 薄 Harness 精神**。

---

## §10 决策日志(本 spec)

### §10.1 S6 决策

| 决策 | 含义 | 状态 | 备注 |
|---|---|---|---|
| S6-A | 受控范围(三轴) | ✅ **已批 A2** | 观察 + 截断,回写留 v2 | 本 spec §3.1 / m00685 |
| S6-B | 嵌入路径 | ✅ **已批 B1** | 复用 standalone | 本 spec §3.1 / m00685 |
| S6-C | 兜底策略 | ✅ **已批 C1** | 失败降级 `_blank` | 本 spec §3.1 / m00685 |

### §10.2 继承自上游 spec 的已批决策(不重复拍板)

- 3.1.1 = A(flex 第三列)
- 3.1.2 = trait(可注册)
- 3.1.3 = subagent 默认 Tab,capability 次级,tray 次级
- S2-A/B/C = 阶段 2 容器过渡 + ChatOnly 分支 + tool API 直读
- S3-A/B/C = subagent 默认 Tab / 分 2 slot / 内联 drawer
- S4-A/B/C = capability 次级展开 / diagnostic 默认折叠 / 内联 drawer
- S5-A/B/C = chip 升级 icon+缩略图 / tray user-pinned / LRU 双约束
- S6-A/B/C = 观察+截断 / 复用 standalone / 嵌入失败降级 _blank(本 spec,2026-10-15 拍板)

### §10.3 自有决策(实施者不可改)

- ❌ 不引入新 iframe(B1 路径下)
- ❌ 不动 `BrowserWorkspace` model
- ❌ 不删 `open_in_new_tab`
- ❌ 不引入 Tauri webview(B3 留 v2)
- ❌ 不写 postMessage 回写(A2 之外留 v2)
- ❌ TUI 端不实施右列(阶段 7 独立审批)

---

## §11 引用

- **上游 spec**:
  - [阶段 0 总览(2026-10-15-rightside-optimization-plan.md)](2026-10-15-rightside-optimization-plan.md)
  - [阶段 1 Slot 接口(2026-10-15-rightpane-stage1-slot-spec.md)](2026-10-15-rightpane-stage1-slot-spec.md)
  - [阶段 2 宿主切换(2026-10-15-rightpane-stage2-host-switching-spec.md)](2026-10-15-rightpane-stage2-host-switching-spec.md)
  - [阶段 3 子代理 slot(2026-10-15-rightpane-stage3-subagent-slot-spec.md)](2026-10-15-rightpane-stage3-subagent-slot-spec.md)
  - [阶段 4 能力 slot(2026-10-15-rightpane-stage4-capabilities-slot-spec.md)](2026-10-15-rightpane-stage4-capabilities-slot-spec.md)
  - [阶段 5 附件 slot(2026-10-15-rightpane-stage5-attachments-slot-spec.md)](2026-10-15-rightpane-stage5-attachments-slot-spec.md)
- **参考依据**:
  - `interfaces/webchat/src/platform/wide/views/browser/` 7 文件(§9.1 全表)
  - `interfaces/webchat/src/platform_host.rs:15-37` —— 平台探测
  - `interfaces/webchat/src/platform/wide/views/canvas/shape_view.rs:892` —— iframe 唯一先例
  - `interfaces/webchat/src/i18n_census.rs:1556` —— "three sandboxed pin" 注释
- **下游**:
  - 阶段 7 TUI 右列(独立审批,本 spec 不写)

---

*Last updated: 2026-10-15 · 阶段 6 spec v1.1 · S6-A=A2 / S6-B=B1 / S6-C=C1 已批*

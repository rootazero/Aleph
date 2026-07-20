# Aleph iOS 实现计划 (iOS Implementation Plan)

> Stack 决策 = **Hybrid (B)**: 复用现有 Leptos/WASM Panel，装进 Tauri 2 iOS 的 WKWebView 壳里，只为 WebView 做不到的能力（APNs 后台推送 / 原生音频 / haptics / share / safe-area）补一层薄原生桥。
> 全 Swift 方案已否决 —— 它会引入第二 UI 源（违 R2），并带来永久同步税。
>
> 关联红线: **R1**（大脑与四肢分离，平台 API 必须走原生桥 IPC）· **R2**（UI 唯一源，所有业务 UI 在 WASM Panel）· **R5**（AI 主动到达 = 推送）· **R6**（一核多端）。
>
> 设计输入: 6 个 MVP 屏（Chat / Memory / Agents / Settings / Voice / Notifications-Approvals）已在 claude.ai/design 设计并批准。组件库见 `docs/design-system/aleph-mobile/`（aleph.css + foundations + components），设计简报见 `docs/superpowers/specs/2026-06-25-aleph-panel-ios-mobile-design-brief.md`，批准截图归档于 `docs/design-system/aleph-mobile/screens/`。

---

## 1. 目标与范围 (Goal & Scope)

**交付物**: 一个 iOS 版 Aleph Panel —— 远程 core 的**瘦客户端**（thin client），结构上对齐桌面的 **lite 变体**（`ai.aleph.panel`，`--no-default-features` 构建：零本地 daemon、LAN 发现 + 远程连接 RPC、远程 server 主动推送而非客户端轮询）。证据: `desktop/shell/src/main.rs:17-22, 447-475`（panel-only shell 仅在 `--no-default-features` 下编译 `mdns-sd`，`bring_target_online()` 用 probe-gated navigation + `connect_setup` 模块，无 daemon 监管）。

**MVP 范围 = 6 个已设计屏**:
1. Chat
2. Memory
3. Agents
4. Settings
5. Voice
6. Notifications / Approvals

**明确不在范围内 (out of scope)**:
- 在 iOS 上内嵌 `aleph-server` daemon。iOS 完整版 `ai.aleph.desktop` 是桌面专属设计 —— 它内嵌 `aleph-server` 作为受监管本地 daemon（`tauri.conf.json:21` — `externalBin: ["binaries/aleph-server"]`），与 iOS 沙盒不兼容。
- macOS 专属能力（vibrancy、overlay title-bar、traffic lights、菜单栏、TCC 权限监控），iOS 无等价物。证据: `desktop/shell/src/main.rs:185-188, 322-328, 935-949`（这些都在 `#[cfg(target_os="macos")]` 下）。

**Bundle identifier**: 复用 `ai.aleph.panel`（已是反向域名格式，iOS 兼容）。证据: `desktop/shell/tauri.lite.conf.json:4 — "identifier": "ai.aleph.panel"`。

### 1.1 已确认决策 (User decisions, 2026-06-25)

- **分发路径 = 先 TestFlight 再上架**: v1 目标 = TestFlight 内测跑通核心体验（P1 + P2 + P3-dictation），公开 App Store 上架是后续步骤。→ **App Store 4.2 只在「冲公开上架」那一步才真正卡**;TestFlight 阶段不被 4.2 拦,但仍受 §8.7 的 beta-toolchain / release-Xcode 约束。
- **分期顺序 = 确认 risk-ordered**（P0 → P1 → P2 → P3 → P4），无调整。
- **Voice v1 = dictation-only**: 长按说话 → 转写入框;**immersive 全屏 VAD 沉浸态延后到 v2**（WKWebView Web Audio 风险最高,见 §6 / §8.3）。Phase 3 据此收窄。

---

## 2. 前置条件 (Prerequisites)

| 项 | 说明 |
|----|------|
| **macOS** | iOS 构建必须在 macOS 上。当前主开发机 = **macOS 27 beta**。 |
| **Xcode** | 完整 Xcode（非仅 CLT），含 iOS SDK + Simulator。macOS 27 beta 须配 **Xcode 27 beta**（+ iOS 27 beta SDK）。⚠️ beta 工具链:Tauri/CocoaPods/wry 可能有未适配点(见 §8.7);且**公开上架 App Store 通常要求 release 版 Xcode/SDK 构建** —— beta-SDK 包一般只接受进 TestFlight 内测,正式提审前可能需切 release Xcode 27。 |
| **CocoaPods** | Tauri iOS 工程依赖 CocoaPods 管理原生依赖。 |
| **Apple Developer account** | 真机 + TestFlight + APNs（Phase 4）必需。Simulator 不需要账号，但**真机调试必须签名**。 |
| **签名配置 (`developmentTeam`)** | Tauri iOS 需在配置里声明 Apple Team ID。**这是一个待补充的 iOS-only 配置** —— 当前 `tauri.conf.json` 完全没有 iOS 节点（验证事实: `tauri.conf.json` 无 iOS bundle / 无 iOS 插件；`Cargo.toml:44-54` 仅有 macOS/Windows/Linux 的 `[target.'cfg(...)']` 段，零 iOS 构建）。 |
| **APNs 凭证（仅 Phase 4）** | Apple Developer 后台生成 `.p8` token-auth key + Key ID + Team ID，用于服务端 APNs HTTP/2 JWT 签名。 |

> ⚠️ 现状基线: **iOS target 在整个 shell crate 中尚未准备**。验证事实: `grep -r 'target_os.*ios|ios'` 在 `desktop/shell` 零命中。Tauri 配置只声明 `targets: "all"`，其展开**仅桌面**（`tauri.conf.json:20`）。

---

## 3. Phase 0 — Secure-context / getUserMedia 去风险探针 (~1 天)

> **目的**: 在投入壳工程之前，先验证两个会决定后续方向的硬不确定性 —— (a) iOS WKWebView 里 Panel 是否拿得到 secure context；(b) voice 是否必须走原生音频桥。这是整个计划里**风险最高的单点**，必须最先打掉。

### 步骤

1. 在 **lite 变体**上跑 `cargo tauri ios init`（`--no-default-features` 路径），生成 iOS Xcode 工程脚手架。
   - ✅ 已确认（高置信）: **Tauri 2 自 2.0 GA（2024-10）起官方支持 iOS** —— `cargo tauri ios init/dev/build` 与 wry 的 WKWebView 抽象均为正式能力。先前一份探查误报「Tauri v2 不支持 iOS」，已订正。**真正的不确定性不是「Tauri 能否上 iOS」，而是「我们这个 shell 的插件集 + 配置能否干净地 init/build 出 iOS 工程」**（桌面专属插件需先 gating，见 §4.1；个别第三方插件可能缺 iOS 绑定）。Phase 0 第一任务即用 `cargo tauri ios init` 实测我们的壳（见 §8.2）。
2. 在 **Simulator** 与 **真机**上加载 Panel。
3. 在 WebView 控制台检查 `window.isSecureContext`。
4. 用 `wss://` 连一台**真实 `aleph-server`**（局域网或远程），跑通 handshake + 一轮 chat。
5. 观察 `getUserMedia` 行为（点 Voice 屏的 mic 入口）。

### 关键背景（来自验证事实）

- **getUserMedia / AudioContext 需要 secure context（https 或 localhost）**，非 secure context 的 WebView 拿不到。证据: `interfaces/webchat/src/views/voice/audio.rs:92-95, 154-156, 180-185`（`MicError::Unsupported` 文档写明「non-secure context or a webview that doesn't expose the API」，错误提示「needs safe context: https or localhost」）。
- **自定义 scheme（`aleph://`）在 WKWebView 里对 Web Audio API 被当作非 secure context**。证据: `interfaces/webchat/src/views/voice/audio.rs:92-94`。
- macOS WKWebView 已知 getUserMedia 因 GPU 进程音频授权失败（ad-hoc 签名不足）→ 已改走原生桥。证据: `docs/handoff-voice-loop.md:30-33`。iOS 极可能复现同类限制。

### 决策门 (Decision Gate)

| 观测结果 | 决策 |
|---------|------|
| `window.isSecureContext === true` **且** getUserMedia 弹权限并能录 | Voice 可走 Web API，**Phase 3（原生音频桥）可降级/跳过**沉浸式部分仍需复核（见下） |
| `isSecureContext === false` **或** getUserMedia 报 `MicError::Unsupported` | **Phase 3 原生音频桥为必需项**，对齐已验证、已用户测试的 macOS Swift AVFoundation 路径（`docs/handoff-voice-loop.md:33-35`） |

> 即便 secure context 成立，**沉浸式 voice 模式**（全屏 VAD 循环）依赖 `ScriptProcessorNode` 连续 PCM tap（证据: `audio.rs:1-11`），且 `ScriptProcessorNode` 在现代 WebKit 已 deprecated（见 §8 风险）—— 这条 Phase 0 必须单独勾验，结论可能是「dictation 走 Web、immersive 走原生桥」的混合。

### Phase 0 验收

- [ ] `cargo tauri ios init` 在 lite 变体上成功产出 Xcode 工程（或明确得出「Tauri iOS 不可用，需 fork/自研 webview 桥」的裁决）。
- [ ] Panel 在 Simulator 加载并渲染（⚠️ **当前 panel 是桌面两栏布局,移动端 6 屏响应式 UI 尚未实现 —— 见 §10.1 新增 Phase 0.5**）。Phase 0 只需确认 WKWebView 能加载 WASM panel（桌面布局即可），**不要求** 6 屏移动布局。无 macOS 专属样式阻断（`data-platform=macos` 在 iOS 不命中，证据 `main.rs:90-94`）。
- [ ] 控制台明确记录 `window.isSecureContext` 的值。
- [ ] `wss://` 到真实 server 的 handshake 成功（token 流程见 Phase 1）。
- [ ] 一份书面决策: 「Voice 是否需要原生音频桥」+ 「dictation/immersive 各走哪条路」。

---

## 4. Phase 1 — Lite iOS 壳 (Lite iOS Shell)

> **目标交付**: Panel 经 TestFlight 在真机运行，连远程 core，**暂无推送**。

### 4.1 桌面专属能力 gating 清单（来自审计事实，必须在 iOS 构建前 cfg-gate）

桌面 shell 当前**无条件注册**多个仅桌面插件 —— 这些在 iOS 上会**编译/链接失败**。每一项必须 `#[cfg(not(target_os = "ios"))]` 或移入 `[target.'cfg(not(target_os = "ios"))'.dependencies]`：

| 能力 | 当前位置（无 guard） | iOS 后果 | 处置 |
|------|---------------------|---------|------|
| System Tray | `main.rs:207 tray::build(&handle)?;`（`tray.rs` 无条件编译） | CRITICAL: Tauri tray 桌面专属，构建/链接失败 | cfg-gate 调用 + 模块 |
| single-instance | `main.rs:112-123`（`Cargo.toml:27` 无条件） | CRITICAL: 窗口管理概念，init 失败 | cfg-gate 插件链 + 依赖 |
| global-shortcut | `main.rs:140` + `hotkey::setup` @ `main.rs:214`（`main.rs:27 mod hotkey;` 无条件） | CRITICAL: iOS 无系统级热键 API | cfg-gate 模块 + setup 调用 + 依赖 |
| autostart | `main.rs:125-128` + `ensure_autostart` @ `main.rs:211`（`Cargo.toml:26` 无条件） | CRITICAL: iOS 无 autostart 注册表 | cfg-gate 插件 + 调用 + 依赖 |
| daemon supervision | `daemon.rs` / `perm_monitor.rs` | 已被 `#[cfg(feature="embedded-core")]` 隔离 | iOS 用 `--no-default-features`，天然排除 |

**已正确隔离、iOS 无需处理**:
- macOS vibrancy: `main.rs:217-218` 已 `#[cfg(target_os="macos")]`，`window-vibrancy` 在 `Cargo.toml:44-45` 已 target-gated。
- macOS menu: `main.rs:28-29` + 注册 `main.rs:185-188` 均已 gated。
- RevealGate（首窗显示）: `main.rs:73-75` 跨平台，iOS 适用。

> 依赖层处置（HIGH 风险）: 四个仅桌面插件当前在 `Cargo.toml:26-31` 是无条件依赖，iOS 构建会尝试拉取/编译不存在的原生绑定。必须移入 `[target.'cfg(not(target_os = "ios"))'.dependencies]`。

### 4.2 justfile iOS targets（新增）

参照现有桥接链（`just build` = `swift-bridge` → `wasm` → `cargo build`，证据 `justfile:46-48`）与 `wasm` target（`justfile:144-193`），新增三个 iOS target：

- `ios-init` — 一次性: `cargo tauri ios init`（lite 变体）。
- `ios-dev` — `just wasm`（共享，不变）→ `cargo tauri ios dev`（Simulator/真机热跑）。
- `ios-build` — `just wasm` → `cargo tauri ios build`（产 `.ipa`/`.app`）。

> ⚠️ **rust_embed stale-embed footgun**（已在记忆/事实中多次踩中）: Panel WASM 经 `assets.rs #[folder]` 在 **编译期**静态嵌入二进制（证据: `gateway/http_server.rs` / `assets.rs`）。但 iOS lite 壳是**瘦客户端连远程 server**，Panel 由**远程 server serve**，不走本地嵌入 —— 这反而规避了本地 stale-embed 问题。**但**若 iOS 壳走 App-protocol 内嵌资源路径（`WebviewUrl::App`，对照 `main.rs:286`），则改完 Panel 必须先 `just wasm` 再重打 iOS 包，否则带旧 UI。Phase 1 默认走远程 serve（lite 语义），明确记录此选择。

### 4.3 Info.plist + ATS（ws/wss 传输）

iOS WKWebView 继承 App Transport Security。桌面 macOS Info.plist 已禁 ATS 以允许明文 LAN（证据: `desktop/shell/Info.plist:11-23` — `NSAppTransportSecurity` 含 `NSAllowsArbitraryLoads=true`，注释说明 loopback 豁免完整版、remote-HTTP panel 需此项）。iOS 必须做等价处理：

- **`ws://` 到 LAN IP 默认被 iOS ATS 阻断**（RFC1918 私网 + 非 TLS）。证据: WKWebView 继承 Info.plist ATS；明文 `ws://` 到私网默认被拦。
- 处置（择一，按安全权衡）:
  - (A) `NSAllowsArbitraryLoads=true`（最宽，削弱全 App 网络安全）—— 对齐桌面现状，最快。
  - (B) 域名作用域 `NSExceptionDomains` + 每域 `NSExceptionAllowsInsecureHTTPLoads` —— 更紧，但需在构建期硬编 LAN IP/host，对 DHCP 家庭网络脆弱。
- **`.local`（mDNS）默认被 iOS 16+ 强制 https** —— 若走 mDNS 发现，WKWebView 可能强制 `https://` 断掉 `ws://`。缓解: 用 IP 字面量，或自签证书 + 本地 CA pin，或 `NSBonjourServiceTypes` + `NSLocalNetworkUsageDescription`（单独不够，仍需 ATS 例外）。
- **`NSMicrophoneUsageDescription`** 必须声明（Voice 屏；对照 macOS `desktop/shell/Info.plist` + Entitlements）。

### 4.4 在 TestFlight 之前 LOCK webview scheme

> **关键约束**: Panel 把 Gateway token 存在 localStorage（key `aleph_gateway_token`，证据: `interfaces/webchat/src/context.rs:48, 81-85`）。**localStorage 按 origin（scheme+host+port）隔离**。WKWebView 对每个 scheme 独立 localStorage —— **scheme 一变，token 被清空**。

- WS URL 由 `location.protocol` + `location.host` 同源推导（`ws://` ↔ http、`wss://` ↔ https；证据: `context.rs:228-244, 232-235`）。
- **必须在首次 TestFlight 发布前固定 webview scheme**，否则后续切 scheme → localStorage 重置 → 用户每次都要重输 token。
- scheme-lock 攻击场景（验证事实 §risks）: 攻击者诱导用户开 `http://LAN-IP:18790` 登录 → 强制跳 https → 新 origin localStorage 空 → 重新提示。缓解: iOS 壳把 gateway pin 到**单一固定 scheme**，且 panel 加载 scheme 与 gateway 协议一致。

> Token 细节（用于实现连接屏）: token 格式 `aleph-<UUID>`，服务端 `SharedTokenManager` 生成（`gateway/security/shared_token.rs:98-109`）；URL `?token=` 优先于 localStorage，连上后从地址栏/历史 scrub（`context.rs:70-77, 110-131`）；被拒/轮换时清空（`context.rs:144-149, 564`）。QR/链接传 token 仅适合一次性，不适合长期委托（见 §8）。

### 4.5 连接 RPC 复刻

桌面连接 RPC（`set_connection_target` / `get_connection_target` / `is_lite_shell`）当前仅由 Panel webview 经 Tauri IPC 调用。iOS 若沿用 Tauri，IPC 由 Tauri 处理；若自研壳，则需原生方法分发复刻这些命令。连接形态由构建决定（lite = 恒远程），Panel 一律由 `location.host` 判定连接形态。

### 4.6 Phase 1 验收

- [ ] iOS lite 壳在真机经 **TestFlight** 安装并启动。
- [ ] 6 屏全部可渲染、可导航（底部 TabBar: Chat/Memory/Agents/Settings，Voice 与 Notifications 为入口）。
- [ ] `wss://`（或经 ATS 例外的 `ws://`）连远程 core 成功，handshake 拿到 role。
- [ ] token 持久化跨冷启动有效（验证 scheme 已锁）。
- [ ] 一轮端到端 chat（发消息 → 收 core 流式回复）。
- [ ] iOS 构建无 tray/single-instance/global-shortcut/autostart 链接错误（gating 清单全绿）。
- [ ] **暂无推送**（Notifications 屏仅展示 WS 实时事件，不依赖 APNs）。

---

## 5. Phase 2 — 易实现原生能力 (Easy Native Capabilities)

> 这些是低风险、用官方/社区插件即可补齐的「iOS 打磨」能力，也是 App Store 4.2 缓解的一部分（见 §8）。

| 能力 | 实现 | 说明 |
|------|------|------|
| **Haptics** | Tauri 官方 haptics 插件 | 触感反馈（发送、长按、approve）。轻量，优先。 |
| **Share sheet** | 社区 share 插件 | 分享会话片段 / Memory 条目。 |
| **Safe-area** | CSS `env(safe-area-inset-*)` + `viewport-fit=cover` + 小原生 inset shim | ⚠️ 移动响应式布局由 **§10.1 Phase 0.5** 提供（当前 panel 为桌面两栏,**非响应式**）。`data-platform=macos` guard 在 iOS 不命中无需移除（证据 `main.rs:90-94`）。原生侧仅在 WebView 注入 `data-platform=ios`，把 safe-area 数值喂给 CSS 变量。 |

> R1/R2 合规: 这些能力的原生部分仅做系统 API 调用 + 经 IPC 把结果喂 WebView，**业务 UI 仍全在 WASM Panel**。

### Phase 2 验收

- [ ] 关键交互（发送 / approve / 长按）有 haptic 反馈。
- [ ] Share sheet 能从会话/Memory 唤起系统分享。
- [ ] 刘海/底部 home indicator 区域无内容遮挡（safe-area 生效，刷竖屏 + 横屏）。

---

## 6. Phase 3 — Voice 原生音频桥 (Native Audio Bridge)

> **仅当 Phase 0 决策门判定「需要」时执行。**

### 背景（验证事实）

- 原生音频桥（Swift AVFoundation recorder）是 macOS 已测试、已验证的 voice 路径。证据: `docs/handoff-voice-loop.md:33-35`（「AVFoundation audio capture via Swift bridge works; already user-tested with agent media tool record_audio」）。
- Panel 已有双后端架构: native（macOS，Swift 桥）+ browser（Win/Linux，Web MediaRecorder），先试 native RPC，遇 `NATIVE_AUDIO_UNAVAILABLE` sentinel 回退 browser。证据: `interfaces/webchat/src/views/chat/composer/voice.rs:9-15`。**iOS 复用此分发框架**: native 路径接 iOS Swift 桥，回退路径在 iOS WKWebView 大概率无声（见风险），故 iOS 应以 native 为主路径。

### 实现

- **AVAudioEngine / AVAudioRecorder Swift 插件**，经 JSON-RPC 把音频喂 web 层，复刻 macOS 的 `RecordStart` / `RecordStop` 桥方法（`voice.rs:9-15` 已定义协议）。
- **Dictation 模式**（长按 mic，450ms timer 区分长按/tap，证据 `voice.rs:385-408`）: 可经 native 桥工作。
- **Immersive 模式**（全屏，连续 `ScriptProcessorNode` PCM tap，证据 `audio.rs:1-11`）: **(v1 延后至 v2 —— 用户决策)** 若 Phase 0 判定 WKWebView 无 secure-context，此模式需 native 桥流式协议（当前 `voice.stream.audio` 硬编 Web API 帧率/格式假设，见 §8）。v1 不交付,Phase 0 仅记录其可行性结论供 v2 参考。
- **TTS 播放**: synthesize RPC 返 base64 音频，iOS 解码 + 经 output graph 作为 buffer source 播放（与 macOS 同；`audio.rs:398-400, 632-645`）。`data:` URL 在 WKWebView 不可靠，用 buffer source / blob。`AudioContext` 须在用户激活手势（mic grant）时 `resume_output()` 解锁自动播放（`audio.rs:572-577`）。
- **权限**: `NSMicrophoneUsageDescription`（Phase 1 已声明）。iOS 权限 grant 流程未在代码中文档化，可能需额外桥 entitlement + UIAlertController 集成（见风险）。

### Phase 3 验收

- [ ] Dictation（长按 mic）经原生桥录音 → 转写 → 入 composer。
- [ ] TTS 回复在 iOS WKWebView 可闻（buffer source 路径，非 `data:` URL）。
- [ ] **(v1 不做)** Immersive 模式按用户决策延后到 v2;v1 仅交付 dictation。Phase 0 仍记录 immersive 可行性结论供 v2 参考。

---

## 7. Phase 4 — APNs 推送 (The Big One)

> **R5（AI 主动到达）的核心落地** + **App Store 4.2 缓解的主力**。当前**完全无 APNs 集成**: `NotificationService` 仅支持通用 HTTP webhook（reqwest），非 APNs HTTP/2。证据: `src/a2a/service/notification.rs:24-25`（`http_client: reqwest::Client`，`send_webhook` 用 POST HTTP），grep 全仓零 `apns`/`APNs` 引用。

### 7.1 客户端插件（iOS 原生侧）

- 无官方 `tauri-plugin-apns`。两条路:
  - (A) fork `Choochmeque/tauri-plugin-notifications`（社区，含 APNs 能力）。
  - (B) 自研 Swift 插件: `UNUserNotificationCenter` 注册 + `aps-environment` entitlement + 抓 APNs device token，经 IPC 把 token + payload 桥给 web 层。
- 现有 `tauri-plugin-notification`（`Cargo.toml:25`）只做系统本地通知，**非 APNs** —— 不可直接复用做远程推送。
- device token 获取后，经 keychain 安全存储（对照 `gateway/security/` 设备配对层），在 `connect` 时或经 RPC 上报 server。

### 7.2 服务端 APNs sender（纯后端 Rust，加进 `aleph-server`）

- 给 `NotificationService` 增加 **APNs provider 路径**: 判断设备是否支持 APNs（iOS vs LAN），分发到 Apple HTTP/2 API（`api.push.apple.com`），或与 webhook 并行做混合投递。
- **APNs HTTP/2 + JWT token-auth**: 用 `.p8` private key 做 JWT 签名（Team ID + Key ID）。`.p8` 必须安全部署（env var / vault），JWT 按请求生成是 CPU-bound，需对失败 device token 做 retry + backoff（用户卸载但 token 仍注册）。
- **per-device 路由层**: 现有 `GatewayEventBus` 用 `tokio::sync::broadcast`（容量 1024）向所有连接 WS 客户端广播（证据: `src/gateway/event_bus.rs:1-126`），**不适合 per-device 推送**。需独立的 device→subscribers 映射层（类比 session presence 追踪），避免广播风暴。

### 7.3 设备生命周期

- device token 失效/轮换需考虑 iOS 配对生命周期（App 卸载、OS 更新、iCloud 备份恢复），不能依赖用户手动操作。

### Phase 4 验收

- [ ] iOS 真机注册 APNs 拿到 device token 并上报 server（落 keychain）。
- [ ] `aleph-server` 经 APNs HTTP/2 + `.p8` JWT 向真机投递一条推送，App 后台/锁屏收到。
- [ ] Notifications/Approvals 屏: 推送点击深链进对应 approval（深链需 iOS Info.plist URL scheme + `application(_:open:)` 处理，对照桌面 `aleph://` deep-link 插件 `Cargo.toml:25-30`）。
- [ ] per-device 路由不向无关设备广播。
- [ ] 失败 token（模拟卸载）触发 backoff，不无限重试。

---

## 8. 风险与 App Store 4.2 (Risks & App Store Guideline 4.2)

### 8.1 App Store 4.2 拒审风险（首要产品风险）

- 「连自己 server 的瘦客户端」是 App Store **4.2（最低功能性）**典型拒审点 —— 纯 WebView 包装易被判为「缺少原生 App 价值」。
- **缓解 = Phase 2/3/4 的原生能力**: push（R5）、native audio voice、share sheet、haptics、iOS 打磨（safe-area / 底部 TabBar / 原生 popover/modal）。这些把 App 从「网页包装」拉成「具备原生集成的客户端」。**这是把这些 Phase 排进 MVP 而非可选的核心理由。**

### 8.2 我们的 shell 在 iOS 上能否干净构建（Phase 0 实测）

- **订正先验**: 一份探查误报「Tauri v2 不支持 iOS」。事实: **Tauri 2 自 2.0 GA（2024-10）官方支持 iOS/Android**，`cargo tauri ios init/dev/build` + wry WKWebView 为正式能力。故「Tauri 能否上 iOS」**不是**开放问题。
- **真正的不确定性（窄而具体）**: (a) 我们 shell 的桌面专属插件（tray / single-instance / global-shortcut / autostart）必须先 cfg-gate，否则 iOS 链接失败（见 §4.1）；(b) 个别第三方插件可能缺 iOS 绑定，需在 init/build 时逐个确认。
- **现状确证**: iOS target 在 shell crate 零准备（`grep` 零命中），`targets: "all"` 仅展开桌面（`tauri.conf.json:20`）—— 即「还没配」，非「配不了」。
- **处置**: Phase 0 第一任务 `cargo tauri ios init` + 一次 `ios build` 实测我们的壳。**低概率 fallback**（仅当某关键插件确无 iOS 支持且无可替代时）= 自研 Swift WKWebView 壳，复刻 lite 壳的: webview window host、后台 WS 监管（→ Gateway）、连接 RPC（`set_connection_target` 等作原生方法分发）、无 daemon、LAN/远程切换。**此 fallback 仍满足 R2**（UI 仍是 WASM Panel）。

### 8.3 Voice / Web Audio

- 自定义 scheme `aleph://` 在 iOS WKWebView **不被 WebKit 视为 secure context**，getUserMedia/AudioContext 会以 `MicError::Unsupported` 失败，且**当前代码无 iOS guard**，会静默 error（`audio.rs:92-94`）。→ 必须 Phase 3 原生桥兜底。
- Immersive voice（VAD/ScriptProcessorNode/streaming）完全依赖 Web Audio，iOS WKWebView 零支持，需完整 native 桥重构才能运行。
- `ScriptProcessorNode` 在现代 WebKit 已 deprecated，iOS 弃用时间线未知，未来可能直接杀死 immersive voice —— 当前代码无 AudioWorklet 迁移路径。
- 回退逻辑假设 Web API 可用；iOS WKWebView 回退后大概率仍无声 → 用户面对静默失败。iOS 应以 native 为主路径。

### 8.4 ATS / 网络

- `ws://` 到 LAN IP 默认被 iOS ATS 阻断（RFC1918 + 非 TLS）。`NSAllowsArbitraryLoads` 削弱全 App 安全；每域例外需硬编 IP，对 DHCP 脆弱。
- Gateway 默认绑 `127.0.0.1`（`gateway/config.rs:186`；`commands/start/helpers.rs:29`），LAN 需显式 `--bind 0.0.0.0` 或 `[gateway] host = "0.0.0.0"`。开放后同子网任意 peer 可尝试连接，仅 token 把关，无 IP allowlist。
- WS 明文传输: token 在 RPC params 明文发送（非 TLS 通道）。同 LAN 攻击者可嗅探 `?token=` / 明文 RPC / 重放旧 token。缓解需 operator 显式开 gateway TLS（`tls_cert`/`tls_key`）—— **当前代码未实现**。

### 8.5 Scheme-lock / token

- WKWebView 按 scheme 隔离 localStorage —— **scheme 一变 token 被清**。必须在 TestFlight 前锁 scheme（见 §4.4）。
- localStorage token 无超时，永久持有至显式登出/轮换。`gateway.token.rotate` 一旦轮换，**同时失效所有远端**，无渐进。
- `aleph_device_id` 随机生成，非硬件绑定；清 App 数据生成新 id 但旧会话仍授权（无 per-device 撤销）。

### 8.6 签名 / 构建

- 真机/TestFlight/APNs 必须 Apple Developer 签名 + `developmentTeam`（当前配置缺失，§2）。
- 深链 `aleph://` 需 iOS Info.plist URL scheme + `application(_:open:options:)` 原生 handler（对照桌面 deep-link 插件）。
- `embedded-core` feature 编译出 daemon 模块；iOS 必须用 `--no-default-features` 或在 Cargo.toml 层条件排除，否则 linker 错误（无 `aleph-server` 二进制可链）。

### 8.7 Beta 工具链（macOS 27 beta / Xcode 27 beta）

- 开发机 = macOS 27 beta，须配 Xcode 27 beta + iOS 27 beta SDK。**beta 工具链放大 Phase 0 的去风险价值** —— spike 同时要验 Tauri CLI / CocoaPods / wry 在 Xcode 27 beta 下能否干净 `ios init` / `build`（beta SDK 常有 API / 签名 / Pod 兼容缺口）。
- **公开上架约束**: Apple 通常要求用 **release 版 Xcode/SDK** 构建 App Store 提审包;beta-SDK 包一般只接受进 **TestFlight 内测**。与「先 TestFlight 再上架」决策兼容 —— v1 内测可用 Xcode 27 beta,正式提审前切 release Xcode 27（待其 GA）。提审前须复核当时 Apple 的 SDK 版本要求。
- 处置: Phase 0 记录 Xcode / CocoaPods / Tauri 各自版本与遇到的 beta 兼容问题;pin 版本,避免 beta 工具链自动更新打断构建。

---

## 9. 验收标准汇总 (Acceptance Criteria — per Phase)

| Phase | 核心验收 |
|-------|---------|
| **0** | `cargo tauri ios init`（lite）产出工程或得出明确裁决；Panel 在 Simulator 渲染 6 屏；记录 `window.isSecureContext`；`wss://` handshake 成功；书面「voice 是否需原生桥 + dictation/immersive 各走哪条」决策。 |
| **1** | TestFlight 真机安装启动；6 屏可渲染可导航；`wss://`/例外 `ws://` 连远程 core 拿到 role；token 跨冷启动持久（scheme 已锁）；一轮端到端 chat；iOS 构建无桌面专属插件链接错误。 |
| **2** | 关键交互有 haptic；share sheet 可唤起；safe-area 生效（竖+横屏无遮挡）。 |
| **3** | Dictation 经原生桥录音→转写→入 composer；TTS 在 WKWebView 可闻（buffer source）；immersive 按 Phase 0 决策工作或标注 dictation-only。 |
| **4** | 真机注册 APNs 拿 token 上报落 keychain；`aleph-server` 经 APNs HTTP/2 + `.p8` JWT 投递推送、后台/锁屏可收；推送深链进对应 approval；per-device 路由不广播；失败 token 触发 backoff。 |

---

## 附: 与 CONTEXT 先验假设的对照

| CONTEXT 先验 | 验证事实裁决 |
|--------------|-------------|
| 复用 lite 变体作 iOS 基座 | ✅ 确认: lite (`ai.aleph.panel`, `--no-default-features`) 结构正确 —— 零 daemon、mdns 发现、连接 RP-only（`main.rs:17-22, 447-475`；`tauri.lite.conf.json:4,7`）。 |
| Tauri 2 iOS WKWebView 壳 | ✅ **成立（订正）**: Tauri 2 自 2.0 GA 官方支持 iOS（先前「不支持」探查为误报）。真正待验的是**我们 shell 的插件/配置能否干净 init/build**（桌面专属插件先 gating，§4.1）→ Phase 0 实测；低概率 fallback = 自研 Swift 壳（仍合 R2）。 |
| 薄原生桥只补 WebView 做不到的 | ✅ 确认: getUserMedia 需 secure context、`aleph://` 非 secure（`audio.rs:92-95`）、macOS 已走原生桥（`handoff-voice-loop.md:30-35`）→ voice 桥几乎必需。 |
| 推送补 R5 | ✅ 确认且**缺口比预期大**: 完全无 APNs，`NotificationService` 仅 webhook（`notification.rs:24-25`），`GatewayEventBus` 广播不适合 per-device（`event_bus.rs`）→ 客户端 + 服务端均需新建。 |

---

## 10. Gap 审查补遗 (Gap-analysis addenda — 4 视角对抗式审查, 2026-06-25)

> 对本计划做了 4 视角对抗式审查(生命周期/可达性 · 离网安全 · 产品/4.2/UX · 完整性),共 31 项 gap。下列为**改变计划形状**的发现 + 已 grep 核实的纠正 + 按 phase 的加固清单。

### 10.1 🔴🔴 最大纠正:移动端响应式 Panel UI 尚未实现(只在设计稿里)→ 新增 Phase 0.5

- **核实证据(本会话 grep)**: `interfaces/webchat/styles/tailwind.css` 仅 `@media (min-width:1600px)` 一条桌面大屏断点,**零移动断点**;`src/components/mode_sidebar.rs:63` 侧栏硬编码 `w-64`;`src/app.rs:155` 固定 `flex h-screen` 两栏 shell(sidebar-collapsed 是桌面折叠,非移动 reflow);`src/views/` **无** notifications/approvals 视图;无移动 TabBar 组件(.rs 命中 2 皆 incidental;55 处 viewport/resize 命中基本来自 WebGL canvas,非布局)。
- **含义**: claude.ai/design 的 6 屏是**设计稿**,不是 WASM 实现。当前 panel 在 390px 下**布局崩坏**(256px 侧栏 + 119px 主区)。计划原先"Panel 已是响应式"假设**错误**(已纠正 §3/§5)。
- **→ 新增 Phase 0.5「移动端响应式 Panel(Leptos)」** —— **纯 Leptos/WASM/CSS,可立即开工,不需要 Mac/Xcode/beta 工具链**(与 P0 并行,甚至先行):
  1. viewport 宽度检测(Leptos signal + resize)→ `<640px` 单列;
  2. **BottomTabBar 组件**(Chat/Memory/Agents/Settings,route-based active)替代移动端左侧栏;
  3. `<640px` 隐藏 `w-64` 侧栏 + 桌面 7 模式切换器降级;
  4. **新建 `views/notifications.rs`**:pending/resolved 审批列表 + `approval.respond(id,decision)` 批准/拒绝按钮 + 空态(当前只有 home 活动流 + bell popover);
  5. 6 屏移动布局对齐 `docs/design-system/aleph-mobile/screens/exported/` 的导出 HTML(类/token 已是我们的 aleph.css);
  6. 验收: Simulator 375×812 / 390×844 渲染 6 屏不崩、TabBar 可导航。
- **这是 v1 关键路径** —— 没有它 iOS 壳里没东西可放。**好消息:它与 iOS 工具链解耦,beta Xcode 没到位也能现在做。**

#### Phase 0.5 可执行任务清单(2026-06-25 勘测后确认,用户全数同意)

> 纯 Leptos/WASM/CSS。**Tailwind v4.2**,scanner 扫 `src/**/*.rs` → 在类串写 `max-sm:` 前缀即自动产移动断点(`just wasm` 重建后 `dist/tailwind.css` 出现 `@media …640`)。布局 reflow 主要靠 CSS;**仅 bell 行为**(桌面 popover vs 移动全屏)需轻量 `is_mobile` 信号(仿 `views/canvas/galaxy_canvas.rs:114-141` 的 resize 监听)。审批 RPC 现成:`exec.approvals.pending` / `exec.approval.resolve{id,decision,resolved_by}`(`api/exec_approval.rs:29-60`),事件 `approval.*`(`context.rs:996-1021`),`NotificationCenter`(`notification_center.rs:138-302`)逻辑可整段复用。路由 7 模式 `PanelMode::from_path()`(`mode_sidebar.rs:23-53`),`MainContent`(`app.rs:370-398`)一次性挂载全部视图靠 display 切换保状态 → TabBar 只改 route 不丢状态。

| # | 任务 | 文件 | 验收 |
|---|------|------|------|
| T1 | `is_mobile` viewport 信号(<640px)+ resize 监听 + `provide_context` | 新 `state/viewport.rs` + `app.rs` AppContent | 缩放在 640px 翻转 |
| T2 | 抽共享 mode 图标/标签/路由(DRY) | 新 `components/nav_meta.rs`(从 `nav_menu.rs:31-81` 抽) | NavMenu 视觉不变 |
| T3 | **MobileTabBar** 4 tab(Chat/Memory/Agents/Settings),active=`from_path`,`use_navigate`,`hidden max-sm:flex`+safe-area | 新 `components/mobile_tab_bar.rs` + `app.rs` shell | <640px 底部 4 tab 可点+高亮;≥640px 隐藏 |
| T4 | Shell CSS reflow:`mode_sidebar.rs:63` `w-64`→`w-64 max-sm:hidden`;隐藏桌面 drag-band/LayoutToggle;main 加 TabBar 底距;`viewport-fit=cover`+safe-area 变量 | `mode_sidebar.rs` / `app.rs` / HTML 模板 / `tailwind.css` | 390px 单列、无侧栏、不被遮挡 |
| T5 | Chat 移动顶栏(agent 下拉 + bell)`max-sm` 显示 | `views/chat/*` | Chat@390px 顶栏对 |
| T6 | **Notifications 全屏视图**(复用审批 RPC+alerts;`/notifications` 路由;bell 移动端跳路由,用 T1)+ i18n | 新 `views/notifications.rs` + `views/mod.rs` + `app.rs` 路由 + locales | 移动 bell→全屏;批准/拒绝调 resolve;空态 |
| T7 | 4 tab 屏 + Voice 移动布局对齐导出 HTML(Memory 移动默认走 Vault 列表) | 各 `views/*` 加 `max-sm:` | 各屏 390px 干净 |
| T8 | `just wasm` 重建 + 多断点验证(375/390/640/1024)+ 桌面零回归 | — | `dist/tailwind.css` 现 `@media …640`;桌面不变 |

**微决策(已定)**: ① Dashboard/Teams/Extensions **不进**移动 TabBar(非 MVP);② Memory 移动默认 = **Vault 列表**(galaxy 性能/2D fallback 留 Phase 0);③ "Clear all" = **本地消除**(不真 deny)。
**顺序**: 先 T1–T4(立骨架)→ T5–T7 逐屏。验证只在 T8 跑**一次** `just wasm`(节制 cargo)。

### 10.2 🔴 新决策(需你拍板):iOS 离网(蜂窝)如何够到家里的 core?

iOS 是移动设备,常不在家庭 LAN。计划"连远程 core"在**离网时无路径**(grep: `gateway/config.rs` 默认绑 loopback;`context.rs:234-235` WS URL 由 `location.host` 同源推导,无 relay)。三选一:

- **(A) 集群 reverse-RPC** —— Aleph 已有(`docs/reference/CLUSTER.md`),iOS 作 node 由 center 反向触达。最"正统",但要把 iOS 接进 cluster 客户端 + 可能撞 R1(本地 daemon)。
- **(B) 隧道 / VPN(Tailscale / WireGuard / Cloudflare Tunnel)** —— 把 core 暴露给 iOS。**用户侧零代码、端到端加密、天然绕过 ATS 明文与 TLS 问题。推荐 v1 走这条。**
- **(C) v1 仅 LAN** —— iOS 须与 core 同 WiFi;离网只收 APNs 通知不可交互。最小但体验残。

> ✅ **已定(用户决策 2026-06-25)= (B) Tailscale 类隧道**:用户侧零代码、端到端加密、绕开 10.3 的 TLS 工作量。(A) 集群 reverse-RPC 留作产品级长期方案,(C) 仅兜底。→ v1 离网走隧道,gateway TLS 因此**延后**(见 §10.3)。

### 10.3 🔴 核心侧前提:Gateway 当前零 TLS

- 核实: `src/gateway/config.rs` 无 `tls_cert/tls_key`,全仓无 rustls。离网 `wss://` 无从谈起,token 现在明文随 WS 发(§8.4 已述)。
- ✅ **v1 已选 §10.2-(B) 隧道 → 本节 gateway TLS 工作延后**(隧道自带端到端加密)。以下为**将来直连公网**(A/C 或产品级)时的要求,v1 不做: `[gateway]` 加 `tls_cert_path/tls_key_path` + 首启自签证书 bootstrap(`rcgen` → `~/.aleph/certs/`),iOS 壳 pin 证书。

### 10.4 其余加固清单(HIGH/MEDIUM,按 phase,已并入认知,实施时落实)

- **Phase 0**: WebGL2 Memory canvas(`views/canvas/gl/` ~2163 LOC)真机性能/兼容实测 + 2D 列表 fallback;记录 `isSecureContext` 同时归类连接失败类型(无网/坏 token/端口闭/core 离线/网络切换)。
- **Phase 0.5/1(WASM 侧)**: **首次配对引导**(新用户怎么拿 token:QR / 手输 / LAN 发现 —— 当前连接 UI 只读,无录入);空/错/离线态 6 屏全覆盖;i18n 补 iOS 串(`locales/{en,zh}.json`);a11y(VoiceOver / Dynamic Type / Liquid glass 对比度)。
- **Phase 1(壳/网络)**: WS 生命周期重连(后台挂起→前台恢复 onPause/onResume + 重连退避 + Offline banner,当前 `context.rs` 无 iOS 生命周期钩子);**CI/构建**(justfile `ios-init/ios-dev/ios-build` + GH Actions iOS job,当前**零**);scheme/host-lock 明确策略(推荐 IP/host 字面量,token per-origin);launch screen + app icon(1024,安全区内缩,对照 [[project-macos-dock-icon-padding-fix]] 的满幅教训)。
- **Phase 2**: 早跑 `ios build` 验 haptics/share 插件**真能为 iOS 编译**(Tauri 插件 iOS 矩阵常不全);iPad / 横屏策略;safe-area 真机实测(iPhone 刘海 + iPad)。
- **Phase 4(APNs,大头)**: 服务端 0 APNs → `ApnsProvider` + `jsonwebtoken` `.p8` JWT + **per-device 路由**(`event_bus` 现广播)+ 失败 token backoff/清理;**deep-link** `aleph://approval/<id>` iOS URL scheme + `application(_:open:)` handler(当前**无**);device token 注册/轮换同步;`.p8` 部署(env/vault + 日志脱敏);通知权限 UX(延后到 Settings 再请求,别冷启动 hammer);**per-device 撤销** + 可选 Face ID app-lock(当前单 token 全局授权,丢机无法单独撤销);token 轮换 UX(轮换→广播→各端重认证)。

### 10.5 App Store 4.2 背书强化(仅"冲公开上架"那一步)

- push 单独可能不够"原生价值";dictation-only 是半个特性。叠加 push + haptics + share + 原生 UI + 安全区/TabBar 才稳。fallback 备选:iOS widget / Siri shortcuts / 通知 swipe actions。备 App Store 截图 + "为何不是网页包装"一页纸,提审前找有经验 iOS 开发 gut-check。

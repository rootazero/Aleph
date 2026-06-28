# iOS Panel 配对屏产品化 — 设计

> 日期：2026-06-28 ｜ 范围：`mobile/ios/AlephPaneliOS`（原生 Swift 壳层）
> 触发：把 iOS Panel 从「模拟器/开发调试壳」做成「可发布产品」的前置工作第一切片。
> 目标：让真人能在真机上用「壳内配对」而非「build 时注入 PANEL_URL」连上 server。
> 分发渠道（App Store / TestFlight / 未签名 IPA）**延后**；本 spec 全程不碰 CI / 签名。

## 0. 背景与现状

`mobile/ios/` 是一个 ~90 行 Swift 的 `WKWebView` 薄壳，加载由 `aleph-server` 提供的
WASM panel（`interfaces/webchat/src/platform/phone/`）。它**不是 UI 重写**——所有 panel UI
仍在 Leptos/WASM（R2 单一 UI 源），壳只负责加载 + 适配 phone form-factor。

**现状连接解析链**（`Views/ContentView.swift`）：
```
PANEL_URL 启动环境变量 → UserDefaults("panelURL") → about:blank
```
唯一的写入口是启动时注入的 `PANEL_URL` env（纯调试手段，由 `generate.sh` / `launch-local.sh` 烤进
gitignored 的 scheme）。持久化（`UserDefaults`）已在，**缺的是用户可达的写入口**。

**问题清单**（本 spec 处理 ①②③，④ 另起 spec）：
- ① 用户无任何途径设置 server——拿到 App 直接 `about:blank`。
- ① token 跟 URL 一起明文存进 `UserDefaults`，违反 `rules/swift/security.md`（secrets 必须进 Keychain）。
- ② Bundle ID `ai.aleph.panel.iossim` 是模拟器残留命名。
- ③ `project.yml` 硬编码 `CFBundleShortVersionString "0.1"`，违反「禁止硬编码版本」。
- ④（**不在本 spec**）iPad 设备族 + wide 布局在 webview 的渲染——有独立 UI/横屏/测试决策。

## 1. 范围与红线立场

**本 spec 覆盖**：
- ① 原生配对屏（pairing screen）+ Keychain 持久化 + 落地前探活。
- ② 正式 Bundle ID。
- ③ 版本号接 VERSION 文件（CalVer）。

**R2/R4 立场（关键）**：原生配对屏**只做 transport 配置**——「连哪个 server / 探活 / 存目标」，
不做任何业务设置。这是桌面 `desktop/shell/src/connection.rs:256` 注释里 spec §5.2 那张通行证的
iOS 翻版（原文：这些命令是「I/O config toggles, not business logic — R2/R4 boundary held」）。
**一旦连上，所有 app UI / 设置仍在 WASM panel 里**。若将来想加业务设置，必须回 panel，不得在原生壳堆。

**参照同源**：iOS 配对的地址格式、解析规则、探活策略，全部对齐桌面 lite 壳
（`connection.rs` / `connect_setup.rs` / `splash/connect.html`），使两端配对体验一致（R6 一核多端），
且**不引入任何新格式**。

## 2. 组件设计

全部新增/改动均在 `mobile/ios/AlephPaneliOS/`，纯 Swift 表现/壳层（R1：壳层调用系统 API 合法；
核心契约不在此）。遵循 `rules/swift/*`：值类型优先、`let` 优先、Keychain 存密钥、typed throws、
协议注入便于测试。

### 2.1 新文件

| 文件 | 职责 | 关键点 |
|---|---|---|
| `Models/PairingTarget.swift` | 值类型 + `parse(_:)` | 照抄桌面 `ConnectionTarget::parse` 规则：接受 `host` / `host:port` / `http(s)://host[/route][?token=…]`；默认 scheme `http`、默认端口 `18790`；空串拒绝；非 `http/https` scheme 拒绝。**iOS 无 Local 态**（iPhone 不内嵌 server），故只有 remote URL，比桌面少一个枚举分支。 |
| `Services/ConnectionStore.swift` | Keychain 持久化 | `kSecClassGenericPassword`，固定 service + account。`load() -> URL?` / `save(_ url: URL) throws` / `clear()`。存**整条含 token 的 URL**。协议 `ConnectionStoring` 便于注入 fake。 |
| `Services/ReachabilityProbe.swift` | 裸 TCP 探活 | `NWConnection`（Network framework），2s 超时（对齐桌面 `PROBE_TIMEOUT`）。`probe(host:String, port:UInt16) async -> Bool`。协议 `ReachabilityProbing` 便于注入 fake。只判端口是否接受连接，不做 HTTP/TLS（与桌面一致——真实可达/认证/TLS 是 webview 的事）。 |
| `State/AppState.swift` | `ObservableObject` 驱动显示哪屏 | `@Published var screen: Screen`，`enum Screen { case pairing(message: String?), connected(URL) }`。`resolve()` async 启动解析；`requestReconfigure()`（摇一摇/加载失败调用）。依赖经构造注入 `ConnectionStoring` + `ReachabilityProbing`。 |
| `Views/PairingView.swift` | 原生配对屏 | 对应 `connect.html` 手填卡片：标题 + 提示 + 地址 `TextField`（`.keyboardType(.URL)`、关闭自动大写/纠错）+ Connect 按钮 + 内联错误文本 + 预填当前目标（从 `ConnectionStore` 读）。提交走 `AppState`。 |
| `Views/ShakeDetector.swift` | 摇动手势 | `UIViewControllerRepresentable`（或 `UIWindow.motionEnded` 注入），`.motionEnded(.motionShake)` → `AppState.requestReconfigure()`。 |

### 2.2 改动文件

| 文件 | 改什么 |
|---|---|
| `Views/ContentView.swift` | 删掉内联 `panelURL` 解析；改成 `switch appState.screen`：`.pairing` → `PairingView`，`.connected(url)` → `PanelWebView(url)`。挂 `ShakeDetector`。`.task { await appState.resolve() }`。 |
| `Views/PanelWebView.swift` | 加 `WKNavigationDelegate`：`didFail` / `didFailProvisionalNavigation` → `appState.requestReconfigure(message:)`（防御性，P7）。其余不变（viewport-fit cover 注入等保留）。 |
| `App/AlephPaneliOSApp.swift` | `@StateObject private var appState = AppState(...)`；`ContentView().environmentObject(appState)`。 |
| `project.yml` | Bundle ID `ai.aleph.panel.iossim` → `ai.aleph.panel`；`CFBundleShortVersionString` / `CFBundleVersion` 改 `${ALEPH_VERSION}`（复用现有 `${PANEL_URL}` 同款 env 替换）。新增单测 target（见 §6）。 |
| `generate.sh` | `export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"` 后再 `xcodegen generate`；保留 PANEL_URL 调试注入路径不变。 |
| `README.md` | 写明配对屏 + 摇一摇重配 + Keychain 存储。 |

## 3. 数据流

```
启动 → AppState.resolve():
  env PANEL_URL 非空且可解析?  → ConnectionStore.save(它); target = 它   (保留 sim/dev 注入路径)
  否则 ConnectionStore.load()  → target
  target == nil                → screen = .pairing(nil)
  target != nil → probe(target) → 可达? screen=.connected(url)
                                : screen=.pairing("上次的 server 不可达")

PairingView.submit(raw):
  PairingTarget.parse(raw) 失败 → 内联报错，留在原屏
  probe(parsed) 不可达         → 内联报错 "host:port 不可达"，留在原屏
  可达 → ConnectionStore.save(parsed.url); screen = .connected(parsed.url)

摇一摇 / WebView didFail → AppState.requestReconfigure() → screen = .pairing(预填当前 / error)
```

**env 优先级**：`PANEL_URL` env 仍最高优先（dev/sim 注入），命中即写进 Keychain 并持久。
这保留你现有的 `generate.sh` / `launch-local.sh` 测试链不变。

## 4. 存储 / 安全

- **整条 pairing URL（含 `?token=`）进 Keychain**（`kSecClassGenericPassword`），替代现在
  `UserDefaults("panelURL")` 明文存 token。满足 `rules/swift/security.md`。
- **比桌面更简的理由**：桌面把 token 从 URL 单拆出来另存（`gateway-token` marker, `0o600`），是因为
  Rust 通知 bridge 读不到 webview 的 localStorage、需要自己留一份 token。iOS **当前没有原生通知 bridge /
  无第二消费者**，整条 URL 进 Keychain 即可（KISS / P6）。将来若 iOS 加原生通知 bridge，再按桌面方式拆分。
- **不记录 token**：任何 `print` / 日志不得打印含 token 的 URL。
- **迁移**：旧 `UserDefaults("panelURL")` 是开发态遗留、无真实用户，**不做迁移**（YAGNI）；
  resolve 链直接忽略它，env 注入路径会把新值写进 Keychain。
- **已知延后项（不在本 spec）**：`Info.plist` 的 `NSAppTransportSecurity.NSAllowsArbitraryLoads: true`
  （为连明文 http 的 LAN/公网 core 全局关 ATS）。它绑死 App Store 审核渠道——渠道延后则它延后。
  当前保持原样。

## 5. ②③ 细节

### 5.1 ② Bundle ID
- `project.yml` 中 `PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel.iossim` → `ai.aleph.panel`。
- 实施前 `grep -rn iossim mobile/ios` 确认无他处引用旧 ID。
- 副作用：改 Bundle ID 会让既有 sim 安装变成「另一个 app」（旧的不会被覆盖）——开发态可接受。

### 5.2 ③ 版本号接 VERSION
- 主路：`project.yml` 的 info properties 用 `${ALEPH_VERSION}`，`generate.sh` 在 `xcodegen` 前
  `export ALEPH_VERSION=$(cat ../../VERSION | tr -d '[:space:]')`。这与现有 `${PANEL_URL}` scheme env
  替换同款机制。
- `CFBundleShortVersionString` = VERSION（CalVer `YY.M.D`，合法 short version）。
- `CFBundleVersion` = 暂同值。**单调递增 build number 是 App Store 渠道才强制的**——渠道延后则记一笔延后。
- 备路（若 xcodegen 不支持 info 属性的 `${env}` 展开）：`generate.sh` 在 `xcodegen` **后**用
  `PlistBuddy -c "Set :CFBundleShortVersionString $ALEPH_VERSION"` 打生成出的 `Info.plist`。
  实施时先验主路，不通再走备路；spec 两路都列以防卡壳。

## 6. 测试

### 6.1 单元测试（Swift Testing，`import Testing`）
新增一个单测 target 到 `project.yml`——**这是本 spec 唯一的工程基建新增**。可经
`xcodebuild test -scheme … -destination 'platform=iOS Simulator,…'` 跑（CLI，无需开 Xcode，
对齐 `build-iphone-apps` 技能）。

- `PairingTarget.parse` —— **最高价值**，照抄桌面 `connection.rs` 那批断言：
  - 裸 host → `http://host:18790`
  - `host:port` → `http://host:port`（保留用户端口）
  - `https://host` → scheme 保留、补默认端口
  - `https://host:443` → 显式端口保留
  - 带 `?token=…` → token 落在 query、parse 通过
  - 非法 scheme（`ftp://` / `ws://`）→ 拒绝
  - 空 / 纯空白 → 拒绝
  - IPv6（`http://[::1]:9000` / `http://[::1]`）→ 端口处理正确
- `ReachabilityProbe` —— 关闭端口（loopback:1）→ false；绑 ephemeral listener → true。
- `AppState.resolve` —— 协议注入 fake `ConnectionStoring` + `ReachabilityProbing`：
  - env 优先（env 命中 → 写 store + connected）
  - Keychain 回落（无 env、store 有值、可达 → connected）
  - nil → pairing
  - store 有值但不可达 → pairing(带提示)

### 6.2 运行时 QA（权威门）
走既定 iOS 测试规范流程（重编**完整版 macOS app** → 其内置 core 在 `:18790` 重嵌当前 dist →
sim 连本地核）：
1. 全新安装 → 直接进**配对屏**（非 about:blank）。
2. 填本地 `127.0.0.1:18790/?token=…` → 探活通过 → 连上，panel 正常渲染。
3. **摇一摇** → 回配对屏，预填当前地址。
4. 填一个不可达地址 → **内联错误**，不导航、不白屏。
5. 杀掉本地 core → webview 加载失败 → 回落配对屏。
6. 重启 App → Keychain 记住上次 server，探活通过直接连上（无需重填）。

## 7. 文件清单汇总

**新增（6 Swift + 1 test target）**：
`Models/PairingTarget.swift`、`Services/ConnectionStore.swift`、`Services/ReachabilityProbe.swift`、
`State/AppState.swift`、`Views/PairingView.swift`、`Views/ShakeDetector.swift`、单测 target 文件。

**改动（6）**：
`Views/ContentView.swift`、`Views/PanelWebView.swift`、`App/AlephPaneliOSApp.swift`、
`project.yml`、`generate.sh`、`README.md`。

**不碰**：`interfaces/webchat`（WASM panel，零 panel 耦合）、CI / workflow、签名、`Info.plist` 的 ATS。

## 8. 非目标（Out of Scope）

- iPad 设备族 / wide 布局（④，另起 spec）。
- 任何分发渠道、CI、签名、TestFlight/App Store 上传。
- 扫码配对、Bonjour/mDNS 局域网发现（v2 锦上添花，跟桌面同源可后补）。
- panel 内「换 server」按钮（需 WKScriptMessageHandler 桥 + 改 Leptos，panel↔壳耦合，本期不做）。
- ATS 收紧（绑渠道）。
- App Store 用的单调递增 build number。

## 9. 验收标准

- [ ] 全新安装首启进配对屏（非 about:blank）。
- [ ] 手填 `host` / `host:port` / `http(s)://host[?token=]` 能连上本地 core。
- [ ] token 存 Keychain，`UserDefaults` 不再出现明文 token。
- [ ] 探活失败内联报错、不白屏；摇一摇可回配对屏；webview 加载失败回落配对屏。
- [ ] 重启后记住上次 server。
- [ ] Bundle ID = `ai.aleph.panel`；版本号来自 VERSION（CalVer），无硬编码。
- [ ] 单测（parse / probe / resolve）通过；运行时 QA §6.2 六步通过。
- [ ] 零改动 `interfaces/webchat`；R2/R4 边界守住（原生层仅 transport 配置）。

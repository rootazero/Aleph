# Batch 5 审查报告 — mobile-ios（Swift iOS 客户端）

- 审查路径：`mobile/ios`
- 审查方式：无 diff 全量静态阅读（20 个 Swift 文件全部逐行读完，另含 project.yml / Info.plist / 3 个 shell 脚本 / README）
- 审查日期：2026-07-22（基于 main worktree `/tmp/aleph-review-batch-5`）

## 统计

| 项 | 数量 |
|---|---|
| Swift 文件 | 20（生产代码 12，测试 8） |
| Swift LOC | 1313（生产约 971，测试约 342） |
| 其他 | project.yml (90 行）、Info.plist (47 行）、release-testflight.sh (104 行）、generate.sh (39 行）、launch-local.sh.example (34 行）、README.md |
| 超大文件（>500 行） | 无（最大 177 行） |

## 发现列表（按严重级排序）

### High

**H1. 配对 token（网关完整访问凭证）默认经明文 HTTP 传输，ATS 全局关闭**
- 位置：`AlephPaneliOS/Models/PairingTarget.swift:31`、`AlephPaneliOS/Services/ConnectionStore.swift:5`、`AlephPaneliOS/Resources/Info.plist:25-29`、`project.yml:57-63`
- 描述：`PairingTarget.parse` 在用户未写 scheme 时默认补 `http://`；连接 URL 内含 `?token=` 承载密钥（ConnectionStore 头注释明确说明）。Info.plist 设 `NSAllowsArbitraryLoads: true`，ATS 对**所有**主机（含 https）失效。TOFU 证书校验只覆盖 https 分支；走默认 http 时网关无任何服务端认证，同网段攻击者嗅探或 MITM 即可拿到 token（= 网关完整权限）。project.yml 注释表明这是为支持裸 IP 部署的已知取舍，但"默认 scheme 为 http"使大多数用户无感知地落在最弱路径上。
- 建议：至少对 http 目标在配对界面给出明文警告；中期可让 aleph-server 默认签发自签证书并引导 https+TOFU（iOS 侧 TOFU 已实现，链路已通）。

### Medium

**M1. Release 构建常驻 `webView.isInspectable = true`**
- 位置：`AlephPaneliOS/Views/PanelWebView.swift:48`
- 描述：`isInspectable`（iOS 16.4+）无条件开启，TestFlight/Release 构建也允许 Mac Safari 附加 Web Inspector，可直接读取面板 DOM、网络请求与含 `?token=` 的当前 URL。
- 建议：用 `#if DEBUG` 包裹，或至少由编译配置控制。

**M2. 第二个证书审批会覆盖 `pendingCert`，第一个 TLS challenge 永远不被 resolve**
- 位置：`AlephPaneliOS/State/AppState.swift:89-91`（`presentCertPrompt` 直接覆盖）、`AlephPaneliOS/Views/PanelWebView.swift:133-161`
- 描述：WKWebView 对同一 host:port 可并发发起多个 server-trust challenge（主文档 + WASM/静态资源多个连接）。`presentCertPrompt` 无条件替换 `pendingCert`，被替换的那个 `CertPromptRequest.decide` 再也不会被调用——其 `completionHandler` 悬挂，对应加载永久挂起（WebView 卡死，需摇一摇重配）。`prompt` 内的 `resolved` latch 只防同一请求重复 resolve，防不了跨请求覆盖。
- 建议：`presentCertPrompt` 中若已有 pending，直接 `request.decide(false)` 拒绝新 challenge（fail-closed），或排队处理。

**M3. `launch-local.sh.example` 的 bundle id 与实际产物不一致，文档中的 Option 2 流程默认必失败**
- 位置：`launch-local.sh.example:17`（`BID="ai.aleph.panel.iossim"`）vs `project.yml:66`（`PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel`）
- 描述：模板用 `xcrun simctl launch "$UDID" ai.aleph.panel.iossim`，但应用目标构建出的 bundle id 是 `ai.aleph.panel`（`.iossim` 只出现在测试 target 的 id `ai.aleph.panel.iossim.tests` 中）。用户按 README Option 2 拷贝模板、只改 UDID/PANEL_URL 后，`simctl launch` 会报 "bundle not installed"。
- 建议：把模板中的 `BID` 改为 `ai.aleph.panel`。

### Low

**L1. ReachabilityProbe 存在保留环，每次探测泄漏一个 NWConnection**
- 位置：`AlephPaneliOS/Services/ReachabilityProbe.swift:27-41`
- 描述：`connection` 强引用其 `stateUpdateHandler` → 闭包强引用 `resumed` → `ResumeOnce.onResume` 闭包强引用 `connection`，形成环。fire 后只调 `connection.cancel()`，未置空 handler，环无法断开。每次启动/提交各泄漏 NWConnection + DispatchQueue 一套，量小但确定存在。
- 建议：`onResume` 里先 `connection.stateUpdateHandler = nil` 再 `cancel()`，或 handler 中 `[weak connection]`。

**L2. Keychain 条目用 `kSecAttrAccessibleAfterFirstUnlock`（非 ThisDeviceOnly）**
- 位置：`AlephPaneliOS/Services/ConnectionStore.swift:58`、`AlephPaneliOS/Services/CertTrustStore.swift:94`
- 描述：含 token 的配对 URL 与证书 pin 均可随备份迁移到新设备，且首次解锁后后台可访问。对承载密钥更稳妥的是 `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`。
- 建议：评估是否改为 ThisDeviceOnly 变体（迁移新设备时重新配对是可接受的）。

**L3. Keychain pin 为读-改-写整图，并发 pin 会互相覆盖**
- 位置：`AlephPaneliOS/Services/CertTrustStore.swift:103-107`
- 描述：`pin()` 先 `loadMap()` 再 `saveMap()`，无同步；两个并发 challenge 同时 pin 不同 host 时后写者覆盖先写者，丢失一个 pin（退化为下次再提示，不影响安全方向）。与 M2 同源，出现概率低。
- 建议：随 M2 一起修（串行化 challenge 处理即自然解决）。

**L4. `resolve()` 中 `try? store.save(...)` 静默吞掉 Keychain 写入失败**
- 位置：`AlephPaneliOS/State/AppState.swift:56`、`75`
- 描述：保存失败无任何日志/提示，表现为"重启后又要重新配对"，难以排查。
- 建议：至少 `print`/`os_log` 记录失败状态。

**L5. 配对输入框预填完整 URL（含明文 token）**
- 位置：`AlephPaneliOS/State/AppState.swift:103-105`、`AlephPaneliOS/Views/PairingView.swift:10`
- 描述：`currentTargetString()` 返回含 `?token=` 的完整 URL 并预填到 TextField 明文展示。肩窥/截图场景下 token 暴露。属可用性取舍，标注备查。
- 建议：可选——预填时剥离 query，仅在原样提交时保留旧 token。

## 已验证无问题 / 不重复报告的点

- TOFU 主链路 fail-closed 正确：默认 CA 校验先行；读不到 leaf → cancel；未知/变更证书均走人工审批；`resolved` latch 防双击重复 resolve；sheet 禁滑动关闭（`PanelWebView.swift:90-161`、`ContentView.swift:21-28`）。
- 指纹算法（SHA-256 over leaf DER，大写冒号 hex）与 Rust 端/`openssl` 一致（`CertTrustStore.swift:46-50`）。
- `ResumeOnce` 的 NSLock 一次性 resume 逻辑本身正确（泄漏点仅在保留环，见 L1）。
- token 存 Keychain 而非 UserDefaults，密钥不入库：`project.yml`/`generate.sh`/`release-testflight.sh`/README 的密钥卫生（PANEL_URL 只进 gitignored scheme、签名材料全走 env）已核对，无明文秘密入库。
- 生产代码无 `unwrap`/`expect` 等价物（无 `!` 强解包、无 `fatalError`）；`Color(hex:)` 对坏输入回退 `.clear` 而非崩溃。
- 端口边界处理正确：`PairingTarget.parse` 显式拒绝 >65535 端口（`PairingTarget.swift:48`）。
- 脚本 `set -euo pipefail`、发布脚本不携带 PANEL_URL、env 校验前置——均无问题。

## 架构红线合规快照

| 红线 | 结论 |
|---|---|
| R1（core 不调平台 API） | 合规（本单元无 Rust；iOS 侧全部平台 API 均在 shell 内） |
| R2（复杂 UI 在 Leptos/WASM） | 合规——shell 仅 WKWebView 容器 + 配对/证书两个传输态原生屏，README 明确说明 |
| R4（接口层纯 I/O） | 合规——`AppState`/`ConnectionStore` 只持有"连哪台服务器"这一传输配置，无业务状态 |
| R7（Rust Core 唯一大脑） | 合规——面板逻辑全在服务端 WASM，shell 无决策逻辑 |
| R3 / R8 / R9 / R10 | 不适用（无 core 依赖、无正则路由、无可配置工具面、无 prompt） |

唯一可议处：配对屏预填 token（L5）与 `isInspectable` 常开（M1）属"原生 shell 的最小职责"边界内的实现瑕疵，不构成红线违反。

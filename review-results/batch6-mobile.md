# Batch 6 静态审查候选清单：mobile 模块

> **基线**：worktree `feat/severed-wire-audit-batch6`，与 `main` 一致。
> **审查日期**：2026-08-04

## 范围

- `mobile/ios/AlephPaneliOS/**` (12 源文件 + 8 测试文件)

## 统计

| 项 | 数量 |
|---|---|
| 生产 Swift 文件 | 12（971 LOC） |
| 测试 Swift 文件 | 8（342 LOC） |
| 协议声明 | 3（全部连通） |
| 候选 severed wires | 3（1 DECIDE + 1 CUT/CONNECT + 1 待验证） |

## 候选清单

### W1. `CertInfo` payload producer 构造但 consumer 从未读取（DECIDE）
- **producer**: `AlephPaneliOS/Services/CertTrustStore.swift:8-12, 33-41`
- **consumer**: `PanelWebView.swift:115-124, 147` + `CertTrustSheet.swift:29-31`
- **问题**: `CertInfo(sans: [], subject: "", reason: "")` 占位；`sans` 始终空，UI 分支永远不触发
- **form**: stub（两端占位）
- **triage**: **DECIDE**
- **fix sketch (a)**: 保留 + 文档化 + 加 `// TODO(san-parser)`
- **fix sketch (b)**: CUT — 砍掉 `info:` 参数与 `CertInfo` 类型

### W2. `ConnectionStoring.clear()` 协议孤儿方法（CUT/CONNECT）
- **producer**: `ConnectionStore.swift:10, 68-70`
- **test**: `AlephPaneliOSTests/KeychainConnectionStoreTests.swift:10, 29`
- **生产**: 零调用（`AppState.requestReconfigure` 不调）
- **form**: dead-code（协议表面孤儿方法）
- **triage**: **CUT**（激进）或 **CONNECT**（在 `AppState.requestReconfigure` 加 `store.clear()`）
- **fix sketch (CUT)**: 删 `ConnectionStoring.clear()` + 2 conformer + 测试 `clearRemoves`
- **fix sketch (CONNECT)**: `AppState.requestReconfigure` 加 `store.clear()` + UI 加 "Forget saved server" 按钮

### W3. ShakeDetector 在 `.background()` 可能不触发（DECIDE — 需设备验证）
- **producer**: `ShakeDetector.swift:21-34`
- **wiring**: `ContentView.swift:20` `.background(ShakeDetector { ... })`
- **风险**: SwiftUI `.background(UIViewControllerRepresentable)` 嵌入模式下 `viewDidAppear → becomeFirstResponder()` 是否触发不确定
- **form**: ghost-call（生产中 wire 可能未连通）
- **triage**: **DECIDE** — 需在真机/simulator 上验证

## 已验证无问题

- 协议表面全部连通：3 个 protocol 各有 ≥1 生产 + ≥1 测试 conformer
- `WKNavigationDelegate` 实现完整
- SHA-256 fingerprint 链路完整
- `AppState` ↔ UI 双向连通
- `PANEL_URL` 注入链路连通
- Keychain 读写对称

## 未做

1. 未在设备/simulator 上验证 W3 motion 事件链
2. 未执行 `xcodebuild` 编译（worktree 内无 macOS toolchain）
3. 未覆盖 iOS Push / Scene / Widget Extension
4. 未审查 `.xcodeproj/` 生成产物（gitignored）
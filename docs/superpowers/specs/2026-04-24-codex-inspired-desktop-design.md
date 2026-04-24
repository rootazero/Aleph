# Codex-Inspired macOS Desktop Bridge & Capability Enhancements

**Status:** Design approved, awaiting implementation plan
**Date:** 2026-04-24
**Author:** Aleph team (via brainstorming session)
**Scope focus:** Architecture (A) + Capability gaps (D); UI layer explicitly excluded

---

## 1. 上下文与动机

OpenAI 的 codex 项目在 macOS 桌面集成上采用**极简路径**：CLI 负责启动独立 DMG app，Core 层几乎无平台依赖，`app-server-protocol` 用 `schemars + ts-rs` 生成 9441 行类型安全的 JSON-RPC 协议。桌面能力层非常薄（仅 IOKit 电源管理、CFPreferences 配置、sysctl 检测），但工程纪律高（codesign + Notary + Staple 全自动化）。

Aleph 的桌面集成采用 **Trait-driven 分离架构**，结构完整但有多处未完成：

1. **JSON-RPC bridge 是"空壳"**：`shared/protocol/src/desktop_bridge.rs` (144 行) 和 `desktop/shared/src/bridge.rs` (163 行 SwiftBridge 包装) 已经定义，但 `desktop/macos/bridge/` 下的 Swift CLI **从未写出来**，所有调用实际走 in-process trait（`Arc<dyn ScreenCapability>`）。
2. **`MediaCapability` trait 与 macOS 实现不同步**：trait 默认返回 `NotImplemented`，但 `desktop/macos/src/media.rs` 有 934 行完整的 AVFoundation 实现。
3. **`media.rs` 934 行违反 CLAUDE.md P2**（单文件 >500 行应拆分）。
4. **无 sleep inhibitor**：长 agent 任务会被 system idle sleep 打断。
5. **无 AX (Accessibility API) 树检索能力**：违反 R6 "AI 主动到达" 的上下文感知要求。
6. **权限未授权时静默失败**（典型：`hotkey.rs` 的全局快捷键在没有 InputMonitoring 时无 warning、无引导）。

本 spec 的目标：**启用已规划的 Swift helper 进程**，填上这个空壳，并借此一次性清理上述 5 项债务，同时融合 codex 的三项工程实践：
- `schemars` 驱动的 schema 单源化 + 跨语言 golden fixtures 校验
- `IOPMAssertion` sleep inhibitor 模式
- `anyhow::Context` 链式错误上下文（内部传播时使用）

**非融合项**：codex 的"独立 DMG app"路径违反 Aleph R1/R2/R7，不借鉴；codesign / notarize 自动化、MDM 配置读取属于其他 scope。

**UI 层明确排除**：菜单栏、Halo 浮窗、托盘、任何 Aleph 自绘界面均不在本 spec 内。所有对用户的引导通过**结构化错误数据**呈现，由 LLM 在对话中转述，或由 Aleph 调用系统 URL scheme 让系统自己显示其设置界面。

相关既往决策：
- `2026-03-21-desktop-native-capabilities-design.md`（Capability trait 原始设计）
- `2026-03-25-macos-native-api-upgrade-design.md`
- `2026-03-26-media-capability-design.md`
- `2026-03-26-tcc-permission-management-design.md`
- `2026-04-03-desktop-computer-use-phase1-design.md` / `phase2-design.md`

---

## 2. 非目标（Scope 边界）

| 明确不做 | 原因 |
|----------|------|
| 菜单栏、Halo 浮窗、Tauri UI 壳 | 用户明确排除 UI 层元素 |
| codesign / notarize / staple 自动化 | 属独立 scope，避免范围蔓延 |
| MDM 配置读取 (`CFPreferences`) | 非本次优先级 |
| `screen_record.rs` (363 行 stub) 完成 | 不在用户选的 A+D scope 内 |
| Windows / Linux sleep inhibitor 深度实现 | 本次只做 macOS 完整 + 其他平台 `NotImplemented` |
| 代码生成工具链（quicktype / openapi-generator） | 手写 Codable + schema-diff 已足够，避免生成代码屎山 |
| Feature flag | CLAUDE.md 明确"所有生产功能始终编译"，降级靠运行时 fallback |
| 完全移除 Rust 侧所有 ObjC/Swift FFI（A3 路径） | 风险过大，可能用新屎山替旧屎山；保留小而干净的 FFI 模块（hotkey / clipboard / notification / workspace / pim / automation / permission check） |

---

## 3. 总体架构

### 3.1 三层进程模型

```
┌─────────────────────────────────────────────────┐
│ Rust Core (aleph-server)                        │
│  ├─ desktop/shared/traits/*       (契约)        │
│  ├─ desktop/shared/bridge.rs      (客户端)      │
│  └─ desktop/macos/ (瘦身后)                     │
│     ├─ sleep_inhibitor.rs   ← 纯 IOKit C API    │
│     ├─ hotkey.rs            ← 保留 NSEvent      │
│     ├─ system/clipboard     ← 保留              │
│     ├─ system/notification  ← 保留              │
│     ├─ system/workspace     ← 保留              │
│     └─ pim/automation/permission(check)  保留   │
└─────────────────┬───────────────────────────────┘
                  │ JSON-RPC 2.0 over stdio
                  │ (长驻子进程，line-delimited)
┌─────────────────▼───────────────────────────────┐
│ aleph-bridge (Swift helper subprocess)          │
│  ├─ Sources/Bridge/main.swift                   │
│  ├─ Sources/Bridge/RPC/         ← JSON-RPC 框架 │
│  ├─ Sources/Bridge/Media/       ← AVFoundation  │
│  │    ├─ Camera.swift / Audio.swift / Speech.swift
│  ├─ Sources/Bridge/Vision/      ← Vision OCR    │
│  └─ Sources/Bridge/Accessibility/               │
│       ├─ AxQuery.swift   ← 新能力                │
│       └─ PermissionGuide.swift ← 新能力          │
└─────────────────────────────────────────────────┘
```

### 3.2 关键设计决定

- **长驻子进程**，非每调用 spawn。冷启动开销对 AX/OCR 等高频调用不可接受。
- **stdio 通信**（line-delimited JSON-RPC 2.0）。不走 Unix socket，避免文件系统权限问题；进程退出即关闭。
- **进程组绑定 + kqueue 父进程监听**：aleph-server 死亡时 Swift helper 必须随之退出（防止僵尸进程触发 CLAUDE.md 警告过的 `.shared_token` 竞写导致 vault 数据丢失）。
- **迁 vs 留的划分原则**：
  - 纯 C API（IOKit / sysctl / libc）→ Rust 直接调，不走 bridge
  - Swift/ObjC-heavy 且当前 Rust FFI 写得别扭 → 迁 Swift helper
  - 短小干净的 objc2 实现（< 200 行，高内聚）→ 保留 Rust

### 3.3 R1 合规性

重构后 Rust Core 侧的 `cocoa` / `objc2` / `core-foundation` 依赖**显著减少但不归零**。归零需要 A3 路径（代价过大）。保留以下 Rust 侧 ObjC 模块：
- `hotkey.rs`（NSEvent 全局监听，ObjC runloop 与 tokio runloop 耦合已验证）
- `system/clipboard.rs` / `system/notification.rs` / `system/workspace.rs`（每个 < 200 行，干净）
- `pim/*` / `automation/*` / `permission.rs` check/request 部分

这个平衡贴合 R1 "大脑不直接调平台重 API" 的精神而非字面。

---

## 4. JSON-RPC 协议设计

### 4.1 消息层

**Line-Delimited JSON-RPC 2.0 over stdio**，双向 notification-capable：

```
Rust stdin   →  Swift stdin   (Request / Notification)
Rust stdout  ←  Swift stdout  (Response / Server-Push Notification)
Swift stderr →  日志（Rust 侧结构化 forwarding，tracing target="bridge_stderr"）
```

Server-Push notifications（Swift → Rust）覆盖：`ax.mutation`、`perm.status_changed`、`log.entry`、`bridge.shutdown_ack`。不引入 SSE 或其他流式传输。

### 4.2 Schema 的单源

**Rust 侧 `shared/protocol/src/desktop_bridge/` 是唯一真源**，每个 params/result struct 都 `#[derive(Serialize, Deserialize, JsonSchema)]`：

```
shared/protocol/src/desktop_bridge/
  mod.rs              ← 公共接口（重构自 144 行原文件）
  envelope.rs         ← JSONRPCMessage / Request / Response / Notification
  methods/screen.rs   ← ScreenshotParams / ScreenshotResult / OcrParams / ...
  methods/window.rs
  methods/input.rs    ← ClickParams / TypeTextParams / ...
  methods/media.rs    ← CameraSnapParams / ...
  methods/ax.rs       ← AxQueryParams / AxElement / AxTree / ...
  methods/perm.rs     ← CheckPermissionParams / PermissionGuide / ...
  methods/system.rs   ← NotificationParams / ClipboardReadResult / ...
  errors.rs           ← BridgeError → JSON-RPC error code 映射
```

**Swift 侧手写 `Codable` struct**：
- `just bridge-schema` 导出 `desktop_bridge.schema.json`
- CI 里 Swift 单测加载 golden JSON fixtures，双向解码校验
- **不做代码生成**（避免 build pipeline 耦合 + 生成代码维护负担）

### 4.3 方法清单（破坏 legacy 命名）

当前 `desktop_bridge.rs` 的扁平命名（`desktop.screenshot` / `desktop.click` / `canvas.*` / `webview.*` / `tray.*`）—— 因为 bridge 从未真正启用，**借此次重构理顺**，不背历史包袱：

| 域 | 方法 | 备注 |
|----|------|------|
| `screen.*` | `capture` / `ocr` / `list_displays` | |
| `window.*` | `list` / `focus` | |
| `input.*` | `click` / `double_click` / `drag` / `hover` / `type` / `key_combo` / `scroll` / `cursor` / `mouse_button` | |
| `media.*` | `camera_snap` / `camera_clip` / `list_audio_devices` / `record_audio` / `speech_to_text` | **全部迁 Swift** |
| `ax.*` | `query_focused` / `query_tree` / `query_by_role` / `subscribe_mutations` | **新增** |
| `perm.*` | `check` / `guide` / `open_settings` | **新增** |
| `system.*` | `notify` / `clipboard_read` / `clipboard_write` / `launch_app` / `quit_app` / `list_running_apps` | 这些方法**保留 Rust 实现**，也在 bridge schema 中声明只是为了方便未来扩展 —— 目前 Rust 侧不通过 bridge 调用它们 |
| `bridge.*` | `handshake` / `ping` / `shutdown` | 生命周期 |

**删除**：`canvas.*` / `webview.*` / `tray.*` 方法常数（均为原计划 Tauri UI 预留，UI 被排除后失去意义）。

### 4.4 握手与版本协商

```jsonc
// Rust → Swift
{"jsonrpc":"2.0","id":1,"method":"bridge.handshake",
 "params":{"rust_version":"2026.04.24","protocol_version":2}}

// Swift → Rust
{"jsonrpc":"2.0","id":1,
 "result":{"swift_version":"2026.04.24","protocol_version":2,
           "supported_methods":["screen.capture","screen.ocr",...]}}
```

握手失败或版本不匹配 → Rust 侧回退到现有 in-process 实现（渐进迁移期间并存；Stage 6 后 in-process 路径删除，回退变为直接 `BridgeDisabled` 错误）。

### 4.5 错误模型

JSON-RPC 2.0 标准错误码保留；业务错误映射：

| Rust `DesktopError` | JSON-RPC code | 含义 |
|---------------------|---------------|------|
| `PermissionDenied` | -32001 | TCC / Accessibility 未授权 |
| `NotImplemented` | -32002 | 方法已注册但未实现（Swift 侧占位） |
| `PlatformError(...)` | -32003 | 底层 API 报错 |
| `Timeout` | -32004 | 响应超时 |
| `HelperCrashed` | -32005 | Swift helper 进程异常终止 |
| `InvalidArgument(...)` | -32602 | JSON-RPC 标准 |
| `BridgeDisabled` | -32006 | helper 多次重启失败后进入 disabled 模式 |

错误 `data` 字段装结构化上下文，**权限错误时强制包含 `PermissionGuide`**（见 §6）。

### 4.6 并发与超时

- Rust 侧：`Arc<SwiftBridge>` + `tokio::sync::Mutex<HashMap<u64, oneshot::Sender<...>>>` in-flight 表
- Swift 侧：`DispatchQueue.concurrent` 处理请求，AppKit API 调用时切 main queue
- **每方法在 schema 里声明 `suggested_timeout_ms`**（如 `screen.capture: 2000`、`media.record_audio: 60000`），由 Rust 侧 `tokio::time::timeout` 执行
- 超时后迟到响应按 in-flight id 丢弃

---

## 5. 迁移清单与 6 个 Stage

### 5.1 迁 / 留矩阵

| 模块 | 当前位置 | 行数 | 最终归属 | 动作 |
|------|---------|------|----------|------|
| `media.rs` (AVFoundation) | `desktop/macos/src/media.rs` | 934 | **Swift** | 重写到 `Sources/Bridge/Media/{Camera,Audio,Speech}.swift`；Rust 侧删除 |
| `ocr_macos.rs` | `desktop/shared/src/perception/ocr_macos.rs` | ~ | **Swift** | 重写到 `Sources/Bridge/Vision/Ocr.swift`；`ocr_windows.rs` 原样保留 |
| AX 树检索（新增） | — | — | **Swift** | 新建 `Sources/Bridge/Accessibility/AxQuery.swift` |
| 权限引导（新增） | — | — | **Swift** | 新建 `Sources/Bridge/Accessibility/PermissionGuide.swift` |
| `sleep_inhibitor`（新增） | — | — | **Rust** | 新建 `desktop/macos/src/sleep_inhibitor.rs`，纯 IOKit C API |
| `hotkey.rs` | `desktop/macos/src/hotkey.rs` | 277 | **Rust 保留** | 不动 |
| `system/clipboard.rs` | `desktop/macos/src/system/` | 短 | **Rust 保留** | 不动 |
| `system/notification.rs` | `desktop/macos/src/system/` | 短 | **Rust 保留** | 不动 |
| `system/workspace.rs` | `desktop/macos/src/system/` | 短 | **Rust 保留** | 不动 |
| `pim/*` / `automation/*` / `permission.rs` check/request | `desktop/macos/src/` | — | **Rust 保留** | 不动 |
| `perception/screen_record.rs` stub | `desktop/shared/src/perception/` | 363 | — | **搁置**（不在本次 scope） |
| `desktop_bridge.rs` 内 `canvas.*/webview.*/tray.*` 方法常数 | `shared/protocol/src/` | — | — | **删除** |

### 5.2 执行顺序

每个 Stage 独立可合并，合并前必须 `just test-all` 绿。旧代码**就地删除**，不留 "稍后清理" 尾巴。

**Stage 0 · 地基**（无用户可见价值，所有后续前提）
- 创建 `desktop/macos/bridge/` Swift Package，填上当前空目录
- Swift：`RPC/Server.swift`（stdio line-delimited JSON-RPC dispatcher），实现 `bridge.handshake` / `bridge.ping` / `bridge.shutdown`
- Rust：完成 `SwiftBridge::ensure_running()`（长驻子进程、指数退避重启、进程组绑定、kqueue 父进程死亡监听）
- `shared/protocol/src/desktop_bridge/` 按 §4.2 结构重构为子目录，加 `JsonSchema` derive
- `justfile`：`bridge-build` / `bridge-schema` / `bridge-test`；`just build` 级联调用 `bridge-build`
- CI：Swift 侧单测加载 golden fixtures 做 schema-diff
- **完成定义**：`cargo test -p alephcore` 全绿；`bridge.ping` 往返成功；helper 崩溃 → 自动重启可见

**Stage 1 · 迁 media**（拆成 1a/1b/1c 三个独立 PR）

- **1a · Camera**：Swift `Sources/Bridge/Media/Camera.swift`（AVCaptureSession）；Rust 侧 `media.rs` 中 camera 相关块迁走
- **1b · Audio**：Swift `Sources/Bridge/Media/Audio.swift`（AVAudioRecorder / AVAudioEngine）；Rust 侧 audio 相关块迁走
- **1c · Speech**：Swift `Sources/Bridge/Media/Speech.swift`（SFSpeechRecognizer）；Rust 侧 speech 相关块迁走

每个子 PR < 300 行 Swift + 对应 Rust 行数净减。全部完成后：`desktop/macos/src/media.rs` 归零；`MediaCapability` trait 的"实现不同步"问题消失（接口由 bridge 统一承载，Linux/Windows 仍 `NotImplemented`）。

**完成定义**：`media.rs` 删除；trait 和实现不再漂移；代码净减 ~700+ 行。

**Stage 2 · 迁 OCR**
- Swift：`Sources/Bridge/Vision/Ocr.swift`（`VNRecognizeTextRequest`，支持 language hint / recognition level）
- Rust：`ocr_macos.rs` 删除；`ocr_windows.rs` 原样保留；`perception` 模块 macOS 分支走 bridge
- **完成定义**：macOS OCR 调用走 Swift Vision；`ocr_macos.rs` 归零

**Stage 3 · AX 能力（新增）**
- Swift：`Sources/Bridge/Accessibility/AxQuery.swift`（`AXUIElementCopyAttributeValue` 封装）
- 暴露：`ax.query_focused` / `ax.query_tree` / `ax.query_by_role` / `ax.subscribe_mutations`（Server-Push）
- Rust：新增 `desktop/shared/src/traits/ax.rs` —— **新 trait `AccessibilityCapability`**（独立，符合 P2 单一职责，不扩展 ScreenCapability）
- `PlatformContext` / 依赖注入处加上 `Arc<dyn AccessibilityCapability>`
- **完成定义**：LLM 可通过工具拿到前台 app 的 focused element / UI 树；权限缺失返回结构化 `PermissionDenied` + `PermissionGuide`

**Stage 4 · 权限引导（新增）**
- Swift：`Sources/Bridge/Accessibility/PermissionGuide.swift`
  - 覆盖 kind：Accessibility / InputMonitoring / ScreenRecording / FullDisk / Camera / Microphone / Automation / Contacts / Calendars / Reminders / Photos
  - `perm.check(kind) → PermissionStatus` —— 纯查询，不弹对话框
  - `perm.guide(kind) → PermissionGuide` —— 纯数据，不弹 UI
  - `perm.open_settings(kind) → {ok}` —— 内部 `NSWorkspace.open(URL)`，系统设置被拉起（不是 Aleph 自绘 UI）
- Rust：扩展 `PermissionCapability` trait 的 `guide_permission` 方法
- **错误自描述化**：任何桌面工具调用失败，`DesktopError::PermissionDenied` 的 data 字段自动装载 `PermissionGuide`，LLM 不需要主动多调一次
- **修复 hotkey.rs 静默失败**：启动时预检 InputMonitoring + Accessibility，未授权则 `tracing::warn!` + 通过事件总线 emit 一次性 `PermissionDenied` 事件（带 guide 数据）
- **新工具**：`builtin_tools/desktop/` 暴露 `desktop.check_permissions([kinds])` 供 LLM 主动预检
- **完成定义**：LLM 拿到 JSON 结构化引导信息（含 deep link + steps），在对话里告诉用户步骤

**Stage 5 · sleep inhibitor（不走 bridge）**
- Rust：`desktop/macos/src/sleep_inhibitor.rs`，`IOPMAssertionCreateWithName` + `IOPMAssertionRelease` 直接 FFI（参考 codex `sleep-inhibitor/src/macos.rs`，不新增 crate 依赖）
- 封装 RAII：`SleepInhibitor::acquire(reason: &str) -> InhibitorGuard`，Drop 自动释放
- 每次 acquire 独立 assertion id；多 guard 并存 = 多 assertion，天然引用计数
- 集成点：**agent loop `run_turn()` 入口 acquire，turn 结束 drop**（粗粒度；见 §7）
- Windows：`SetThreadExecutionState` + refcount（Windows API 粘性，需内部计数）
- Linux：返回 `NotImplemented`
- **完成定义**：macOS 下 1 小时长 agent 任务不被 system idle sleep 打断；`pmset -g assertions` 可见 "Aleph agent loop" 条目

**Stage 6 · 清理 legacy**
- 删除 `shared/protocol/src/desktop_bridge/methods/` 里任何未使用或替代的 legacy 常数（包括 `canvas.*` / `webview.*` / `tray.*`）
- `cargo udeps` 报告无 unused dependencies；`desktop/macos/Cargo.toml` 删除因迁移而不再引用的 crate
- `just clippy` 全 workspace 零告警
- 移除 Stage 1~5 期间的 in-process fallback 代码（Rust 侧 media / OCR 旧实现已删）
- 文档更新（见 §10）
- **完成定义**：grep `canvas|webview|tray` 在 `shared/protocol/` 下 0 命中；`cargo udeps` 干净；文档同步

---

## 6. Sleep Inhibitor

### 6.1 Trait（Rust 侧）

```rust
// desktop/shared/src/traits/power.rs
pub trait PowerCapability: Send + Sync {
    /// Prevent system idle sleep while the returned guard is alive.
    /// `reason` is surfaced in macOS `pmset -g assertions` output.
    fn inhibit_sleep(&self, reason: &str) -> DesktopResult<InhibitorGuard>;
}

pub struct InhibitorGuard {
    release: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl Drop for InhibitorGuard {
    fn drop(&mut self) {
        if let Some(f) = self.release.take() { f(); }
    }
}
```

### 6.2 平台实现

- **macOS**：直接 FFI 调 `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep, kIOPMAssertionLevelOn, reason, &mut id)` + `IOPMAssertionRelease(id)`。不引入 `io-kit-sys` crate（codex 就没引）。
- **Windows**：`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`；API 粘性，需在 `PowerCapability` impl 内维护 refcount，最后一个 guard drop 时清 flag。
- **Linux**：返回 `DesktopError::NotImplemented`（systemd-inhibit / Wayland idle-inhibit / X11 差异太大，留后续 spec）。

### 6.3 集成粒度

**粗粒度**：在 `src/agent/loop.rs` 的 `run_turn()` 入口 acquire 一个 guard，`reason = "Aleph agent loop"`，turn 结束 drop。

不做细粒度 tool-level —— 实现复杂、收益有限。"agent 正在思考/执行工具"本身就是不该休眠的时刻。

### 6.4 可观测性

```rust
tracing::debug!("sleep inhibitor acquired: reason={reason} id={id:x}");
tracing::debug!("sleep inhibitor released: id={id:x} duration_ms={elapsed}");
```

不加 metrics gauge（YAGNI）。

---

## 7. 权限引导（结构化数据，无 UI）

### 7.1 PermissionKind

```rust
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Accessibility,
    InputMonitoring,
    ScreenRecording,
    FullDisk,
    Camera,
    Microphone,
    Automation,
    Contacts,
    Calendars,
    Reminders,
    Photos,
}
```

### 7.2 数据结构

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct PermissionStatus {
    pub kind: PermissionKind,
    pub granted: bool,
    pub can_request_programmatically: bool,
    pub restricted: bool,  // MDM 强制限制
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct PermissionGuide {
    pub kind: PermissionKind,
    pub status: PermissionStatus,
    pub deep_link: String,
    pub human_readable_steps: Vec<String>,
    pub rationale: String,
}
```

Swift helper 按 macOS 版本构造正确的 `deep_link`（13+ 新 URL scheme / <13 旧 scheme），Rust 侧不关心细节。

### 7.3 错误自描述化（关键）

`DesktopError::PermissionDenied` 强制内嵌 `PermissionGuide`：

```rust
pub enum DesktopError {
    PermissionDenied {
        kind: PermissionKind,
        guide: PermissionGuide,
    },
    // ...
}
```

序列化后：

```jsonc
{
  "jsonrpc": "2.0", "id": 42,
  "error": {
    "code": -32001,
    "message": "permission denied: accessibility",
    "data": {
      "kind": "accessibility",
      "status": {"granted": false, "can_request_programmatically": false, "restricted": false},
      "deep_link": "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
      "human_readable_steps": [
        "打开「系统设置」→「隐私与安全性」→「辅助功能」",
        "在列表中找到 Aleph（或 aleph-server）",
        "拨动开关至开启状态"
      ],
      "rationale": "Aleph 需要辅助功能权限以监听全局快捷键 / 访问 UI 元素树"
    }
  }
}
```

LLM 看到错误即自带完整引导。builtin_tools prompt 指引：

> 当桌面工具返回 `PermissionDenied` 错误时，`error.data` 字段里包含 `deep_link` 和 `human_readable_steps`。在回复用户时，用 `rationale` 解释为什么需要权限，然后把 `human_readable_steps` 转述给用户，最后附上 `deep_link`。**不要只说"权限不足"就结束**。

### 7.4 主动预检 tool

`builtin_tools/desktop/` 新增：

```jsonc
// Tool: desktop.check_permissions
// Input:  {kinds?: PermissionKind[]}  // 省略 → 检查 Aleph 常用的一批
// Output: PermissionStatus[]
```

LLM 在涉及桌面能力前可以主动检查。

### 7.5 `perm.open_settings` 的定位

调用方法：`NSWorkspace.shared.open(URL(string: deepLink))` —— 让系统打开自身设置 app。这不是 Aleph 自绘 UI，是系统行为。用户最终确认保留此方法。

### 7.6 修复 D#10（hotkey 静默失败）

`hotkey.rs` 启动时同步调 `perm.check(InputMonitoring)` + `perm.check(Accessibility)`：

- 任一未授权 → `tracing::warn!("global hotkey disabled: {kind} permission missing")`
- 通过事件总线 emit 一次性 `PermissionDenied` 事件（载荷是 `PermissionGuide`）
- 上层 agent / 首次对话 pickup 事件后，LLM 主动告知用户

---

## 8. 错误处理与 Fallback

### 8.1 错误类型边界

- `DesktopError` enum 作为 Rust 侧对外类型（builtin_tools 层不变）
- Swift 错误经 JSON-RPC → `BridgeError` → 映射回 `DesktopError`
- **`anyhow::Context` 仅在 bridge 内部传播路径使用**（借鉴 codex），跨公共边界时转回 `DesktopError`，不让 `anyhow::Error` 泄露破坏 trait 契约

```rust
// desktop/shared/src/bridge.rs（内部）
let raw = self.send_request(req)
    .await
    .with_context(|| format!("bridge request: method={method}"))?;
// 出 bridge.rs 时转 DesktopError::BridgeError { source }
```

### 8.2 Fallback 规则

**迁移期（Stage 1~5）**：

| 错误类型 | 行为 |
|----------|------|
| `PermissionDenied` / `InvalidArgument` | **不回退**（回退结果一样），直接上抛含 `PermissionGuide` |
| `NotImplemented`（Swift 侧未迁） | 回退到 Rust in-process（如还存在） |
| `HelperCrashed` / `Timeout` | **仅对只读方法回退一次**（screen.capture / ocr / ax.query_*）；写操作（input.click / media.record_audio）不回退，避免幂等性事故 |
| `PlatformError(...)` | 不回退，上抛 |

**最终态（Stage 6 后）**：Rust 侧 media/OCR in-process 已删，`NotImplemented` 回退自然消失。bridge 挂了就返回 `BridgeDisabled`。

**不做无限重试**：回退最多 1 跳。

### 8.3 崩溃恢复

- Swift helper 崩溃 → in-flight table 所有 oneshot 返回 `HelperCrashed`
- `SwiftBridge` 指数退避重启（1s / 2s / 4s / max 30s）
- 重启 >5 次 / 10 分钟 → 进入 `BridgeDisabled` 模式，后续调用直接返回该错误
- 每次调用的重试策略由调用方决定，不自动重试（语义不同：读可、写不可）

### 8.4 日志

- Swift `stderr` → Rust `tracing` target=`bridge_stderr`，逐行透传
- Swift 结构化日志走 `log.entry` notification，带 level / message / context，Rust 侧转成本地 tracing 事件
- Debug log：每个 RPC 打 `method={m} id={id} duration_ms={d}`

### 8.5 安全边界

- **Swift helper 不触碰 `~/.aleph/data/`**（vault / shared_token / embedding key 一概不读不写）—— 硬性规则，防止 CLAUDE.md 里警告过的 `.shared_token` 竞写导致 vault 数据永久丢失
- helper 运行权限 = Rust 父进程（同 UID，无提权）
- stdio 天然私有通道，helper 不对外开任何 socket / port

---

## 9. 测试策略

### 9.1 Unit（快）

- `shared/protocol/src/desktop_bridge/`：每个 params/result 的 serde 往返 + `JsonSchema` 输出 snapshot
- `desktop/macos/sleep_inhibitor.rs`：acquire / drop 对 IOKit 调用次数的 mock 验证

### 9.2 Integration（中速）

`desktop/macos/tests/bridge_fakeserver.rs`：用 **Rust 实现的假 stdio echo server** 当 Swift helper，验证：
- 握手流程（版本协商、supported_methods）
- 崩溃 → 重启（kill helper，下次 call 自愈）
- 超时清理 in-flight table
- 协议级错误（malformed JSON、未知 method）返回正确 code

### 9.3 E2E（慢，macOS CI only）

`just test-bridge-e2e`：启动真实编译的 `aleph-bridge` 二进制，覆盖：
- `bridge.ping` 往返
- `screen.capture` 返回非空字节
- `perm.check(Accessibility)` 返回合法 `PermissionStatus`（不关心 granted 值）
- `ax.query_focused` 返回 element 或 `PermissionDenied`

### 9.4 Golden Fixtures（共享）

`shared/protocol/tests/fixtures/*.json` —— Rust 和 Swift 双读：
- Rust test：序列化结果 == fixture
- Swift test：Codable 解码 fixture 成功，字段值与期望一致

### 9.5 Property

`proptest` 对 `AxQueryParams` / `ClickParams` 等生成随机合法/非法输入，验证 schema 边界校验不 panic。

### 9.6 Mock Capabilities

- `MockPowerCapability` / `MockAccessibilityCapability` / `MockMediaCapability`（in-memory fake）给 agent loop / integration 测试用，不依赖真实 macOS

### 9.7 Schema-diff CI gate

- `just bridge-schema` 输出 `desktop_bridge.schema.json` 提交到仓库
- PR 检查：schema 变更必须更新 `CHANGELOG.md`，不兼容变更 bump `protocol_version`

---

## 10. 依赖、构建、文档

### 10.1 新增依赖

**Rust 侧**：
- `shared/protocol/Cargo.toml`：如未启用，加 `schemars = "0.8"`
- `desktop/macos/Cargo.toml`：不引入 `io-kit-sys`，直接 FFI（参考 codex）
- Stage 6 **删除**：`objc2-av-foundation` / `objc2-vision` 等因 media/OCR 迁走而不再引用的 crate

**Swift 侧**：
- `desktop/macos/bridge/Package.swift` —— 新建
- 仅标准库 + 系统框架（AVFoundation / Vision / AppKit / ApplicationServices / IOKit）
- 测试用 XCTest
- **不引入任何外部 Swift package**

### 10.2 构建管线

`justfile` 增量：

```make
bridge-build:
    cd desktop/macos/bridge && swift build -c release --arch arm64 --arch x86_64

bridge-schema:
    cargo run -p alephcore --bin export-desktop-bridge-schema \
        > desktop/macos/bridge/Tests/BridgeTests/Fixtures/schema.json

bridge-test:
    cd desktop/macos/bridge && swift test

test-bridge-e2e:
    cargo test --package aleph-desktop-macos --test bridge_e2e -- --include-ignored

build: bridge-build
    # ... 原有步骤
```

**Release 产物**：
- `target/release/aleph-server` 启动时 exec `aleph-bridge`
- 路径约定：同级 `./aleph-bridge` 或 `$ALEPH_BRIDGE_PATH` 环境变量覆盖
- `aleph-bridge` 必须跟 `aleph-server` 一起打包分发
- GitHub workflow macOS job 增加 `just bridge-build`，把 `aleph-bridge` 纳入 tarball

**不做**：codesign / notarize / staple（独立 scope）。

### 10.3 文档更新（Stage 6 的一部分）

- `docs/reference/ARCHITECTURE.md`：增加"Swift Helper Process"小节，画三层进程图
- `docs/reference/DESIGN_PATTERNS.md`：增加"JSON-RPC Bridge Pattern"
- `docs/reference/SANDBOX.md`：更新桌面能力边界
- `docs/reference/AGENT_SYSTEM.md`：增加 sleep inhibitor 在 agent loop 的集成点
- `docs/reference/SECURITY.md`：记录"Swift helper 不触碰 vault"硬性规则
- **新增 `docs/reference/DESKTOP_BRIDGE.md`**：专门文档，覆盖协议、方法清单、错误码、调试手段（`tail -f ~/.aleph/logs/bridge.log`）
- 本 spec 保留为历史决策记录

---

## 11. 成功标准

### 架构层（A）
- [ ] `desktop/macos/bridge/` 不再是空目录，包含可编译的 Swift Package
- [ ] `just bridge-build` 产出 universal binary（arm64 + x86_64）
- [ ] `SwiftBridge::ensure_running()` 长驻子进程、崩溃自愈测试通过
- [ ] JSON-RPC schema 在 Rust/Swift 双端 golden fixtures 校验通过
- [ ] `shared/protocol/src/desktop_bridge.rs` 单文件 144 行重构为 `desktop_bridge/` 子目录
- [ ] `canvas.*/webview.*/tray.*` 方法常数在 `shared/protocol/` 下 grep 0 命中

### 基础能力（D）
- [ ] `ax.query_focused` / `ax.query_tree` / `ax.query_by_role` 返回合法 `AxElement` 或 `PermissionDenied`
- [ ] `DesktopError::PermissionDenied` 自带 `PermissionGuide`，含 `deep_link` + `human_readable_steps`
- [ ] `hotkey.rs` 启动时预检 InputMonitoring + Accessibility，未授权时显式 warn + event，不静默失败
- [ ] Sleep inhibitor：agent loop `run_turn()` 入口 acquire，turn 结束 drop；`pmset -g assertions` 可见 "Aleph agent loop"
- [ ] 1 小时长 agent 任务不被 system idle sleep 打断（手测）

### 技术债清理
- [ ] `desktop/macos/src/media.rs` 934 行 → 删除
- [ ] `desktop/shared/src/perception/ocr_macos.rs` → 删除
- [ ] `MediaCapability` trait 与实现不同步问题消失
- [ ] `cargo udeps` 报告无 unused dependencies
- [ ] `just clippy` workspace 零告警

### 代码质量
- [ ] Swift 侧每个文件 < 300 行
- [ ] Rust 侧 bridge 客户端 < 400 行（复杂逻辑拆子模块）
- [ ] 6 个 Stage 独立可合并，每 PR `just test-all` 绿

---

## 12. 风险与缓解

| 风险 | 缓解 |
|------|------|
| Swift helper 启动延迟拖慢首次 tool 调用 | 服务启动时立即预热握手，不等第一次调用 |
| Universal Binary 编译拖慢 `just build` | `bridge-build` 可并行到 `cargo build` 阶段 |
| Swift/Rust 两边类型漂移 | golden fixtures + CI schema-diff 强校验 |
| Helper 进程意外驻留（aleph-server kill -9） | 父进程组绑定 + kqueue 监听 + helper 侧每 10s poll `getppid()` 自杀 |
| 迁移期 in-process + bridge 并存产生"幽灵"路径 | 每 Stage 合并即删除被替代的 Rust 代码，不留 else 分支 |
| Windows sleep inhibitor refcount bug | 单独单测覆盖并发 acquire/drop 场景 |

---

## 13. 附录：codex 借鉴映射

| codex 做法 | Aleph 采纳度 | 备注 |
|-----------|-------------|------|
| `schemars + ts-rs` schema 单源 | ✅ 采纳（只用 schemars，Swift 侧手写 Codable + schema-diff CI） | 不引入 `ts-rs` |
| `IOPMAssertion` sleep inhibitor | ✅ 完全借鉴 | 直接 FFI，不加 `io-kit-sys` crate |
| `anyhow::Context` 链式错误 | 🟡 部分借鉴 | 仅 bridge 内部传播使用，公共边界仍用 `DesktopError` |
| 独立 DMG app 路径 | ❌ 不采纳 | 违反 Aleph R1/R2/R7 |
| codesign / notarize 自动化 | ⏭ 后续 spec | 本次 scope 外 |
| `CFPreferences` MDM 配置读取 | ⏭ 后续 spec | 本次 scope 外 |
| Desktop App 完全独立进程（CLI 只 open -a） | ❌ 不采纳 | Aleph 选择 Swift helper + Rust Core 协同模型 |

---

## 14. Open Questions

无（设计已在 §1-§6 分段 review 中逐点确认）。

如实施过程中发现未覆盖问题，走正常 review 通道补充（不在本 spec 内预留"待定"占位符）。

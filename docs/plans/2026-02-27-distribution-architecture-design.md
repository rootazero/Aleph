# Distribution Architecture Design

> 日期: 2026-02-27
> 状态: Approved

## 概述

Aleph 采用**双轨分发**策略：纯 Server 单二进制分发 + 按操作系统构建的桌面应用分发。桌面应用在 macOS 上使用原生 Xcode (Swift/SwiftUI) 构建，Linux/Windows 继续使用 Tauri 构建。所有桌面应用内嵌 aleph-server 二进制，用户双击即用。

## 动机

- **系统集成深度**: macOS 原生 Swift 代码可以直接调用 Vision Framework、Accessibility API、CoreGraphics 等系统框架，避免 Tauri 的 objc/cocoa FFI 间接性
- **降低部署成本**: 桌面应用内嵌 server，用户无需单独安装和配置
- **平台最优**: macOS 走 Xcode 原生链 (签名/公证/上架)，Linux/Windows 走 Tauri 成熟生态

## 分发矩阵

```
                    ┌──────────────────────────────────────────────┐
                    │         aleph-server (Rust Binary)           │
                    │  Gateway + Agent + Memory + Control Plane UI │
                    │         ~40-50MB, 全平台统一代码              │
                    └──────────────┬───────────────────────────────┘
                                   │
              ┌────────────────────┼────────────────────┐
              │                    │                    │
    ┌─────────▼──────────┐ ┌──────▼───────┐ ┌─────────▼──────────┐
    │   macOS Native App │ │  Pure Server │ │  Tauri Desktop App │
    │   (Xcode/Swift)    │ │  (单二进制)   │ │  (Linux + Windows) │
    │   .app → .dmg      │ │  + install.sh│ │  .AppImage/.deb    │
    │   内嵌 server       │ │              │ │  .exe (NSIS)       │
    │   UDS IPC          │ │              │ │  内嵌 server        │
    └────────────────────┘ └──────────────┘ └────────────────────┘
```

### 三种分发产物

| 产物 | 平台 | 构建工具 | 包含 | 用户体验 |
|------|------|----------|------|----------|
| **Aleph.app** | macOS | Xcode | server + Swift bridge + WebView | 拖入 Applications，双击即用 |
| **aleph-server** | 全平台 | cargo build | server + 嵌入式 UI | curl \| sh 安装，systemd/launchd 服务 |
| **Aleph Desktop** | Linux/Windows | Tauri | server + Tauri bridge + WebView | 安装器安装，双击即用 |

## 项目结构变更

```
aleph/
├── apps/
│   ├── macos/                  # 新增: Xcode 项目
│   │   ├── Aleph.xcodeproj/
│   │   ├── Aleph/
│   │   │   ├── App/            # AppDelegate, 生命周期
│   │   │   ├── Bridge/         # UDS IPC 客户端/服务端
│   │   │   ├── Desktop/        # 原生能力 (Vision OCR, AX API, 截图)
│   │   │   ├── UI/             # 菜单栏、Halo (NSPanel)、窗口管理
│   │   │   ├── Server/         # aleph-server 进程管理
│   │   │   └── Settings/       # 设置持久化
│   │   ├── Resources/          # aleph-server 二进制 (构建时复制)
│   │   └── Aleph.entitlements  # 权限声明
│   ├── desktop/                # 现有: Tauri (移除 macOS 目标)
│   │   └── src-tauri/
│   └── cli/                    # 现有: CLI 客户端
├── scripts/
│   ├── install.sh              # 新增: 纯 server 一键安装脚本
│   └── build-macos.sh          # 新增: macOS 构建脚本
```

## macOS 原生 App 架构

### 模块职责

| 模块 | 职责 | 对应 Tauri 代码 |
|------|------|----------------|
| **App/** | AppDelegate, 生命周期, CLI 参数解析 | lib.rs, main.rs |
| **Server/** | aleph-server 进程管理 (启动/停止/监控) | 新增 (Tauri 未有) |
| **Bridge/** | UDS JSON-RPC 2.0 服务端, 方法路由 | bridge/mod.rs, protocol.rs |
| **Desktop/** | 原生桌面能力实现 | bridge/action.rs, perception.rs, canvas.rs |
| **UI/** | 菜单栏、Halo 浮窗、设置窗口、快捷键 | tray.rs, shortcuts.rs, commands/ |
| **Settings/** | 设置持久化 | settings.rs |

### Swift 模块详细设计

```
App/
├── AlephApp.swift              # @main SwiftUI App
├── AppDelegate.swift           # NSApplicationDelegate
│   ├── applicationDidFinishLaunching()  → 启动 server + bridge
│   ├── applicationWillTerminate()       → 停止 server
│   └── applicationShouldTerminateAfterLastWindowClosed() → false
└── BridgeModeConfig.swift      # --bridge-mode 参数解析

Server/
├── ServerManager.swift         # aleph-server 进程管理
│   ├── start()                 #   Bundle.main.path(forResource:) 找二进制
│   ├── stop()                  #   SIGTERM → 5s 等待 → SIGKILL
│   ├── monitor()               #   心跳监测, 崩溃自动重启
│   └── isRunning: Bool         #   进程状态检查
└── ServerPaths.swift           # ~/.aleph/ 路径常量

Bridge/
├── BridgeServer.swift          # UDS JSON-RPC 2.0 服务端
│   ├── listen(at: socketPath)  #   绑定 Unix Domain Socket
│   ├── dispatch(method:)       #   路由到对应 handler
│   └── handshake()             #   返回平台能力列表
└── BridgeProtocol.swift        # Codable 请求/响应模型

Desktop/
├── ScreenCapture.swift         # CGWindowListCreateImage
├── OCRService.swift            # VNRecognizeTextRequest
│   ├── recognize(image:)       #   zh-Hans + en-US
│   └── Result: (text, lines)   #   与 Tauri 版输出格式一致
├── AccessibilityService.swift  # AXUIElement API
│   ├── inspectApp(bundleId:)   #   获取应用辅助功能树
│   └── inspectFrontmost()      #   获取前台应用辅助功能树
├── InputAutomation.swift       # CGEvent 键鼠模拟
│   ├── click(x:y:button:)
│   ├── typeText(_:)
│   ├── keyCombo(modifiers:key:)
│   └── scroll(direction:amount:)
├── WindowManager.swift         # CGWindowListCopyWindowInfo
│   ├── listWindows()
│   └── focusWindow(pid:)       #   NSRunningApplication.activate
└── CanvasOverlay.swift         # NSPanel + WKWebView
    ├── show(html:position:)
    ├── hide()
    └── update(patch:)          #   A2UI patch 应用

UI/
├── MenuBarController.swift     # NSStatusItem
│   ├── setupMenu()             #   与 Tauri tray.rs 菜单结构一致
│   └── updateStatus(_:)        #   idle/thinking/acting/error
├── HaloWindow.swift            # NSPanel (non-activating, floating)
│   ├── show()                  #   居中, 距底部 30%
│   ├── hide()
│   └── webView: WKWebView      #   加载 Halo UI
├── SettingsWindow.swift        # NSWindow + WKWebView
│   └── webView: WKWebView      #   加载 Settings UI
└── GlobalShortcuts.swift       # CGEvent tap
    └── register(Cmd+Opt+/)     #   显示 Halo

Settings/
└── SettingsStore.swift         # JSON 文件持久化
    ├── load() -> Settings      #   ~/.config/aleph/settings.json
    └── save(_: Settings)       #   与 Tauri 版格式一致
```

### 与 Tauri 实现的关键差异

| 能力 | Tauri (objc/cocoa FFI) | Swift (原生) | 优势 |
|------|----------------------|-------------|------|
| OCR | ~160 行 objc FFI | `import Vision`, ~40 行 | 代码量减 75%, 类型安全 |
| AX API | ~180 行 C FFI | `import ApplicationServices`, ~60 行 | 原生 API, 更清晰 |
| 截图 | xcap crate | `CGWindowListCreateImage`, ~20 行 | 无第三方依赖 |
| 键鼠 | enigo crate | `CGEvent` API, ~80 行 | 更细粒度控制 |
| 菜单栏 | tauri_plugin_* | `NSStatusItem`, ~50 行 | 原生外观和行为 |
| 浮窗 | Tauri WebView window | `NSPanel` (non-activating) | 真正的浮窗行为 |
| 快捷键 | tauri_plugin_global_shortcut | `CGEvent.tapCreate` | 系统级事件拦截 |

### UI 策略

遵循 R2 (UI 逻辑唯一源) 红线:

- **原生壳 (Swift)**: 菜单栏 (NSStatusItem)、Halo 浮窗 (NSPanel)、窗口管理
- **WebView (WKWebView)**: 加载 Control Plane UI (Leptos/WASM)，与 Tauri 版共享同一套 UI
- Swift 只负责窗口容器和系统集成，不实现业务逻辑 UI

### Server 生命周期管理

```
App 启动
  │
  ├─ 检查 ~/.aleph/bridge.sock 是否已存在
  │   ├─ 存在 → 尝试连接, 确认 server 是否在运行
  │   │   ├─ 在运行 → 复用现有 server (跳过启动)
  │   │   └─ 不在运行 → 删除旧 socket, 继续启动
  │   └─ 不存在 → 继续启动
  │
  ├─ 定位 server 二进制
  │   Bundle.main.path(forResource: "aleph-server", ofType: nil)
  │
  ├─ 启动 Process (Foundation.Process)
  │   arguments: ["--bridge-mode", "--socket", socketPath]
  │   设置 stdout/stderr 管道 (日志)
  │
  ├─ 等待 server 就绪 (轮询 socket 可连接)
  │
  ├─ 启动 Bridge Server (UDS 监听)
  │
  └─ 加载 WebView (http://127.0.0.1:SERVER_PORT)

App 退出
  │
  ├─ 发送 SIGTERM 给 server 进程
  ├─ 等待 5s 优雅关闭
  ├─ 超时则 SIGKILL
  └─ 清理 socket 文件
```

## Tauri 变更: 移除 macOS 目标

### 移除的 macOS 依赖

```toml
# apps/desktop/src-tauri/Cargo.toml — 移除:
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.26"
objc = "0.2"
core-graphics = "0.24"
core-foundation = "0.10"
```

### 移除的 macOS 代码

| 文件 | 移除内容 | 行数 |
|------|---------|------|
| perception.rs | macOS OCR (Vision FFI) | ~160 行 |
| perception.rs | macOS AX API (AXUIElement FFI) | ~180 行 |
| perception.rs | macOS 权限检查 | ~15 行 |
| action.rs | macOS 窗口列表 (CoreGraphics) | ~100 行 |
| action.rs | macOS 应用启动 (`open -b`) | ~20 行 |

### 保留的代码

- UDS IPC 服务端 (Linux 也用 Unix Socket)
- Windows 桌面能力 (WinRT OCR, UI Automation)
- Linux 桌面能力 (wmctrl, 基础截图)
- Canvas overlay, WebView 管理
- Settings 持久化, Tray, 快捷键
- enigo (键鼠) — Windows/Linux 仍需要
- xcap (截图) — Windows/Linux 仍需要

### Tauri 嵌入 Server

Tauri App 需要新增 server 进程管理模块:

```rust
// apps/desktop/src-tauri/src/server_manager.rs (新增)
pub struct ServerManager {
    process: Option<Child>,
    socket_path: PathBuf,
}

impl ServerManager {
    pub fn start(&mut self) -> Result<()> {
        let server_bin = tauri::api::path::resource_dir()
            .join("aleph-server");
        self.process = Some(Command::new(server_bin)
            .args(["--bridge-mode", "--socket", &self.socket_path])
            .spawn()?);
        self.wait_for_ready()?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.process {
            child.kill()?;
        }
        Ok(())
    }
}
```

### Tauri Bundle 配置

```json
{
  "bundle": {
    "targets": ["nsis", "deb", "appimage"],
    "resources": ["binaries/aleph-server"]
  }
}
```

## 纯 Server 分发

### 安装脚本 (install.sh)

```bash
#!/bin/bash
# 1. 检测平台 (uname -s) 和架构 (uname -m)
# 2. 从 GitHub Releases 下载对应二进制
# 3. 安装到 /usr/local/bin/aleph-server
# 4. 创建 ~/.aleph/ 目录结构
# 5. 可选: 配置 systemd/launchd 服务
```

### GitHub Releases 产物矩阵

| 产物 | 平台 | 架构 |
|------|------|------|
| `aleph-server-darwin-aarch64` | macOS | Apple Silicon |
| `aleph-server-darwin-x86_64` | macOS | Intel |
| `aleph-server-linux-x86_64` | Linux | x86_64 |
| `aleph-server-linux-aarch64` | Linux | ARM64 |
| `aleph-server-windows-x86_64.exe` | Windows | x86_64 |
| `Aleph.dmg` | macOS | Universal (.app) |
| `Aleph.AppImage` | Linux | x86_64 |
| `Aleph-Setup.exe` | Windows | x86_64 |

## 构建流水线

### 依赖图

```
cargo build --bin aleph-server --features control-plane --release
              │
              ├─────────────────────────────────────────────┐
              │                                             │
    ┌─────────▼──────────┐                    ┌─────────────▼────────────┐
    │  macOS: Xcode Build │                    │  Linux/Win: Tauri Build  │
    │                      │                    │                          │
    │  1. 复制 server 二进制│                    │  1. 复制 server 二进制    │
    │     → Resources/     │                    │     → resources/binaries/ │
    │  2. xcodebuild       │                    │  2. cargo tauri build    │
    │  3. codesign + 公证  │                    │  3. 签名 (Windows)       │
    │  4. 打包 .dmg        │                    │  4. 打包 .AppImage/.exe  │
    └──────────────────────┘                    └──────────────────────────┘
```

### CI/CD 工作流

**1. server-release.yml — 纯 Server 发布**
- 触发: tag push (v*)
- 矩阵: macOS (aarch64, x86_64), Linux (x86_64, aarch64), Windows
- 产物: aleph-server-{platform}-{arch} 二进制
- 发布: GitHub Releases + install.sh

**2. macos-app.yml — macOS 原生 App 发布**
- 触发: tag push (v*) 或手动
- Runner: macos-latest
- 步骤:
  1. cargo build --bin aleph-server (aarch64 + x86_64)
  2. lipo -create → universal binary
  3. 复制到 Xcode 项目 Resources/
  4. xcodebuild archive -scheme Aleph
  5. xcodebuild -exportArchive → Aleph.app
  6. codesign --deep --force
  7. xcrun notarytool submit (可选)
  8. 打包 DMG
  9. 上传到 GitHub Releases

**3. desktop-app.yml — Tauri Desktop 发布 (Linux/Windows)**
- 触发: tag push (v*) 或手动
- 矩阵: ubuntu-22.04, windows-latest
- 步骤:
  1. cargo build --bin aleph-server --release
  2. 复制 server 二进制到 Tauri resources
  3. cargo tauri build
  4. 上传到 GitHub Releases

### 本地开发

```bash
# macOS App 开发
cd apps/macos && open Aleph.xcodeproj
# 需要先构建 server:
cargo build --bin aleph-server --features control-plane

# macOS 构建完整 App
./scripts/build-macos.sh

# Linux/Windows 开发
cd apps/desktop && pnpm tauri dev

# 纯 Server 开发
cargo run --bin aleph-server --features control-plane
```

## 架构红线遵守

| 红线 | 遵守方式 |
|------|---------|
| R1 大脑与四肢分离 | server 是独立进程, Swift/Tauri 通过 UDS IPC 通信 |
| R2 UI 逻辑唯一源 | WebView 加载 Leptos/WASM UI, 原生壳只做窗口容器 |
| R3 核心轻量化 | 桌面能力在 Swift/Tauri bridge 中实现, 不在 core |
| R4 Interface 禁止业务逻辑 | App 是纯 I/O, 所有智能逻辑在 server |
| R5 菜单栏优先 | NSStatusItem (macOS), 系统托盘 (Linux/Windows) |
| R7 一核多端 | 同一个 aleph-server, 三种壳 |

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| Swift/Tauri 两端 Bridge 协议不同步 | 共享 JSON-RPC 协议定义 (shared/protocol/), 后续可提取协议一致性测试 |
| macOS App 签名/公证复杂 | 先不签名开发, CI/CD 流水线稳定后再接入 |
| Server 嵌入增大 App 体积 | aleph-server ~40-50MB, 对桌面 App 可接受 |
| 两套桌面能力代码维护成本 | macOS 原生 API 更简洁 (代码量约为 Tauri FFI 的 1/3), 且功能已稳定 |

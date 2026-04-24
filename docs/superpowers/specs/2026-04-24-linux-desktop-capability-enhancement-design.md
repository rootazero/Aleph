# Aleph Linux Desktop Capability Enhancement Spec

**Status:** Draft — 待审阅  
**Date:** 2026-04-24  
**Author:** Aleph Team  
**Scope:** `desktop/linux/`, `desktop/shared/`  
**Related:** `2026-03-21-desktop-native-capabilities-design.md`, `2026-04-03-desktop-computer-use-phase1-design.md`, `2026-04-24-codex-inspired-desktop-design.md`

---

## 1. 上下文与动机

### 1.1 当前状态

Aleph 的 Linux 桌面实现目前是一个**78行的空壳**（`desktop/linux/src/lib.rs`），仅实现了 `ScreenCapability`（通过共享的 `NativeScreen`），其余所有能力均返回 `None`：

- `SystemCapability` → `None`
- `PimCapability` → `None`
- `AutomationCapability` → `None`
- `PermissionCapability` → `None`
- `MediaCapability` → `None`
- `EscapeAbort` → `None`

这意味着在 Linux 上，Aleph 只能截图和控制鼠标键盘，**无法发送通知、管理应用、读取系统信息、执行脚本、进行 OCR 等**。

### 1.2 与 Codex 的对比

OpenAI Codex 的 Linux 实现采用**极简路径**：
- 仅实现终端通知（OSC 9 / BEL）
- 睡眠抑制（`systemd-inhibit` / `gnome-session-inhibit`）
- Linux 沙盒（bubblewrap + landlock）
- **不做窗口管理、不做输入自动化、不做系统通知** —— 因为 Codex 是终端 CLI 工具，不是后台 AI 助理

**关键差异**：Aleph 是纯后台运行的 AI 个人助理，需要**完整的桌面控制能力**（看屏幕、控制鼠标、发送通知、管理应用等）。Codex 的路径**不可照搬**。

### 1.3 目标

在不引入 UI 元素（菜单栏、托盘、浮窗）的前提下，为 Linux 平台实现一套**完整、现代、可维护**的桌面能力层，使 Aleph 在 Linux 上的功能接近 macOS 水平。

---

## 2. 非目标（Scope 边界）

| 明确不做 | 原因 |
|----------|------|
| 菜单栏、系统托盘、Halo 浮窗 | Aleph 是纯后台 AI 助理，非桌面 App |
| PIM 能力（Notes/Calendar/Reminders/Contacts）| Linux 无统一 PIM 框架，需依赖特定桌面环境（GNOME/KDE），过于碎片化 |
| Media 能力（Camera/Audio/STT）| Linux 媒体栈复杂（V4L2/PipeWire），且非核心需求；留待后续专项 |
| 完整的 Accessibility API（AX 树）| Linux 辅助功能栈（AT-SPI）极不稳定，且 Wayland 下支持极差 |
| Windows 实现 | 本次 scope 仅限 Linux |
| Swift Bridge / JSON-RPC 进程模型 | macOS 专属架构，Linux 使用纯 Rust 实现 |
| 代码生成工具链 | 手写实现，避免生成代码维护负担 |

---

## 3. 总体架构

### 3.1 设计原则

1. **纯 Rust 实现** —— Linux 不引入 Swift/ObjC bridge，所有能力在 Rust 中直接实现
2. **现代 API 优先** —— 优先使用 D-Bus / xdg-desktop-portal（跨桌面环境通用），保留 X11 fallback
3. **Wayland 兼容** —— 所有新实现必须考虑 Wayland 兼容性，避免硬依赖 X11
4. **零 UI 原则** —— 不创建任何窗口、菜单、托盘图标；所有用户交互通过 LLM 对话完成
5. **清理优先** —— 每实现一个新能力，同步清理旧代码，避免屎山堆积

### 3.2 能力矩阵

| 能力 | macOS 状态 | Linux 当前 | Linux 目标 | 实现方式 |
|------|-----------|-----------|-----------|----------|
| **ScreenCapability** | 完整 | 完整（共享） | 保持 | `xcap` + `enigo` |
| **SystemCapability** | 完整 | `None` | **完整实现** | D-Bus + `sysinfo` + `arboard` |
| **AutomationCapability** | 完整 | `None` | **Shell 脚本** | `std::process::Command` |
| **PermissionCapability** | 完整 | `None` | **基础实现** | `xdg-desktop-portal` |
| **MediaCapability** | 完整 | `None` | `NotImplemented` | 留待后续 |
| **PimCapability** | 完整 | `None` | `None` | 明确不做 |
| **EscapeAbort** | 完整 | `None` | **实现** | `evdev` / `xinput` |
| **OCR** | Vision 框架 | `NotImplemented` | **Tesseract** | `tesseract` CLI |
| **Sleep Inhibit** | IOKit | `None` | **实现** | `systemd-inhibit` |

### 3.3 模块结构

```
desktop/linux/
├── Cargo.toml
└── src/
    ├── lib.rs                    # LinuxPlatform: impl DesktopPlatform
    ├── system/                   # SystemCapability 实现
    │   ├── mod.rs
    │   ├── notification.rs       # D-Bus notify-send / notify-rs
    │   ├── app_management.rs     # .desktop 解析 + 应用启动/关闭
    │   ├── clipboard.rs          # arboard 封装
    │   ├── sysinfo.rs            # sysinfo 封装
    │   └── idle_detection.rs     # xprintidle / Mutter IdleMonitor
    ├── automation.rs             # AutomationCapability: shell/python 脚本
    ├── permission.rs             # PermissionCapability: xdg-desktop-portal
    ├── escape_listener.rs        # EscapeAbort: evdev/xinput
    ├── ocr.rs                    # OCR: tesseract 封装
    └── sleep_inhibitor.rs        # SleepInhibit: systemd-inhibit
```

---

## 4. 详细设计

### 4.1 SystemCapability（系统能力）

#### 4.1.1 通知（`send_notification`）

**方案**：使用 `notify-rust` crate（封装 D-Bus Notification 协议）

- **X11/Wayland 通用**：通过 D-Bus 发送通知，不依赖特定显示服务器
- **Fallback**：如果 D-Bus 不可用，回退到 `notify-send` CLI
- **图标**：使用 Aleph 应用图标（如果可用），否则无图标

```rust
// desktop/linux/src/system/notification.rs
use notify_rust::Notification;

pub fn send_notification(title: &str, body: &str) -> Result<()> {
    Notification::new()
        .summary(title)
        .body(body)
        .appname("Aleph")
        .show()
        .map_err(|e| DesktopError::SystemError(format!("通知发送失败: {e}")))?;
    Ok(())
}
```

#### 4.1.2 应用管理（`launch_app`, `quit_app`, `list_running_apps`）

**问题**：当前 `launch_app` 使用 `xdg-open`，这实际上是打开文件/URL，不是启动应用。

**方案**：
1. **解析 `.desktop` 文件**：扫描 `/usr/share/applications/` 和 `~/.local/share/applications/`
2. **启动应用**：使用 `gtk-launch <desktop-file-id>` 或 `dex <desktop-file>`
3. **关闭应用**：使用 `killall <name>`（比 `pkill -f` 安全）
4. **列出运行中应用**：解析 `/proc/*/status` 或 `ps` 输出

```rust
// desktop/linux/src/system/app_management.rs

/// 从 .desktop 文件 ID 启动应用（如 "firefox.desktop" -> "firefox"）
pub fn launch_app(app_name: &str) -> Result<()> {
    // 尝试 gtk-launch（GNOME/GTK 环境）
    let status = Command::new("gtk-launch")
        .arg(app_name)
        .status()
        .or_else(|_| {
            // Fallback: 直接执行 Exec= 行
            let desktop_file = find_desktop_file(app_name)?;
            let exec_line = parse_exec_line(&desktop_file)?;
            Command::new("sh").arg("-c").arg(exec_line).status()
        })
        .map_err(|e| DesktopError::SystemError(format!("启动应用失败: {e}")))?;
    
    if !status.success() {
        return Err(DesktopError::SystemError("应用启动返回非零退出码".into()));
    }
    Ok(())
}

/// 使用 killall 安全关闭应用
pub fn quit_app(app_name: &str) -> Result<()> {
    let status = Command::new("killall")
        .arg(app_name)
        .status()
        .map_err(|e| DesktopError::SystemError(format!("关闭应用失败: {e}")))?;
    
    if !status.success() {
        return Err(DesktopError::SystemError(format!("未找到运行中的应用: {app_name}")));
    }
    Ok(())
}
```

#### 4.1.3 剪贴板（`clipboard_read`, `clipboard_write`）

**方案**：使用 `arboard` crate（跨平台，自动处理 X11/Wayland）

- **X11**：使用 `xclip` / `xsel` 协议
- **Wayland**：使用 `wl-copy` / `wl-paste` 协议
- **自动检测**：arboard 自动检测当前会话类型

```rust
// desktop/linux/src/system/clipboard.rs
use arboard::Clipboard;

pub fn clipboard_read() -> Result<ClipboardContent> {
    let mut clipboard = Clipboard::new()
        .map_err(|e| DesktopError::SystemError(format!("剪贴板访问失败: {e}")))?;
    
    let text = clipboard.get_text().ok();
    
    Ok(ClipboardContent {
        text,
        has_image: false,  // arboard 支持图片，但先实现文本
        image_base64: None,
    })
}
```

**清理**：当前 `ScreenCapability` 中的 `clipboard_read` / `clipboard_write` 是历史遗留，应标记为 deprecated，引导调用方使用 `SystemCapability`。

#### 4.1.4 系统信息（`system_info`）

**方案**：使用 `sysinfo` crate（已在 Aleph 其他模块中使用）

```rust
// desktop/linux/src/system/sysinfo.rs
use sysinfo::{System, SystemExt};

pub fn system_info() -> Result<SystemInfo> {
    let mut sys = System::new_all();
    sys.refresh_all();
    
    Ok(SystemInfo {
        os_name: "Linux".to_string(),
        os_version: sys.os_version().unwrap_or_default(),
        hostname: sys.host_name().unwrap_or_default(),
        arch: std::env::consts::ARCH.to_string(),
        username: std::env::var("USER").unwrap_or_default(),
    })
}
```

#### 4.1.5 空闲检测（`user_idle_seconds`）

**方案**：多后端 fallback

1. **X11**：`xprintidle` CLI（返回毫秒级空闲时间）
2. **Wayland + GNOME**：D-Bus `org.gnome.Mutter.IdleMonitor`
3. **Wayland + KDE**：D-Bus `org.kde.KWin`（如果可用）
4. **通用**：`logind` D-Bus 接口

```rust
// desktop/linux/src/system/idle_detection.rs

pub fn user_idle_seconds() -> Result<f64> {
    // 优先尝试 xprintidle（X11 / XWayland）
    if let Ok(output) = Command::new("xprintidle").output() {
        let ms = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        return Ok(ms as f64 / 1000.0);
    }
    
    // Fallback: D-Bus IdleMonitor（GNOME/Mutter）
    if let Ok(idle_time) = query_mutter_idle() {
        return Ok(idle_time as f64 / 1000.0);
    }
    
    Err(DesktopError::NotImplemented(
        "空闲检测在当前 Linux 环境不可用".into()
    ))
}
```

### 4.2 AutomationCapability（自动化）

Linux 没有 AppleScript / Shortcuts 等价物，但 AI 助理需要脚本执行能力。

**方案**：实现 `Shell` 和 `Python` 脚本执行

```rust
// desktop/linux/src/automation.rs

#[async_trait]
impl AutomationCapability for LinuxAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String> {
        match language {
            ScriptLanguage::Shell => {
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(source)
                    .output()
                    .await
                    .map_err(|e| DesktopError::AutomationError(format!("脚本执行失败: {e}")))?;
                
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(DesktopError::AutomationError(format!("脚本错误: {stderr}")));
                }
                
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            }
            ScriptLanguage::Python => {
                // 检查 python3 是否可用
                let output = tokio::process::Command::new("python3")
                    .arg("-c")
                    .arg(source)
                    .output()
                    .await
                    .map_err(|e| DesktopError::AutomationError(format!("Python 执行失败: {e}")))?;
                
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            }
            _ => Err(DesktopError::NotImplemented(
                "该脚本语言在 Linux 上不可用".into()
            )),
        }
    }
    
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        // Linux 无 Shortcuts 等价物
        Ok(vec![])
    }
    
    async fn run_shortcut(&self, _name: &str, _input: Option<&str>) -> Result<String> {
        Err(DesktopError::NotImplemented(
            "Shortcuts 在 Linux 上不可用".into()
        ))
    }
}
```

### 4.3 PermissionCapability（权限管理）

Linux 没有 macOS TCC 那样的统一权限框架，但现代 Linux 有 `xdg-desktop-portal`。

**方案**：
1. **屏幕录制权限**：`xdg-desktop-portal` 的 `ScreenCast` 接口（用户首次截图时由系统弹窗授权）
2. **其他权限**：Linux 传统上无显式权限模型，返回 `PermissionStatus::Granted` 或 `Unknown`

```rust
// desktop/linux/src/permission.rs

#[async_trait]
impl PermissionCapability for LinuxPermission {
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo> {
        match permission {
            TccPermission::ScreenRecording => {
                // Linux 无持久权限状态，假设需要时由 portal 处理
                Ok(PermissionInfo {
                    permission,
                    status: PermissionStatus::Unknown,
                    can_request: true,
                })
            }
            _ => Ok(PermissionInfo {
                permission,
                status: PermissionStatus::Unknown,
                can_request: false,
            }),
        }
    }
    
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo> {
        // Linux 权限通常由操作触发（如 portal 弹窗），非显式请求
        self.check(permission).await
    }
}
```

### 4.4 EscapeAbort（紧急停止）

**方案**：使用 `evdev` crate 读取键盘事件，监听 Escape 键

- **X11**：也可用 `xinput` 监听
- **Wayland**：必须用 `evdev`（直接读取 /dev/input/event*）
- **权限**：需要用户属于 `input` 组，或以 root 运行

```rust
// desktop/linux/src/escape_listener.rs

pub struct LinuxEscapeListener {
    aborted: AtomicBool,
}

impl EscapeAbort for LinuxEscapeListener {
    fn start(&self) -> Result<()> {
        // 在后台线程启动 evdev 监听
        std::thread::spawn(|| {
            // 打开所有键盘设备，监听 KEY_ESC
            // 按下 Escape 时设置 aborted = true
        });
        Ok(())
    }
    
    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }
    
    fn reset(&self) {
        self.aborted.store(false, Ordering::Release);
    }
}
```

### 4.5 OCR（光学字符识别）

Linux 无原生 OCR API，但 `tesseract` 是行业标准。

**方案**：
1. **依赖**：系统安装 `tesseract-ocr` 包
2. **调用**：`tesseract <image> stdout -l chi_sim+eng`
3. **输出解析**：解析 tesseract 的 stdout 和 bounding box 数据

```rust
// desktop/linux/src/ocr.rs

pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    // 写入临时 PNG 文件
    let tmp_path = write_temp_png(png_bytes)?;
    
    // 调用 tesseract
    let output = Command::new("tesseract")
        .arg(&tmp_path)
        .arg("stdout")
        .arg("-l")
        .arg("chi_sim+eng")  // 中文简体 + 英文
        .output()
        .map_err(|e| DesktopError::OcrFailed(format!("Tesseract 调用失败: {e}")))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::OcrFailed(format!("Tesseract 错误: {stderr}")));
    }
    
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    
    // TODO: 解析 bounding box（使用 tesseract 的 tsv 输出格式）
    
    Ok(OcrResult {
        full_text: text,
        lines: vec![],  // 先实现文本，后加 bounding box
    })
}
```

### 4.6 Sleep Inhibitor（睡眠抑制）

参考 Codex 的实现，使用 `systemd-inhibit` 或 `gnome-session-inhibit`。

```rust
// desktop/linux/src/sleep_inhibitor.rs

pub struct LinuxSleepInhibitor {
    child: Option<Child>,
}

impl LinuxSleepInhibitor {
    pub fn acquire(&mut self) {
        // 尝试 systemd-inhibit
        if let Ok(child) = spawn_systemd_inhibit() {
            self.child = Some(child);
            return;
        }
        
        // Fallback: gnome-session-inhibit
        if let Ok(child) = spawn_gnome_inhibit() {
            self.child = Some(child);
            return;
        }
        
        tracing::warn!("无可用睡眠抑制后端");
    }
    
    pub fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LinuxSleepInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}
```

---

## 5. 依赖规划

### 5.1 新增依赖

```toml
# desktop/linux/Cargo.toml
[dependencies]
aleph-desktop = { path = "../shared" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt", "process"] }

# 系统通知
notify-rust = "4"

# 剪贴板（X11/Wayland 自动检测）
arboard = "3"

# 系统信息
sysinfo = "0.30"

# D-Bus 客户端（用于 IdleMonitor、portal）
zbus = "4"

# 键盘事件监听（Escape 键）
evdev = "0.12"

# 日志
tracing = "0.1"
```

### 5.2 系统依赖

运行时需要以下系统包：

```bash
# Debian/Ubuntu
sudo apt install libnotify-bin xprintidle tesseract-ocr tesseract-ocr-chi-sim

# Fedora
sudo dnf install libnotify xprintidle tesseract tesseract-langpack-chi_sim

# Arch
sudo pacman -S libnotify xprintidle tesseract tesseract-data-chi_sim
```

---

## 6. 实施阶段

### Phase 1: 基础架构（Week 1）

**目标**：搭建 `desktop/linux/src/` 目录结构，实现 `LinuxPlatform` 的完整 trait 分发。

- [ ] 创建 `desktop/linux/src/system/` 目录
- [ ] 创建 `LinuxSystem` struct，实现 `SystemCapability`
- [ ] 实现 `send_notification`（`notify-rust`）
- [ ] 实现 `system_info`（`sysinfo`）
- [ ] 更新 `LinuxPlatform`：所有 capability 返回 `Some` 而非 `None`

**验证**：
- `cargo check -p aleph-desktop-linux` 通过
- `cargo test -p aleph-desktop-linux` 通过

### Phase 2: 应用管理与剪贴板（Week 1-2）

- [ ] 实现 `launch_app` / `quit_app` / `list_running_apps`
- [ ] 实现 `.desktop` 文件解析器
- [ ] 实现 `clipboard_read` / `clipboard_write`（`arboard`）
- [ ] 实现 `user_idle_seconds`

**验证**：
- `system` 工具在 Linux 上可用
- 剪贴板读写测试通过

### Phase 3: 自动化与权限（Week 2）

- [ ] 实现 `LinuxAutomation`（Shell/Python 脚本）
- [ ] 实现 `LinuxPermission`（基础 portal 支持）
- [ ] 更新 `builtin_tools/automation_tool.rs` 处理 Linux

**验证**：
- `automation` 工具可执行 shell 脚本
- 权限检查不 panic

### Phase 4: OCR 与 Escape（Week 2-3）

- [ ] 实现 `LinuxOcr`（tesseract 封装）
- [ ] 实现 `LinuxEscapeListener`（evdev）
- [ ] 更新 `perception/mod.rs`：Linux 走 tesseract 而非 `NotImplemented`

**验证**：
- OCR 在 Linux 上返回文本
- Escape 键监听可用

### Phase 5: 睡眠抑制与清理（Week 3）

- [ ] 实现 `LinuxSleepInhibitor`
- [ ] 集成到 agent loop（参考 macOS）
- [ ] **清理旧代码**：
  - [ ] 删除 `ScreenCapability` 中的 `clipboard_read` / `clipboard_write`（已迁移到 `SystemCapability`）
  - [ ] 删除 `action/input.rs` 中的 Linux clipboard 代码（重复）
  - [ ] 更新文档

**验证**：
- `cargo udeps` 无未使用依赖
- `just clippy` 零警告
- 所有测试通过

---

## 7. 测试策略

### 7.1 单元测试

```rust
// desktop/linux/src/system/tests.rs

#[test]
fn test_parse_desktop_file() {
    let content = r#"[Desktop Entry]
Name=Firefox
Exec=/usr/bin/firefox %u
Type=Application
"#;
    let entry = parse_desktop_file(content).unwrap();
    assert_eq!(entry.name, "Firefox");
    assert_eq!(entry.exec, "/usr/bin/firefox %u");
}

#[test]
fn test_system_info() {
    let info = system_info().unwrap();
    assert_eq!(info.os_name, "Linux");
    assert!(!info.hostname.is_empty());
}
```

### 7.2 集成测试

```rust
// desktop/linux/tests/integration.rs

#[tokio::test]
#[ignore = "需要桌面环境"]
async fn test_notification() {
    let system = LinuxSystem::new();
    system.send_notification("Test", "Hello from Aleph").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 tesseract"]
async fn test_ocr() {
    let ocr = LinuxOcr::new();
    // 使用测试图片
}
```

### 7.3 CI 测试

- **GitHub Actions**：使用 `ubuntu-latest` 运行单元测试
- **集成测试**：标记 `#[ignore]`，仅在手动测试时运行

---

## 8. 清理计划

### 8.1 删除的代码

| 代码 | 位置 | 原因 |
|------|------|------|
| `clipboard_read` in `ScreenCapability` | `desktop/shared/src/traits/screen.rs` | 已迁移到 `SystemCapability` |
| `clipboard_write` in `ScreenCapability` | `desktop/shared/src/traits/screen.rs` | 已迁移到 `SystemCapability` |
| Linux clipboard in `action/input.rs` | `desktop/shared/src/action/input.rs` | 重复实现，使用 `arboard` 替代 |
| `quit_app` in `action/app_launch.rs` | `desktop/shared/src/action/app_launch.rs` | 迁移到 `LinuxSystem` |

### 8.2 标记 Deprecated

```rust
// desktop/shared/src/traits/screen.rs

/// 已弃用：请使用 SystemCapability::clipboard_read
#[deprecated(since = "2026.04.24", note = "Use SystemCapability::clipboard_read instead")]
async fn clipboard_read(&self) -> Result<String>;
```

### 8.3 文档更新

- [ ] 更新 `docs/reference/ARCHITECTURE.md`：添加 Linux 桌面能力小节
- [ ] 更新 `docs/reference/DESKTOP_BRIDGE.md`：说明 Linux 不使用 Swift Bridge
- [ ] 更新 `README.md`：Linux 功能清单

---

## 9. 风险与缓解

| 风险 | 可能性 | 影响 | 缓解 |
|------|--------|------|------|
| Wayland 兼容性差 | 中 | 高 | 优先使用 D-Bus/portal，保留 X11 fallback |
| 依赖系统包未安装 | 高 | 中 | 运行时检测，返回友好错误信息 |
| evdev 权限不足 | 中 | 中 | 检测权限，返回引导信息（加入 input 组） |
| tesseract 中文识别差 | 低 | 中 | 先实现英文，中文作为 enhancement |
| 与 macOS 实现不同步 | 中 | 低 | 保持 trait 契约一致，内部实现自由 |

---

## 10. 成功标准

- [ ] `LinuxPlatform` 所有 capability 返回 `Some`（除 `PimCapability` / `MediaCapability`）
- [ ] `system` 工具在 Linux 上可用（通知、应用管理、剪贴板、系统信息）
- [ ] `automation` 工具可执行 shell/python 脚本
- [ ] OCR 在 Linux 上可用（tesseract）
- [ ] Escape 键监听可用
- [ ] 睡眠抑制可用
- [ ] `cargo udeps` 无未使用依赖
- [ ] `just clippy` 零警告
- [ ] 所有单元测试通过
- [ ] 旧代码清理完成，无重复实现

---

## 11. 附录：与现有 Spec 的关系

| 现有 Spec | 关系 |
|-----------|------|
| `2026-03-21-desktop-native-capabilities-design.md` | 原始 trait 设计，本 spec 遵循其架构 |
| `2026-04-03-desktop-computer-use-phase1-design.md` | Phase 1 已实现 ScreenCapability 扩展，本 spec 在其基础上填充 Linux |
| `2026-04-24-codex-inspired-desktop-design.md` | macOS 专属 Swift Bridge 设计，**Linux 不采用** |

---

## 12. Open Questions

1. **是否需要在 Linux 上实现 `PimCapability`？**
   - 建议：不做。Linux PIM 栈过于碎片化（GNOME Evolution、KDE KOrganizer、Thunderbird 等），且非核心需求。

2. **Wayland 下窗口管理（`window_list`、`focus_window`）是否可行？**
   - Wayland 安全模型禁止客户端枚举/操作其他窗口。当前 `wmctrl` 在 XWayland 下可能工作，但原生 Wayland 不支持。
   - 建议：保持现有 `wmctrl` 实现（X11/XWayland），Wayland 下返回 `NotImplemented` 并附带说明。

3. **是否需要 Flatpak/Snap 支持？**
   - 建议：Phase 1 不做。Flatpak 需要 portal 权限，可在后续 enhancement 中支持。

---

*本 spec 待审阅通过后，将拆分为具体实施计划。*

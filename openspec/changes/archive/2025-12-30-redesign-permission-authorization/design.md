# Design: Permission Authorization Redesign

## Overview

本设计文档阐述 Aleph 权限授权系统的重新设计架构，旨在彻底解决 "授权界面闪退" 和 "无限重启循环" 问题。设计采用 **三层防护架构**:

1. **Swift UI Layer** - 被动监听 + 瀑布流引导
2. **Rust Core Layer** - Panic 防护 + 权限预检查
3. **System Integration Layer** - 深链接 + macOS 原生重启提示

## Architecture

### Layer 1: Swift UI Layer (Permission Monitoring & Guidance)

#### PermissionManager (New)

**职责**: 被动监听权限状态变化，绝不主动触发应用重启

**核心设计**:
```swift
class PermissionManager: ObservableObject {
    @Published var accessibilityGranted: Bool = false
    @Published var inputMonitoringGranted: Bool = false

    private var statusCheckTimer: Timer?

    // 核心方法: 每秒轮询，仅更新状态
    private func checkPermissions() {
        let axStatus = AXIsProcessTrusted()
        let inputStatus = checkInputMonitoringViaHID()

        // ✅ 关键: 仅更新 @Published 属性，绝不调用 exit/terminate
        DispatchQueue.main.async {
            if self.accessibilityGranted != axStatus {
                print("Accessibility status changed: \(axStatus)")
                self.accessibilityGranted = axStatus
                // ❌ 移除: restartApp() / NSApp.terminate()
            }

            if self.inputMonitoringGranted != inputStatus {
                print("Input Monitoring status changed: \(inputStatus)")
                self.inputMonitoringGranted = inputStatus
                // ❌ 移除: restartApp() / NSApp.terminate()
            }
        }
    }

    // 新增: 使用 IOHIDManager 检测输入监控权限（更准确）
    private func checkInputMonitoringViaHID() -> Bool {
        guard let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone)) else {
            return false
        }

        let deviceMatching: [String: Any] = [
            kIOHIDDeviceUsagePageKey: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey: kHIDUsage_GD_Keyboard
        ]
        IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)

        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        if result == kIOReturnSuccess {
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            return true
        }
        return false
    }
}
```

**与现有 PermissionStatusMonitor 的区别**:

| 特性 | PermissionStatusMonitor (旧) | PermissionManager (新) |
|------|----------------------------|----------------------|
| Debounce 机制 | ✅ 3次连续读取防抖 | ❌ 移除（Apple API 已稳定） |
| 自动重启 | ❌ 检测到 Accessibility 授权时重启 | ✅ 绝不主动重启 |
| Input Monitoring 检测 | `CGPreflightListenEventAccess()` | `IOHIDManager` (更底层、更准确) |
| 初始化延迟 | ❌ 复杂的 "初始化阶段" 逻辑 | ✅ 简单的 0.3s 延迟后直接检查 |

#### PermissionGateView (Redesigned)

**职责**: 瀑布流权限引导，用户手动控制重启时机

**核心设计**:
```swift
struct PermissionGateView: View {
    @StateObject var manager = PermissionManager()
    @State private var currentStep: PermissionGateStep = .accessibility

    var body: some View {
        VStack {
            // Step 1: Accessibility
            PermissionRow(
                title: "辅助功能 (Accessibility)",
                isGranted: manager.accessibilityGranted,
                isEnabled: true,  // Step 1 总是可点击
                action: { openSystemSettings(.accessibility) }
            )

            // Step 2: Input Monitoring
            PermissionRow(
                title: "输入监控 (Input Monitoring)",
                isGranted: manager.inputMonitoringGranted,
                isEnabled: manager.accessibilityGranted,  // ✅ 依赖 Step 1 完成
                action: { openSystemSettings(.inputMonitoring) }
            )

            // ✅ 两个权限都授予后，显示 "进入 Aleph" 按钮
            if manager.accessibilityGranted && manager.inputMonitoringGranted {
                Button("进入 Aleph") {
                    // 用户主动点击后才重启
                    restartApp()
                }
            }
        }
        .onAppear {
            checkInitialPermissions()  // ✅ 移除复杂初始化逻辑
            manager.startMonitoring()  // ✅ 启动被动监听
        }
    }

    private func checkInitialPermissions() {
        // ✅ 简化: 延迟 0.3s 后直接检查，无需复杂的 "初始化阶段" 判断
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            // 如果已有权限，自动跳过相应步骤
            if manager.accessibilityGranted && !manager.inputMonitoringGranted {
                currentStep = .inputMonitoring
            }
            if manager.accessibilityGranted && manager.inputMonitoringGranted {
                onAllPermissionsGranted()
            }
        }
    }

    // ✅ 用户主动触发重启（而非自动检测）
    private func restartApp() {
        let url = URL(fileURLWithPath: Bundle.main.bundlePath)
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true

        NSWorkspace.shared.openApplication(at: url, configuration: config) { _, _ in
            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
        }
    }
}
```

**关键改进**:
- ❌ 移除: 自动检测权限变化后的重启逻辑
- ✅ 新增: 用户主动点击 "进入 Aleph" 按钮才重启
- ✅ 新增: Step 2 仅在 Step 1 完成后才启用（`isEnabled: manager.accessibilityGranted`）

#### PermissionChecker (Enhanced)

**职责**: 提供准确的权限状态检查和系统设置深链接

**新增方法**:
```swift
class PermissionChecker {
    // ✅ 使用 IOHIDManager 检测输入监控权限（比 IOHIDRequestAccess 更准确）
    static func hasInputMonitoringViaHID() -> Bool {
        guard let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone)) else {
            return false
        }

        let deviceMatching: [String: Any] = [
            kIOHIDDeviceUsagePageKey: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey: kHIDUsage_GD_Keyboard
        ]
        IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)

        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        if result == kIOReturnSuccess {
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            return true
        }
        return false
    }

    // ✅ 深链接到系统设置特定权限页面
    static func openSystemSettings(for permissionType: PermissionType) {
        let urlString: String
        switch permissionType {
        case .accessibility:
            urlString = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        case .inputMonitoring:
            urlString = "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }

        if let url = URL(string: urlString) {
            NSWorkspace.shared.open(url)
        }
    }
}
```

### Layer 2: Rust Core Layer (Panic Protection & Permission Pre-Check)

#### rdev_listener.rs (Panic Protection)

**职责**: 防止 `rdev::listen()` 的 panic 导致整个应用崩溃

**核心设计**:
```rust
use std::panic;

pub fn start_rdev_listener(callback: impl Fn(Event) + Send + 'static) -> Result<(), HotkeyError> {
    // ✅ 使用 catch_unwind 捕获 rdev::listen() 的 panic
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        rdev::listen(move |event| {
            callback(event);
        })
    }));

    match result {
        Ok(Ok(())) => {
            log::info!("rdev listener stopped gracefully");
            Ok(())
        }
        Ok(Err(e)) => {
            log::error!("rdev listen error: {:?}", e);
            Err(HotkeyError::ListenFailed(format!("{:?}", e)))
        }
        Err(panic_payload) => {
            // ✅ 捕获 panic，转换为错误日志
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };

            log::error!("❌ rdev listener panicked: {}", panic_msg);
            log::error!("This usually means Input Monitoring permission is not granted.");
            log::error!("Please grant permission in: System Settings > Privacy & Security > Input Monitoring");

            Err(HotkeyError::PermissionDenied(
                "Input Monitoring permission required. rdev::listen() panicked.".to_string()
            ))
        }
    }
}
```

**错误处理流程**:
```
rdev::listen() panic
    ↓
catch_unwind() 捕获
    ↓
记录详细错误日志（告知用户授予权限）
    ↓
返回 Err(HotkeyError::PermissionDenied)
    ↓
应用继续运行（不崩溃）
```

#### core.rs (Permission Pre-Check)

**职责**: 在调用 `rdev::listen()` 前检查权限，避免触发 panic

**核心设计**:
```rust
impl AlephCore {
    pub fn start_listening(&self) -> Result<(), AlephError> {
        // ✅ 权限预检查（通过 Swift 层传递权限状态）
        // 注意: Rust 层无法直接调用 macOS API 检查权限，需要 Swift 层提供
        if !self.has_input_monitoring_permission {
            let error_msg = "Input Monitoring permission not granted. Cannot start hotkey listener.";
            log::warn!("{}", error_msg);

            // ✅ 通过 UniFFI 回调通知 Swift 层
            self.event_handler.on_error(error_msg.to_string());

            return Err(AlephError::PermissionDenied(error_msg.to_string()));
        }

        // ✅ 权限检查通过，启动 rdev 监听器
        match start_rdev_listener(|event| {
            // 处理热键事件...
        }) {
            Ok(()) => {
                log::info!("Hotkey listener started successfully");
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to start hotkey listener: {:?}", e);
                self.event_handler.on_error(format!("Hotkey listener error: {:?}", e));
                Err(AlephError::HotkeyError(e))
            }
        }
    }
}
```

**权限状态传递**:
```
Swift: PermissionChecker.hasInputMonitoringViaHID()
    ↓
Swift → Rust (UniFFI): core.setInputMonitoringPermission(true/false)
    ↓
Rust: self.has_input_monitoring_permission = true/false
    ↓
Rust: start_listening() 前检查 has_input_monitoring_permission
```

### Layer 3: System Integration Layer

#### Deep Links to System Settings

**Accessibility 权限页面**:
```
x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility
```

**Input Monitoring 权限页面**:
```
x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent
```

#### macOS System Restart Prompt

当用户在系统设置中授予 Input Monitoring 权限时，macOS 会自动弹出提示:
```
"Aleph needs to be restarted to use Input Monitoring permission."
[Quit Now] [Quit Later]
```

**设计决策**: 应用 **不应该** 尝试拦截或替代这个系统提示，而是让 macOS 系统处理重启流程。

## Key Design Decisions

### Decision 1: 移除 Accessibility 权限的自动重启逻辑

**理由**:
- macOS Accessibility 权限是 **实时生效** 的，无需重启应用
- Apple 文档和实践证明: `AXIsProcessTrusted()` 授权后立即可用
- 自动重启会导致不必要的用户体验中断

**结论**: 检测到 Accessibility 授权后，仅更新 UI 状态，不调用 `exit()` 或 `terminate()`

### Decision 2: Input Monitoring 权限由用户手动控制重启

**理由**:
- macOS 系统会在授予 Input Monitoring 权限时自动弹窗提示用户重启
- 应用主动重启可能与系统弹窗冲突，导致 UX 混乱
- 用户可能希望在授权后继续当前工作，稍后再重启

**结论**: 显示 "进入 Aleph" 按钮，由用户主动点击触发重启（可选操作）

### Decision 3: 使用 IOHIDManager 检测输入监控权限

**理由**:
- `IOHIDRequestAccess()` 仅检查 TCC 数据库权限，但不保证实际可用性
- `IOHIDManager` 尝试打开键盘设备流，更准确反映 "应用是否真的能读到数据"
- 对于基于 `rdev` 的底层输入监听，HID 检测更匹配实际需求

**结论**: `PermissionChecker` 新增 `hasInputMonitoringViaHID()` 方法作为主要检测手段

### Decision 4: Rust 核心使用 catch_unwind 防护 rdev panic

**理由**:
- `rdev::listen()` 在权限不足时会直接 panic，而非返回错误
- Panic 会导致整个进程崩溃，无法优雅降级
- UniFFI 无法捕获跨 FFI 边界的 panic

**结论**: 在 `rdev_listener.rs` 中使用 `std::panic::catch_unwind()` 包裹 `rdev::listen()`，转换 panic 为 `Result<(), HotkeyError>`

### Decision 5: 移除 Debounce 机制

**理由**:
- 现有 3 次连续读取防抖机制导致延迟 3-6+ 秒
- Apple API (`AXIsProcessTrusted`, `IOHIDRequestAccess`) 已足够稳定
- Debounce 无法可靠区分 "新授予" vs "缓存延迟"

**结论**: `PermissionManager` 每秒直接检查一次，无需防抖逻辑

## Data Flow

### Startup Flow (No Permissions)

```
App Launch
    ↓
AppDelegate.applicationDidFinishLaunching()
    ↓
PermissionChecker.hasAllRequiredPermissions() → false
    ↓
Show PermissionGateView (Step 1: Accessibility)
    ↓
PermissionManager.startMonitoring() (每 1s 轮询)
    ↓
User clicks "Open System Settings"
    ↓
NSWorkspace.open(x-apple.systempreferences:...Privacy_Accessibility)
    ↓
User grants Accessibility permission in System Settings
    ↓
PermissionManager detects: AXIsProcessTrusted() → true
    ↓
Update @Published var accessibilityGranted = true
    ↓
PermissionGateView auto-progress to Step 2
    ↓
User clicks "Open System Settings" for Input Monitoring
    ↓
User grants Input Monitoring permission
    ↓
PermissionManager detects: IOHIDManagerOpen() → success
    ↓
Update @Published var inputMonitoringGranted = true
    ↓
PermissionGateView shows "进入 Aleph" button
    ↓
User clicks "进入 Aleph"
    ↓
restartApp() (NSWorkspace.openApplication + NSApp.terminate)
    ↓
App relaunches with all permissions
    ↓
AppDelegate initializes AlephCore
    ↓
Normal operation
```

### Rust Core Protection Flow (Permission Missing)

```
AppDelegate calls core.start_listening()
    ↓
Rust: Check has_input_monitoring_permission
    ↓ (if false)
Rust: Return Err(PermissionDenied)
    ↓
Rust: event_handler.on_error("Input Monitoring permission required")
    ↓
Swift: EventHandler receives on_error() callback
    ↓
Swift: Show alert or update UI status
    ↓
Core NOT initialized, app remains functional (degraded mode)
```

### Rust Core Protection Flow (Permission Granted but rdev panics)

```
AppDelegate calls core.start_listening()
    ↓
Rust: Check has_input_monitoring_permission → true
    ↓
Rust: Call start_rdev_listener()
    ↓
Rust: rdev::listen() panics (unexpected error)
    ↓
Rust: catch_unwind() catches panic
    ↓
Rust: Log detailed error message
    ↓
Rust: Return Err(PermissionDenied("rdev::listen() panicked"))
    ↓
Rust: event_handler.on_error("Hotkey listener crashed")
    ↓
Swift: EventHandler receives on_error() callback
    ↓
Swift: Show alert or retry prompt
    ↓
App remains running (not crashed)
```

## Trade-offs

### Trade-off 1: 用户体验 vs 自动化

**选择**: 用户手动控制重启时机（点击按钮）而非自动重启

**优势**:
- 用户可以完成当前工作后再重启
- 避免与 macOS 系统重启提示冲突
- 更符合 macOS Human Interface Guidelines

**劣势**:
- 需要用户额外操作（多一次点击）
- 如果用户忽略提示，可能导致功能不完整

**结论**: 优势大于劣势，UX 流畅性优先

### Trade-off 2: 权限检测准确性 vs API 稳定性

**选择**: 使用 `IOHIDManager` 而非 `IOHIDRequestAccess`

**优势**:
- 更准确反映实际可用性（能否真的读到键盘数据）
- 与 `rdev` 底层实现更匹配

**劣势**:
- `IOHIDManager` API 更底层，可能在未来 macOS 版本中变化
- 需要额外引入 `IOKit.framework`

**结论**: 准确性优先，但需做好 API 兼容性测试

### Trade-off 3: Panic 防护 vs 性能开销

**选择**: 使用 `catch_unwind` 包裹 `rdev::listen()`

**优势**:
- 彻底防止 panic 导致的应用崩溃
- 提供详细的错误日志和用户指引

**劣势**:
- `catch_unwind` 有轻微性能开销
- 某些类型的 panic 可能仍无法捕获（如栈溢出）

**结论**: 可靠性优先，性能开销可忽略（仅在启动时调用）

## Testing Strategy

### Unit Tests

**Swift 层**:
- `PermissionManagerTests.swift`:
  - 测试权限状态变化时的回调触发
  - 测试 Timer 轮询逻辑
  - 测试 IOHIDManager 检测逻辑

- `PermissionGateViewTests.swift`:
  - 测试瀑布流步骤切换逻辑
  - 测试 "进入 Aleph" 按钮显示条件
  - 测试权限已授予时的自动跳过逻辑

**Rust 层**:
- `hotkey_tests.rs`:
  - 测试 `catch_unwind` 是否成功捕获 panic
  - 测试权限不足时的预检查逻辑
  - 测试错误日志的正确性

### Integration Tests

- 启动应用（无权限）→ 显示 PermissionGateView
- 授予 Accessibility 权限 → UI 自动切换到 Step 2
- 授予 Input Monitoring 权限 → 显示 "进入 Aleph" 按钮
- 点击 "进入 Aleph" → 应用重启
- 重启后（有权限）→ 跳过 PermissionGateView，初始化 Core

### Manual Testing Checklist

- [ ] Debug 环境（DerivedData 路径）下权限授权流程
- [ ] Release 环境下权限授权流程
- [ ] 已有权限时启动应用（无闪退）
- [ ] 无权限时启动应用（显示 PermissionGateView）
- [ ] Accessibility 授权后不自动重启
- [ ] Input Monitoring 授权后显示重启按钮
- [ ] 用户忽略重启提示，macOS 系统弹窗是否正常
- [ ] Rust Core 权限不足时不崩溃

## Future Enhancements

1. **运行时权限撤销检测**:
   - 监听用户在应用运行时撤销权限
   - 显示提示并禁用相关功能

2. **权限降级模式**:
   - 无 Input Monitoring 时提供 "仅剪贴板" 模式
   - 允许用户在有限功能下使用应用

3. **后台权限监控服务**:
   - 持续监听权限变化（而非仅启动时检查）
   - 自动恢复功能当权限重新授予

4. **自动化权限重新请求**:
   - 当检测到权限撤销时，自动弹出系统授权对话框
   - 需要研究 macOS 系统对话框触发机制

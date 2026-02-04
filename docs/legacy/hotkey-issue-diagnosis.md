# Aleph 热键无法工作问题诊断与解决方案

## 问题现象

用户反馈热键（Command + `）无法正常工作，控制台日志显示：

```
[Aleph] Accessibility permission not granted, requesting...
[EventHandler] Escape key monitor installed
[Memory] Warning: No tokio runtime, skipping background cleanup task
[Aleph] AlephCore initialized
[EventHandler] State changed: listening
[Aleph] Hotkey listening started (⌘~)
[EventHandler] VoiceOver announcement: Listening for input
AFIsDeviceGreymatterEligible Missing entitlements for os_eligibility lookup
cannot open file at line 51043 of [f0ca7bba1c]
os_unix.c:51043: (2) open(/private/var/db/DetachedSignatures) - No such file or directory
[EventHandler] State changed: idle
[Aleph] Core stopped successfully
```

## 根本原因分析

### 1. 权限检查与热键监听分离

**问题**：
- Swift 层的 `checkAccessibilityPermission()` 使用 `AXIsProcessTrusted()` 检查权限
- 但这个检查只用于 **窗口标题捕获**（记忆功能），而不用于热键监听
- Rust 层的 `rdev::listen()` **没有权限检查**，直接启动监听
- 即使 Swift 检测到权限未授予，Rust 仍会尝试启动（静默失败）

**代码位置**：
- `AppDelegate.swift:229-240` - Swift 权限检查
- `Aleph/core/src/hotkey/rdev_listener.rs:65-100` - Rust 热键监听启动

### 2. rdev 的权限要求

**macOS 系统限制**：
- `rdev::listen()` 需要 **辅助功能权限** 才能监听全局键盘事件
- 没有权限时，`rdev::listen()` 不会报错，但无法捕获任何事件
- 这解释了为什么日志显示 "Hotkey listening started" 但实际不工作

### 3. Info.plist 缺少权限说明

**缺失的配置**：
- 当前只有 `NSAppleEventsUsageDescription`（用于键盘模拟）
- **缺少** `NSAccessibilityUsageDescription`（用于辅助功能权限说明）
- 系统可能无法正确提示用户授权

## 解决方案

### 已完成：统一权限提示为软件内弹窗

**提交**: `a005eda` - feat(ui): 统一权限提示为软件内弹窗

**修改内容**：
1. 创建 `PermissionPromptView` - 统一的权限提示 SwiftUI 组件
2. 在 `HaloState` 添加 `.permissionRequired(type:)` 状态
3. 修改 `HaloView` 支持显示权限提示
4. 修改 `EventHandler` 添加 `showPermissionPrompt(type:)` 方法
5. 修改 `AppDelegate` 使用新的权限提示替代 NSAlert

**效果**：
- ✅ 权限提示现在显示在屏幕中央（480x450 尺寸）
- ✅ 提供清晰的授权步骤说明
- ✅ 与 Halo 系统保持一致的动画效果
- ✅ 支持「打开系统设置」和「稍后设置」操作

### 待完成：在 Rust 层添加权限检查

**需要修改的文件**：

1. **project.yml** - 添加 NSAccessibilityUsageDescription
```yaml
info:
  properties:
    NSAccessibilityUsageDescription: "Aleph needs Accessibility permission to detect global hotkeys and capture window context for memory features."
```

2. **Aleph/core/src/hotkey/rdev_listener.rs** - 在 start_listening 前检查权限

**实现方案 A**（推荐）：
```rust
// 在 Swift 层检查，如果没有权限则显示提示，不启动 Rust 监听
// AppDelegate.swift
private func initializeRustCore() {
    // 检查权限
    if !ContextCapture.hasAccessibilityPermission() {
        print("[Aleph] Accessibility permission required for hotkey listening")
        eventHandler?.showPermissionPrompt(type: .accessibility)
        return // 不初始化 Rust core
    }

    // 有权限，正常初始化
    do {
        core = try AlephCore(handler: eventHandler!)
        try core?.startListening()
    } catch {
        // 错误处理
    }
}
```

**实现方案 B**：
```rust
// 在 Rust 层检查权限（需要通过 UniFFI 调用 Swift）
// 可能更复杂，不推荐
```

### 待完成：添加权限重新检查机制

**问题**：
- 用户授权后，需要重启应用才能生效
- 应该提供「重新检查权限」功能

**实现方案**：
1. 在权限提示中添加「已授权，重新检查」按钮
2. 点击后调用 `AppDelegate.initializeRustCore()`
3. 如果权限已授予，启动热键监听

## 测试步骤

### 1. 清空权限并重新测试

```bash
# 完全退出 Aleph
killall Aleph

# 清理 TCC 数据库（需要重启系统）
# 或者手动在系统设置中移除 Aleph 的辅助功能权限

# 清理构建缓存
rm -rf ~/Library/Developer/Xcode/DerivedData/Aleph-*

# 重新构建并运行
xcodegen generate
open Aleph.xcodeproj
# 在 Xcode 中点击 Run (Cmd+R)
```

### 2. 验证权限提示流程

1. 启动 Aleph（没有辅助功能权限）
2. **预期**：1.5秒后显示权限提示（居中显示）
3. 点击「打开系统设置」
4. 在系统设置中授予权限
5. **问题**：此时热键仍然不工作（需要重启应用）
6. 完全退出 Aleph，重新启动
7. **预期**：热键现在应该正常工作

### 3. 验证热键功能

1. 在任意应用中选中一段文字
2. 按 Command + `
3. **预期**：Halo 出现在光标位置，显示 "Listening" 状态
4. （目前 AI 集成未完成，会显示模拟的处理动画）

## 下一步建议

### 优先级 1：完成权限检查逻辑

- [ ] 修改 `AppDelegate.initializeRustCore()` 检查权限
- [ ] 添加 `NSAccessibilityUsageDescription` 到 Info.plist
- [ ] 测试权限授予流程

### 优先级 2：添加权限重新检查

- [ ] 在权限提示添加「重新检查」按钮
- [ ] 实现 `EventHandler.recheckPermissions()` 方法
- [ ] 无需重启应用即可生效

### 优先级 3：改进错误处理

- [ ] 当 `rdev::listen()` 失败时显示明确错误
- [ ] 提供诊断工具（检查权限状态脚本）

## 参考文档

- [macOS Accessibility API](https://developer.apple.com/documentation/accessibility)
- [TCC (Transparency, Consent, and Control)](https://developer.apple.com/documentation/bundleresources/information_property_list)
- [rdev 库文档](https://docs.rs/rdev/latest/rdev/)

---

**创建时间**: 2025-12-29
**作者**: Claude Sonnet 4.5
**状态**: 已完成 Step 1 (统一权限提示), 待完成 Step 2-3

# Change: Redesign Permission Authorization Flow to Eliminate Crash Issues

## Why

当前的权限授权代码存在严重的 **闪退问题**，根本原因是对 macOS 权限系统机制的误解，导致了以下关键缺陷:

### 核心问题分析

1. **误判 Accessibility 权限需要重启**:
   - 现有代码假设 Accessibility 权限授予后需要应用重启才能生效
   - **实际情况**: macOS 的 Accessibility 权限是 **实时生效** 的，无需重启
   - 问题: 当检测到 `accessibilityGranted` 从 false 变为 true 时，代码主动调用 `NSApp.terminate()` 导致闪退

2. **无法区分 "新授予" vs "缓存延迟"**:
   - 应用启动时，macOS 权限查询 API 可能返回缓存值（延迟 0.3-1s）
   - 代码尝试用 "初始化阶段" 时间窗口来区分，但 debounce 延迟不可预测（3-6s+）
   - 结果: 即使用户已有权限，启动时也可能触发 "检测到新授权" 误判，导致无限重启循环

3. **Input Monitoring 权限处理不当**:
   - macOS 系统会在授予 Input Monitoring 权限时 **自动弹窗提示用户重启**
   - 代码不应该自己实现重启逻辑，应交由 macOS 系统和用户控制
   - 问题: 代码主动重启可能与系统弹窗冲突，导致 UX 混乱

4. **与 rdev 库的致命冲突**:
   - 当 Input Monitoring 权限未授予时，`rdev::listen()` 会直接 **panic 并终止整个进程**
   - 现有代码没有防护机制，即使权限检查失败，仍会尝试初始化 rdev
   - 问题: 导致应用在权限不足时崩溃，而不是优雅降级

### 用户体验问题

- 用户进入权限授权界面就立即闪退，无法完成授权流程
- 已授权用户在应用启动时也可能遇到误判导致的无限重启循环
- Debug 环境下问题更严重（DerivedData 路径变化导致权限状态重置）
- 用户对应用信任度下降，无法正常使用核心功能

### Apple 官方机制 vs 现有实现的误区

| 权限类型 | Apple 官方机制 | 现有错误实现 | 后果 |
|---------|--------------|------------|------|
| **Accessibility** | 实时生效，无需重启 | 检测到授权后主动重启应用 | 不必要的闪退 |
| **Input Monitoring** | 系统自动提示重启 | 代码主动重启，逻辑冲突 | 重启循环、UX 混乱 |

## What Changes

本次变更将采用 **"被动监听 + 瀑布流引导 + Rust 核心防护"** 的三层架构彻底修复闪退问题:

### 核心设计原则

1. **Passive Monitoring, No Proactive Restart**
   - 移除所有 "检测到权限变化就自动重启" 的逻辑
   - 改为 **被动监听** 权限状态变化，仅更新 UI 状态
   - 让 macOS 系统和用户控制重启时机

2. **Rust Core Panic Protection**
   - 在 Rust 核心层面使用 `std::panic::catch_unwind()` 包裹 `rdev::listen()`
   - 捕获 panic 并转换为优雅的错误日志，防止整个应用崩溃
   - 在权限检查失败时不初始化 rdev 监听器

3. **Waterfall Permission Flow**
   - 分离 Accessibility 和 Input Monitoring 两个步骤
   - 只有 Accessibility 授权后，才显示 Input Monitoring 步骤
   - 用户手动点击 "重启应用" 按钮（仅在 Input Monitoring 授权后显示）

### 具体改动

**Swift 层 (UI & 权限监听)**:

- **重写 `PermissionManager`** (取代现有 `PermissionStatusMonitor`):
  - 使用 `Timer` 轮询权限状态（每 1s）
  - 当 `accessibilityGranted` 变为 true 时，**仅更新 @Published 属性，绝不调用 exit/terminate**
  - 移除所有 debounce 逻辑（Apple API 本身已足够稳定）
  - 添加 `checkInputMonitoringViaHID()` 方法（使用 IOHIDManager）

- **重写 `PermissionGateView`**:
  - 瀑布流设计: Step 1 (Accessibility) → Step 2 (Input Monitoring)
  - Step 2 仅在 Step 1 完成后才可点击
  - 移除 "自动重启" 按钮，改为 "进入 Aleph" 按钮（仅在两个权限都授予后显示）
  - 添加可选的 "手动重启应用" 按钮（仅在 Input Monitoring 授权后，用户主动点击）

- **重写 `PermissionChecker`**:
  - 移除重试机制（retry with sleep），改用单次检查
  - 添加 `checkInputMonitoringViaHID()` 方法（更底层、更准确）
  - 添加 `openSystemSettings(for:)` 方法（支持深链接到特定权限页面）

**Rust 层 (核心防护)**:

- **修改 `Aleph/core/src/hotkey/rdev_listener.rs`**:
  - 使用 `std::panic::catch_unwind()` 包裹 `rdev::listen()`
  - 捕获 panic 后记录详细错误日志
  - 返回 `Result<(), HotkeyError>` 而不是让 panic 传播

- **修改 `Aleph/core/src/core.rs`**:
  - 在 `start_listening()` 中增加权限预检查
  - 如果 Input Monitoring 权限未授予，**不调用** `rdev::listen()`
  - 通过 UniFFI 回调 `on_error()` 通知 Swift 层权限问题

**配置文件**:

- **修改 `Aleph/Sources/AppDelegate.swift`**:
  - 启动时调用 `PermissionChecker.hasAllRequiredPermissions()`
  - 如果权限不足，显示新的 `PermissionGateView`
  - 权限检查通过后才初始化 `AlephCore`

### Deliverables

- **NEW**: `Aleph/Sources/Utils/PermissionManager.swift` - 新的被动监听权限管理器
- **MODIFIED**: `Aleph/Sources/Components/PermissionGateView.swift` - 重写为瀑布流设计
- **MODIFIED**: `Aleph/Sources/Utils/PermissionChecker.swift` - 添加 HID 检测方法
- **MODIFIED**: `Aleph/core/src/hotkey/rdev_listener.rs` - 添加 panic 防护
- **MODIFIED**: `Aleph/core/src/core.rs` - 添加权限预检查
- **MODIFIED**: `Aleph/Sources/AppDelegate.swift` - 优化权限门控逻辑
- **NEW**: 单元测试 - 覆盖权限状态变化的所有场景
- **NEW**: 文档 - 权限授权流程说明和故障排查指南

### Key Behaviors

1. **启动时权限检查** (无闪退):
   - 调用 `PermissionChecker.hasAllRequiredPermissions()`
   - 如果缺失权限，显示 `PermissionGateView`
   - 如果权限齐全，直接初始化 `AlephCore`

2. **Accessibility 权限授予** (无重启):
   - `PermissionManager` 检测到 `AXIsProcessTrusted()` 返回 true
   - 更新 `@Published var accessibilityGranted = true`
   - UI 自动显示绿色勾选，进入 Step 2
   - **绝不调用** `exit()` 或 `NSApp.terminate()`

3. **Input Monitoring 权限授予** (用户控制重启):
   - `PermissionManager` 检测到 `IOHIDRequestAccess()` 返回 true
   - UI 显示 "进入 Aleph" 按钮
   - 用户点击按钮后，调用 `restartApp()` 方法（用户主动触发）
   - 或者，用户忽略提示，macOS 系统弹窗会引导重启

4. **Rust 核心防护** (无 panic 崩溃):
   - `start_listening()` 前检查 Input Monitoring 权限
   - 如果未授予，跳过 `rdev::listen()`，回调 `on_error("Permission required")`
   - 如果调用 `rdev::listen()` 时 panic，`catch_unwind()` 捕获并记录错误
   - 应用继续运行，不会整体崩溃

### Out of Scope (Future Proposals)

- 运行时权限撤销检测（目前仅处理启动时检查）
- 权限降级模式（如无 Input Monitoring 时提供只读模式）
- 后台权限监控服务（持续监听权限变化）
- 自动化权限重新请求（需要系统对话框）

## Impact

### Affected Specs

- **MODIFIED**: `permission-gating` - 移除自动重启逻辑，添加被动监听设计
- **MODIFIED**: `macos-client` - 更新权限门控要求
- **MODIFIED**: `core-library` - 添加 Rust 核心 panic 防护
- **MODIFIED**: `hotkey-detection` - 添加权限预检查

### Affected Code

**Swift 层**:
- `Aleph/Sources/Utils/PermissionManager.swift` - **重写**
- `Aleph/Sources/Components/PermissionGateView.swift` - **重写**
- `Aleph/Sources/Utils/PermissionChecker.swift` - **修改**（添加 HID 方法）
- `Aleph/Sources/AppDelegate.swift` - **修改**（优化权限门控逻辑）
- `Aleph/Sources/Utils/PermissionStatusMonitor.swift` - **删除**（被 PermissionManager 取代）

**Rust 层**:
- `Aleph/core/src/hotkey/rdev_listener.rs` - **修改**（添加 panic 防护）
- `Aleph/core/src/core.rs` - **修改**（添加权限预检查）
- `Aleph/core/src/aleph.udl` - **修改**（如需添加权限检查相关 UniFFI 接口）

**测试**:
- `AlephTests/PermissionManagerTests.swift` - **新建**
- `Aleph/core/tests/hotkey_tests.rs` - **修改**（添加 panic 防护测试）

### Dependencies

- macOS 13+ (Ventura)
- IOKit.framework (用于 IOHIDManager API)
- 现有 UniFFI 桥接层
- 现有 `PermissionChecker` 基础设施

### Breaking Changes

**用户体验变化**:
- ✅ **不再闪退**: Accessibility 权限授予后不会自动重启，用户体验更流畅
- ✅ **手动控制**: Input Monitoring 权限授予后，由用户决定何时重启（而非强制）
- ⚠️ **瀑布流**: 权限授予流程改为分步骤（先 Accessibility，后 Input Monitoring）

**代码层面**:
- 删除 `PermissionStatusMonitor` 类（被 `PermissionManager` 取代）
- `PermissionGateView` API 保持兼容（回调函数 `onAllPermissionsGranted` 不变）
- Rust 核心新增权限预检查，可能影响 `start_listening()` 的调用流程

### Migration

**现有用户**:
- 已授权用户: 无影响，应用正常启动
- 未授权用户: 看到新的瀑布流权限界面（更友好）
- 之前遇到闪退的用户: 问题彻底解决

**开发者**:
- 删除所有引用 `PermissionStatusMonitor` 的代码，改用 `PermissionManager`
- 检查 `AppDelegate` 中的权限检查逻辑是否正确调用新 API
- 运行新的单元测试确保权限流程正确

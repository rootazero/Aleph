# Spec: Screen Capture Lifecycle

## Capability Overview

管理 OCR 截图功能中的窗口和视图生命周期，确保无闪退和无内存泄漏。

## MODIFIED Requirements

### Requirement: SCL-001 - Window/View Reuse Pattern

窗口和视图 MUST 采用单例重用模式，在整个应用生命周期中只创建一次。

#### Scenario: First capture creates window lazily

**Given** 应用启动后尚未进行过截图
**When** 用户首次按下 OCR 热键
**Then** 创建 `NSWindow` 和 `ScreenCaptureOverlayView` 实例
**And** 窗口配置为透明、无边框、浮动层级
**And** 视图设置为窗口的 contentView

#### Scenario: Subsequent captures reuse existing window

**Given** 已经完成过至少一次截图（窗口已创建）
**When** 用户再次按下 OCR 热键
**Then** 不创建新的 `NSWindow` 或 `ScreenCaptureOverlayView`
**And** 调用现有视图的 `reset()` 方法
**And** 重新显示现有窗口 (`orderFront`)

#### Scenario: Window hidden but not destroyed after capture

**Given** 用户正在进行区域选择
**When** 用户完成选区（mouseUp）或按 ESC 取消
**Then** 窗口调用 `orderOut(nil)` 从屏幕移除
**And** 不调用 `close()` 或设置 `contentView = nil`
**And** 窗口和视图保持在内存中供下次使用

### Requirement: SCL-002 - View Reset Protocol

视图 MUST 提供 `reset()` 方法用于重置状态以便重用。

#### Scenario: Reset clears selection state

**Given** 视图在上次截图中有选区记录
**When** 调用 `reset()` 方法
**Then** `selectionRect` 设为 nil
**And** `startPoint` 设为 nil
**And** `isDragging` 设为 false
**And** `isDismissed` 设为 false

#### Scenario: Reset recreates tracking area

**Given** 视图的 tracking area 在上次关闭时可能处于无效状态
**When** 调用 `reset()` 方法
**Then** 移除旧的 `NSTrackingArea`
**And** 创建新的 `NSTrackingArea`（覆盖整个 bounds）
**And** 添加新的 tracking area 到视图

#### Scenario: Reset triggers redraw

**Given** 视图可能显示上次的选区残影
**When** 调用 `reset()` 方法
**Then** 调用 `setNeedsDisplay(bounds)` 请求重绘

### Requirement: SCL-003 - Callback Safety

回调 MUST 在视图关闭后无效化以防止意外触发。

#### Scenario: Callbacks cleared on dismiss

**Given** 视图正在显示并已设置 `onComplete` 和 `onCancel` 回调
**When** 调用 `dismissOverlay()`（用户完成或取消选区）
**Then** `overlayView.onComplete` 设为 nil
**And** `overlayView.onCancel` 设为 nil

#### Scenario: Callbacks set on show

**Given** 窗口隐藏状态，准备开始新的截图
**When** 调用 `showRegionSelector()`
**Then** 设置新的 `onComplete` 回调
**And** 设置新的 `onCancel` 回调
**And** 这发生在 `reset()` 之后、`orderFront` 之前

### Requirement: SCL-004 - Memory Stability

多次截图后内存使用 MUST 保持稳定。

#### Scenario: No memory growth after repeated captures

**Given** 应用已完成初始化
**When** 执行 50 次连续截图操作
**Then** NSWindow 实例数保持为 1（不含其他窗口）
**And** ScreenCaptureOverlayView 实例数保持为 1
**And** 总内存增长不超过初始状态的 5%

## REMOVED Requirements

### Removed: Retained Arrays Pattern

移除 `retainedViews` 和 `retainedWindows` 数组，因为窗口重用模式不再需要这种内存泄漏式的保护。

---

## Cross-References

- `macos-client` - 窗口管理基础
- `hotkey-detection` - OCR 热键触发
- `event-handler` - 回调机制

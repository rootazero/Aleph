# Spec: Focus Detection

## Overview

在调出Halo窗口前，检测用户光标是否聚焦于文本输入区域。这确保AI输出能够正确打印到目标位置。

## ADDED Requirements

### Requirement: Input Focus Check
调出Halo前 MUST 检测当前是否有聚焦的文本输入元素。

#### Scenario: Detect focused text field
**Given** 用户在Notes应用的文本编辑区打字
**When** 检测光标聚焦状态
**Then** 返回成功，包含目标应用信息
**And** 信息包括：bundleId, windowTitle, caretPosition, focusedElement

#### Scenario: Detect focused in web text input
**Given** 用户在Safari的Google搜索框输入
**When** 检测光标聚焦状态
**Then** 返回成功
**And** bundleId为"com.apple.Safari"
**And** caretPosition为搜索框光标位置

#### Scenario: No focused input element
**Given** 用户在Finder中浏览文件
**When** 检测光标聚焦状态
**Then** 返回失败（notFocused）
**And** 不包含目标应用信息

#### Scenario: Accessibility permission denied
**Given** 应用没有Accessibility权限
**When** 尝试检测光标聚焦状态
**Then** 返回失败（accessibilityDenied）
**And** 可以触发权限请求

### Requirement: Target App Info
成功检测后 MUST 保存目标应用信息用于后续输出。

#### Scenario: Store target app info
**Given** 检测到聚焦的输入框
**When** 调出Halo窗口
**Then** 保存 TargetAppInfo 结构
**And** 结构包含 bundleId, windowTitle, caretPosition, focusedElement

#### Scenario: Use stored info for output
**Given** AI生成响应文本
**When** 准备输出到目标应用
**Then** 使用保存的 caretPosition 定位输出位置
**And** 使用 KeyboardSimulator 模拟输入

### Requirement: Caret Position Extraction
系统 SHALL 从聚焦元素中提取光标屏幕坐标。

#### Scenario: Get caret position from text field
**Given** 光标在文本框中间位置
**When** 提取光标坐标
**Then** 返回光标的屏幕坐标（NSPoint）
**And** 坐标为光标底部位置（用于在下方显示Halo）

#### Scenario: Fallback to element bounds
**Given** 无法获取精确光标位置
**When** 提取光标坐标
**Then** 返回聚焦元素的中心坐标作为fallback
**And** 记录日志表明使用了fallback

#### Scenario: Fallback to mouse position
**Given** 无法获取元素坐标
**When** 提取光标坐标
**Then** 返回当前鼠标位置作为最终fallback

### Requirement: Supported Input Types
系统 MUST 支持检测多种类型的文本输入元素。

#### Scenario: Standard NSTextField
**Given** 聚焦元素role为 AXTextField
**When** 检测输入类型
**Then** 识别为有效的文本输入

#### Scenario: TextArea (multi-line)
**Given** 聚焦元素role为 AXTextArea
**When** 检测输入类型
**Then** 识别为有效的文本输入

#### Scenario: ComboBox
**Given** 聚焦元素role为 AXComboBox
**When** 检测输入类型
**Then** 识别为有效的文本输入

#### Scenario: Web text input
**Given** 聚焦元素role为 AXTextField 且在浏览器中
**When** 检测输入类型
**Then** 识别为有效的文本输入
**And** 正确获取光标位置（如果浏览器支持）

#### Scenario: Unsupported element type
**Given** 聚焦元素role为 AXButton
**When** 检测输入类型
**Then** 返回失败（notFocused）
**And** 不视为有效的文本输入

### Requirement: Warning Toast
当光标未聚焦时 MUST 显示友好的警告提示。

#### Scenario: Show warning toast
**Given** 检测结果为 notFocused
**When** 用户按下 `Cmd+Opt+/`
**Then** 显示Toast提示 "请先点击输入框"
**And** Toast类型为warning（橙色）
**And** 2秒后自动消失

#### Scenario: Permission prompt
**Given** 检测结果为 accessibilityDenied
**When** 用户按下 `Cmd+Opt+/`
**Then** 显示权限提示Toast
**And** 提供打开系统设置的选项

## Error Handling

### FocusDetectionResult Enum

```swift
enum FocusDetectionResult {
    case focused(TargetAppInfo)
    case notFocused
    case accessibilityDenied
    case unknownError(Error)
}
```

### Fallback Chain

```
1. 尝试获取AXFocusedUIElement
   ↓ 失败
2. 返回 accessibilityDenied 或 notFocused

1. 尝试获取光标精确位置
   ↓ 失败
2. 使用元素bounds中心
   ↓ 失败
3. 使用鼠标位置
```

## Cross-References

- **Parent**: `unified-halo-window/spec.md` - 统一Halo窗口规范
- **Related**: `permission-authorization/spec.md` - 权限授权架构
- **Uses**: `halo-toast/spec.md` - Toast提示

# Spec: SubPanel Component

## Overview

副窗口是Halo主输入框下方的可伸缩区域，用于展示命令补全、AI选择器、CLI输出等多种内容。设计灵感来自Raycast的自动展开列表。

## ADDED Requirements

### Requirement: Dynamic Height
副窗口高度 MUST 根据内容动态调整，SHALL 支持平滑动画。

#### Scenario: Expand from hidden to show content
**Given** 副窗口当前处于隐藏状态（高度=0）
**When** 切换到需要显示内容的模式（如commandCompletion）
**Then** 副窗口从0平滑展开到内容所需高度
**And** 动画使用spring曲线，持续0.3秒
**And** 不超过最大高度300px

#### Scenario: Collapse when content removed
**Given** 副窗口当前显示内容
**When** 切换到hidden模式
**Then** 副窗口平滑收缩到高度0
**And** 完全收缩后不占用任何空间

#### Scenario: Height adjusts with content change
**Given** 副窗口显示5个命令
**When** 用户输入导致过滤后只剩2个命令
**Then** 副窗口高度平滑减小到适应2个命令
**And** 不会出现空白区域

### Requirement: Visual Design
副窗口 MUST 具有精美的视觉设计，SHALL 包括阴影、圆角、毛玻璃背景。

#### Scenario: Visual appearance
**Given** 副窗口可见
**When** 渲染到屏幕
**Then** 具有8px圆角
**And** 具有轻微阴影（y偏移4px，模糊半径12px，透明度0.15）
**And** 背景使用半透明毛玻璃效果（ultraThinMaterial）
**And** 顶部有1px的分隔线

### Requirement: Multi-Mode Support
副窗口 MUST 支持多种显示模式，每种模式 SHALL 有独特的内容和交互。

#### Scenario: Command Completion Mode
**Given** 用户输入以 `/` 开头
**When** 副窗口切换到commandCompletion模式
**Then** 显示过滤后的命令列表
**And** 每行显示命令图标、命令名、描述
**And** 支持上下键导航
**And** 支持Enter键选择
**And** 支持鼠标点击选择

#### Scenario: Selector Mode
**Given** AI请求用户选择选项
**When** 副窗口切换到selector模式
**Then** 显示选项列表和提示文本
**And** 支持单选或多选（根据配置）
**And** 选中项有视觉高亮
**And** 支持键盘和鼠标交互

#### Scenario: CLI Output Mode
**Given** AI执行后台操作需要显示进度
**When** 副窗口切换到cliOutput模式
**Then** 显示滚动的日志输出
**And** 新日志自动滚动到底部
**And** 不同类型日志有不同颜色（info=蓝，success=绿，error=红）
**And** 每行显示时间戳

#### Scenario: Confirmation Mode
**Given** 操作需要用户确认
**When** 副窗口切换到confirmation模式
**Then** 显示标题、描述信息
**And** 显示确认和取消按钮
**And** 支持Enter确认、Escape取消

### Requirement: Keyboard Hints
副窗口底部 SHALL 显示可用的键盘快捷键提示。

#### Scenario: Show navigation hints
**Given** 副窗口处于commandCompletion或selector模式
**When** 渲染副窗口
**Then** 底部显示 "↑↓ Navigate  ⏎ Select  ⎋ Cancel"
**And** 提示文字使用较小字号和淡色

#### Scenario: Hide hints when not needed
**Given** 副窗口处于cliOutput模式
**When** 渲染副窗口
**Then** 不显示键盘提示（或仅显示 "⎋ Close"）

### Requirement: State Machine
副窗口 MUST 使用明确的状态机管理模式切换。

#### Scenario: Mode transitions
**Given** 副窗口处于任意模式
**When** 调用 `setMode(_:)` 方法
**Then** 状态立即更新
**And** 触发对应的视图重渲染
**And** 高度动画平滑过渡

#### Scenario: Invalid transition handling
**Given** 副窗口处于cliOutput模式
**When** 尝试切换到commandCompletion模式
**Then** 正常完成切换（无限制）
**And** 旧内容被新内容替换

## Height Calculation Spec

| Mode | Height Formula | Max Height |
|------|----------------|------------|
| hidden | 0 | 0 |
| commandCompletion | items × 36 + 40 (header) | 300 |
| selector | items × 44 + 60 (prompt+buttons) | 300 |
| cliOutput | lines × 20 + 20 (padding) | 300 |
| confirmation | 120 (fixed) | 120 |

## Cross-References

- **Parent**: `unified-halo-window/spec.md` - 统一Halo窗口规范
- **Related**: `command-completion/spec.md` - 命令补全功能（被包含）
- **Related**: `halo-state/spec.md` - Halo状态管理

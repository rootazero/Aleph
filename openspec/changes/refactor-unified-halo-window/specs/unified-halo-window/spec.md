# Spec: Unified Halo Window

## Overview

统一的Halo窗口，整合原有的命令补全窗口和多轮对话窗口，提供单一入口的AI交互界面。

## ADDED Requirements

### Requirement: Unified Entry Point
统一入口 SHALL 通过 `Cmd+Opt+/` 热键调出，MUST 在光标位置显示Halo窗口。

#### Scenario: User presses Cmd+Opt+/ with focused input
**Given** 用户光标聚焦在任意应用的文本输入框
**When** 用户按下 `Cmd+Opt+/`
**Then** Halo窗口在光标位置下方显示
**And** 窗口包含主输入框和可选的副窗口区域
**And** 主输入框自动获得焦点
**And** 系统记录目标应用信息用于后续输出

#### Scenario: User presses Cmd+Opt+/ without focused input
**Given** 用户光标未聚焦于任何文本输入框
**When** 用户按下 `Cmd+Opt+/`
**Then** 显示Toast提示"请先点击输入框"
**And** Halo窗口不显示
**And** Toast在2秒后自动消失

### Requirement: Integrated Layout
Halo窗口 MUST 采用主输入框+副窗口的集成布局。

#### Scenario: Initial unified halo display
**Given** Halo窗口被调出
**When** 窗口首次显示
**Then** 显示头部信息栏（Turn计数，ESC提示）
**And** 显示主输入框
**And** 副窗口默认隐藏（无内容时）

#### Scenario: SubPanel appears on command input
**Given** Halo窗口已显示
**When** 用户在主输入框输入以 `/` 开头的文本
**Then** 副窗口从底部展开显示命令补全列表
**And** 展开动画流畅（spring动画，0.3s）

### Requirement: Multi-turn Conversation Start
调出Halo窗口 SHALL 立即开始多轮对话，MUST NOT 需要额外操作。

#### Scenario: Conversation starts immediately
**Given** 用户通过 `Cmd+Opt+/` 调出Halo
**When** Halo窗口显示
**Then** 自动创建新的对话会话
**And** Turn计数从1开始
**And** 后续输入都属于同一对话上下文

#### Scenario: Conversation continues after AI response
**Given** 用户已发送消息并收到AI响应
**When** AI响应输出完成
**Then** Halo窗口重新显示（如果之前隐藏）
**And** Turn计数增加
**And** 用户可以继续输入

### Requirement: Command Inline Execution
命令 MUST NOT 在目标应用中输入，SHALL 直接在Halo中解析执行。

#### Scenario: Execute slash command with content
**Given** 用户在Halo输入 `/en 你好世界`
**When** 用户按下Enter
**Then** 系统解析命令为 `en`，内容为 `你好世界`
**And** 路由到对应的翻译处理器
**And** AI响应（翻译结果）输出到目标应用
**And** `/en 你好世界` 这些字符不会出现在目标应用中

#### Scenario: Filter commands as user types
**Given** 用户输入 `/en`
**When** 副窗口显示命令补全
**Then** 只显示以 `en` 开头的命令
**And** 列表随输入实时更新

## MODIFIED Requirements

### Requirement: HaloState Update (was: Separate states)
原有的 `commandMode` 和 `conversationInput` 状态 MUST 统一为新的 `unifiedInput` 状态。

#### Scenario: Unified state handles both modes
**Given** Halo处于 `unifiedInput` 状态
**When** 用户输入文本
**Then** 系统根据输入内容（是否以`/`开头）决定是命令模式还是对话模式
**And** 副窗口自动显示或隐藏相应内容

### Requirement: Hotkey Configuration (was: Separate hotkeys)
原有的 `command_prompt` 热键配置 SHALL 与对话热键统一。

#### Scenario: Single hotkey in config
**Given** 用户配置文件
**When** 设置 `[shortcuts].unified_summon`
**Then** 该热键用于调出统一Halo窗口
**And** 旧的 `command_prompt` 配置作为别名（向后兼容）

## REMOVED Requirements

### Requirement: Separate Command Mode Window (was: CommandModeCoordinator)
删除独立的命令补全窗口，功能已合并到统一Halo。

#### Scenario: Old command mode deprecated
**Given** 用户升级到新版本
**When** 使用应用
**Then** 不再存在独立的命令补全窗口
**And** 原有热键触发统一Halo窗口

### Requirement: Shift Key to Start Conversation (was: Shift+hotkey)
删除需要Shift键启动对话的要求。

#### Scenario: Direct conversation start
**Given** 用户调出Halo
**When** 直接输入内容
**Then** 自动进入对话模式
**And** 无需按住Shift键

## Cross-References

- **Related**: `subpanel-component/spec.md` - 副窗口组件规范
- **Related**: `focus-detection/spec.md` - 光标聚焦检测规范
- **Depends**: `halo-toast/spec.md` - Toast提示功能
- **Supersedes**: `halo-command-mode/spec.md` - 原命令模式规范

# Change: Add Phantom Flow Interaction (Global)

**Status**: Draft
**Author**: AI Assistant
**Created**: 2026-01-07

## Why

Aether 作为 OS-Level Inline Agent，需要一种全局的、符合 "Ghost" 美学的交互模式。当 AI 或任何功能需要用户澄清/选择时，传统方案是弹出对话框或聊天气泡，这破坏了 "幽灵般存在" 的沉浸感。

**Phantom Flow（幽灵流）** 是 Aether 的核心交互模式：
- **原地交互**：所有交互在 Halo 内完成，无弹窗
- **菜单驱动**：通过候选词列表快速选择
- **行内提示**：通过输入框占位符收集文本
- **用完即焚**：交互完成后 Halo 消失

此模式是 **全局基础设施**，可被任何功能调用：
- 普通对话：AI 需要用户澄清时
- Skills：参数收集时
- MCP：工具调用参数收集时
- 未来扩展：任何需要用户交互的场景

## What Changes

### New Capabilities

1. **ClarificationRequest 数据类型（Rust Core）**
   - 统一的澄清请求结构：`prompt`, `options`, `input_type`, `default`
   - 支持两种类型：`select`（选项列表）、`text`（自由输入）
   - 通过 UniFFI 暴露给 Swift

2. **ClarificationHandler Callback（Rust Core）**
   - 新增 `AetherEventHandler` 回调方法
   - `on_clarification_needed(request)` → 触发 Halo 澄清 UI
   - `on_clarification_completed()` → 恢复正常流程

3. **Halo Clarification Mode（Swift UI）**
   - 新增 `HaloState.clarification(...)` 状态
   - **选项模式**：垂直候选词列表 + 键盘导航
   - **输入模式**：带占位符的输入框
   - 键盘交互：↑↓选择、⏎确认、⎋取消

4. **ClarificationManager（Swift）**
   - 管理澄清请求队列（支持多轮澄清）
   - 提供同步/异步 API
   - 与 HaloViewModel 集成

### Modified Capabilities

- **HaloState**：添加 `.clarification(request: ClarificationRequest, onResult: (ClarificationResult) -> Void)`
- **HaloView**：扩展以渲染澄清 UI
- **HaloWindow**：处理澄清模式的窗口尺寸和键盘事件
- **AetherEventHandler（UniFFI）**：添加澄清相关回调

## Impact

- **Affected specs**:
  - `phantom-flow` (NEW) - 全局交互模式规范

- **Affected code**:
  - `Aether/core/src/aether.udl` - UniFFI 接口扩展
  - `Aether/core/src/clarification/` - 新增模块（数据类型）
  - `Aether/Sources/HaloState.swift` - 澄清状态
  - `Aether/Sources/HaloView.swift` - 澄清 UI
  - `Aether/Sources/Components/ClarificationView.swift` - 新增组件
  - `Aether/Sources/Managers/ClarificationManager.swift` - 新增管理器

- **Breaking changes**: None（增量添加）

## Design Philosophy

Phantom Flow 遵循 Aether 的 "Ghost" 美学：

1. **Halo 是光标伴侣**：智能的 IME 扩展，而非传统对话框
2. **指尖交互**：所有操作在用户当前焦点位置完成
3. **最小干扰**：快速选择/输入，然后消失
4. **无状态保持**：每次澄清独立，不累积对话历史

## Success Criteria

1. 任何 Rust 代码可以通过 `event_handler.on_clarification_needed()` 触发澄清
2. 澄清 UI 在 50ms 内出现
3. 键盘响应 < 16ms
4. 支持选项列表和文本输入两种模式
5. 与现有 Command Mode 视觉风格一致
6. 不影响现有功能

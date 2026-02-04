# Design: Phantom Flow Interaction (Global)

## Context

Aleph 是 OS-Level Inline Agent，定位为 "幽灵般的存在"。传统 AI 助手使用聊天气泡或对话框与用户交互，这破坏了沉浸感。Phantom Flow 是 Aleph 的核心交互范式，实现 "原地交互、用完即焚" 的理念。

### Stakeholders

- **终端用户**：需要快速、无干扰的交互体验
- **功能开发者**：需要统一的澄清 API
- **UI 团队**：需要可复用的交互组件

### Constraints

1. **无弹窗**：所有交互在 Halo 内完成
2. **无焦点窃取**：Halo 不能抢占用户当前应用的焦点
3. **低延迟**：UI 出现 < 50ms，键盘响应 < 16ms
4. **全局可用**：任何 Rust 代码都能触发澄清

## Goals / Non-Goals

### Goals

- 提供统一的澄清请求 API（Rust → Swift）
- 实现选项列表和文本输入两种澄清模式
- 与现有 Command Mode 视觉风格一致
- 支持多轮澄清（队列机制）

### Non-Goals

- 复杂的表单（多字段同时输入）
- 富文本编辑
- 文件选择对话框
- 持久化对话历史

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     Caller (Any Rust Code)                       │
│  - CapabilityExecutor (Skills 参数收集)                           │
│  - AiProvider (AI 需要澄清)                                       │
│  - McpClient (MCP 工具参数)                                       │
└──────────────────────────────┬──────────────────────────────────┘
                               │ event_handler.on_clarification_needed(request)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│              AlephEventHandler (UniFFI Callback)                │
│  - 接收 ClarificationRequest                                      │
│  - 阻塞等待用户响应                                                │
│  - 返回 ClarificationResult                                       │
└──────────────────────────────┬──────────────────────────────────┘
                               │ Swift callback implementation
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                  ClarificationManager (Swift)                    │
│  - 管理请求队列                                                   │
│  - 更新 HaloViewModel.state                                       │
│  - 等待用户响应（Semaphore/Continuation）                          │
└──────────────────────────────┬──────────────────────────────────┘
                               │ state = .clarification(...)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                      HaloView (SwiftUI)                          │
│  - 渲染 ClarificationView                                         │
│  - 处理键盘事件                                                   │
│  - 调用 onResult callback                                         │
└─────────────────────────────────────────────────────────────────┘
```

## Data Types

### ClarificationRequest (Rust → UniFFI)

```rust
/// 澄清请求类型
enum ClarificationType {
    /// 选项列表（菜单驱动）
    Select,
    /// 自由文本输入
    Text,
}

/// 选项定义
struct ClarificationOption {
    /// 选项标签
    label: String,
    /// 选项值（发送给后端）
    value: String,
    /// 可选描述
    description: Option<String>,
}

/// 澄清请求
struct ClarificationRequest {
    /// 唯一请求 ID（用于多轮澄清）
    id: String,
    /// 提示文本（如 "What style would you like?"）
    prompt: String,
    /// 请求类型
    clarification_type: ClarificationType,
    /// 选项列表（仅 Select 类型）
    options: Option<Vec<ClarificationOption>>,
    /// 默认值索引或默认文本
    default_value: Option<String>,
    /// 占位符（仅 Text 类型）
    placeholder: Option<String>,
    /// 来源标识（如 "skill:refine-text", "mcp:git"）
    source: Option<String>,
}
```

### ClarificationResult (Swift → Rust)

```rust
/// 澄清结果
enum ClarificationResult {
    /// 用户选择了选项
    Selected { index: u32, value: String },
    /// 用户输入了文本
    TextInput { value: String },
    /// 用户取消
    Cancelled,
    /// 超时
    Timeout,
}
```

## UniFFI Interface

```idl
// aleph.udl additions

enum ClarificationType {
    "Select",
    "Text"
};

dictionary ClarificationOption {
    string label;
    string value;
    string? description;
};

dictionary ClarificationRequest {
    string id;
    string prompt;
    ClarificationType clarification_type;
    sequence<ClarificationOption>? options;
    string? default_value;
    string? placeholder;
    string? source;
};

enum ClarificationResultType {
    "Selected",
    "TextInput",
    "Cancelled",
    "Timeout"
};

dictionary ClarificationResult {
    ClarificationResultType result_type;
    u32? selected_index;
    string? value;
};

// Add to AlephEventHandler callback interface
callback interface AlephEventHandler {
    // ... existing methods ...

    // Called when clarification is needed from user
    // This is a blocking call - implementation must return result
    ClarificationResult on_clarification_needed(ClarificationRequest request);
};
```

## UI Design

### Select Mode (选项列表)

```
┌─────────────────────────────────────────┐
│ What style would you like?              │  ← prompt
├─────────────────────────────────────────┤
│ ▸ Professional                          │  ← selected (highlight)
│   Casual                                │
│   Humorous                              │
│   Concise                               │
│                                         │
│ ↑↓ Navigate  ⏎ Select  ⎋ Cancel         │  ← hints
└─────────────────────────────────────────┘
  Window size: 350 x 280
```

### Text Mode (文本输入)

```
┌─────────────────────────────────────────┐
│ Enter target language:                  │  ← prompt
├─────────────────────────────────────────┤
│ ┌─────────────────────────────────────┐ │
│ │ e.g., Spanish, French...         │ │ │  ← placeholder
│ └─────────────────────────────────────┘ │
│                                         │
│ ⏎ Confirm  ⎋ Cancel                     │  ← hints
└─────────────────────────────────────────┘
  Window size: 350 x 180
```

## Keyboard Handling

| Key | Select Mode | Text Mode |
|-----|-------------|-----------|
| ↑ | 上移选择 | N/A |
| ↓ | 下移选择 | N/A |
| ⏎ | 确认选择 | 确认输入 |
| ⎋ | 取消 | 取消 |
| Tab | 确认选择 | N/A |
| 字母/数字 | 快速跳转 | 输入文本 |

## Swift Implementation Sketch

```swift
// ClarificationManager.swift
class ClarificationManager {
    weak var viewModel: HaloViewModel?
    private var pendingContinuation: CheckedContinuation<ClarificationResult, Never>?

    /// Called from UniFFI callback (on background thread)
    func handleClarificationRequest(_ request: ClarificationRequest) async -> ClarificationResult {
        return await withCheckedContinuation { continuation in
            DispatchQueue.main.async {
                self.pendingContinuation = continuation
                self.viewModel?.state = .clarification(
                    request: request,
                    onResult: { result in
                        self.pendingContinuation?.resume(returning: result)
                        self.pendingContinuation = nil
                    }
                )
            }
        }
    }
}

// HaloState.swift
enum HaloState {
    // ... existing cases ...
    case clarification(request: ClarificationRequest, onResult: (ClarificationResult) -> Void)
}

// ClarificationView.swift
struct ClarificationView: View {
    let request: ClarificationRequest
    let onResult: (ClarificationResult) -> Void
    @State private var selectedIndex: Int = 0
    @State private var textInput: String = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Prompt
            Text(request.prompt)
                .font(.headline)

            // Content based on type
            if request.clarificationType == .select {
                SelectionList(options: request.options ?? [],
                              selectedIndex: $selectedIndex)
            } else {
                TextField(request.placeholder ?? "", text: $textInput)
            }

            // Hints
            HintBar()
        }
        .onKeyPress { key in
            handleKeyPress(key)
        }
    }
}
```

## Decisions

### Decision 1: 阻塞 vs 异步回调

**选择**：阻塞调用（`on_clarification_needed` 返回 `ClarificationResult`）

**理由**：
- 简化 Rust 侧的控制流（无需状态机管理）
- 调用者代码更直观（像普通函数调用）
- Swift 侧使用 `async/await` 实现，不阻塞 UI

### Decision 2: 单请求 vs 队列

**选择**：支持请求队列（但 MVP 先实现单请求）

**理由**：
- 某些场景可能需要连续多个澄清（如 Skills 多参数）
- 队列机制为未来扩展预留
- MVP 只处理单请求，简化实现

### Decision 3: 与 Command Mode 的关系

**选择**：独立状态，共享视觉风格

**理由**：
- Command Mode 是命令发现/导航
- Clarification Mode 是参数收集
- 两者功能不同，但视觉体验一致

## Risks / Trade-offs

### Risk 1: UniFFI 回调阻塞主线程

**缓解**：Swift 使用 `async/await` + `DispatchQueue.main.async`，不阻塞 UI

### Risk 2: 用户无响应导致永久阻塞

**缓解**：添加超时机制（默认 60s），超时返回 `Timeout`

## Open Questions

1. **Q: 是否支持输入验证（如正则校验）？**
   - A: MVP 不支持，预留给未来

2. **Q: 是否支持富文本选项（图标、颜色）？**
   - A: MVP 只支持纯文本，预留给未来

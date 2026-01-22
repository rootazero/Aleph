# 中文/英文纠错功能设计

**日期**: 2026-01-22
**状态**: 待实现

## 概述

为 Aether 添加快速文本纠错功能，通过双击空格快捷键触发，对当前输入框中的文字进行错别字和语法纠正。

## 功能规格

| 项目 | 规格 |
|------|------|
| 功能范围 | 中文错别字 + 语法纠错 + 英文拼写纠错（不润色） |
| 触发方式 | 双击空格（200ms 检测窗口） |
| 目标平台 | macOS Native (Swift) |
| 模型选择 | 配置文件指定 (`[typo_correction]`) |
| 用户反馈 | 最小化（失败时系统通知） |
| 延迟目标 | < 800ms（短句场景） |

## 架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                    macOS Swift 层                           │
│  ┌─────────────────┐    ┌─────────────────────────────┐     │
│  │ KeyboardMonitor │───▶│ TypoCorrectionCoordinator   │     │
│  │ (双击空格检测)   │    │ - 获取当前输入框文字         │     │
│  └─────────────────┘    │ - 调用 Rust Core             │     │
│                         │ - 替换文字                   │     │
│                         └──────────────┬──────────────┘     │
└────────────────────────────────────────│────────────────────┘
                                         │ FFI (UniFFI)
┌────────────────────────────────────────│────────────────────┐
│                    Rust Core                                │
│                         ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              typo_correction 模块                    │    │
│  │  - 直接调用 AiProvider.process()                    │    │
│  │  - 绕过 dispatcher 三层路由                         │    │
│  │  - 内置纠错 prompt                                  │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### 设计原则

- Swift 层负责快捷键监听和文字操作（Accessibility API）
- Rust Core 新增 `typo_correction` 模块，直接调用 provider，绕过复杂路由
- 单次 AI 调用，无多轮对话
- 极简调用链路，最小化延迟

## Rust Core 实现

### 模块结构

```
core/src/typo_correction/
├── mod.rs        # TypoCorrector 主逻辑
└── prompt.rs     # Prompt 常量
```

### 核心结构

```rust
// core/src/typo_correction/mod.rs

pub struct TypoCorrector {
    provider: Arc<dyn AiProvider>,
    config: TypoCorrectionConfig,
}

impl TypoCorrector {
    /// 纠正文本中的错别字和语法错误
    /// 直接调用 provider.process()，无路由开销
    pub async fn correct(&self, text: &str) -> Result<CorrectionResult> {
        let prompt = self.build_prompt(text);
        let response = self.provider
            .process(&prompt, Some(SYSTEM_PROMPT))
            .await?;
        self.parse_response(&response, text)
    }
}

pub struct CorrectionResult {
    pub corrected_text: String,
    pub has_changes: bool,
}
```

### 配置结构

```rust
// core/src/config/types/typo_correction.rs

pub struct TypoCorrectionConfig {
    pub enabled: bool,
    pub provider: String,      // e.g., "openai", "gemini"
    pub model: Option<String>, // e.g., "gpt-4o-mini"，None 则用 provider 默认
}
```

### 配置文件示例

```toml
[typo_correction]
enabled = true
provider = "openai"
model = "gpt-4o-mini"
```

## Prompt 设计

### System Prompt

```rust
const SYSTEM_PROMPT: &str = r#"你是一个文本纠错助手。你的任务是：

中文纠错：
1. 纠正同音字错别字（如：在→再、的→得→地、以→已）
2. 纠正前后鼻音/平翘舌音导致的错误（如：分→风、知→资）
3. 纠正基本语法错误（如：把/被字句误用、主谓搭配）

英文纠错：
4. 纠正常见拼写错误（如：teh→the、recieve→receive）
5. 纠正大小写错误（如：句首字母）

严格规则：
- 只改错误，绝不润色或改变表达方式
- 如果没有错误，原样返回
- 直接输出纠正后的文本，不要任何解释或标记
- 保持原文的标点符号和格式"#;
```

### User Prompt

```rust
fn build_prompt(&self, text: &str) -> String {
    format!("请纠正以下文本中的错别字和语法错误：\n\n{}", text)
}
```

### 响应解析

- AI 直接返回纠正后的文本（无 JSON 包装）
- 比较原文和返回文本，判断 `has_changes`
- 如果返回为空或解析失败，返回原文

## FFI 接口

### UniFFI 定义

```webidl
// core/src/aether.udl 新增

[Enum]
interface CorrectionResult {
    Success(string corrected_text, boolean has_changes);
    Error(string message);
};

namespace aethecore {
    [Async]
    CorrectionResult correct_typo(string text);
};
```

### 调用链路

```
Swift: AetherCore.correctTypo(text)
  → UniFFI 生成的绑定
    → Rust: correct_typo(text)
      → TypoCorrector.correct()
        → AiProvider.process()
```

## Swift 层实现

### 文件结构

```
platforms/macos/Aether/Sources/Features/TypoCorrection/
├── KeyboardMonitor.swift              # 快捷键监听
├── TypoCorrectionCoordinator.swift    # 协调器
└── AccessibilityHelper.swift          # 辅助功能封装
```

### KeyboardMonitor

```swift
class TypoCorrectionKeyboardMonitor {
    private var lastSpaceTime: Date?
    private let triggerInterval: TimeInterval = 0.2 // 200ms

    func handleKeyEvent(_ event: NSEvent) -> Bool {
        guard event.keyCode == 49 else { return false } // 空格键

        let now = Date()
        if let last = lastSpaceTime,
           now.timeIntervalSince(last) <= triggerInterval {
            // 双击空格触发
            lastSpaceTime = nil
            triggerCorrection()
            return true // 吞掉第二个空格
        }
        lastSpaceTime = now
        return false
    }
}
```

### TypoCorrectionCoordinator

```swift
class TypoCorrectionCoordinator {
    private var isProcessing = false

    func triggerCorrection() {
        guard !isProcessing else { return }
        isProcessing = true

        Task {
            defer { isProcessing = false }

            // 1. 通过 Accessibility API 获取当前焦点元素的文字
            guard let text = AccessibilityHelper.getFocusedText() else { return }

            // 2. 删除末尾的两个空格（触发用）
            let cleanText = String(text.dropLast(2))

            // 3. 调用 Rust Core 纠错
            let result = await AetherCore.shared.correctTypo(cleanText)

            // 4. 如果有改动，替换文字
            if result.hasChanges {
                AccessibilityHelper.setFocusedText(result.correctedText)
            } else {
                // 无改动，恢复原文（不含触发空格）
                AccessibilityHelper.setFocusedText(cleanText)
            }
        }
    }
}
```

### Accessibility 权限

- 需要在 `Info.plist` 添加 `NSAccessibilityUsageDescription`
- 首次使用时引导用户授权辅助功能权限

## 错误处理

| 场景 | 处理方式 |
|------|----------|
| 无法获取焦点文字 | 静默失败，不做任何操作 |
| 文字为空或只有空格 | 静默失败，删除触发空格 |
| 未配置 typo_correction | 静默失败，日志记录 |
| 配置的 provider 不存在 | 系统通知："纠错功能配置错误" |
| AI 调用超时（5s） | 系统通知："纠错超时"，保留原文 |
| AI 调用失败 | 系统通知显示错误，保留原文 |
| AI 返回空内容 | 保留原文，不做替换 |

## 边界情况

| 场景 | 处理方式 |
|------|----------|
| 文本过长（> 2000 字符） | 截断处理，只纠错前 2000 字符 |
| 正在进行上一次纠错 | 忽略新触发，防止重复请求 |
| 输入框只读 | 静默失败（无法写入） |

## 文件变更清单

### Rust Core 新增

- `core/src/typo_correction/mod.rs` - 纠错逻辑
- `core/src/typo_correction/prompt.rs` - Prompt 常量
- `core/src/config/types/typo_correction.rs` - 配置结构
- `core/src/ffi/typo_correction.rs` - FFI 入口

### Rust Core 修改

- `core/src/aether.udl` - 新增接口定义
- `core/src/lib.rs` - 导出模块
- `core/src/config/mod.rs` - 加载配置
- `core/src/config/types/mod.rs` - 导出配置类型

### Swift 新增

- `Features/TypoCorrection/KeyboardMonitor.swift` - 快捷键监听
- `Features/TypoCorrection/TypoCorrectionCoordinator.swift` - 协调器
- `Features/TypoCorrection/AccessibilityHelper.swift` - 辅助功能封装

### Swift 修改

- `Info.plist` - 辅助功能权限说明

## 调用流程图

```
用户双击空格
    │
    ▼
KeyboardMonitor 检测到触发
    │
    ▼
TypoCorrectionCoordinator.triggerCorrection()
    │
    ├─▶ AccessibilityHelper.getFocusedText()
    │
    ▼
AetherCore.correctTypo(text)  ──FFI──▶  correct_typo()
                                              │
                                              ▼
                                        TypoCorrector.correct()
                                              │
                                              ▼
                                        AiProvider.process()
                                              │
                                              ▼
                                        返回 CorrectionResult
    │
    ▼
AccessibilityHelper.setFocusedText(correctedText)
    │
    ▼
完成
```

## 后续扩展

- Tauri 跨平台支持
- 可配置的快捷键
- 纠错历史记录
- 自定义纠错规则

# Design Document: Native API Separation Architecture

## Overview

本文档详细说明了将 Aether 架构从"Rust 统一系统 API"迁移到"原生 API + Rust 纯计算核心"的设计决策和技术细节。

## Problem Space

### 当前架构的问题

#### 1. FFI 边界划分不合理

**当前 FFI 边界**:
```
┌──────────────────────────────────────────────┐
│            Swift UI Layer                    │
│  - SwiftUI views                             │
│  - Event handler implementation              │
│  - Basic UI state management                 │
└──────────────────────────────────────────────┘
                    ↕ (UniFFI - Frequent calls)
┌──────────────────────────────────────────────┐
│            Rust Core Layer                   │
│  - Hotkey listening (rdev)                   │
│  - Clipboard operations (arboard)            │
│  - Keyboard simulation (enigo)               │
│  - AI routing logic                          │
│  - AI provider HTTP clients                  │
│  - Memory system (vector DB + embeddings)    │
│  - Config management                         │
└──────────────────────────────────────────────┘
```

**问题分析**:
- 每次用户操作需要 **5+ 次 FFI 调用**:
  1. Swift → Rust: `start_listening()`
  2. Rust → Swift: `on_hotkey_detected(clipboard_content)`
  3. Swift → Rust: `process_clipboard()`
  4. Rust → Swift: `on_state_changed()` (多次)
  5. Rust → Swift: 返回 AI 响应

- FFI 调用开销（每次 ~1-2ms）:
  - 序列化/反序列化
  - 类型转换（Rust String ↔ Swift String）
  - 线程切换（Rust 异步线程 → Swift main thread）

#### 2. 系统 API 跨语言包装的复杂性

**案例: 剪贴板读取图片**

**Rust 实现**（当前）:
```rust
// arboard_manager.rs
pub fn read_image(&self) -> Result<Option<ImageData>> {
    let mut clipboard = Clipboard::new()?;  // 内部调用 macOS NSPasteboard
    let img = clipboard.get_image()?;

    // arboard 返回 arboard::ImageData (自定义类型)
    // 需要转换为 UniFFI 的 ImageData
    let bytes = img.bytes.to_vec();
    let format = detect_format(&bytes)?;

    Ok(Some(ImageData { data: bytes, format }))
}
```

**Swift 调用**:
```swift
// EventHandler.swift
let imageData = try? core.readClipboardImage()
// imageData 是 UniFFI 生成的 Swift 类型，需要再转换为 NSImage
```

**问题**:
- 数据经过 **3 次类型转换**: `NSImage` → `arboard::ImageData` → `UniFFI ImageData` → `Swift ImageData` → `NSImage`
- 每次转换都有内存拷贝和类型检查
- arboard 对 macOS 高级剪贴板特性支持不完整（如 RTF, PDF）

**原生实现**（目标）:
```swift
// ClipboardManager.swift
func getImage() -> NSImage? {
    return NSPasteboard.general.readObjects(
        forClasses: [NSImage.self],
        options: nil
    )?.first as? NSImage
}
```

**优势**:
- 零类型转换
- 零 FFI 调用
- 完整支持 macOS 剪贴板类型系统

#### 3. 跨平台维护成本

**当前模型**: Rust 核心需要处理所有平台差异

```rust
// clipboard/arboard_manager.rs
impl ClipboardManager for ArboardManager {
    fn get_text(&self) -> Result<Option<String>> {
        #[cfg(target_os = "macos")]
        {
            // macOS-specific code using NSPasteboard via arboard
        }

        #[cfg(target_os = "windows")]
        {
            // Windows-specific code using Clipboard API via arboard
        }

        #[cfg(target_os = "linux")]
        {
            // Linux-specific code using X11/Wayland via arboard
        }
    }
}
```

**问题**:
- Rust 开发者需要了解所有平台的剪贴板机制
- arboard/rdev/enigo 库可能在某些平台上有 bug，需要等待上游修复或自己 fork
- 添加新平台特性（如 macOS 的 Handoff）需要修改 Rust 核心

**新模型**: 每个平台用原生语言处理系统 API

```
macOS:     Swift → NSPasteboard (Apple 官方 API)
Windows:   C# → Clipboard class (Microsoft 官方 API)
Linux:     Rust → gtk4-rs (GNOME 官方 bindings)
```

**优势**:
- 各平台开发者使用熟悉的语言和 API
- 可充分利用平台特性（如 macOS Universal Clipboard）
- Rust 核心完全平台无关，专注业务逻辑

### 为什么现在重构？

1. **技术债累积**:
   - `GlobalHotkeyMonitor.swift` 已实现，但 rdev 仍在代码库中
   - UniFFI 接口已标注 `DEPRECATED`，但未删除

2. **权限系统重构需求**:
   - `redesign-permission-authorization` 已将权限检查移到 Swift
   - 权限检查和系统 API 调用应在同一层

3. **跨平台计划**:
   - 未来需支持 Windows 和 Linux
   - 当前架构下，Rust 核心需要维护 3 个平台的系统 API 包装
   - 新架构下，只需实现 3 个独立的前端层

## Solution Design

### 新架构概览

```
┌─────────────────────────────────────────────────────────────────┐
│                    Platform UI Layer (Swift)                    │
│  ┌───────────────┬────────────────┬───────────────────────────┐ │
│  │ System API    │ User Interaction│ UI Components            │ │
│  │ - Hotkey      │ - EventHandler  │ - Halo overlay           │ │
│  │ - Clipboard   │ - Callbacks     │ - Settings window        │ │
│  │ - Keyboard    │ - State mgmt    │ - Permission gate        │ │
│  │ - Context     │                 │                          │ │
│  └───────────────┴────────────────┴───────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
                    ↕ (UniFFI - Minimal calls)
              [High-level business operations]
┌─────────────────────────────────────────────────────────────────┐
│           Rust Core (Platform-Agnostic Logic)                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ AI Pipeline                                                 │ │
│  │ - Smart routing (regex matching)                           │ │
│  │ - Provider selection and fallback                          │ │
│  │ - PII scrubbing                                            │ │
│  │                                                             │ │
│  │ Memory System                                               │ │
│  │ - Vector database (rusqlite + sqlite-vec)                  │ │
│  │ - Embedding inference (ONNX Runtime)                       │ │
│  │ - Semantic search and ranking                              │ │
│  │                                                             │ │
│  │ AI Providers                                                │ │
│  │ - OpenAI HTTP client                                       │ │
│  │ - Claude HTTP client                                       │ │
│  │ - Gemini CLI wrapper                                       │ │
│  │ - Ollama CLI wrapper                                       │ │
│  │                                                             │ │
│  │ Config & Utilities                                          │ │
│  │ - TOML config parsing                                      │ │
│  │ - Keychain interface (callback to Swift)                   │ │
│  │ - Logging system                                           │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### 详细设计

#### 1. Swift 系统 API 层

**职责**: 所有与 macOS 系统交互的操作

##### 1.1 GlobalHotkeyMonitor (已有)

**文件**: `Aether/Sources/Utils/GlobalHotkeyMonitor.swift`

**技术选型**: `CGEventTap`

**关键决策**:
- **为什么用 CGEventTap 而不是 NSEvent.addGlobalMonitorForEvents？**
  - `addGlobalMonitorForEvents` 无法阻止事件传播（` 字符仍会输入）
  - `CGEventTap` 可以返回 `nil` 来"吞掉"事件
  - `CGEventTap` 是 macOS 推荐的全局热键方案

- **为什么不用 Carbon Event Manager？**
  - Carbon 已 deprecated
  - CGEventTap 是现代 API

**实现细节**:
```swift
class GlobalHotkeyMonitor {
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    func startMonitoring() -> Bool {
        let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,  // Can modify/delete events
            eventsOfInterest: CGEventMask(eventMask),
            callback: handleEvent,
            userInfo: selfPointer
        )

        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
    }

    private func handleEvent(...) -> Unmanaged<CGEvent>? {
        if keyCode == graveKeyCode {
            callback()  // Trigger Aether
            return nil  // Swallow event (prevent ` from typing)
        }
        return Unmanaged.passRetained(event)  // Propagate
    }
}
```

**权限要求**: Accessibility（已在 `redesign-permission-authorization` 处理）

##### 1.2 ClipboardManager (新建)

**文件**: `Aether/Sources/Utils/ClipboardManager.swift`

**技术选型**: `NSPasteboard`

**接口设计**:
```swift
/// Clipboard manager using native NSPasteboard
class ClipboardManager {
    // MARK: - Text Operations

    /// Read text from clipboard
    func getText() -> String? {
        return NSPasteboard.general.string(forType: .string)
    }

    /// Write text to clipboard
    func setText(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    // MARK: - Image Operations

    /// Read image from clipboard
    func getImage() -> NSImage? {
        return NSPasteboard.general.readObjects(
            forClasses: [NSImage.self],
            options: nil
        )?.first as? NSImage
    }

    /// Write image to clipboard
    func setImage(_ image: NSImage) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.writeObjects([image])
    }

    /// Check if clipboard contains image
    func hasImage() -> Bool {
        let types = NSPasteboard.general.types ?? []
        return types.contains(.tiff) || types.contains(.png)
    }

    // MARK: - Advanced Operations (Future)

    /// Get clipboard change count (for detecting external changes)
    func changeCount() -> Int {
        return NSPasteboard.general.changeCount
    }

    /// Read RTF content
    func getRTF() -> Data? {
        return NSPasteboard.general.data(forType: .rtf)
    }
}
```

**优势**:
- 完整支持 macOS 剪贴板类型（String, Image, RTF, PDF, URL, etc.）
- 可检测剪贴板变化（`changeCount`）
- 支持多种图片格式（TIFF, PNG, JPEG）自动转换

##### 1.3 KeyboardSimulator (新建)

**文件**: `Aether/Sources/Utils/KeyboardSimulator.swift`

**技术选型**: `CGEvent`

**关键决策**:
- **为什么不用 Accessibility API 的 AXUIElement？**
  - AXUIElement 需要获取目标应用的 AXUIElement reference
  - CGEvent 可以直接发送全局键盘事件
  - CGEvent 更简单，性能更好

**实现细节**:
```swift
/// Keyboard simulator using CGEvent
class KeyboardSimulator {
    // MARK: - Shortcut Simulation

    /// Simulate Cmd+X (Cut)
    func simulateCut() {
        pressModifierKey(key: .command)
        pressKey(character: "x")
        releaseModifierKey(key: .command)
    }

    /// Simulate Cmd+C (Copy)
    func simulateCopy() {
        pressModifierKey(key: .command)
        pressKey(character: "c")
        releaseModifierKey(key: .command)
    }

    /// Simulate Cmd+V (Paste)
    func simulatePaste() {
        pressModifierKey(key: .command)
        pressKey(character: "v")
        releaseModifierKey(key: .command)
    }

    // MARK: - Typewriter Effect

    /// Type text character by character with delay
    /// - Parameters:
    ///   - text: Text to type
    ///   - speed: Characters per second (default: 50)
    ///   - cancellationToken: Optional token to cancel typing
    func typeText(
        _ text: String,
        speed: Int = 50,
        cancellationToken: CancellationToken? = nil
    ) async {
        let delayMs = 1000 / speed

        for char in text {
            if cancellationToken?.isCancelled == true {
                break
            }

            typeCharacter(char)
            try? await Task.sleep(nanoseconds: UInt64(delayMs) * 1_000_000)
        }
    }

    // MARK: - Private Helpers

    private func pressModifierKey(key: CGKeyCode) {
        let event = CGEvent(
            keyboardEventSource: nil,
            virtualKey: key,
            keyDown: true
        )
        event?.flags = .maskCommand
        event?.post(tap: .cghidEventTap)
    }

    private func typeCharacter(_ char: Character) {
        let string = String(char)
        let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: 0,
            keyDown: true
        )
        keyDown?.keyboardSetUnicodeString(
            stringLength: string.utf16.count,
            unicodeString: Array(string.utf16)
        )
        keyDown?.post(tap: .cghidEventTap)

        // Key up event
        let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: 0,
            keyDown: false
        )
        keyUp?.post(tap: .cghidEventTap)
    }
}
```

**权限要求**: Accessibility

**兼容性考虑**:
- 某些应用（如密码管理器）可能阻止 CGEvent 模拟
- 需要在 20+ 常用应用中测试
- 文档记录已知不兼容应用

##### 1.4 ContextCapture (已有)

**文件**: `Aether/Sources/ContextCapture.swift`

**技术选型**: `NSWorkspace` + Accessibility API

**保持现状**:
```swift
class ContextCapture {
    func getCurrentAppBundleId() -> String {
        return NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? ""
    }

    func getCurrentWindowTitle() -> String? {
        // Use Accessibility API to get window title
        let app = NSWorkspace.shared.frontmostApplication
        // ... AXUIElement code
    }
}
```

#### 2. Rust 核心层重构

**职责**: 纯业务逻辑，无系统 API 依赖

##### 2.1 简化的 UniFFI 接口

**文件**: `Aether/core/src/aether.udl`

**删除的接口**:
```diff
interface AetherCore {
-  void start_listening();
-  void stop_listening();
-  boolean is_listening();
-  string get_clipboard_text();
-  boolean has_clipboard_image();
-  ImageData? read_clipboard_image();
-  void write_clipboard_image(ImageData image);
}
```

**新增/保留的接口**:
```rust
interface AetherCore {
    // 核心 AI 处理管线
    // 接收 Swift 预处理的输入（已从剪贴板读取）
    [Throws=AetherException]
    string process_input(string user_input, CapturedContext context);

    // 记忆增强（可选，UI 可选择是否启用）
    [Throws=AetherException]
    string augment_with_memory(string input, CapturedContext context);

    // 配置管理（保持不变）
    [Throws=AetherException]
    FullConfig load_config();

    [Throws=AetherException]
    void update_provider(string name, ProviderConfig provider);

    // 记忆管理（保持不变）
    [Throws=AetherException]
    MemoryStats get_memory_stats();

    [Throws=AetherException]
    sequence<MemoryEntry> search_memories(...);
}
```

**回调接口简化**:
```diff
callback interface AetherEventHandler {
-  void on_hotkey_detected(string clipboard_content);  // Swift 直接处理
   void on_state_changed(ProcessingState state);
   void on_error(string message, string? suggestion);
   void on_response_chunk(string text);  // 流式响应
   void on_ai_processing_started(string provider_name, string color);
}
```

##### 2.2 新的处理流程

**文件**: `Aether/core/src/core.rs`

**旧流程**（删除）:
```rust
// OLD: Rust 负责监听热键和读取剪贴板
pub fn start_listening(&self) -> Result<()> {
    self.hotkey_listener.start_listening()?;  // rdev
    Ok(())
}

fn on_hotkey_callback(&self) {
    let content = self.clipboard_manager.get_text()?;  // arboard
    self.event_handler.on_hotkey_detected(content);
}
```

**新流程**（简化）:
```rust
// NEW: Rust 只负责 AI 处理逻辑
pub fn process_input(
    &self,
    user_input: String,
    context: CapturedContext,
) -> Result<String> {
    // 1. PII 过滤
    let sanitized = self.pii_filter.scrub(&user_input)?;

    // 2. 记忆增强（如果启用）
    let augmented = if self.config.memory.enabled {
        self.memory_system.augment_prompt(&sanitized, &context)?
    } else {
        sanitized
    };

    // 3. AI 路由
    let provider = self.router.select_provider(&augmented)?;
    self.event_handler.on_ai_processing_started(
        provider.name(),
        provider.color()
    );

    // 4. 调用 AI
    let response = provider.generate(&augmented).await?;

    // 5. 存储记忆
    if self.config.memory.enabled {
        self.memory_system.store_interaction(
            user_input,
            response.clone(),
            context
        )?;
    }

    Ok(response)
}
```

**关键变化**:
- 输入由 Swift 预处理（剪贴板已读取）
- Rust 专注于：PII 过滤 → 记忆 → 路由 → AI 调用
- 输出直接返回给 Swift（由 Swift 负责打字机效果）

##### 2.3 依赖清理

**文件**: `Aether/core/Cargo.toml`

```diff
[dependencies]
- rdev = { git = "https://github.com/Narsil/rdev.git", branch = "main" }
- arboard = "3.3"
- enigo = "0.2.1"
- core-foundation = "0.10"  # 仅用于 rdev
- core-graphics = "0.24"     # 仅用于 rdev

# 保留的核心依赖
uniffi = { version = "0.25", features = ["cli"] }
tokio = { version = "1.35", features = ["rt-multi-thread", "sync", "time"] }
reqwest = { version = "0.11", features = ["json", "rustls-tls"] }
rusqlite = { version = "0.30", features = ["bundled"] }
serde = { version = "1.0", features = ["derive"] }
# ... 其他业务逻辑依赖
```

**Binary size 影响**:
- 删除 rdev: ~500KB
- 删除 arboard: ~200KB
- 删除 enigo: ~300KB
- 删除 core-graphics: ~1MB
- **总减少**: ~2MB

#### 3. 新的工作流程

##### 3.1 端到端流程图

```
┌────────────────────────────────────────────────────────────────┐
│ 1. User Action: Select text + Press ` key                     │
└────────────────────────────────────────────────────────────────┘
                            ↓
┌────────────────────────────────────────────────────────────────┐
│ 2. Swift: GlobalHotkeyMonitor detects key                     │
│    - CGEventTap callback triggered                            │
│    - Returns nil to swallow ` character                       │
└────────────────────────────────────────────────────────────────┘
                            ↓
┌────────────────────────────────────────────────────────────────┐
│ 3. Swift: EventHandler.onHotkeyPressed()                      │
│    a. ClipboardManager.getText() → user_input                 │
│    b. ContextCapture.getCurrentContext() → context            │
│    c. Show Halo overlay at cursor position                    │
└────────────────────────────────────────────────────────────────┘
                            ↓
┌────────────────────────────────────────────────────────────────┐
│ 4. Swift → (UniFFI) → Rust: core.process_input(input, ctx)   │
│    [Single FFI call]                                          │
└────────────────────────────────────────────────────────────────┘
                            ↓
┌────────────────────────────────────────────────────────────────┐
│ 5. Rust Core Pipeline:                                        │
│    a. PII scrubbing (regex-based)                             │
│    b. Memory retrieval (vector search, optional)              │
│    c. Router: Select provider based on rules                  │
│    d. AI Provider HTTP call (async)                           │
│    e. Store interaction in memory DB                          │
│    f. Return response                                         │
│                                                                │
│    Callbacks during processing:                               │
│    - on_state_changed(RetrievingMemory)                       │
│    - on_ai_processing_started("openai", "#10a37f")            │
│    - on_response_chunk(text) [if streaming]                   │
└────────────────────────────────────────────────────────────────┘
                            ↓
┌────────────────────────────────────────────────────────────────┐
│ 6. Swift: Receives AI response                                │
│    a. Update Halo state (show checkmark)                      │
│    b. KeyboardSimulator.typeText(response, speed: 50)         │
│       - Typewriter effect with async/await                    │
│       - User can press Esc to cancel                          │
│    c. Hide Halo after completion                              │
└────────────────────────────────────────────────────────────────┘
```

**FFI 调用统计**:
- **旧架构**: 5-7 次（start_listening, on_hotkey, get_clipboard, process, on_state × N, 返回）
- **新架构**: 2 次（process_input, 返回）+ N 次回调（可选）
- **性能提升**: ~70% 减少 FFI 开销

##### 3.2 错误处理流程

```
Rust 错误
  ↓
AetherException (UniFFI enum)
  ↓
Swift: catch AetherException
  ↓
根据错误类型显示 UI:
  - Network error → "网络错误，请检查连接"
  - Permission error → 引导用户授权
  - Quota error → "API 配额已用完"
  - Timeout error → "请求超时，重试？" (show retry button)
```

### Trade-offs 分析

#### Trade-off 1: 原生 API vs 跨平台库

| 方面 | 原生 API (选择) | 跨平台库 (arboard/rdev) |
|------|----------------|------------------------|
| **性能** | ✅ 最优（零 FFI） | ❌ FFI 开销 |
| **功能完整性** | ✅ 完整平台特性 | ⚠️ 受限于库实现 |
| **维护成本** | ⚠️ 每个平台单独维护 | ✅ 统一维护 |
| **调试难度** | ✅ 平台原生工具 | ❌ 跨语言调试 |
| **跨平台一致性** | ❌ 各平台可能有差异 | ✅ 行为一致 |

**决策**: 选择原生 API，因为：
- Aether 优先考虑 macOS 体验（"Ghost" 美学需要原生 API）
- 性能是核心竞争力（< 100ms 延迟）
- 跨平台时可针对各平台优化，而不是妥协于最小公分母

#### Trade-off 2: Swift 实现 vs Rust 实现（系统 API）

| 方面 | Swift (选择) | Rust |
|------|-------------|------|
| **开发速度** | ✅ macOS 开发者熟悉 | ⚠️ 需要学习 macOS API |
| **类型安全** | ✅ Swift 类型系统 | ✅ Rust 类型系统 |
| **错误处理** | ⚠️ Swift optionals | ✅ Rust Result<T,E> |
| **社区支持** | ✅ Apple 官方文档 | ❌ 第三方库文档 |
| **FFI 复杂度** | ✅ 无 FFI | ❌ 需要 UniFFI 包装 |

**决策**: 选择 Swift，因为：
- 系统 API 本就是为 Swift/Objective-C 设计
- 消除 FFI 边界是性能优化的关键
- macOS 开发者更容易贡献代码

#### Trade-off 3: 大改动 vs 渐进式重构

| 方面 | 大改动 (选择) | 渐进式 |
|------|-------------|-------|
| **风险** | ⚠️ 高（可能引入新 bug） | ✅ 低（逐步验证） |
| **时间** | ⚠️ 3-4 周 | ✅ 可分散到多个版本 |
| **技术债** | ✅ 彻底清理 | ⚠️ 长期存在过渡代码 |
| **测试复杂度** | ⚠️ 需要全量回归测试 | ✅ 每步独立测试 |

**决策**: 选择大改动，因为：
- 技术债已累积（rdev + CGEventTap 并存）
- 架构清晰后更易维护
- 有完整的回滚策略（保留 Rust 代码 2 个版本）

### 迁移策略

#### Phase 1: 准备阶段 (1 周)

**目标**: 实现 Swift 层功能，不破坏现有代码

**任务**:
1. 实现 `ClipboardManager.swift`
2. 实现 `KeyboardSimulator.swift`
3. 为现有 `GlobalHotkeyMonitor` 添加单元测试
4. 在 20+ 应用中测试键盘模拟兼容性

**验证**:
- Swift 实现通过所有单元测试
- 手动测试确认功能正常

#### Phase 2: 并行运行阶段 (1 周)

**目标**: 同时运行两套实现，对比验证

**任务**:
1. 添加 feature flag: `use_native_apis` (默认 false)
2. 修改 `EventHandler.swift`，根据 flag 选择实现
3. 日志记录两套实现的性能差异
4. Beta 测试，收集用户反馈

**验证**:
- 两套实现结果一致
- 原生实现性能更优

#### Phase 3: 切换阶段 (3 天)

**目标**: 切换到原生实现

**任务**:
1. 设置 `use_native_apis = true`
2. 发布 beta 版本
3. 监控崩溃率和性能指标

**回滚计划**:
- 如果崩溃率上升 > 5%，立即回滚到 Rust 实现
- 修复问题后再次尝试

#### Phase 4: 清理阶段 (3 天)

**目标**: 删除旧代码

**任务**:
1. 从 `Cargo.toml` 移除依赖
2. 删除 Rust 系统 API 模块
3. 更新 UniFFI 接口定义
4. 重新生成 Swift bindings
5. 更新文档

**验证**:
- 编译通过
- Binary size 减少 ~2MB
- 所有测试通过

### 风险缓解措施

#### 风险 1: Swift 实现有未发现的 bug

**缓解**:
- Phase 2 并行运行，对比验证
- 保留 Rust 代码 2 个版本周期
- 完整的回滚流程

**监控指标**:
- 崩溃率（目标: 无上升）
- 热键响应延迟（目标: < 100ms p95）
- 键盘模拟成功率（目标: > 95%）

#### 风险 2: 某些应用不兼容 CGEvent 模拟

**缓解**:
- 在 20+ 常用应用中测试
- 文档记录已知不兼容应用
- 提供"兼容模式"（使用 Accessibility API）

**测试应用列表**:
- 编辑器: VSCode, Xcode, Sublime Text, Vim
- 浏览器: Safari, Chrome, Firefox
- 通讯: WeChat, Slack, Discord
- 办公: Notes, Pages, Microsoft Word
- 终端: iTerm2, Terminal.app

#### 风险 3: macOS 版本兼容性问题

**缓解**:
- 使用 `@available` 标注
- 最低支持 macOS 13（Ventura）
- 在 macOS 13/14/15 上完整测试

**API 兼容性检查**:
```swift
if #available(macOS 13.0, *) {
    // Use modern API
} else {
    // Fallback for older macOS
}
```

## Implementation Checklist

- [ ] **Phase 1: 准备**
  - [ ] 实现 ClipboardManager.swift
  - [ ] 实现 KeyboardSimulator.swift
  - [ ] 添加单元测试
  - [ ] 兼容性测试（20+ 应用）

- [ ] **Phase 2: 并行运行**
  - [ ] 添加 feature flag
  - [ ] 修改 EventHandler 支持双实现
  - [ ] 性能对比测试
  - [ ] Beta 测试

- [ ] **Phase 3: 切换**
  - [ ] 启用原生实现
  - [ ] 发布 beta
  - [ ] 监控指标

- [ ] **Phase 4: 清理**
  - [ ] 删除 Rust 依赖
  - [ ] 删除 Rust 模块
  - [ ] 更新 UniFFI
  - [ ] 更新文档

## Success Metrics

### 性能指标

- [ ] 热键响应延迟 < 100ms (p95)，相比当前提升 >= 10%
- [ ] 剪贴板操作延迟 < 50ms (p95)
- [ ] 内存占用减少 >= 1MB
- [ ] Binary size 减少 >= 2MB

### 质量指标

- [ ] 崩溃率无上升
- [ ] 所有现有测试通过
- [ ] 新增测试覆盖率 >= 80%
- [ ] 代码行数净减少 >= 300 lines

### 用户体验指标

- [ ] Beta 测试满意度 >= 4.5/5
- [ ] 零关键 bug（P0/P1）
- [ ] 兼容性: >= 95% 常用应用正常工作

## Open Questions

1. **是否需要为 Linux 也用原生 API（GTK）？**
   - 当前决策: 是，因为 GTK4 有优秀的 Rust bindings
   - 需要验证: GTK4-rs 的成熟度

2. **是否支持用户自定义热键（非 ` 键）？**
   - 当前决策: Phase 1 暂不支持，Phase 2 考虑
   - 技术可行性: CGEventTap 支持，需要 UI 配置

3. **是否需要 "兼容模式" 用于不支持 CGEvent 的应用？**
   - 当前决策: 先测试，如果需要再添加
   - 备选方案: 使用 Accessibility API 的 AXUIElement

4. **Windows 版本何时开始开发？**
   - 当前决策: macOS 重构完成后 6 个月
   - 前提: 完成 Phase 4，架构稳定

---

**文档版本**: 1.0
**最后更新**: 2025-12-30
**作者**: Claude Code

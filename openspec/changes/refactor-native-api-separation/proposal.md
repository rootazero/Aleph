# Change: Refactor to Native API Separation Architecture

## Why

当前的 Aether 架构存在严重的 **前后端职责混淆** 问题，违反了现代软件工程的分层原则，导致以下关键缺陷：

### 核心问题分析

1. **系统交互 API 在 Rust 层，违反原生优先原则**:
   - **热键监听**: 使用第三方 Rust 库 `rdev`，通过 FFI 跨越语言边界
     - macOS 有原生 `CGEventTap` API（已实现 `GlobalHotkeyMonitor.swift`）
     - `rdev` 存在主线程问题、权限检测不准确、panic 崩溃等问题
     - UniFFI 中 `start_listening()` / `stop_listening()` 已标记为 DEPRECATED

   - **剪贴板管理**: 使用第三方 Rust 库 `arboard`
     - macOS 有原生 `NSPasteboard` API，功能更完善、类型支持更好
     - 跨 FFI 传输二进制数据（图片）增加复杂度和性能开销

   - **键盘模拟**: 使用第三方 Rust 库 `enigo`
     - macOS 有原生 `CGEvent` API，可直接模拟 Cmd+X / Cmd+V
     - `enigo` 在不同 macOS 版本上行为不一致

2. **跨平台架构不合理**:
   - 当前设计：Rust 层处理所有平台的系统 API 差异
   - 问题：Rust 核心需要维护 macOS / Windows / Linux 三套系统 API 包装
   - 后果：核心业务逻辑与平台代码耦合，难以维护

3. **FFI 边界划分不当**:
   - 当前边界：`Swift UI ←→ (UniFFI) ←→ Rust (业务逻辑 + 系统 API)`
   - 问题：频繁的跨 FFI 调用（每次剪贴板操作、键盘模拟都要跨边界）
   - 性能开销：序列化/反序列化、类型转换、线程切换

4. **违反"前端负责交互，后端负责计算"原则**:
   - 系统交互属于前端职责（用户输入、系统剪贴板、窗口焦点）
   - AI 推理、路由、记忆检索属于后端职责（纯计算逻辑）
   - 当前实现将两者混在 Rust 层，职责不清

### 具体案例

**案例 1: 热键监听的复杂性**
```
当前流程:
User presses ` key
  ↓
rdev (Rust thread) detects key → 需要 set_is_main_thread(false) 修复
  ↓
UniFFI callback → Swift
  ↓
Swift 通知 UI

问题：
- rdev 跨线程调用 macOS API 导致 dispatch_assert_queue_fail
- Swift 已有完美的 CGEventTap 实现 (GlobalHotkeyMonitor.swift)
```

**案例 2: 剪贴板操作的 FFI 开销**
```
当前流程:
Swift: core.get_clipboard_text()
  ↓ (FFI 边界)
Rust: arboard::Clipboard::new().get_text()
  ↓ (调用 macOS NSPasteboard)
  ↓ (FFI 边界)
Swift: 接收 String

理想流程:
Swift: NSPasteboard.general.string(forType: .string)
  ↓
直接调用系统 API，零开销
```

### 架构债务累积

- **技术债**: 已有 `GlobalHotkeyMonitor.swift` 但 Rust 仍保留 rdev
- **维护成本**: 同时维护两套热键实现（Rust rdev + Swift CGEventTap）
- **文档矛盾**: UniFFI 标注 DEPRECATED 但代码未删除

## What Changes

本次变更将实施 **"Native First + Clean Separation"** 架构重构：

### 新架构原则

1. **原生 API 优先 (Native First)**:
   - 所有平台相关的系统交互使用原生 API
   - macOS: Swift + AppKit / Cocoa
   - Windows (未来): C# + WinRT
   - Linux (未来): Rust + GTK4 (因为没有更好的原生选择)

2. **清晰的职责分离 (Clean Separation)**:
   - **前端 (Swift/C#)**: 系统交互层
     - 热键监听、剪贴板、键盘模拟
     - 上下文捕获（窗口信息、应用信息）
     - UI 渲染、动画、状态管理
   - **后端 (Rust)**: 纯计算层
     - AI 路由决策
     - AI Provider HTTP 调用
     - 记忆系统（向量检索、嵌入推理）
     - 配置管理、PII 过滤

3. **最小化 FFI 调用 (Minimal FFI)**:
   - 新边界：`Swift (系统交互) ←→ (UniFFI) ←→ Rust (纯业务逻辑)`
   - 只在必要时跨 FFI（如：调用 AI 接口、检索记忆）
   - 减少数据传输量（不再传输原始剪贴板数据到 Rust）

### 具体改动

#### Phase 1: 移除 Rust 层的系统 API 依赖

**删除 Rust 依赖**:
- 移除 `rdev` 依赖（热键已在 Swift 实现）
- 移除 `arboard` 依赖（剪贴板将用 Swift 实现）
- 移除 `enigo` 依赖（键盘模拟将用 Swift 实现）

**更新 `Cargo.toml`**:
```diff
[dependencies]
- rdev = { git = "https://github.com/Narsil/rdev.git", branch = "main" }
- arboard = "3.3"
- enigo = "0.2.1"
+ # Platform interaction moved to native layers (Swift/C#/GTK)
```

**删除 Rust 模块**:
- `Aether/core/src/hotkey/rdev_listener.rs`
- `Aether/core/src/clipboard/arboard_manager.rs`
- `Aether/core/src/input/enigo_simulator.rs`

#### Phase 2: 扩展 Swift 层功能

**新增 Swift 组件**:

1. **`ClipboardManager.swift`** (已有部分，需扩展):
   - `getText() -> String?` - 读取文本
   - `setText(_ text: String)` - 写入文本
   - `getImage() -> NSImage?` - 读取图片
   - `setImage(_ image: NSImage)` - 写入图片
   - 使用 `NSPasteboard.general`

2. **`KeyboardSimulator.swift`** (新建):
   - `simulateCut()` - 模拟 Cmd+X
   - `simulateCopy()` - 模拟 Cmd+C
   - `simulatePaste()` - 模拟 Cmd+V
   - `typeText(_ text: String, speed: Int)` - 打字机效果
   - 使用 `CGEvent.keyboardEvent()`

3. **`ContextCapture.swift`** (已有，保持):
   - `getCurrentAppBundleId() -> String`
   - `getCurrentWindowTitle() -> String?`
   - 使用 `NSWorkspace` + Accessibility API

4. **`GlobalHotkeyMonitor.swift`** (已有，保持):
   - 使用 `CGEventTap` 监听 ` 键

#### Phase 3: 简化 UniFFI 接口

**更新 `aether.udl`**:

移除不再需要的接口：
```diff
interface AetherCore {
-  void start_listening();  // 已在 Swift 实现
-  void stop_listening();   // 已在 Swift 实现
-  string get_clipboard_text();  // 已在 Swift 实现
-  boolean has_clipboard_image();  // 已在 Swift 实现
-  ImageData? read_clipboard_image();  // 已在 Swift 实现
-  void write_clipboard_image(ImageData image);  // 已在 Swift 实现
}
```

新增简化的核心接口：
```diff
interface AetherCore {
+  // 核心 AI 处理管线（接收预处理的输入）
+  string process_input(string user_input, CapturedContext context);
+
+  // 记忆检索和增强
+  string augment_with_memory(string input, CapturedContext context);
}
```

移除不再需要的回调：
```diff
callback interface AetherEventHandler {
-  void on_hotkey_detected(string clipboard_content);  // Swift 直接处理
}
```

#### Phase 4: 重写工作流程

**新的用户交互流程**:

```
1. User presses ` key
   ↓
2. GlobalHotkeyMonitor (Swift) detects → callback
   ↓
3. Swift: ClipboardManager.getText() → get user input
   ↓
4. Swift: ContextCapture.getCurrentContext() → app info
   ↓
5. Swift → (UniFFI) → Rust: core.process_input(text, context)
   ↓
6. Rust: Memory retrieval → AI routing → HTTP call
   ↓
7. Rust → (UniFFI) → Swift: AI response
   ↓
8. Swift: KeyboardSimulator.typeText(response)
   ↓
9. Swift: Show success animation
```

**关键优化点**:
- FFI 调用从 **5 次减少到 2 次**（调用 + 返回）
- 所有系统 API 在 Swift 层调用，零跨语言开销
- Rust 专注于纯计算任务（AI 逻辑、记忆检索）

### 跨平台策略

**新的跨平台模型**:

```
┌─────────────────────────────────────────────────────┐
│                 Platform UI Layer                    │
│  ┌────────────┬────────────────┬──────────────────┐ │
│  │   macOS    │    Windows     │      Linux       │ │
│  │  (Swift)   │     (C#)       │  (Rust + GTK)    │ │
│  └────────────┴────────────────┴──────────────────┘ │
│   - Hotkey      - Hotkey         - Hotkey           │
│   - Clipboard   - Clipboard      - Clipboard        │
│   - Keyboard    - Keyboard       - Keyboard         │
│   - UI          - UI             - UI               │
└─────────────────────────────────────────────────────┘
                        ↕ (UniFFI)
┌─────────────────────────────────────────────────────┐
│          Rust Core (Platform-Agnostic)              │
│  - AI Routing Logic                                 │
│  - AI Provider Clients (HTTP)                       │
│  - Memory System (Vector DB + Embeddings)           │
│  - Config Management                                │
│  - PII Filtering                                    │
└─────────────────────────────────────────────────────┘
```

**优势**:
- 新平台只需实现前端层（Swift → C# → GTK）
- Rust 核心完全平台无关，零改动
- 每个平台使用最佳原生 API

## Impact

### 代码变更范围

**删除**:
- `Aether/core/src/hotkey/rdev_listener.rs` (~234 lines)
- `Aether/core/src/clipboard/arboard_manager.rs` (~150 lines)
- `Aether/core/src/input/enigo_simulator.rs` (~200 lines)
- UniFFI 接口定义（~50 lines in aether.udl）
- **总计删除: ~634 lines Rust + FFI**

**新增**:
- `Aether/Sources/Utils/ClipboardManager.swift` (~100 lines)
- `Aether/Sources/Utils/KeyboardSimulator.swift` (~150 lines)
- **总计新增: ~250 lines Swift**

**净减少**: ~384 lines，代码库更简洁

### 性能影响

- **热键延迟**: 减少 ~5-10ms（移除 FFI 调用）
- **剪贴板操作**: 减少 ~2-3ms per operation
- **内存占用**: 减少 ~1-2MB（移除 rdev/arboard/enigo 依赖）

### 兼容性影响

**破坏性变更**:
- UniFFI 接口签名改变（移除系统 API 方法）
- 需要重新生成 Swift bindings
- **不影响外部用户**（Aether 是独立应用，无公开 API）

**迁移策略**:
- Phase 1: 实现 Swift 层功能
- Phase 2: 并行运行两套实现，测试验证
- Phase 3: 切换到 Swift 实现
- Phase 4: 删除 Rust 旧代码

### 文档影响

**需要更新的文档**:
- `CLAUDE.md` - 更新架构说明
- `docs/PLATFORM_NOTES.md` - 更新平台特定实现
- `docs/DEBUGGING_GUIDE.md` - 更新调试指南
- `README.md` - 更新技术栈说明

## Risks

### 技术风险

1. **Swift 实现可能有未发现的边界情况**:
   - 缓解：保留 Rust 代码 2 个版本周期，充分测试
   - 回滚：可快速切回 Rust 实现

2. **不同 macOS 版本的 API 兼容性**:
   - 缓解：使用 `@available` 标注，最低支持 macOS 13
   - 测试：在 macOS 13/14/15 上完整测试

3. **键盘模拟在不同应用中的行为差异**:
   - 缓解：在 20+ 常用应用中测试（VSCode, WeChat, Notes, Safari, etc.）
   - 文档：记录已知不兼容应用

### 项目风险

1. **开发时间估算**:
   - 乐观：2 周
   - 现实：3-4 周（包含测试）
   - 悲观：6 周（如遇重大问题）

2. **与其他进行中的变更冲突**:
   - `redesign-permission-authorization`: 中等冲突（都涉及 AppDelegate）
   - `add-i18n-localization`: 低冲突（仅 UI 字符串）
   - 缓解：合并前同步代码，解决冲突

## Success Criteria

### 功能验证

- [ ] 热键监听在所有测试应用中正常工作
- [ ] 剪贴板读写支持文本和图片
- [ ] 键盘模拟（Cut/Paste/Type）功能正常
- [ ] 完整的 AI 处理流程端到端测试通过
- [ ] 所有现有单元测试和集成测试通过

### 性能验证

- [ ] 热键响应延迟 < 100ms (p95)
- [ ] 剪贴板操作延迟 < 50ms (p95)
- [ ] 内存占用减少 >= 1MB

### 代码质量

- [ ] Swift 代码通过 SwiftLint 检查
- [ ] Rust 代码通过 `cargo clippy` 检查
- [ ] 代码覆盖率保持 >= 80%
- [ ] 所有 deprecated API 已移除

### 文档完整性

- [ ] 所有受影响的文档已更新
- [ ] API 变更已在 CHANGELOG 记录
- [ ] 新增代码有完整的文档注释

## Dependencies

### 阻塞此变更的前置条件

- `redesign-permission-authorization` 必须完成（避免权限检查逻辑冲突）

### 被此变更阻塞的后续工作

- **Windows 客户端开发**: 需要此架构才能清晰地隔离平台代码
- **Linux 客户端开发**: 同上
- **性能优化**: 必须先消除 FFI 开销才能进一步优化

## Alternatives Considered

### 备选方案 1: 保持现状，仅修复 rdev 问题

**方案**: 继续使用 rdev，修复主线程问题

**优点**:
- 改动最小
- 保持跨平台一致性

**缺点**:
- 无法消除 FFI 开销
- 仍需维护 Rust 系统 API 包装
- 未来添加新平台时 Rust 核心变更大

**为什么拒绝**: 治标不治本，不符合长期架构目标

### 备选方案 2: 全部移到 Swift，包括 AI 逻辑

**方案**: 将 AI routing 和 memory 也用 Swift 实现

**优点**:
- 完全原生，无 FFI
- 代码库统一语言

**缺点**:
- Swift 不适合 ML/AI 计算（无成熟的向量数据库、嵌入库）
- 跨平台时需要在 Swift/C#/Rust 三个地方重复实现核心逻辑
- 失去 Rust 的内存安全和并发优势

**为什么拒绝**: 违反"Rust 负责计算"原则，不利于跨平台

### 备选方案 3: 使用统一的跨平台库（如 flutter_rust_bridge）

**方案**: 使用 Flutter + Rust，统一 UI 层

**优点**:
- UI 层也跨平台
- 社区有成熟方案

**缺点**:
- 违反"原生优先"哲学
- Aether 的核心价值是"隐形"，Flutter 不适合做透明悬浮窗
- 增加依赖和复杂度

**为什么拒绝**: 与项目核心价值观冲突

## Implementation Plan

见 `tasks.md`

## Related Changes

- `redesign-permission-authorization` - 权限系统重构（必须先完成）
- `add-i18n-localization` - 国际化（低冲突）
- `enforce-permission-gating` - 权限门控（低冲突）

## Notes

### 关键决策记录

1. **为什么 Linux 还用 Rust + GTK？**
   - 因为 Linux 没有统一的"原生"UI 框架
   - GTK4 有 Rust bindings (`gtk4-rs`)，与核心语言一致
   - 可接受在 Rust 层处理 Linux 系统 API

2. **为什么不用 Swift Package Manager 管理依赖？**
   - Aether 使用 XcodeGen 管理项目
   - 系统框架（AppKit, Cocoa）无需包管理器
   - 保持简单

3. **UniFFI 接口是否需要版本号？**
   - 当前不需要（Aether 是单体应用）
   - 如果未来开放 API，需要添加版本管理

---

**变更 ID**: `refactor-native-api-separation`
**提案日期**: 2025-12-30
**状态**: Draft
**负责人**: Claude Code

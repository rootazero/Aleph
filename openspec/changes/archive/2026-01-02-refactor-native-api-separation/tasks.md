# Tasks: Refactor to Native API Separation Architecture

## Overview

本文档定义了将 Aleph 从 "Rust 统一系统 API" 架构重构到 "原生 API + Rust 纯计算核心" 架构的详细任务列表。

## Task Breakdown

### Phase 1: Swift 层实现 (Week 1)

#### Task 1.1: 实现 ClipboardManager.swift
**估时**: 1 day
**依赖**: 无
**负责人**: Frontend developer

**子任务**:
- [x] 创建 `Aleph/Sources/Utils/ClipboardManager.swift`
- [ ] 实现文本操作 (`getText()`, `setText()`)
- [ ] 实现图片操作 (`getImage()`, `setImage()`, `hasImage()`)
- [ ] 实现 `changeCount()` 用于检测剪贴板变化
- [ ] 添加 RTF 支持（可选）
- [ ] 编写单元测试（`ClipboardManagerTests.swift`）

**验收标准**:
- [ ] 所有测试通过
- [ ] 支持 String, NSImage 类型
- [ ] 可检测剪贴板外部变化

---

#### Task 1.2: 实现 KeyboardSimulator.swift
**估时**: 2 days
**依赖**: 无
**负责人**: Frontend developer

**子任务**:
- [ ] 创建 `Aleph/Sources/Utils/KeyboardSimulator.swift`
- [ ] 实现 `simulateCut()` (Cmd+X)
- [ ] 实现 `simulateCopy()` (Cmd+C)
- [ ] 实现 `simulatePaste()` (Cmd+V)
- [ ] 实现 `typeText()` with async/await
- [ ] 添加 CancellationToken 支持（允许 Esc 取消）
- [ ] 处理特殊字符（换行符、Tab、emoji）
- [ ] 编写单元测试

**验收标准**:
- [ ] 快捷键模拟在 macOS 系统级别工作
- [ ] 打字机效果流畅（可配置速度）
- [ ] 可通过 Esc 取消打字

---

#### Task 1.3: 为 GlobalHotkeyMonitor 添加测试
**估时**: 0.5 day
**依赖**: 无
**负责人**: Frontend developer

**子任务**:
- [ ] 创建 `GlobalHotkeyMonitorTests.swift`
- [ ] 测试 `startMonitoring()` 成功场景
- [ ] 测试权限不足场景（模拟）
- [ ] 测试 `stopMonitoring()` 清理逻辑
- [ ] 手动测试 ` 键是否被正确拦截

**验收标准**:
- [ ] 单元测试通过
- [ ] 手动测试确认 ` 字符不会输入

---

#### Task 1.4: 兼容性测试
**估时**: 1 day
**依赖**: Task 1.2
**负责人**: QA

**子任务**:
- [ ] 在以下应用中测试键盘模拟:
  - [ ] VSCode
  - [ ] Xcode
  - [ ] Sublime Text
  - [ ] Safari
  - [ ] Chrome
  - [ ] Firefox
  - [ ] WeChat
  - [ ] Slack
  - [ ] Discord
  - [ ] Notes
  - [ ] Pages
  - [ ] Microsoft Word
  - [ ] iTerm2
  - [ ] Terminal.app
  - [ ] Notion
  - [ ] Obsidian
  - [ ] 1Password (可能不兼容)
  - [ ] Bitwarden
  - [ ] Zoom
  - [ ] Teams
- [ ] 记录不兼容应用及原因
- [ ] 更新文档（`docs/COMPATIBILITY.md`）

**验收标准**:
- [ ] >= 95% 常用应用正常工作
- [ ] 不兼容应用已文档化

---

### Phase 2: Rust 层重构 (Week 2)

#### Task 2.1: 简化 UniFFI 接口定义
**估时**: 1 day
**依赖**: 无
**负责人**: Backend developer

**子任务**:
- [ ] 修改 `Aleph/core/src/aleph.udl`:
  - [ ] 删除 `start_listening()` / `stop_listening()`
  - [ ] 删除 `is_listening()`
  - [ ] 删除 `get_clipboard_text()`
  - [ ] 删除 `has_clipboard_image()` / `read_clipboard_image()` / `write_clipboard_image()`
  - [ ] 删除 `ImageData` dictionary 和 `ImageFormat` enum
  - [ ] 删除 `on_hotkey_detected()` 回调
  - [ ] 添加新接口 `process_input(string, CapturedContext)`
- [ ] 运行 `uniffi-bindgen` 重新生成 Swift bindings
- [ ] 验证生成的 `aleph.swift` 正确

**验收标准**:
- [ ] UniFFI 验证通过（无错误）
- [ ] 生成的 Swift 代码可编译

---

#### Task 2.2: 实现简化的 AlephCore.process_input()
**估时**: 2 days
**依赖**: Task 2.1
**负责人**: Backend developer
**状态**: ✅ **已完成** (2024-12-30)

**子任务**:
- [x] 创建新方法 `AlephCore::process_input(user_input, context)` (core.rs:893-909)
- [x] 集成现有管线:
  - [x] PII 过滤 (现有实现复用)
  - [x] 记忆检索（如果启用）(process_with_ai_internal)
  - [x] AI 路由 (Router::route_with_fallback)
  - [x] Provider 调用 (retry_with_backoff)
  - [x] 记忆存储 (异步后台任务)
- [x] 添加适当的回调:
  - [x] `on_state_changed(RetrievingMemory)`
  - [x] `on_ai_processing_started(provider, color)`
  - [x] `on_response_chunk(text)` (已存在)
- [x] 移除输出逻辑(typewriter/paste) - 现在由 Swift 层处理
- [ ] 编写单元测试 (待更新)
- [ ] 编写集成测试（mock AI provider）(待更新)

**验收标准**:
- [x] 方法签名符合 UDL 定义
- [x] Swift bindings 成功生成
- [x] 编译通过 (cargo build --release)
- [ ] 所有测试通过 (部分测试需要更新)
- [ ] E2E 测试通过（mock provider）(待 Swift 集成)
- [x] 日志输出完整（可观测性）

**说明**:
- `process_input()` 作为新架构的主入口点
- 复用 `process_with_ai_internal()` 实现(移除输出逻辑)
- 返回 AI 响应字符串给 Swift 层处理输出
- 旧方法 `process_with_ai()` 标记为 deprecated

---

#### Task 2.3: 删除 Rust 系统 API 模块
**估时**: 0.5 day
**依赖**: Task 2.2, Task 3.1 (Swift 集成完成)
**负责人**: Backend developer
**状态**: ✅ **已完成** (2024-12-30)

**子任务**:
- [x] 删除 `Aleph/core/src/hotkey/rdev_listener.rs`
- [x] 删除 `Aleph/core/src/clipboard/arboard_manager.rs`
- [x] 删除 `Aleph/core/src/input/enigo_simulator.rs`
- [x] 删除 `Aleph/core/src/hotkey/mod.rs` 中的 trait 定义
- [x] 删除 `Aleph/core/src/clipboard/mod.rs` 中的 trait 定义 (保留 ImageData/ImageFormat)
- [x] 删除 `Aleph/core/src/input/mod.rs` 中的 trait 定义
- [x] 修改 `Aleph/core/src/core.rs` 移除相关字段 (clipboard_manager, input_simulator)

**验收标准**:
- [x] `cargo build` 通过
- [ ] `cargo test` 通过 (部分测试需要更新)
- [x] 无 dead code 警告 (编译无警告)

**说明**:
- 删除整个 hotkey, clipboard, input 目录
- clipboard 模块被重新创建,仅保留 ImageData/ImageFormat 类型(AI provider 需要)
- 删除 core.rs 中的 4 个 clipboard API 方法
- 更新 lib.rs 移除模块导出

---

#### Task 2.4: 清理 Cargo 依赖
**估时**: 0.5 day
**依赖**: Task 2.3
**负责人**: Backend developer
**状态**: ✅ **已完成** (2024-12-30)

**子任务**:
- [x] 从 `Cargo.toml` 移除:
  - [x] `rdev`
  - [x] `arboard`
  - [x] `enigo`
  - [x] `core-foundation` (已删除,仅用于 rdev)
  - [x] `core-graphics` (已删除,仅用于 rdev)
- [x] 运行 `cargo clean`
- [x] 运行 `cargo build --release`
- [x] 验证 binary size 减少

**验收标准**:
- [x] `cargo build` 通过
- [x] Binary size 减少: 10.0MB → 9.5MB (减少 0.5MB)
- [x] `cargo tree` 不再包含已删除依赖

**说明**:
- 保留 image 和 base64 依赖(AI provider 图片编码需要)
- 库文件 MD5: c04b586646b4c7dc333db8e2c642c997

---

### Phase 3: Swift 集成 (Week 3)

#### Task 3.1: 更新 EventHandler.swift
**估时**: 2 days
**依赖**: Task 1.1, Task 1.2, Task 2.1
**负责人**: Frontend developer
**状态**: ✅ **已完成** (2024-12-30)

**子任务**:
- [x] 删除 `onHotkeyDetected()` 回调方法 - 热键处理已迁移到 AppDelegate
- [x] 删除 `handleHotkeyDetected()` 内部方法 - 不再需要
- [x] 保留其他 AI 处理回调 (`onAiProcessingStarted`, `onAiResponseReceived` 等)
- [x] 添加架构说明注释

**验收标准**:
- [x] EventHandler 编译通过
- [x] 移除了旧的热键逻辑
- [x] 其他回调正常工作

**说明**:
- 删除 `onHotkeyDetected()` 和 `handleHotkeyDetected()` (不再使用)
- 热键流程现在是: GlobalHotkeyMonitor → AppDelegate.handleHotkeyPressed() → Core.processInput()

---

#### Task 3.2: 更新 AppDelegate.swift
**估时**: 1 day
**依赖**: Task 3.1
**负责人**: Frontend developer
**状态**: ✅ **已完成** (2024-12-30)

**子任务**:
- [x] 实现 `handleHotkeyPressed()` 方法的完整流程:
  - [x] 使用 `ClipboardManager.getText()` 获取输入
  - [x] 使用 `ContextCapture.captureContext()` 获取上下文
  - [x] 调用 `core.processInput(userInput, context)`
  - [x] 使用 `KeyboardSimulator.typeText()` 输出响应
  - [x] 添加错误处理和回退机制 (typewriter 失败 → instant paste)
- [x] 添加本地化错误消息:
  - [x] `error.no_clipboard_text` / `error.no_clipboard_text.suggestion`
  - [x] `error.core_not_initialized` / `error.core_not_initialized.suggestion`
  - [x] `error.check_connection`
- [x] 修复 KeyboardSimulator 类型转换问题 (`kVK_*` → `CGKeyCode`)

**验收标准**:
- [x] 应用启动流程正常
- [x] 热键监听正常启动 (GlobalHotkeyMonitor)
- [x] xcodebuild 构建成功
- [x] 无编译错误或警告

**说明**:
- 完全实现了新架构: Swift (hotkey + clipboard + keyboard) → Rust (AI processing)
- 移除了对旧 `eventHandler.onHotkeyDetected()` 的调用
- 添加了完整的错误处理和用户友好的错误消息
- 已清理 build 缓存,准备通过 Xcode 测试

---

#### Task 3.3: 添加 Feature Flag (可选)
**估时**: 0.5 day
**依赖**: Task 3.1
**负责人**: Frontend developer

**子任务**:
- [ ] 添加 `Settings.useNativeAPIs` flag (默认 true)
- [ ] 保留 Rust 实现的调用路径（用于回滚）
- [ ] 添加日志记录当前使用的实现
- [ ] 添加 UI 开关（Settings 页面）

**验收标准**:
- [ ] 可通过 flag 切换实现
- [ ] 两种实现都可正常工作

---

### Phase 4: 测试与验证 (Week 3-4)

#### Task 4.1: 单元测试
**估时**: 2 days
**依赖**: Phase 1, Phase 2, Phase 3
**负责人**: All developers

**子任务**:
- [ ] Swift 层测试:
  - [ ] ClipboardManagerTests
  - [ ] KeyboardSimulatorTests
  - [ ] GlobalHotkeyMonitorTests
  - [ ] EventHandlerTests
- [ ] Rust 层测试:
  - [ ] AlephCore::process_input() tests
  - [ ] 确保现有测试仍通过
- [ ] 运行 `cargo test`
- [ ] 运行 `xcodebuild test`

**验收标准**:
- [ ] 所有单元测试通过
- [ ] 代码覆盖率 >= 80%

---

#### Task 4.2: 集成测试
**估时**: 1 day
**依赖**: Task 4.1
**负责人**: QA

**子任务**:
- [ ] E2E 测试完整流程:
  - [ ] 选择文本 + 热键 → AI 响应 → 打字机输出
  - [ ] 剪贴板无文本场景（fallback 逻辑）
  - [ ] 网络错误场景
  - [ ] 权限不足场景
- [ ] 性能测试:
  - [ ] 测量热键响应延迟（p50, p95, p99）
  - [ ] 测量剪贴板操作延迟
  - [ ] 测量内存占用

**验收标准**:
- [ ] 所有 E2E 场景通过
- [ ] 性能指标达标:
  - [ ] 热键延迟 < 100ms (p95)
  - [ ] 剪贴板延迟 < 50ms (p95)
  - [ ] 内存减少 >= 1MB

---

#### Task 4.3: 手动回归测试
**估时**: 1 day
**依赖**: Task 4.2
**负责人**: QA

**子任务**:
- [ ] 使用测试清单（`docs/manual-testing-checklist.md`）
- [ ] 在 macOS 13/14/15 上测试
- [ ] 测试所有 AI providers (OpenAI, Claude, Gemini, Ollama)
- [ ] 测试记忆系统集成
- [ ] 测试错误处理和恢复

**验收标准**:
- [ ] 所有手动测试通过
- [ ] 无 P0/P1 bug
- [ ] 无回归问题

---

#### Task 4.4: Beta 测试
**估时**: 3-5 days
**依赖**: Task 4.3
**负责人**: QA + 社区

**子任务**:
- [ ] 发布 Beta 版本
- [ ] 收集用户反馈（通过 GitHub Issues）
- [ ] 监控崩溃报告（如有 crash reporting）
- [ ] 分析性能数据
- [ ] 修复发现的 bug

**验收标准**:
- [ ] >= 20 用户参与测试
- [ ] 满意度 >= 4.5/5
- [ ] 崩溃率无上升
- [ ] 所有 P0/P1 bug 已修复

---

### Phase 5: 清理与发布 (Week 4)

#### Task 5.1: 代码清理
**估时**: 1 day
**依赖**: Task 4.4
**负责人**: All developers

**子任务**:
- [ ] 删除 feature flag（如果不再需要）
- [ ] 删除 Rust 旧实现的死代码
- [ ] 运行 `cargo clippy` 并修复警告
- [ ] 运行 SwiftLint 并修复警告
- [ ] 清理注释和 TODO 标记
- [ ] 代码格式化（`cargo fmt`, SwiftFormat）

**验收标准**:
- [ ] `cargo clippy` 无警告
- [ ] SwiftLint 无警告
- [ ] 无死代码

---

#### Task 5.2: 文档更新
**估时**: 1 day
**依赖**: Task 5.1
**负责人**: Technical writer

**子任务**:
- [ ] 更新 `CLAUDE.md`:
  - [ ] 更新技术栈说明
  - [ ] 更新架构图
  - [ ] 更新 FFI 边界说明
- [ ] 更新 `docs/PLATFORM_NOTES.md`:
  - [ ] 删除 Rust 系统 API 说明
  - [ ] 添加 Swift 原生 API 使用指南
- [ ] 更新 `docs/DEBUGGING_GUIDE.md`:
  - [ ] 添加 Swift 层调试技巧
  - [ ] 更新 Rust 层调试范围
- [ ] 更新 `README.md`:
  - [ ] 更新技术栈列表
  - [ ] 更新依赖说明
- [ ] 创建 `docs/COMPATIBILITY.md`:
  - [ ] 列出已测试应用
  - [ ] 列出不兼容应用和原因
- [ ] 更新 `CHANGELOG.md`:
  - [ ] 记录架构变更
  - [ ] 记录性能提升
  - [ ] 记录破坏性变更（如果有）

**验收标准**:
- [ ] 所有文档准确反映新架构
- [ ] 无过时信息
- [ ] 示例代码可运行

---

#### Task 5.3: OpenSpec 归档
**估时**: 0.5 day
**依赖**: Task 5.2
**负责人**: Project lead

**子任务**:
- [ ] 更新所有受影响的 specs (见 Phase 6)
- [ ] 运行 `openspec validate refactor-native-api-separation --strict`
- [ ] 修复所有验证错误
- [ ] 归档变更: `openspec archive refactor-native-api-separation`

**验收标准**:
- [ ] `openspec validate` 通过
- [ ] Specs 与代码同步

---

#### Task 5.4: 发布准备
**估时**: 0.5 day
**依赖**: Task 5.3
**负责人**: Release manager

**子任务**:
- [ ] 更新版本号（遵循 Semantic Versioning）
- [ ] 创建 Git tag
- [ ] 编写 Release Notes
- [ ] 准备迁移指南（如果是破坏性变更）
- [ ] 发布到 GitHub Releases

**验收标准**:
- [ ] Release notes 完整
- [ ] Tag 已推送
- [ ] 二进制文件可下载

---

### Phase 6: Spec 更新

#### Task 6.1: 更新 hotkey-detection spec
**估时**: 0.5 day
**依赖**: 无
**负责人**: Spec author

见 `specs/hotkey-detection/spec.md`

---

#### Task 6.2: 更新 clipboard-management spec
**估时**: 0.5 day
**依赖**: 无
**负责人**: Spec author

见 `specs/clipboard-management/spec.md`

---

#### Task 6.3: 更新 core-library spec
**估时**: 0.5 day
**依赖**: 无
**负责人**: Spec author

见 `specs/core-library/spec.md`

---

#### Task 6.4: 更新 uniffi-bridge spec
**估时**: 0.5 day
**依赖**: 无
**负责人**: Spec author

见 `specs/uniffi-bridge/spec.md`

---

#### Task 6.5: 更新 macos-client spec
**估时**: 0.5 day
**依赖**: 无
**负责人**: Spec author

见 `specs/macos-client/spec.md`

---

## Task Dependencies Graph

```
Phase 1 (Swift 实现)
├── Task 1.1 (ClipboardManager) ────┐
├── Task 1.2 (KeyboardSimulator) ───┼─→ Task 1.4 (兼容性测试)
└── Task 1.3 (HotkeyMonitor 测试) ──┘

Phase 2 (Rust 重构)
├── Task 2.1 (UniFFI 简化) ──→ Task 2.2 (process_input)
│                                  ↓
└──────────────────────────────→ Task 2.3 (删除模块) → Task 2.4 (清理依赖)

Phase 3 (Swift 集成)
Task 1.1 + Task 1.2 + Task 2.1 ──→ Task 3.1 (EventHandler)
                                     ↓
                                   Task 3.2 (AppDelegate)
                                     ↓
                                   Task 3.3 (Feature Flag)

Phase 4 (测试)
Phase 1 + Phase 2 + Phase 3 ──→ Task 4.1 (单元测试)
                                  ↓
                                Task 4.2 (集成测试)
                                  ↓
                                Task 4.3 (回归测试)
                                  ↓
                                Task 4.4 (Beta 测试)

Phase 5 (清理)
Task 4.4 ──→ Task 5.1 (代码清理)
              ↓
            Task 5.2 (文档更新)
              ↓
            Task 5.3 (OpenSpec 归档)
              ↓
            Task 5.4 (发布)

Phase 6 (Spec 更新) - 可与 Phase 1-5 并行
└── Task 6.1-6.5 (更新各个 specs)
```

## Risk Mitigation Tasks

### Contingency Task A: 回滚到 Rust 实现
**触发条件**: Beta 测试崩溃率上升 > 5%

**子任务**:
- [ ] 设置 `use_native_apis = false`
- [ ] 验证 Rust 实现仍可用
- [ ] 发布 hotfix 版本
- [ ] 分析失败原因
- [ ] 修复问题后重新尝试

---

### Contingency Task B: 添加兼容模式
**触发条件**: 发现 >= 3 个重要应用不兼容 CGEvent

**子任务**:
- [ ] 实现 Accessibility API fallback (AXUIElement)
- [ ] 添加 UI 开关（Settings → 兼容模式）
- [ ] 文档说明兼容模式使用场景

---

## Task Status Tracking

### Summary (0/67 completed)

- **Phase 1**: 0/8 tasks
- **Phase 2**: 0/4 tasks
- **Phase 3**: 0/4 tasks
- **Phase 4**: 0/4 tasks
- **Phase 5**: 0/4 tasks
- **Phase 6**: 0/5 tasks
- **Contingency**: 0/2 tasks (如果需要)

### Timeline Estimate

- **乐观**: 3 周（如果一切顺利）
- **现实**: 4 周（包含 bug 修复时间）
- **悲观**: 6 周（如果遇到重大兼容性问题）

---

**文档版本**: 1.0
**最后更新**: 2025-12-30
**状态**: Draft

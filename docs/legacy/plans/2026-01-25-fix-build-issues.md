# 修复构建警告实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复 Xcode 构建中的 21 个警告问题（9 个 Rust 警告 + 12 个 Swift 警告）

**Architecture:** 分别处理 Rust 核心代码警告和 Swift UI 代码警告，使用自动修复工具和手动清理

**Tech Stack:** Rust, Swift, SwiftLint, cargo fix

---

## Task 1: 修复 Rust 未使用导入警告（3个）

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs:6` (移除未使用的 Result 导入)
- Modify: `core/src/agent_loop/agent_loop.rs:17` (移除未使用的 Thinking 导入)
- Modify: `core/src/mcp/auth/callback.rs:164` (移除未使用的 AsyncWriteExt 导入)

**Step 1: 读取第一个文件查看导入上下文**

```bash
head -n 20 core/src/agent_loop/agent_loop.rs
```

**Step 2: 移除 agent_loop.rs 中的未使用导入**

移除第 6 行的 `crate::error::Result` 和第 17 行的 `Thinking`

**Step 3: 读取 callback.rs 查看导入**

```bash
sed -n '160,170p' core/src/mcp/auth/callback.rs
```

**Step 4: 移除 callback.rs 中的未使用导入**

移除第 164 行的 `AsyncWriteExt`

**Step 5: 验证编译**

```bash
cd core && cargo build 2>&1 | grep "unused import"
```

Expected: 不再有 "unused import" 警告

**Step 6: 提交更改**

```bash
git add core/src/agent_loop/agent_loop.rs core/src/mcp/auth/callback.rs
git commit -m "fix(core): remove unused imports in agent_loop and mcp modules"
```

---

## Task 2: 修复 Rust 未使用类型和字段警告（6个）

**Files:**
- Modify: `core/src/agent_loop/traits.rs:36` (移除或添加注释到 ExecutorTrait)
- Modify: `core/src/agents/sub_agents/coordinator.rs:160,170` (处理未使用的 request_id 字段)
- Modify: `core/src/components/subagent_handler.rs:31` (处理未使用的 request_id 字段)
- Modify: `core/src/dispatcher/registry/registration.rs:242` (处理未使用的方法)
- Modify: `core/src/extension/runtime/mod.rs:605` (处理未使用的字段)

**Step 1: 检查 ExecutorTrait 是否真的未使用**

```bash
grep -r "ExecutorTrait" core/src/ --include="*.rs"
```

**Step 2: 处理 ExecutorTrait**

如果确实未使用，添加 `#[allow(dead_code)]` 或删除定义

**Step 3: 检查 request_id 字段用途**

```bash
grep -B 5 -A 10 "request_id" core/src/agents/sub_agents/coordinator.rs
grep -B 5 -A 10 "request_id" core/src/components/subagent_handler.rs
```

**Step 4: 处理未使用的 request_id 字段**

为结构体添加 `#[allow(dead_code)]` 属性或在字段名前加 `_` 前缀

**Step 5: 检查 register_extension_skills 方法**

```bash
grep -r "register_extension_skills" core/src/ --include="*.rs"
```

**Step 6: 处理未使用的方法**

添加 `#[allow(dead_code)]` 或删除方法（如果确定不需要）

**Step 7: 处理 extension/runtime 未使用字段**

```bash
sed -n '600,610p' core/src/extension/runtime/mod.rs
```

为字段添加 `#[allow(dead_code)]` 或使用 `_` 前缀

**Step 8: 验证编译**

```bash
cd core && cargo build 2>&1 | grep "warning:"
```

Expected: 所有 Rust 警告消失

**Step 9: 提交更改**

```bash
git add core/src/agent_loop/traits.rs core/src/agents/sub_agents/coordinator.rs core/src/components/subagent_handler.rs core/src/dispatcher/registry/registration.rs core/src/extension/runtime/mod.rs
git commit -m "fix(core): suppress warnings for intentionally unused code"
```

---

## Task 3: 修复 Swift SwiftLint 格式警告（4个）

**Files:**
- Modify: `platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift:118` (移除尾随空格)
- Modify: `platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift:144` (修复缩进)
- Modify: `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift:923` (移除未使用变量)
- Modify: `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift:1041` (添加尾随换行)

**Step 1: 修复 InputAreaView.swift 第 118 行尾随空格**

移除行尾空格

**Step 2: 修复 InputAreaView.swift 第 144 行缩进**

将第 145 行的闭包结束括号缩进从 16 改为 17

**Step 3: 读取 UnifiedConversationViewModel.swift 第 923 行上下文**

```bash
sed -n '920,930p' platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift
```

**Step 4: 修复未使用的 eventType 变量**

将 `let eventType = event.eventType` 改为 `let _ = event.eventType` 或直接删除

**Step 5: 在文件末尾添加换行**

确保 UnifiedConversationViewModel.swift 文件末尾有且仅有一个换行符

**Step 6: 验证 SwiftLint**

```bash
cd platforms/macos && swiftlint lint --path Aether/Sources/MultiTurn/
```

Expected: 不再有 SwiftLint 警告

**Step 7: 提交更改**

```bash
git add platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationViewModel.swift
git commit -m "fix(macos): fix SwiftLint violations in MultiTurn module"
```

---

## Task 4: 处理 UniFFI 生成代码的 Sendable 警告（12个）

**Files:**
- Review: `platforms/macos/Aether/Sources/Generated/aether.swift`

**Step 1: 分析警告来源**

```bash
head -n 20 platforms/macos/Aether/Sources/Generated/aether.swift
```

Expected: 确认这是 UniFFI 自动生成的代码

**Step 2: 检查是否有 UniFFI 配置可以禁用警告**

查看 `core/uniffi.toml` 或相关配置

**Step 3: 在 Xcode 项目中禁用此文件的警告**

由于这是生成代码，最佳实践是在编译设置中禁用此文件的 Sendable 警告，或在文件顶部添加：

```swift
// swiftlint:disable redundant_sendable
```

**Step 4: 重新生成 UniFFI 绑定（如果需要）**

```bash
cd core && cargo build
```

**Step 5: 验证 Xcode 构建**

```bash
cd platforms/macos && xcodebuild -scheme Aether -configuration Debug 2>&1 | grep -c "Redundant conformance"
```

Expected: 0（如果添加了 swiftlint:disable）

**Step 6: 提交更改（如果修改了配置）**

```bash
git add platforms/macos/Aether/Sources/Generated/aether.swift
git commit -m "fix(macos): suppress Sendable warnings in UniFFI generated code"
```

---

## Task 5: 最终验证

**Files:**
- Verify: 整个项目构建

**Step 1: 完整 Rust 构建验证**

```bash
cd core && cargo clean && cargo build 2>&1 | tee /tmp/rust-build.log
grep -c "warning:" /tmp/rust-build.log
```

Expected: 0 warnings

**Step 2: 完整 macOS 构建验证**

```bash
cd platforms/macos && xcodebuild clean && xcodebuild -scheme Aether -configuration Debug 2>&1 | tee /tmp/xcode-build.log
grep -c "warning:" /tmp/xcode-build.log | grep -v "Sendable"
```

Expected: 0 warnings（除了可能的 Sendable 警告如果未禁用）

**Step 3: 运行测试确保功能未受影响**

```bash
cd core && cargo test
```

Expected: 所有测试通过

**Step 4: 创建总结提交**

```bash
git add -A
git commit -m "fix: resolve all 21 build warnings (Rust + Swift)"
```

**Step 5: 输出清理报告**

打印警告修复前后对比：
- Rust 警告: 9 → 0
- Swift SwiftLint: 4 → 0
- UniFFI Sendable: 12 → 已禁用或接受

---

## 注意事项

1. **Rust 未使用代码**: 优先检查是否为待实现功能预留的接口，使用 `#[allow(dead_code)]` 而非直接删除
2. **SwiftLint**: 确保修复后代码仍符合项目风格指南
3. **UniFFI 生成代码**: 不要手动修改，通过配置或编译器指令管理警告
4. **测试**: 每个 Task 完成后都应该构建验证，避免引入新问题
5. **提交粒度**: 按功能分类提交（Rust imports、Rust dead code、Swift format、UniFFI）

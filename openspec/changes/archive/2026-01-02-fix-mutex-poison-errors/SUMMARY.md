# Aleph Bug 修复提案 - Mutex Poison Errors

**日期**: 2025-12-31
**状态**: 准备实施
**优先级**: P0 (紧急)

## 🐛 问题总结

你遇到的两个 bug 都是由同一个根本原因导致的：**Rust Core 中存在 11 个不安全的 Mutex 锁调用**。

### Bug 1: 选中文本时出现 PoisonError 弹窗
- **症状**: `called Result::unwrap() on an Err value: PoisonError { .. }`
- **原因**: Mutex 被"毒化"后，所有 `.unwrap()` 调用都会 panic
- **影响**: 应用完全无法使用

### Bug 2: 未选中文本时无反应（噪音）
- **症状**: 只有 beep 声音，没有 Halo，没有 AI 响应
- **原因**: PoisonError 导致 Core 初始化失败或处于不一致状态
- **影响**: 核心功能完全失效

### Bug 3: 点击 Settings 菜单崩溃
- **症状**: `EXC_BAD_ACCESS` 崩溃
- **原因**: Core 对象因 Mutex poison 而损坏，成为悬空指针
- **影响**: 无法访问设置

---

## 💡 根本原因

### 已发现的问题代码

在 `Aleph/core/src/core.rs` 中有 **11 处不安全的 Mutex 调用**：

```rust
// ❌ 不安全 - 会导致 panic
let is_typing = *self.is_typewriting.lock().unwrap();        // 2 处
let mut last_request = self.last_request.lock().unwrap();     // 3 处
let current_context = self.current_context.lock().unwrap();   // 4 处
```

### Mutex Poisoning 机制

1. 当任何线程在持有 Mutex 锁时发生 panic
2. Rust 会将该 Mutex 标记为"poisoned"（毒化）
3. 后续所有 `.unwrap()` 调用都会 panic
4. **级联效应**：一个 panic 会导致所有后续操作都失败

### 为什么之前的修复不完整？

根据 `MUTEX_POISON_FIX.md`，之前已经修复了 `config` Mutex 的 22 处调用，但**遗漏了其他 3 个 Mutex**：
- `is_typewriting`
- `last_request`
- `current_context`

---

## ✅ 解决方案

### 核心修复方法

将所有不安全的 `.unwrap()` 替换为安全的恢复模式：

```rust
// ✅ 安全 - 从 poison 状态恢复
let is_typing = *self.is_typewriting.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in is_typewriting, recovering");
    e.into_inner()  // 提取数据，即使 Mutex 被毒化
});
```

### 为什么这样做是安全的？

- Mutex poisoning 是一个**锁状态问题**，不是数据损坏
- 在我们的场景中：
  - Config 数据是只读的
  - Context 数据是追加式的
  - Request 数据是原子更新的
- **数据本身没有损坏**，只是锁的状态被标记了
- 提取数据并继续运行是安全的

---

## 📋 实施计划

### Phase 1: 修复 Mutex 操作（紧急）⚠️

**预计时间**: 1-2 小时

#### 任务清单

1. **修复 is_typewriting Mutex** (2 处)
   - Line 433: `cancel_typewriter()` 方法
   - Line 446: `is_typewriting()` 方法

2. **修复 last_request Mutex** (3 处)
   - Line 460: `retry_last_request()` 方法
   - Line 517: `store_request_context()` 方法
   - Line 527: `clear_request_context()` 方法

3. **修复 current_context Mutex** (4 处)
   - Line 635: `set_current_context()` 方法
   - Line 684: `store_interaction_memory()` 方法
   - Line 768: `retrieve_and_augment_prompt()` 方法
   - Line 1444: 其他调用

4. **重新构建 Rust Core**
   ```bash
   cd Aleph/core
   cargo clean
   cargo build --release
   cargo run --bin uniffi-bindgen -- generate --library target/release/libalephcore.dylib --language swift --out-dir ../Sources/Generated/
   cp target/release/libalephcore.dylib ../Frameworks/
   ```

5. **在 Xcode 中重新构建并测试**
   ```bash
   open Aleph.xcodeproj
   # Cmd+Shift+K (Clean)
   # Cmd+B (Build)
   # Cmd+R (Run)
   ```

### Phase 2: 初始化验证（高优先级）

**预计时间**: 2-3 小时

- 添加 `isInitialized()` 方法到 Rust Core
- 在 Swift 层检查初始化状态
- 提供清晰的错误提示

### Phase 3: 错误处理改进（中优先级）

**预计时间**: 3-4 小时

- 在 Swift 层添加 try-catch
- 改进错误消息和建议
- 添加本地化支持

---

## 🧪 测试计划

### 场景 1: 选中文本（之前 PoisonError）
1. 打开 Notes.app
2. 输入："测试选中文本"
3. 全选 (Cmd+A)
4. 按 `` ` `` 键
5. ✅ 验证：AI 正常处理文本，无错误
6. ✅ 验证：响应正常输出

### 场景 2: 未选中文本（之前无反应）
1. 打开 Notes.app
2. 输入："测试未选中文本"
3. 不选中文本（光标在文本中）
4. 按 `` ` `` 键
5. ✅ 验证：Halo 出现在光标处
6. ✅ 验证：无 beep 声音
7. ✅ 验证：AI 正常处理和响应

### 场景 3: Settings 菜单（之前崩溃）
1. 启动 Aleph
2. 点击菜单栏图标
3. 点击 "Settings"
4. ✅ 验证：设置窗口正常打开
5. ✅ 验证：无崩溃

---

## 📊 预期结果

### 功能指标
- ✅ PoisonError 崩溃率：0%
- ✅ 未选中文本场景：100% 成功
- ✅ Settings 菜单崩溃：0%
- ✅ Mutex poison 恢复率：100%

### 用户体验
- ✅ 无 PoisonError 弹窗
- ✅ 无静默失败
- ✅ 清晰的错误提示（如果需要）
- ✅ 应用保持稳定，即使发生单个 panic

---

## 📁 相关文件

### OpenSpec 提案文档
- `openspec/changes/fix-mutex-poison-errors/proposal.md` - 完整提案
- `openspec/changes/fix-mutex-poison-errors/tasks.md` - 详细任务清单

### 需要修改的代码
- `Aleph/core/src/core.rs` - Rust Core（11 处修改）
- `Aleph/core/src/aleph.udl` - UniFFI 接口（Phase 2）
- `Aleph/Sources/AppDelegate.swift` - Swift 层（Phase 2-3）

### 参考文档
- `docs/MUTEX_POISON_FIX.md` - 之前的部分修复记录
- `docs/CURRENT_ISSUE_DEBUG.md` - 调试日志
- Crash logs: `~/Library/Logs/DiagnosticReports/Aleph-2025-12-31-*.ips`

---

## 🚀 下一步

### 立即开始（推荐）

我可以帮你立即实施 Phase 1 的修复：

1. **修复所有 11 处 Mutex unwrap 调用**
2. **重新构建 Rust Core**
3. **生成 UniFFI 绑定**
4. **更新 dylib 文件**

修复完成后，你只需要：
- 在 Xcode 中 Clean Build (Cmd+Shift+K)
- Build (Cmd+B)
- Run (Cmd+R)
- 测试上述 3 个场景

### 预计完成时间

- **Phase 1 修复**: 10-15 分钟（自动化）
- **用户测试**: 5-10 分钟
- **总计**: ~30 分钟即可看到效果

---

## ❓ 常见问题

### Q: 为什么不直接重启应用来避免 poison？
**A**: 重启会丢失用户状态，而且无法保证下次不会再次发生。正确的方式是让应用从 poison 状态恢复。

### Q: 这会掩盖底层的 bug 吗？
**A**: 不会。我们添加了 warning 日志，可以追踪到原始 panic 的位置。如果 poison 恢复变得频繁，我们会收到警报。

### Q: 性能影响如何？
**A**: 几乎为零。`unwrap_or_else()` 只在 poison 发生时才执行恢复路径（极少），正常路径没有任何开销。

### Q: 如果修复后还有问题怎么办？
**A**: 我们可以：
1. 回退到之前的 `libalephcore.dylib`（修复前先备份）
2. 查看详细日志定位新问题
3. 创建针对性的 hotfix

---

**准备好开始修复了吗？** 🛠️

只需回复确认，我将立即开始实施 Phase 1 的所有修复。

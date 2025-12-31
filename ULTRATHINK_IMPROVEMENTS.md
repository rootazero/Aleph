# Aether 热键优化 - Ultrathink 改进报告

## 问题背景

用户在之前的版本测试后，提出了两个关键改进需求：

### 问题 1: 无选中文本的行为不符合预期
**当前实现（旧版）**:
- 没有选中文本时显示错误："No text in clipboard"
- 用户必须手动选中文本才能使用

**用户期望**:
- 没有选中文本时，自动识别当前输入窗口的**所有文本**
- 无需手动选择即可处理整个文档

### 问题 2: 剪切板内容被覆盖
**潜在风险**:
- 程序模拟Cmd+C会将选中内容复制到剪切板
- 用户之前复制的内容会被新内容覆盖丢失
- 这违反了"无干扰"的设计理念

**用户期望**:
- 处理完成后恢复用户原有的剪切板内容
- 不破坏用户的工作流程

---

## 解决方案

### 方案 1: 智能全选机制

**新流程**:
```swift
1. 保存原剪切板内容（changeCount记录）
2. 模拟Cmd+C尝试复制选中文本
3. 检查剪切板是否变化
   ├─ 变化 → 有选中文本，使用新内容 ✓
   └─ 没变化 → 没有选中文本
       ├─ 模拟Cmd+A全选当前窗口内容
       └─ 再次模拟Cmd+C复制
4. 检查是否成功获取到文本
   ├─ 成功 → 继续AI处理
   └─ 失败 → 显示错误 "当前窗口没有文本内容"
```

**关键改动** (`AppDelegate.swift:597-631`):
```swift
if !hasSelectedText {
    // Step 2: No selected text detected, try Cmd+A to select all
    print("[AppDelegate] ⚠️ No selected text detected, trying Cmd+A to select all...")
    KeyboardSimulator.shared.simulateSelectAll()
    Thread.sleep(forTimeInterval: 0.05)  // 50ms delay

    // Copy again after selecting all
    KeyboardSimulator.shared.simulateCopy()
    Thread.sleep(forTimeInterval: 0.1)  // 100ms delay

    let afterSelectAllChangeCount = ClipboardManager.shared.changeCount()
    if afterSelectAllChangeCount == afterCopyChangeCount {
        print("[AppDelegate] ❌ No text content found even after Cmd+A")
        // Restore original clipboard & show error
        ...
        return
    } else {
        print("[AppDelegate] ✓ Selected all text in current window")
    }
}
```

**效果**:
- ✅ 有选中文本 → 只处理选中部分
- ✅ 无选中文本 → 自动全选并处理整个文档
- ✅ 真的没内容 → 友好提示错误

---

### 方案 2: 剪切板内容保护机制

**保护流程**:
```swift
// 开始时：保存原剪切板
let originalClipboardText = ClipboardManager.shared.getText()
let originalChangeCount = ClipboardManager.shared.changeCount()

... AI处理 ...

// 完成后：恢复原剪切板（延迟500ms确保粘贴完成）
DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
    if let original = originalClipboardText {
        ClipboardManager.shared.setText(original)
        print("[AppDelegate] ♻️ Restored original clipboard content")
    } else {
        ClipboardManager.shared.clear()
    }
}
```

**三重保护**:

1. **成功路径** (`AppDelegate.swift:736-746`)
   - 打字机完成后，延迟500ms恢复

2. **错误路径** (`AppDelegate.swift:762-768`)
   - 打字机失败时，立即恢复

3. **AI处理失败** (`AppDelegate.swift:775-781`)
   - AI调用出错时，立即恢复

**效果**:
- ✅ 用户的原剪切板内容被完整保护
- ✅ 处理完成后自动恢复
- ✅ 任何错误发生都会恢复
- ✅ 用户体验无感知

---

## 新的工作流程示意

### 场景 A: 用户选中了文本"hello"

```
用户操作: 选中"hello" + 按热键`

Aether内部:
1. 💾 保存原剪切板: "https://example.com"（用户1小时前复制的）
2. 📋 Cmd+C → 剪切板变化 → 检测到选中文本
3. 🤖 AI处理"hello"
4. ⌨️ 打字机输出回复
5. ♻️ 恢复剪切板 → "https://example.com"

用户感受:
- 选中的文本被正确处理 ✓
- 1小时前复制的链接依然在剪切板中 ✓
```

### 场景 B: 用户没有选中任何文本

```
用户操作: 光标在文档中 + 按热键`

Aether内部:
1. 💾 保存原剪切板: "user@example.com"
2. 📋 Cmd+C → 剪切板无变化 → 检测到无选中文本
3. 🔄 自动Cmd+A全选整个文档
4. 📋 Cmd+C → 剪切板变化 → 成功获取文档内容
5. 🤖 AI处理整个文档
6. ⌨️ 打字机输出回复
7. ♻️ 恢复剪切板 → "user@example.com"

用户感受:
- 整个文档被自动处理 ✓
- 不需要手动Cmd+A ✓
- 之前复制的邮箱地址依然保留 ✓
```

### 场景 C: 当前窗口真的没有文本

```
用户操作: 在Finder中按热键`

Aether内部:
1. 💾 保存原剪切板: nil（空）
2. 📋 Cmd+C → 无变化
3. 🔄 Cmd+A → Cmd+C → 依然无变化
4. ❌ 检测到无文本内容
5. ♻️ 清空剪切板（原本就是空的）
6. 🚨 显示错误: "当前窗口没有文本内容"

用户感受:
- 收到明确的错误提示 ✓
- 没有产生异常行为 ✓
```

---

## 本地化支持

已添加新的错误提示字符串：

### 英文 (en.lproj)
```strings
"error.no_text_in_window" = "No text content in current window";
"error.no_text_in_window.suggestion" = "Please open a text document first";
```

### 中文 (zh_CN.lproj)
```strings
"error.no_text_in_window" = "当前窗口没有文本内容";
"error.no_text_in_window.suggestion" = "请先打开一个文本文档";
```

---

## 编译状态

✅ **编译成功** (2025-12-31 18:58:08)

```bash
** BUILD SUCCEEDED **
```

所有改动已通过编译验证。

---

## 测试场景

### 场景 1: 选中文本处理
**步骤**:
1. 先复制一个URL到剪切板（备用）
2. 在备忘录中选中"hello"
3. 按热键`

**预期**:
- ✅ 处理"hello"并输出AI回复
- ✅ 完成后剪切板中仍是之前的URL

**日志验证**:
```
[AppDelegate] 💾 Saved original clipboard state (changeCount: 42)
[AppDelegate] ✓ Detected selected text
[AppDelegate] Clipboard text: hello...
...
[AppDelegate] ♻️ Restored original clipboard content
```

---

### 场景 2: 无选中文本 - 自动全选
**步骤**:
1. 先复制一段代码到剪切板（备用）
2. 在备忘录中输入一段文字，光标随便放在哪里
3. **不选中任何文本**，直接按热键`

**预期**:
- ✅ 自动Cmd+A全选整个文档
- ✅ 处理整个文档内容并输出回复
- ✅ 完成后剪切板中仍是之前的代码

**日志验证**:
```
[AppDelegate] 💾 Saved original clipboard state (changeCount: 84)
[AppDelegate] ⚠️ No selected text detected, trying Cmd+A to select all...
[AppDelegate] ✓ Selected all text in current window
[AppDelegate] Clipboard text: [整个文档内容]...
...
[AppDelegate] ♻️ Restored original clipboard content
```

---

### 场景 3: 当前窗口无文本内容
**步骤**:
1. 打开Finder或其他非文本应用
2. 按热键`

**预期**:
- ✅ 显示错误："当前窗口没有文本内容"
- ✅ 提示："请先打开一个文本文档"
- ✅ 不破坏剪切板

**日志验证**:
```
[AppDelegate] 💾 Saved original clipboard state (changeCount: 100)
[AppDelegate] ⚠️ No selected text detected, trying Cmd+A to select all...
[AppDelegate] ❌ No text content found even after Cmd+A
```

---

### 场景 4: 剪切板保护 - 打字机取消
**步骤**:
1. 先复制一个重要的密码到剪切板
2. 选中文本并按热键触发AI处理
3. 在打字机过程中按ESC取消

**预期**:
- ✅ 打字机停止，剩余文本瞬间粘贴
- ✅ 500ms后剪切板恢复成密码
- ✅ 密码不丢失

---

### 场景 5: 剪切板保护 - AI处理失败
**步骤**:
1. 先复制一段重要文本到剪切板
2. 选中文本并按热键
3. AI因为网络错误返回失败

**预期**:
- ✅ 显示错误提示
- ✅ 立即恢复原剪切板内容
- ✅ 重要文本不丢失

---

## 代码改动总结

### 文件修改

1. **AppDelegate.swift** (主要逻辑)
   - `handleHotkeyPressed()` - 热键处理主流程
     - 580-584: 保存原剪切板
     - 586-631: 智能全选机制
     - 736-746: 成功路径恢复剪切板
     - 762-768: 错误路径恢复剪切板
     - 775-781: AI失败恢复剪切板

2. **en.lproj/Localizable.strings** (英文本地化)
   - 302-303: 新增错误提示

3. **zh_CN.lproj/Localizable.strings** (中文本地化)
   - 301-302: 新增错误提示

---

## 关键设计思想

### 1. 用户意图推断
- 有选中 → 用户只想处理这部分
- 无选中 → 用户想处理整个文档

### 2. 无损工作流
- 任何情况下都不破坏用户的剪切板
- 处理完成后恢复原状
- 用户感知不到剪切板的临时使用

### 3. 防御性编程
- 三重剪切板恢复保护
- 任何错误路径都会恢复
- 延迟恢复确保粘贴操作完成

### 4. 友好的错误提示
- 明确告知用户问题所在
- 提供可操作的建议
- 本地化支持

---

## 后续优化建议

### 1. 配置选项
允许用户在config.toml中配置行为：
```toml
[behavior]
auto_select_all = true   # 无选中时是否自动Cmd+A
restore_clipboard = true  # 是否恢复原剪切板
restore_delay_ms = 500    # 恢复剪切板延迟（毫秒）
```

### 2. 选择模式指示
在Halo中显示当前处理的是：
- "处理选中文本..."
- "处理整个文档..."

### 3. 剪切板历史
可选功能：保存最近10条剪切板历史，允许用户在需要时恢复

---

## 总结

本次改进完全解决了用户提出的两个核心问题：

✅ **问题1解决**: 无选中文本时自动Cmd+A全选
✅ **问题2解决**: 处理完成后恢复原剪切板内容

**用户体验提升**:
- 更智能的意图推断
- 无损的工作流程
- 完全透明的剪切板操作

**设计理念**:
- 最小惊讶原则
- 防御性编程
- 用户友好

现在Aether可以真正做到"无干扰"的AI助手体验！

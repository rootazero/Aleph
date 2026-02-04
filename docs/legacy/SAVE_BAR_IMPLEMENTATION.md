# Save Bar 保存逻辑完整实现报告

## 实施概述

本次实施完成了 `unify-settings-save-bar` 变更提案中缺失的关键保存逻辑,确保用户在有未保存内容时进行窗口关闭或菜单切换操作时会收到提示。

## 实施的功能

### 1. 窗口关闭拦截 (Window Close Interception)

**实现位置**: `Aleph/Sources/Components/Window/RootContentView.swift`

**核心组件**:
- `SettingsWindowDelegate`: NSWindowDelegate 实现类
  - 实现 `windowShouldClose(_:)` 方法
  - 检查 `saveBarState.hasUnsavedChanges` 状态
  - 如果有未保存内容,弹出确认对话框

**用户体验流程**:
1. 用户修改设置但未点击保存
2. 用户尝试关闭窗口(点击红色关闭按钮或按 Cmd+W)
3. 系统弹出对话框:
   - **保存**: 保存更改并关闭窗口
   - **放弃修改**: 丢弃更改并关闭窗口
   - **取消**: 保持窗口打开,继续编辑

**代码片段**:
```swift
func windowShouldClose(_ sender: NSWindow) -> Bool {
    guard let saveBarState = saveBarState else {
        return true  // No save state, allow close
    }

    // If no unsaved changes, allow close
    guard saveBarState.hasUnsavedChanges else {
        return true
    }

    // Show confirmation dialog
    let alert = NSAlert()
    // ... 显示对话框并处理用户选择
}
```

### 2. 菜单切换拦截 (Tab Switch Interception)

**实现位置**: `Aleph/Sources/Components/Window/RootContentView.swift`

**核心逻辑**:
- 在 `onChange(of: selectedTab)` 中检查未保存状态
- 如果有未保存内容,弹出确认对话框
- 用户取消则恢复到原来的标签页

**用户体验流程**:
1. 用户在某个设置标签页修改内容但未保存
2. 用户尝试切换到其他标签页
3. 系统弹出对话框:
   - **保存**: 保存更改并切换标签页
   - **放弃修改**: 丢弃更改并切换标签页
   - **取消**: 保持在当前标签页,继续编辑

**代码片段**:
```swift
.onChange(of: selectedTab) { oldTab, newTab in
    // Check for unsaved changes before allowing tab switch
    if saveBarState.hasUnsavedChanges {
        // Show confirmation dialog
        Task { @MainActor in
            let shouldProceed = showUnsavedChangesDialog(action: "switch tabs")
            if shouldProceed {
                // Reset save bar state when switching tabs
                saveBarState.reset()
            } else {
                // Revert tab selection (prevent switch)
                selectedTab = oldTab
            }
        }
    } else {
        // No unsaved changes, reset save bar state
        saveBarState.reset()
    }
}
```

### 3. 保存按钮状态管理

**现有实现** (已在 `BehaviorSettingsView.swift` 等视图中实现):
- 使用 `hasUnsavedChanges` 计算属性检测表单变更
- 通过 `saveBarState.update()` 更新保存栏状态
- 保存按钮根据 `hasUnsavedChanges` 自动启用/禁用

**状态判断逻辑**:
```swift
private var hasUnsavedChanges: Bool {
    return inputMode != savedInputMode ||
           outputMode != savedOutputMode ||
           abs(typingSpeed - savedTypingSpeed) > 0.1 ||
           piiScrubbingEnabled != savedPiiScrubbingEnabled ||
           piiTypes != savedPiiTypes
}
```

## 本地化支持

### 新增本地化键

**英文** (`en.lproj/Localizable.strings`):
```
"settings.unsaved_changes.discard" = "Discard";
```

**中文** (`zh-Hans.lproj/Localizable.strings`):
```
"settings.unsaved_changes.discard" = "放弃修改";
```

### 现有本地化键(已存在):
- `settings.unsaved_changes.title` = "未保存的修改"
- `settings.unsaved_changes.message` = "你有未保存的修改。是否在离开前保存?"
- `settings.unsaved_changes.close_message` = "你有未保存的修改。是否在关闭前保存?"
- `common.save` = "保存"
- `common.cancel` = "取消"

## 技术实现细节

### 架构模式

1. **状态管理**: 使用 `SettingsSaveBarState` (ObservableObject) 在所有设置视图间共享状态
2. **委托模式**: 使用 NSWindowDelegate 拦截窗口关闭事件
3. **SwiftUI Binding**: 使用 `onChange` 修饰符监听标签页切换

### 关键设计决策

1. **窗口委托设置时机**:
   - 在 `onAppear` 中使用 `DispatchQueue.main.async` 延迟设置
   - 确保窗口已经创建完成后再设置 delegate

2. **标签页切换拦截**:
   - 使用 `onChange(of: selectedTab) { oldTab, newTab in }` 获取旧值
   - 用户取消时恢复到 `oldTab`

3. **对话框操作**:
   - 保存: 异步执行 `saveBarState.onSave?()` 后允许操作
   - 放弃修改: 执行 `saveBarState.onCancel?()` 后允许操作
   - 取消: 返回 `false` 阻止操作

## 测试建议

### 手动测试场景

1. **窗口关闭测试**:
   - [ ] 修改设置 → 点击关闭按钮 → 验证弹出对话框
   - [ ] 点击"保存" → 验证设置已保存且窗口关闭
   - [ ] 点击"放弃修改" → 验证设置未保存且窗口关闭
   - [ ] 点击"取消" → 验证窗口保持打开

2. **标签页切换测试**:
   - [ ] 在 Behavior 标签页修改设置 → 点击 Providers 标签 → 验证弹出对话框
   - [ ] 点击"保存" → 验证设置已保存且标签页切换
   - [ ] 点击"放弃修改" → 验证设置未保存且标签页切换
   - [ ] 点击"取消" → 验证保持在当前标签页

3. **保存按钮状态测试**:
   - [ ] 未修改时保存按钮显示灰色且禁用
   - [ ] 修改设置后保存按钮显示蓝色且启用
   - [ ] 点击保存后保存按钮恢复灰色且禁用

4. **多个设置视图测试**:
   - [ ] General 设置(instant-save 模式,保存按钮始终禁用)
   - [ ] Providers 设置(有完整保存逻辑)
   - [ ] Routing 设置(需要验证)
   - [ ] Shortcuts 设置(需要验证)
   - [ ] Behavior 设置(已实现完整保存逻辑)
   - [ ] Memory 设置(需要验证)

## 兼容性说明

- **macOS 版本**: macOS 13.0+ (使用 NSWindowDelegate)
- **SwiftUI 版本**: iOS 16+ / macOS 13+ (使用 `onChange(of:) { old, new in }`)
- **现有功能**: 完全兼容现有的保存栏实现

## 相关文件

### 修改的文件:
1. `Aleph/Sources/Components/Window/RootContentView.swift`
   - 添加 `SettingsWindowDelegate` 类
   - 添加窗口委托设置逻辑
   - 添加标签页切换拦截逻辑
   - 添加 `showUnsavedChangesDialog` 辅助方法

2. `Aleph/Resources/en.lproj/Localizable.strings`
   - 添加 `settings.unsaved_changes.discard` 键

3. `Aleph/Resources/zh-Hans.lproj/Localizable.strings`
   - 添加 `settings.unsaved_changes.discard` 键

### 依赖的现有文件:
1. `Aleph/Sources/Utils/SettingsViewProtocol.swift` - `SettingsSaveBarState` 类
2. `Aleph/Sources/Components/Molecules/UnifiedSaveBar.swift` - 保存栏 UI 组件
3. `Aleph/Sources/BehaviorSettingsView.swift` - 保存逻辑参考实现

## 实施状态

- ✅ 窗口关闭拦截逻辑
- ✅ 菜单切换拦截逻辑
- ✅ 本地化字符串
- ✅ Swift 语法验证
- ✅ Xcode 编译通过
- ⏳ 手动测试(待用户验证)

## 下一步工作

1. **手动测试**: 运行应用并测试所有场景
2. **其他视图**: 确保所有设置视图(Routing, Shortcuts, Memory)都实现了完整的保存逻辑
3. **边缘情况**: 测试快速切换标签页、多次修改等场景
4. **用户体验优化**: 根据测试反馈调整对话框文案和交互逻辑

## 总结

本次实施完成了 `unify-settings-save-bar` 变更提案中的核心保存逻辑,确保用户在有未保存内容时不会意外丢失数据。实现遵循了 macOS 标准用户体验模式,并完全集成到现有的保存栏架构中。

# Change: Restructure Providers Layout

## Why

当前的 Providers 视图布局存在以下可用性问题：

1. **缺少快速添加自定义供应商的入口**：
   - 用户必须找到并点击列表中的 "Custom (OpenAI-compatible)" preset
   - 对于需要多个自定义供应商的用户，这个流程不够直观

2. **视觉层次不清晰**：
   - 左侧供应商列表和右侧编辑区没有明显的视觉边界
   - 缺少容器感，用户难以快速区分功能区域

3. **工具栏功能不完整**：
   - 搜索框单独放在列表顶部，但缺少操作按钮
   - 没有一个统一的工具栏来容纳常用功能

## What Changes

### 1. 添加左侧工具栏
在左侧供应商列表区域顶部添加一个横栏，包含：
- **左侧**: "Add Custom Provider" 按钮
  - 点击后直接打开空白的自定义供应商编辑表单
  - 图标: `plus.circle` 或 `plus.square.on.square`
  - 使用 ActionButton 组件保持一致性
- **右侧**: 搜索框（从列表区域移到工具栏）
  - 保持当前的 SearchBar 组件

### 2. 增强视觉容器感
- **左侧供应商列表区域**:
  - 添加 `DesignTokens.CornerRadius.medium` (10pt) 圆角
  - 保持当前背景色 (`DesignTokens.Colors.sidebarBackground`)
  - 不添加阴影（保持扁平设计）

- **右侧编辑区域**:
  - 添加 `DesignTokens.CornerRadius.medium` (10pt) 圆角
  - 添加背景色 (`DesignTokens.Colors.contentBackground`)
  - 不添加阴影

### 3. 优化供应商列表显示
- 继续显示所有预设供应商（包括 "Custom" preset）
- 显示所有已配置的自定义供应商实例
- 自定义供应商与预设供应商混合在同一列表中显示
- 保持现有的已配置/未配置状态标记

### 4. 简化自定义供应商创建流程
- 用户点击 "Add Custom Provider" 按钮
- 系统自动：
  - 清空当前选择
  - 将 `selectedPreset` 设为 "custom" preset
  - 设置 `isAddingNew = true`
  - 右侧立即显示空白的自定义供应商编辑表单
- 用户填写表单并保存后，新的自定义供应商出现在列表中

## Impact

### Affected Specs
- `settings-ui-layout` - Providers Tab 的布局结构、工具栏、视觉容器

### Affected Code
- `Aether/Sources/ProvidersView.swift` - 主要修改
  - 添加左侧工具栏布局
  - 添加圆角边框和背景容器
  - 添加 "Add Custom Provider" 按钮处理逻辑
- `Aether/Sources/Components/Atoms/` - 可能需要新的工具栏组件（如果复用 ActionButton 则不需要）

### Breaking Changes
无。此变更仅优化 UI 布局，不影响数据模型或配置文件格式。

### User Benefits
- ✅ 快速创建自定义供应商（一键直达）
- ✅ 更清晰的视觉层次（圆角容器区分功能区）
- ✅ 统一的工具栏（操作 + 搜索集中在一处）
- ✅ 保持现有功能完整性（不移除任何现有能力）

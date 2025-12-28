# Change: Simplify Provider Type Selection UX

## Why

当前的 Provider 编辑界面存在交互逻辑混乱问题：

1. **重复选择器**：左侧供应商列表和右侧 Provider Type 选择器功能重复，用户需要进行两次相同性质的选择
2. **信息不一致**：左侧选择预设供应商（如 OpenAI、Claude、Gemini）后，右侧仍需手动选择 Provider Type，容易导致配置错误
3. **违反标准交互模式**："左侧选择列表 → 右侧编辑详情"是标准的 Master-Detail 模式，但当前实现破坏了这一模式

这些问题导致用户体验混乱，配置流程繁琐，需要简化。

## What Changes

1. **移除 Provider Type 选择器**
   - 从 `ProviderEditPanel.swift` 中删除 Provider Type 的 Picker 控件
   - Provider Type 由左侧选择的预设供应商自动确定

2. **添加供应商信息展示卡片**
   - 在编辑区域顶部显示选中供应商的完整信息
   - 包含：彩色图标、供应商名称、类型标签、描述文本
   - 这些信息来自 `PresetProvider` 数据，只读展示

3. **Provider Name 设为只读**
   - Provider Name 由预设供应商的 ID 自动确定
   - 添加帮助文本说明其用途（用于路由规则引用）
   - 用户无需也不应手动修改

4. **保持现有交互流程**
   - 左侧列表显示所有预设供应商（已支持的 11 个供应商）
   - 点击供应商后，右侧自动加载该供应商的配置模板
   - 根据 `provider_type` 动态显示相关参数区域

## Impact

### Affected Specs
- `settings-ui-layout` - Provider 编辑面板的 UI 交互逻辑

### Affected Code
- `Aether/Sources/Components/Organisms/ProviderEditPanel.swift` - 移除 Provider Type 选择器，添加供应商信息卡片
- 不影响 `ProvidersView.swift` - 左侧列表逻辑保持不变
- 不影响 `PresetProviders.swift` - 预设数据结构保持不变

### Breaking Changes
无。此变更仅简化 UI 交互，不影响配置文件格式或 API 接口。

### User Benefits
- ✅ 减少用户操作步骤（从"选择供应商 + 选择类型"简化为"选择供应商"）
- ✅ 消除配置错误可能性（类型由预设自动确定）
- ✅ 符合标准 Master-Detail 交互模式
- ✅ 信息展示更清晰（彩色图标 + 完整描述）

# Change: Refine Provider Edit UI

## Why

当前的 Provider 编辑界面存在以下问题：

1. **冗余标题**："Add Provider" 和 "Edit Provider" 标题是多余的，因为用户已经通过左侧列表选择了供应商
2. **Active 开关布局不合理**：
   - "Active" 文字标签和帮助文本占用过多垂直空间
   - 开关位置不够突出，用户需要向下滚动才能看到
   - 与供应商信息脱节
3. **预设供应商配置混乱**：
   - Provider Name 和 Theme Color 应该由预设硬编码，但当前仍然可编辑
   - 用户可能错误修改这些品牌标识性的配置
4. **缺少自定义供应商支持**：
   - 用户无法添加 OpenAI 兼容的自定义 API（如企业内部部署的 LLM API）
   - 现有的 "custom" provider type 没有在 UI 中暴露

## What Changes

### 1. 移除 "Add Provider" / "Edit Provider" 标题
- 删除编辑区域顶部的标题文本
- 供应商信息卡片本身已经清楚地表明了正在编辑的供应商

### 2. 重新设计 Active 开关布局
- **删除独立的 Active 区域**（当前在 Divider 之间的独立区域）
- **将 Toggle 开关移到供应商信息卡片内**：
  - 位置：供应商名称所在行的右对齐
  - 不显示 "Active" 文字标签
  - 使用 macOS 原生 Toggle 样式（绿色 = 激活，灰色 = 关闭）
  - 删除帮助文本（状态通过颜色已经非常清晰）

### 3. 预设供应商字段简化
对于所有预设供应商（OpenAI, Anthropic, Google Gemini, Ollama, AiHubMix 等）：
- **完全移除 Provider Name 字段**（硬编码，不可见）
- **完全移除 Theme Color 字段**（硬编码，不可见）
- 这两项配置由 `PresetProvider` 数据硬编码，确保品牌一致性

### 4. 添加自定义供应商功能
- **在预设列表中添加 "Custom (OpenAI-compatible)" 选项**
- **自定义供应商特性**：
  - 允许用户设置多个独立的自定义供应商实例
  - 每个实例需要配置：
    - ✏️ **Provider Name**（可编辑，必填，用于标识）
    - ✏️ **Theme Color**（可编辑，用于 Halo 颜色）
    - ✏️ **Base URL**（可编辑，必填，指向 OpenAI 兼容的 API 端点）
    - 其他标准参数（API Key, Model, Temperature 等）
  - 用户可以创建多个自定义供应商（如 "Company Internal API", "Local LLM", "Proxy API" 等）

### 5. 区分预设供应商和自定义供应商
- **预设供应商（Preset）**：
  - 显示供应商信息卡片（图标、名称、类型、描述）
  - Provider Name 和 Theme Color **不显示**（硬编码）
  - 其他参数可配置（API Key, Model, Base URL, 生成参数等）

- **自定义供应商（Custom）**：
  - 不显示供应商信息卡片（因为是用户自定义）
  - Provider Name 和 Theme Color **可编辑**（必填）
  - Base URL **可编辑**（必填）
  - 使用 OpenAI provider type
  - 其他参数可配置

## Impact

### Affected Specs
- `settings-ui-layout` - Provider 编辑面板的 UI 布局和交互

### Affected Code
- `Aleph/Sources/Components/Organisms/ProviderEditPanel.swift` - 主要修改
- `Aleph/Sources/Models/PresetProviders.swift` - 添加 Custom 预设选项
- `Aleph/Sources/ProvidersView.swift` - 支持多个自定义供应商实例（可能需要调整）

### Breaking Changes
无。此变更仅优化 UI 交互，配置文件格式保持兼容。

### User Benefits
- ✅ 更简洁的界面（移除冗余标题和文本）
- ✅ Active 状态更直观（与供应商名称在同一行）
- ✅ 预设供应商配置更安全（品牌配置不可修改）
- ✅ 支持自定义 OpenAI 兼容 API（企业内部 LLM、本地代理等）
- ✅ 可以创建多个自定义供应商实例

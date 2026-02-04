# Change: Refactor Provider Edit Panel for Direct Editing

## Why

目前Provider界面存在严重的用户体验问题:点击供应商列表中的供应商后,右侧编辑区域显示只读信息和一个"Configure this Provider"按钮,用户必须再点击该按钮才能进入编辑模式。这导致了不必要的额外交互步骤,降低了配置效率。

参考uisample.png中的设计,正确的交互模式应该是:点击供应商列表里的供应商名称后,右侧编辑区域直接呈现此供应商的完整配置选项,允许用户立即进行编辑和保存。

此外,当前实现中的供应商配置字段不够完整,未根据各供应商的官方API规范提供准确的配置参数。

## What Changes

- 移除ProviderEditPanel中的"Configure this Provider"中间状态
- 点击供应商列表项时,右侧直接展示该供应商的完整配置表单(编辑模式)
- 根据OpenAI、Anthropic Claude、Google Gemini、Ollama官方API文档,为每个供应商类型提供准确的配置参数
- 优化配置表单字段组织,区分必填/可选参数,提供参数说明和默认值提示
- 增强表单验证逻辑,确保每个供应商类型的参数符合其API规范
- 保持Active/Inactive状态指示器和测试连接功能

## Impact

**Affected specs:**
- `settings-ui-layout` - 修改Provider编辑面板的交互逻辑和布局

**Affected code:**
- `Aleph/Sources/ProvidersView.swift` - 供应商列表点击逻辑调整
- `Aleph/Sources/Components/Organisms/ProviderEditPanel.swift` - 移除中间状态,直接展示编辑表单
- `Aleph/core/src/config/mod.rs` - 可能需要扩展ProviderConfig支持更多参数
- `Aleph/core/src/aleph.udl` - UniFFI接口可能需要更新以支持新参数

**Breaking changes:**
无 - 这是UI重构,不影响底层数据结构和API

**User benefits:**
- 减少点击次数,提升配置效率
- 更准确的供应商配置参数,符合官方API规范
- 更清晰的必填/可选字段区分和参数说明

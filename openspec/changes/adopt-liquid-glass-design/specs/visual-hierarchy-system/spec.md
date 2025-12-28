# visual-hierarchy-system Specification

## Purpose

定义基于布局和分组的视觉层次系统，遵循 Apple Liquid Glass 设计原则，避免依赖装饰性元素（边框、阴影、背景）表现层次。

## ADDED Requirements

### Requirement: Layout-Based Hierarchy Expression
视觉层次 SHALL 主要通过布局和间距表现，而非装饰性元素。

#### Scenario: Major section spacing
- **GIVEN** 任何设置页面包含多个主要部分（如卡片、表单组）
- **WHEN** 渲染主要部分之间的间距
- **THEN** 主要部分间距 SHALL 使用 `DesignTokens.Spacing.lg`（24pt）
- **AND** 间距 SHALL 足够区分不同功能区块
- **AND** SHALL NOT 使用分隔线或边框分隔主要部分

#### Scenario: Secondary section spacing
- **GIVEN** 主要部分内包含次要分组（如表单字段组）
- **WHEN** 渲染次要分组之间的间距
- **THEN** 次要分组间距 SHALL 使用 `DesignTokens.Spacing.md`（16pt）
- **AND** 间距 SHALL 明显小于主要部分间距
- **AND** 次要分组 SHALL 使用 VStack 或 HStack 包裹

#### Scenario: Form field spacing
- **GIVEN** 表单包含多个输入字段
- **WHEN** 渲染字段之间的间距
- **THEN** 字段间距 SHALL 使用 `DesignTokens.Spacing.sm`（8pt）
- **AND** 标签和输入框之间的间距 SHALL 使用 `DesignTokens.Spacing.xs`（4pt）
- **AND** 相关字段 SHALL 分组在同一 VStack 内

#### Scenario: Related controls grouping
- **GIVEN** 多个控件属于同一功能（如一组单选按钮）
- **WHEN** 渲染控件组
- **THEN** 控件组 SHALL 使用 VStack 或 HStack 包裹
- **AND** 组内间距 SHALL 使用 `DesignTokens.Spacing.xs`（4pt）
- **AND** 组外间距 SHALL 使用 `DesignTokens.Spacing.md`（16pt）
- **AND** SHALL NOT 添加边框或背景框住控件组

---

### Requirement: Color-Based Emphasis
主要操作和重要元素 SHALL 使用着色突出，而非边框或阴影。

#### Scenario: Primary action emphasis
- **GIVEN** 操作按钮（如 Save, Add, Test）
- **WHEN** 渲染主要操作按钮
- **THEN** 按钮 SHALL 使用系统强调色（蓝色 `.accentColor` 或 `DesignTokens.Colors.accentBlue`）
- **AND** 按钮文字 SHALL 使用白色（高对比度）
- **AND** 按钮 SHALL 使用胶囊形状或圆角矩形（非硬边框）

#### Scenario: Secondary action appearance
- **GIVEN** 次要操作按钮（如 Cancel, Reset）
- **WHEN** 渲染次要操作按钮
- **THEN** 按钮 SHALL 使用中性色（灰色 `DesignTokens.Colors.textSecondary`）
- **AND** 按钮背景 SHALL 使用半透明背景或无背景（`.plain` 样式）
- **AND** 按钮 SHALL NOT 使用边框（除非系统默认）

#### Scenario: Destructive action warning
- **GIVEN** 破坏性操作按钮（如 Delete, Remove）
- **WHEN** 渲染破坏性操作按钮
- **THEN** 按钮 SHALL 使用红色或橙色（`.red` 或 `DesignTokens.Colors.error`）
- **AND** 按钮 SHALL 明显区别于主要和次要操作
- **AND** 按钮 SHALL 使用系统标准破坏性样式

#### Scenario: Selected state indication
- **GIVEN** 可选择的列表项或卡片（如侧边栏导航项、provider 卡片）
- **WHEN** 用户选中某项
- **THEN** 选中项 SHALL 使用背景色高亮（浅蓝色或系统选中色）
- **AND** SHALL NOT 使用边框或阴影表示选中状态
- **AND** 选中项背景色 SHALL 符合 macOS 系统选中样式

---

### Requirement: Typography Hierarchy
文字层次 SHALL 通过字体大小、粗细和颜色表现。

#### Scenario: Title typography
- **GIVEN** 页面或卡片标题
- **WHEN** 渲染标题文字
- **THEN** 标题 SHALL 使用 `DesignTokens.Typography.title`（20pt, semibold）
- **AND** 标题颜色 SHALL 使用 `DesignTokens.Colors.textPrimary`
- **AND** 标题 SHALL 与正文有明显字号差异（至少 6pt）

#### Scenario: Heading typography
- **GIVEN** 部分标题或小节标题
- **WHEN** 渲染标题文字
- **THEN** 标题 SHALL 使用 `DesignTokens.Typography.heading`（16pt, semibold）
- **AND** 标题颜色 SHALL 使用 `DesignTokens.Colors.textPrimary`

#### Scenario: Body text typography
- **GIVEN** 正文内容或描述文字
- **WHEN** 渲染正文
- **THEN** 正文 SHALL 使用 `DesignTokens.Typography.body`（14pt, regular）
- **AND** 正文颜色 SHALL 使用 `DesignTokens.Colors.textPrimary`

#### Scenario: Caption typography
- **GIVEN** 辅助说明或提示文字
- **WHEN** 渲染辅助文字
- **THEN** 辅助文字 SHALL 使用 `DesignTokens.Typography.caption`（12pt, regular）
- **AND** 辅助文字颜色 SHALL 使用 `DesignTokens.Colors.textSecondary`
- **AND** 辅助文字 SHALL 放置在相关元素下方或右侧

---

### Requirement: Scroll Edge Effect Integration
滚动视图 SHALL 使用模糊边缘效果，而非硬分隔线。

#### Scenario: Scrollable list edge effect
- **GIVEN** 可滚动列表（provider 列表、routing 规则列表）
- **WHEN** 内容超出视图高度
- **THEN** 列表 SHALL 应用硬样式滚动边缘效果（macOS 适用）
- **AND** 顶部和底部边缘 SHALL 使用渐变遮罩（opacity: 0 → 0.6 → 1）
- **AND** 边缘效果 SHALL 明确 UI 与内容的交界
- **AND** SHALL NOT 使用硬分隔线（Divider）

#### Scenario: Scroll edge effect parameters
- **GIVEN** 应用滚动边缘效果
- **WHEN** 配置效果参数
- **THEN** 硬样式 SHALL 使用不透明度 0.6（`hard(opacity: 0.6, blur: 12)`）
- **AND** 软样式 SHALL 使用不透明度 0.3（`soft(opacity: 0.3, blur: 8)`）
- **AND** 渐变范围 SHALL 占视图高度 5-10%
- **AND** 边缘效果 SHALL 仅在内容可滚动时可见

#### Scenario: Edge effect for fixed content
- **GIVEN** 内容高度小于视图高度（无需滚动）
- **WHEN** 渲染内容
- **THEN** SHALL NOT 应用滚动边缘效果
- **AND** 内容 SHALL 正常显示，无遮罩

---

### Requirement: Prohibition of Decorative Elements
装饰性元素（边框、额外背景、装饰性阴影）SHALL 被禁止使用。

#### Scenario: No decorative borders
- **GIVEN** 任何 UI 组件（卡片、容器、面板）
- **WHEN** 渲染组件
- **THEN** 组件 SHALL NOT 使用装饰性边框（`.stroke()`）
- **AND** 表单输入框边框除外（功能需要）
- **AND** 浮动层边框除外（微妙边框用于定义边界）
- **AND** 组件 SHALL 使用间距和背景色差异表现边界

#### Scenario: No redundant backgrounds
- **GIVEN** 嵌套容器或分组
- **WHEN** 渲染容器
- **THEN** 容器 SHALL NOT 添加冗余背景色（除非功能需要）
- **AND** 背景色仅用于表现层次（轻微对比，如 5% 不透明度差异）
- **AND** SHALL NOT 使用高对比度背景框住内容

#### Scenario: No decorative dividers
- **GIVEN** 内容分组或部分分隔
- **WHEN** 渲染分隔
- **THEN** SHALL NOT 使用硬分隔线（`Divider()`）
- **AND** SHALL 使用间距（`Spacer` 或 `.padding()`）表现分隔
- **AND** 功能性分隔线除外（如标题栏底部、表单分组边界）

#### Scenario: Functional dividers allowed
- **GIVEN** 明确的功能性分隔需求（标题栏与内容、侧边栏与内容）
- **WHEN** 需要视觉分隔
- **THEN** 允许使用微妙分隔线（1pt, 不透明度 0.1-0.2）
- **AND** 分隔线 SHALL 尽可能细和微妙
- **AND** 优先考虑使用间距或材质边界代替

---

### Requirement: Consistent Button and Control Styling
按钮和控件 SHALL 使用一致的样式系统。

#### Scenario: Button shape selection
- **GIVEN** 按钮需要渲染
- **WHEN** 选择按钮形状
- **THEN** 小/中尺寸按钮 SHALL 使用圆角矩形（固定圆角 6pt）
- **AND** 大尺寸按钮 SHALL 使用胶囊形状（`Capsule()` 或 `cornerRadius: height / 2`）
- **AND** 按钮形状 SHALL 符合同心几何原则

#### Scenario: Toggle and switch styling
- **GIVEN** Toggle 开关或 Switch 控件
- **WHEN** 渲染控件
- **THEN** 控件 SHALL 使用系统默认样式（`.toggleStyle(.switch)`）
- **AND** 开启状态 SHALL 显示绿色（系统强调色）
- **AND** 关闭状态 SHALL 显示灰色（中性色）
- **AND** SHALL NOT 自定义开关样式（除非有特殊需求）

#### Scenario: Segmented control styling
- **GIVEN** Picker 使用 `.segmented` 样式
- **WHEN** 渲染 Segmented Control
- **THEN** 控件 SHALL 使用系统默认样式
- **AND** 选中项 SHALL 使用系统选中背景色
- **AND** 控件 SHALL 符合 macOS HIG 规范


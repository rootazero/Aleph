# liquid-glass-materials Specification

## Purpose

定义 Aether 应用中 Liquid Glass 材质的使用规范，包括同心几何布局、材质选择和向后兼容策略。遵循 Apple WWDC 2025 设计系统指南。

## ADDED Requirements

### Requirement: Concentric Geometry Layout System
所有 UI 组件 SHALL 使用同心几何布局系统，确保视觉光学平衡。

#### Scenario: Child radius calculation
- **GIVEN** 一个嵌套容器（child）位于父容器（parent）内部
- **WHEN** 计算子容器的圆角半径
- **THEN** 子容器圆角 SHALL = max(父容器圆角 - 内边距, 最小圆角)
- **AND** 最小圆角 SHALL 默认为 4pt（防止过小的圆角）
- **AND** 如果计算结果 < 最小圆角，则使用最小圆角

#### Scenario: Window-level radius definition
- **GIVEN** Settings 窗口作为最外层容器
- **WHEN** 渲染窗口
- **THEN** 窗口圆角 SHALL = 12pt（符合 macOS 标准窗口圆角）
- **AND** 所有内部组件圆角 SHALL 基于此计算

#### Scenario: Sidebar concentric radius
- **GIVEN** 浮动侧边栏位于窗口内部，内边距 12pt
- **WHEN** 计算侧边栏圆角
- **THEN** 侧边栏圆角 SHALL = 12pt - 12pt = 0pt
- **BUT** 侧边栏 SHALL 使用固定圆角 10pt（因为 0pt 过小）
- **AND** 10pt 圆角 SHALL 提供微妙的视觉柔和感

#### Scenario: Content area concentric radius
- **GIVEN** 内容区域位于窗口内部，内边距 0pt（右侧紧贴）
- **WHEN** 计算内容区域圆角
- **THEN** 内容区域圆角 SHALL = 12pt - 0pt = 12pt
- **AND** 内容区域 SHALL 与窗口共享相同圆角

---

### Requirement: Liquid Glass Material Selection
应用组件 SHALL 根据功能使用适当的 Liquid Glass 材质。

#### Scenario: Sidebar material
- **GIVEN** 侧边栏作为导航组件
- **WHEN** 渲染侧边栏背景
- **THEN** 侧边栏 SHALL 使用 `.sidebar` 材质（macOS 13+）
- **AND** 材质 SHALL 允许内容在其后方微妙可见
- **AND** 材质 SHALL 提供与系统一致的毛玻璃效果

#### Scenario: Title bar material
- **GIVEN** 标题栏作为功能层
- **WHEN** 渲染标题栏背景
- **THEN** 标题栏 SHALL 使用 `.titlebar` 材质（macOS 13+）
- **AND** 标题栏 SHALL 集成系统窗口控制按钮
- **AND** 标题栏 SHALL 保持透明，不阻挡下方内容

#### Scenario: Content area background
- **GIVEN** 内容区域显示主要内容
- **WHEN** 渲染内容区域背景
- **THEN** 内容区域 SHALL 使用 `.windowBackground` 材质（macOS 13+）
- **OR** 使用纯色背景 `DesignTokens.Colors.contentBackground`（简化方案）
- **AND** 背景 SHALL 提供足够对比度以确保可读性

#### Scenario: Floating panel material
- **GIVEN** 浮动面板（如弹窗、工具提示）
- **WHEN** 渲染浮动面板
- **THEN** 浮动面板 SHALL 使用 `.ultraThinMaterial` 材质（macOS 13+）
- **AND** 浮动面板 SHALL 添加微妙阴影以表现浮动层次
- **AND** 阴影 SHALL 符合 `DesignTokens.Shadows.floating` 定义

---

### Requirement: Backward Compatibility Strategy
应用 SHALL 在不同 macOS 版本上提供一致的用户体验。

#### Scenario: macOS 15+ full Liquid Glass support
- **GIVEN** 应用运行在 macOS 15 或更高版本
- **WHEN** 渲染任何 Liquid Glass 组件
- **THEN** 组件 SHALL 使用完整的 Liquid Glass 材质
- **AND** SHALL 使用 `.ultraThinMaterial`, `.sidebar`, `.titlebar` 等系统材质
- **AND** SHALL 使用 `blendingMode: .withinWindow` 增强效果

#### Scenario: macOS 13-14 basic material support
- **GIVEN** 应用运行在 macOS 13 或 14
- **WHEN** 渲染任何 Liquid Glass 组件
- **THEN** 组件 SHALL 使用基础系统材质
- **AND** SHALL 使用 `.sidebar`, `.titlebar`, `.windowBackground`
- **AND** SHALL 使用 `blendingMode: .behindWindow`（默认）
- **AND** 视觉效果 SHALL 略微降低，但保持可用性

#### Scenario: macOS 12 and below fallback
- **GIVEN** 应用运行在 macOS 12 或更低版本
- **WHEN** 渲染任何 Liquid Glass 组件
- **THEN** 组件 SHALL 使用降级方案
- **AND** 侧边栏 SHALL 使用半透明背景 (Color.black.opacity(0.05)) + 模糊 (blur: 20)
- **AND** 标题栏 SHALL 使用半透明背景 (Color.white.opacity(0.8))
- **AND** 内容区域 SHALL 使用纯色背景
- **AND** 降级方案 SHALL 保持功能完整性和基本可读性

#### Scenario: Adaptive material component usage
- **GIVEN** 开发者需要添加新的 Liquid Glass 组件
- **WHEN** 选择材质实现
- **THEN** 开发者 SHALL 使用 `AdaptiveMaterial` 组件
- **AND** `AdaptiveMaterial` SHALL 自动检测 macOS 版本
- **AND** `AdaptiveMaterial` SHALL 选择最佳材质或降级方案
- **AND** 开发者 SHALL NOT 手动编写 `@available` 检查（封装在组件内）

---

### Requirement: Shadow and Border Removal
装饰性阴影和边框 SHALL 被移除，仅保留功能性阴影。

#### Scenario: Remove decorative card shadows
- **GIVEN** 任何卡片组件（MemoryView, BehaviorSettingsView, ShortcutsView 等）
- **WHEN** 渲染卡片
- **THEN** 卡片 SHALL NOT 使用 `.shadow(DesignTokens.Shadows.card)`
- **AND** 卡片 SHALL NOT 使用硬边框 `.stroke()`
- **AND** 卡片 SHALL 使用间距和背景色差异表现层次

#### Scenario: Keep functional floating layer shadows
- **GIVEN** 浮动侧边栏或浮动面板
- **WHEN** 渲染浮动层
- **THEN** 浮动层 SHALL 使用微妙阴影 `DesignTokens.Shadows.floating`
- **AND** 阴影参数 SHALL = (color: .black.opacity(0.1), radius: 8, x: 0, y: 2)
- **AND** 阴影 SHALL 仅用于表现浮动层次，非装饰

#### Scenario: Form input borders preservation
- **GIVEN** 表单输入框（TextField, SecureField, TextEditor）
- **WHEN** 渲染表单控件
- **THEN** 输入框 SHALL 保留边框（功能需要）
- **AND** 边框 SHALL 使用 `.roundedBorder` 或 `.plain` 样式
- **AND** 边框 SHALL 符合系统标准（macOS HIG）

---

### Requirement: Content Extension Behind Sidebar
内容 SHALL 能够在侧边栏后方延伸，创造沉浸感。

#### Scenario: Full extension for simple content
- **GIVEN** 标签页内容为简单列表或卡片（General, Memory, Behavior）
- **WHEN** 渲染内容区域
- **THEN** 内容 SHALL 完全延伸到左侧（x = 0）
- **AND** 内容 SHALL 在侧边栏后方可见（使用 `.zIndex(-1)`）
- **AND** 侧边栏 SHALL 浮动在内容之上（使用 `.zIndex(1)`）
- **AND** 侧边栏材质 SHALL 允许后方内容微妙透过

#### Scenario: Partial extension for two-column layouts
- **GIVEN** 标签页使用两栏布局（Providers, Routing）
- **WHEN** 渲染内容区域
- **THEN** 左栏（列表）SHALL 延伸到左侧
- **AND** 右栏（编辑器）SHALL 不延伸（x = 200pt，侧边栏宽度）
- **AND** 左栏 SHALL 在侧边栏后方可见
- **AND** 右栏 SHALL 与侧边栏齐平，不重叠

#### Scenario: No extension for specific content
- **GIVEN** 标签页需要清晰视觉边界（Shortcuts）
- **WHEN** 渲染内容区域
- **THEN** 内容 SHALL NOT 延伸到侧边栏下方
- **AND** 内容 SHALL 从 x = 200pt 开始
- **AND** 侧边栏左侧 SHALL 显示空白或背景色


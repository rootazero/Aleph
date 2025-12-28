# Proposal: adopt-liquid-glass-design

## Overview

**Change ID**: `adopt-liquid-glass-design`
**Status**: Proposed
**Created**: 2025-12-27
**Author**: AI Assistant

## Problem Statement

当前 Aether 的 Settings 窗口设计虽然功能完整，但未完全遵循 Apple 最新的 Liquid Glass 设计系统（WWDC 2025）。具体问题包括：

1. **布局层次不符合同心几何原则**：
   - 当前使用 ZStack 实现浮动侧边栏，但未遵循同心形状的数学计算
   - 圆角半径未基于父级形状半径和内边距进行计算
   - 缺少光学平衡的视觉效果

2. **侧边栏未集成 Liquid Glass 材质**：
   - 当前侧边栏使用硬阴影和边框
   - 内容未在侧边栏后方流动，缺少沉浸感
   - 滚动视图未延伸到侧边栏下方

3. **标题栏设计不符合功能层概念**：
   - 当前标题栏使用 `.titlebar` 材质，但未完全移除装饰性背景
   - 缺少微妙的材质变化来强化导航意图
   - 工具栏项目未按功能和使用频率组织

4. **缺少滚动边缘效果**：
   - 内容滚动时未使用模糊效果取代硬分隔线
   - 缺少 Scroll Edge Effect 来明确 UI 与内容的交界

5. **视觉层次依赖装饰而非布局**：
   - 过度使用边框、背景色来表现层次
   - 未充分利用布局和分组来表现层级关系

## Proposed Solution

全面采用 Apple Liquid Glass 设计系统，重新设计 Settings 窗口，使其符合 macOS 最新设计语言规范。

### 核心设计原则

1. **同心几何布局（Concentric Geometry）**：
   - 所有形状围绕共同中心点对齐
   - 圆角半径基于数学计算：`子级半径 = 父级半径 - 内边距`
   - 使用三种形状类型：固定形状、胶囊形状、同心形状

2. **Liquid Glass 材质集成**：
   - 侧边栏嵌入 Liquid Glass 材质
   - 内容在侧边栏后方流动
   - 使用微妙的材质变化而非硬边界

3. **功能层悬浮**：
   - UI 元素悬浮在内容之上但不抢焦点
   - 移除冗余的装饰性背景和边框
   - 使用模糊效果取代硬分隔线

4. **布局驱动层次**：
   - 依靠布局和分组表现视觉层次
   - 使用着色突出主要操作
   - 避免装饰性元素（额外背景、边框）

### 具体改造

#### 1. 窗口整体布局

- 采用全屏 Liquid Glass 背景
- 内容区域使用同心圆角（基于窗口圆角 - 内边距）
- 侧边栏作为功能层悬浮在内容之上

#### 2. 侧边栏设计

- 使用 `.ultraThinMaterial` 或 `.sidebar` 材质
- 移除硬阴影和边框
- 允许内容在侧边栏后方延伸（视觉沉浸感）
- 圆角使用同心几何计算

#### 3. 标题栏集成

- 完全透明的标题栏（`.fullSizeContentView`）
- 窗口控制按钮自然集成在内容区域
- 工具栏项目按功能分组，无额外背景

#### 4. 滚动边缘效果

- 内容滚动时添加硬样式滚动边缘效果（macOS 适用）
- 使用模糊效果明确 UI 与内容交界
- 顶部和底部边缘淡入淡出

#### 5. 视觉层次优化

- 移除卡片的硬边框和阴影
- 使用间距和分组表现层次
- 主要操作使用着色（蓝色/系统强调色）
- 次要操作使用中性色

## Affected Specs

### Modified Specs

1. **settings-ui-layout**
   - 更新窗口尺寸和布局原则
   - 添加同心几何布局要求
   - 添加 Liquid Glass 材质要求
   - 更新侧边栏和内容区域交互方式

### New Specs

2. **liquid-glass-materials**（新增）
   - 定义 Liquid Glass 材质使用规范
   - 定义同心几何形状计算规则
   - 定义滚动边缘效果规范

3. **visual-hierarchy-system**（新增）
   - 定义基于布局的层次表现方式
   - 定义着色和分组规则
   - 禁止装饰性元素使用

## Success Criteria

1. **视觉一致性**：
   - 所有圆角使用同心几何计算
   - 侧边栏使用 Liquid Glass 材质
   - 移除所有硬边框和装饰性阴影

2. **交互体验**：
   - 内容可在侧边栏后方流动
   - 滚动时有平滑的边缘效果
   - UI 元素悬浮但不抢焦点

3. **设计规范符合性**：
   - 通过 Apple HIG 设计审查
   - 符合 WWDC 2025 Liquid Glass 示例
   - 遵循同心几何数学计算

4. **性能**：
   - 材质渲染无卡顿（60fps）
   - 窗口缩放流畅
   - 动画过渡自然

## Open Questions

1. **圆角半径基准值**：
   - 窗口最外层圆角应使用多少？建议 12pt（符合 macOS 标准）
   - 内层圆角计算公式：父级 - 内边距，是否需要最小值限制？

2. **Liquid Glass 材质选择**：
   - 侧边栏使用 `.sidebar` 还是 `.ultraThinMaterial`？
   - 内容区域是否需要材质背景？

3. **滚动边缘效果强度**：
   - 硬样式的不透明度应设置为多少？
   - 模糊半径应设置为多少？

4. **后向兼容性**：
   - 是否需要支持 macOS 13/14？
   - 如果需要，如何优雅降级？

5. **侧边栏内容延伸范围**：
   - 哪些内容应该在侧边栏后方延伸？
   - 是否所有标签页都采用相同的延伸策略？

## Dependencies

- 无外部依赖
- 依赖现有的 SwiftUI 和 macOS API
- 需要 macOS 13+ 的 `.ultraThinMaterial` 支持

## Migration Strategy

1. **阶段 1：设计系统更新**（1-2 天）
   - 更新 `DesignTokens.swift` 添加同心几何计算工具
   - 添加 Liquid Glass 材质定义
   - 添加滚动边缘效果组件

2. **阶段 2：Settings 窗口改造**（2-3 天）
   - 重构 `SettingsView.swift` 布局结构
   - 更新 `ModernSidebarView.swift` 使用 Liquid Glass 材质
   - 移除硬边框和装饰性阴影

3. **阶段 3：细节优化**（1-2 天）
   - 添加滚动边缘效果
   - 优化内容延伸和材质交互
   - 调整动画和过渡效果

4. **阶段 4：测试和验证**（1 天）
   - 视觉回归测试
   - 性能测试（帧率、内存）
   - 多分辨率测试

**总时长**：5-8 天

## Risks and Mitigations

### 风险 1：设计过于激进
- **风险**：完全采用 Liquid Glass 可能与用户现有认知不符
- **缓解**：保留核心交互模式，仅更新视觉呈现

### 风险 2：性能影响
- **风险**：Liquid Glass 材质可能增加 GPU 负载
- **缓解**：
  - 使用 Instruments 进行性能测试
  - 必要时降低材质复杂度
  - 提供"减少透明度"选项

### 风险 3：实现复杂度
- **风险**：同心几何计算可能增加代码复杂度
- **缓解**：
  - 封装计算逻辑为可复用工具
  - 添加详细注释和文档
  - 提供示例和测试

### 风险 4：macOS 版本兼容性
- **风险**：某些材质在旧版本 macOS 上不可用
- **缓解**：
  - 使用 `@available` 检查
  - 提供降级方案（使用半透明背景）
  - 明确最低支持版本

## Alternatives Considered

### 替代方案 1：渐进式采用
- **描述**：仅采用部分 Liquid Glass 特性，保留现有边框和阴影
- **优点**：实现简单，风险低
- **缺点**：不符合设计系统一致性，视觉效果不完整
- **决策**：不采用，因为会导致设计语言混杂

### 替代方案 2：自定义材质实现
- **描述**：使用 SwiftUI 自定义视图实现类似 Liquid Glass 的效果
- **优点**：更高的控制度，跨版本一致性
- **缺点**：实现复杂，性能可能不如系统原生材质
- **决策**：不采用，优先使用系统 API

### 替代方案 3：保持现状
- **描述**：不进行任何改造
- **优点**：无实现成本，无风险
- **缺点**：设计语言过时，与 macOS 系统应用视觉不一致
- **决策**：不采用，设计更新对用户体验有明显提升

## Validation Plan

### 设计验证
- [ ] 对比 Apple 系统应用（System Settings、Xcode）
- [ ] 检查同心几何计算正确性
- [ ] 验证材质使用符合 HIG 规范

### 功能验证
- [ ] 所有设置功能正常工作
- [ ] 窗口缩放流畅
- [ ] 侧边栏选择和导航正常

### 性能验证
- [ ] 使用 Instruments 检查帧率（目标：60fps）
- [ ] 检查 GPU 使用率
- [ ] 检查内存占用

### 兼容性验证
- [ ] macOS 13 Ventura
- [ ] macOS 14 Sonoma
- [ ] macOS 15 Sequoia（如果可用）

## References

- [WWDC 2025 - Apple Design System (Liquid Glass)](https://developer.apple.com/cn/videos/play/wwdc2025/356/)
- [Apple Human Interface Guidelines - macOS](https://developer.apple.com/design/human-interface-guidelines/macos)
- [SwiftUI - VisualEffect](https://developer.apple.com/documentation/swiftui/visualeffect)
- [现有设计文档 - settings-ui-layout](../../../specs/settings-ui-layout/spec.md)

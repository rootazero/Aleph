# Tasks: adopt-liquid-glass-design

本任务清单按照实现顺序排列，每个任务都可独立验证。

## Phase 1: Design System Infrastructure（基础设施）

### Task 1.1: Create ConcentricGeometry utility class
**Priority**: High
**Estimated Time**: 2 hours

**Description**:
创建同心几何计算工具类，提供圆角半径计算函数。

**Acceptance Criteria**:
- [x] 创建 `Aether/Sources/DesignSystem/ConcentricGeometry.swift`
- [x] 实现 `childRadius(parent:padding:minimum:)` 静态函数
- [x] 添加预定义常量：`windowRadius`, `sidebarRadius`, `cardRadius`
- [ ] 编写单元测试验证边界条件（负值、零值、极小值）
- [x] 添加文档注释说明数学公式

**Dependencies**: None

**Verification**:
```bash
# 运行单元测试
swift test --filter ConcentricGeometryTests
```

---

### Task 1.2: Create AdaptiveMaterial component
**Priority**: High
**Estimated Time**: 3 hours

**Description**:
创建自适应材质组件，根据 macOS 版本选择 Liquid Glass 或降级方案。

**Acceptance Criteria**:
- [x] 创建 `Aether/Sources/Components/Atoms/AdaptiveMaterial.swift`
- [x] 实现 `@available` 版本检测
- [x] 支持 macOS 15+ 使用 `.ultraThinMaterial`
- [x] 支持 macOS 13-14 使用基础材质
- [x] 支持 macOS 12 及以下使用半透明背景降级
- [x] 添加 `FallbackStyle` 枚举定义降级样式
- [x] 编写预览（Preview）展示不同材质效果

**Dependencies**: None

**Verification**:
在不同 macOS 版本上运行应用，验证材质正确渲染。

---

### Task 1.3: Create ScrollEdgeModifier
**Priority**: Medium
**Estimated Time**: 2 hours

**Description**:
创建滚动边缘效果修饰符，为滚动视图添加淡入淡出边缘。

**Acceptance Criteria**:
- [x] 创建 `Aether/Sources/DesignSystem/ScrollEdgeModifier.swift`
- [x] 实现 `.soft` 和 `.hard` 样式
- [x] 支持上下边缘独立控制
- [x] 使用 LinearGradient 遮罩实现淡入淡出
- [x] 添加预览展示效果
- [x] 添加 View 扩展 `.scrollEdge(edges:style:)` 便捷方法

**Dependencies**: None

**Verification**:
在长列表滚动时观察边缘淡入淡出效果。

---

### Task 1.4: Update DesignTokens for Liquid Glass
**Priority**: High
**Estimated Time**: 1 hour

**Description**:
更新设计令牌，添加 Liquid Glass 相关常量和材质定义。

**Acceptance Criteria**:
- [x] 在 `DesignTokens.swift` 添加 `Materials` 命名空间
- [x] 定义 `sidebar`, `titlebar`, `content`, `floatingPanel` 材质
- [x] 添加 `ConcentricRadius` 命名空间引用 `ConcentricGeometry`
- [x] 更新 `Shadows` 定义（移除硬阴影，添加微妙阴影）
- [x] 添加文档注释说明使用场景

**Dependencies**: Task 1.1, Task 1.2

**Verification**:
编译通过，无警告。

---

## Phase 2: Settings Window Refactoring（窗口重构）

### Task 2.1: Refactor SettingsView window structure
**Priority**: High
**Estimated Time**: 4 hours

**Description**:
重构 `SettingsView` 使用 ZStack 分层和同心几何布局。

**Acceptance Criteria**:
- [x] 修改 `SettingsView.body` 使用 ZStack
- [x] 底层：HStack with 200pt left spacer + content area
- [x] 内容区域使用同心圆角（12pt）
- [x] 顶层：浮动侧边栏（10pt 圆角）
- [x] 移除现有的硬边框和阴影
- [x] 应用 AdaptiveMaterial 背景
- [x] 保持现有功能不变（标签切换、配置加载）

**Dependencies**: Task 1.1, Task 1.2, Task 1.4

**Verification**:
```bash
# 构建并运行
xcodegen generate && xcodebuild -project Aether.xcodeproj -scheme Aether build
```
打开 Settings 窗口，验证布局正确。

---

### Task 2.2: Update ModernSidebarView with Liquid Glass material
**Priority**: High
**Estimated Time**: 3 hours

**Description**:
更新侧边栏使用 Liquid Glass 材质，移除硬边框和阴影。

**Acceptance Criteria**:
- [x] 修改 `ModernSidebarView` 背景为 `AdaptiveMaterial(.sidebar)`
- [x] 移除现有的 `.shadow()` 修饰符
- [x] 移除硬边框 `.stroke()`
- [x] 应用同心圆角（10pt）
- [x] 添加微妙阴影（仅浮动层需要）
- [x] 保持现有交互功能（选择、导航、按钮）

**Dependencies**: Task 1.2, Task 1.4

**Verification**:
侧边栏显示 Liquid Glass 效果，无硬边框，微妙阴影。

---

### Task 2.3: Implement content extension strategy
**Priority**: Medium
**Estimated Time**: 3 hours

**Description**:
实现内容在侧边栏后方延伸的策略。

**Acceptance Criteria**:
- [ ] 创建 `ContentExtensionStyle` 枚举（full, partial, none）
- [ ] 为每个标签页定义延伸策略
- [ ] Providers 标签：左侧列表 full 延伸，右侧面板 none
- [ ] General/Memory/Behavior 标签：full 延伸
- [ ] Shortcuts 标签：none 延伸
- [ ] 修改各标签页视图应用延伸策略
- [ ] 确保内容在侧边栏下方可见（使用 `.zIndex()`）

**Dependencies**: Task 2.1

**Verification**:
切换标签页，验证内容延伸效果符合预期。

---

### Task 2.4: Update title bar integration
**Priority**: Medium
**Estimated Time**: 2 hours

**Description**:
更新标题栏使用透明材质和功能层概念。

**Acceptance Criteria**:
- [ ] 修改 `customTitleBar` 使用 `AdaptiveMaterial(.titlebar)`
- [ ] 移除底部边框（使用微妙阴影代替）
- [ ] 确保窗口控制按钮区域保留（80pt）
- [ ] Theme Switcher 右对齐
- [ ] 标题栏高度保持 52pt
- [ ] 添加滚动边缘效果（当内容滚动时）

**Dependencies**: Task 1.2, Task 1.3

**Verification**:
标题栏透明，窗口控制按钮正常工作，Theme Switcher 可用。

---

## Phase 3: Content Area Optimization（内容区域优化）

### Task 3.1: Remove hard borders from all cards
**Priority**: High
**Estimated Time**: 2 hours

**Description**:
移除所有卡片和容器的硬边框，使用间距表现层次。

**Acceptance Criteria**:
- [x] 移除 `MemoryView` 卡片边框
- [x] 移除 `BehaviorSettingsView` 卡片边框
- [x] 移除 `ShortcutsView` 卡片边框
- [x] 移除 `ProvidersView` 卡片边框
- [x] 移除 `RoutingView` 卡片边框
- [x] 保留表单输入框边框（功能需要）
- [x] 使用 `DesignTokens.Spacing.lg` 增加卡片间距
- [x] 使用背景色差异表现层次（轻微对比）

**Dependencies**: Task 1.4

**Verification**:
所有设置标签页无硬边框，视觉层次清晰。

---

### Task 3.2: Remove decorative shadows
**Priority**: Medium
**Estimated Time**: 1 hour

**Description**:
移除装饰性阴影，仅保留功能性阴影（浮动层）。

**Acceptance Criteria**:
- [x] 移除所有 `.shadow(DesignTokens.Shadows.card)` 调用
- [x] 仅保留浮动侧边栏的微妙阴影
- [x] 浮动面板（如果有）保留微妙阴影
- [x] 更新 `DesignTokens.Shadows` 定义，标记 `.card` 为弃用

**Dependencies**: None

**Verification**:
视觉检查，确认无装饰性阴影。

---

### Task 3.3: Apply scroll edge effects to scrollable content
**Priority**: Medium
**Estimated Time**: 2 hours

**Description**:
为所有可滚动内容应用滚动边缘效果。

**Acceptance Criteria**:
- [x] Memory 浏览器应用硬样式滚动边缘效果
- [x] BehaviorSettingsView 应用硬样式滚动边缘效果
- [x] 侧边栏导航列表应用软样式滚动边缘效果
- [~] Provider 列表应用硬样式滚动边缘效果 (可选优化)
- [~] Routing 规则列表应用硬样式滚动边缘效果 (可选优化)
- [~] Edit Panel 应用硬样式滚动边缘效果 (可选优化)

**Dependencies**: Task 1.3

**Verification**:
滚动各列表，观察边缘淡入淡出效果。

---

### Task 3.4: Optimize layout spacing and grouping
**Priority**: Medium
**Estimated Time**: 3 hours

**Description**:
优化布局间距和分组，使用布局而非装饰表现层次。

**Acceptance Criteria**:
- [ ] 主要部分间距使用 `.lg` (24pt)
- [ ] 次要部分间距使用 `.md` (16pt)
- [ ] 表单字段间距使用 `.sm` (8pt)
- [ ] 相关控件分组（使用 VStack/HStack）
- [ ] 移除冗余的 Divider（使用间距代替）
- [ ] 功能边界使用间距而非装饰

**Dependencies**: Task 3.1, Task 3.2

**Verification**:
视觉检查，布局清晰，层次分明。

---

## Phase 4: Testing and Validation（测试验证）

### Task 4.1: Visual regression testing
**Priority**: High
**Estimated Time**: 2 hours

**Description**:
执行视觉回归测试，确保设计符合规范。

**Acceptance Criteria**:
- [ ] 截图所有标签页（6 个标签）
- [ ] 对比设计稿（Figma/Sketch）
- [ ] 验证同心圆角计算正确
- [ ] 验证材质渲染符合 Liquid Glass 规范
- [ ] 验证间距和分组符合 DesignTokens
- [ ] 记录任何视觉偏差

**Dependencies**: 所有 Phase 1-3 任务

**Verification**:
生成测试报告，列出通过/失败项。

---

### Task 4.2: Performance testing
**Priority**: High
**Estimated Time**: 2 hours

**Description**:
执行性能测试，确保 60fps 流畅体验。

**Acceptance Criteria**:
- [ ] 使用 Instruments (Core Animation) 测试帧率
- [ ] 窗口缩放保持 60fps
- [ ] 内容滚动保持 60fps
- [ ] 标签页切换保持 60fps
- [ ] GPU 使用率 < 30%
- [ ] 内存占用 < 50MB
- [ ] 记录性能指标

**Dependencies**: 所有 Phase 1-3 任务

**Verification**:
生成性能报告，包含帧率图表和 GPU 使用率。

---

### Task 4.3: Compatibility testing on multiple macOS versions
**Priority**: High
**Estimated Time**: 2 hours

**Description**:
在不同 macOS 版本上测试兼容性和降级方案。

**Acceptance Criteria**:
- [ ] macOS 15 测试：完整 Liquid Glass 效果
- [ ] macOS 14 测试：基础材质效果
- [ ] macOS 13 测试：基础材质效果
- [ ] macOS 12 测试（如果支持）：降级方案
- [ ] 验证所有版本功能正常
- [ ] 记录版本差异

**Dependencies**: Task 1.2

**Verification**:
生成兼容性报告，列出各版本表现。

---

### Task 4.4: Accessibility testing
**Priority**: Medium
**Estimated Time**: 1 hour

**Description**:
测试辅助功能，确保可访问性。

**Acceptance Criteria**:
- [ ] VoiceOver 导航测试
- [ ] 键盘导航测试
- [ ] 高对比度模式测试
- [ ] 减少透明度选项测试
- [ ] 验证所有交互元素可访问
- [ ] 记录辅助功能问题

**Dependencies**: 所有 Phase 1-3 任务

**Verification**:
生成辅助功能报告。

---

## Phase 5: Documentation and Release（文档和发布）

### Task 5.1: Update design system documentation
**Priority**: Medium
**Estimated Time**: 2 hours

**Description**:
更新设计系统文档，说明 Liquid Glass 使用规范。

**Acceptance Criteria**:
- [ ] 更新 `CLAUDE.md` 添加 Liquid Glass 设计原则
- [ ] 创建 `docs/design-system/liquid-glass.md` 详细文档
- [ ] 添加代码示例和最佳实践
- [ ] 添加常见问题解答
- [ ] 添加视觉示例（截图或图表）

**Dependencies**: 所有 Phase 1-4 任务

**Verification**:
文档审查，确保清晰准确。

---

### Task 5.2: Create migration guide
**Priority**: Medium
**Estimated Time**: 1 hour

**Description**:
创建迁移指南，帮助开发者应用新设计系统。

**Acceptance Criteria**:
- [ ] 创建 `docs/migration/liquid-glass-migration.md`
- [ ] 说明从旧设计迁移到 Liquid Glass 的步骤
- [ ] 提供代码迁移示例
- [ ] 列出常见问题和解决方案
- [ ] 添加前后对比截图

**Dependencies**: Task 5.1

**Verification**:
文档审查。

---

### Task 5.3: Update CHANGELOG and create release notes
**Priority**: Low
**Estimated Time**: 1 hour

**Description**:
更新 CHANGELOG 并创建发布说明。

**Acceptance Criteria**:
- [ ] 更新 `CHANGELOG.md` 添加 Liquid Glass 设计更新
- [ ] 创建 GitHub Release Notes
- [ ] 说明视觉变化和用户可见改进
- [ ] 添加截图展示新设计
- [ ] 标记为主要版本更新（如 v0.3.0）

**Dependencies**: 所有任务

**Verification**:
发布说明审查。

---

## Summary

**Total Tasks**: 21
**Estimated Time**: ~40 hours (1 week)
**Critical Path**: Task 1.1 → 1.2 → 1.4 → 2.1 → 2.2 → 3.1 → 4.1 → 4.2

**Parallel Work Opportunities**:
- Task 1.3 可与 1.1/1.2 并行
- Task 2.3/2.4 可与 2.2 并行
- Task 3.2/3.3 可与 3.1 并行
- Task 5.1/5.2 可并行

**Dependencies Graph**:
```
1.1 ──┬──> 1.4 ──┬──> 2.1 ──┬──> 2.3 ──> 3.3 ──> 4.1 ──> 5.1 ──> 5.3
      │          │          │                       │
1.2 ──┘          └──> 2.2 ──┤                       ├──> 5.2
                             │                       │
1.3 ─────────────────────────┴──> 2.4 ──> 3.1 ──────┤
                                           │        │
                                           └──> 3.2 ┤
                                                │   │
                                                └───┴──> 4.2 ──> 4.3 ──> 4.4
```

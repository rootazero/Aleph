# Design Document: adopt-liquid-glass-design

## Architecture Overview

本设计文档详细说明如何将 Apple Liquid Glass 设计系统应用到 Aether Settings 窗口，同时保持向后兼容性。

## Design Goals

1. **视觉现代化**：采用 Liquid Glass 材质和同心几何布局
2. **向后兼容**：支持 macOS 13+，优雅降级
3. **性能优先**：保持 60fps 流畅体验
4. **代码可维护性**：封装设计系统为可复用组件

## Core Design Patterns

### 1. 同心几何系统（Concentric Geometry System）

#### 原理
所有形状围绕共同中心点对齐，圆角半径基于数学计算：

```
子级圆角 = max(父级圆角 - 内边距, 最小圆角)
```

#### 形状类型

1. **固定形状（Fixed Shape）**：
   - 使用恒定的圆角半径
   - 示例：按钮、卡片
   - 实现：`RoundedRectangle(cornerRadius: constant)`

2. **胶囊形状（Capsule Shape）**：
   - 圆角半径 = 容器高度 / 2
   - 示例：大尺寸按钮、标签
   - 实现：`Capsule()` 或 `RoundedRectangle(cornerRadius: height / 2)`

3. **同心形状（Concentric Shape）**：
   - 圆角半径 = 父级圆角 - 内边距
   - 示例：嵌套容器、内容区域
   - 实现：自定义修饰符 `.concentricShape(parent: CGFloat, padding: CGFloat)`

#### 实现策略

创建 `ConcentricGeometry` 工具类：

```swift
struct ConcentricGeometry {
    /// 计算同心圆角半径
    static func childRadius(parent: CGFloat, padding: CGFloat, minimum: CGFloat = 4) -> CGFloat {
        return max(parent - padding, minimum)
    }

    /// 窗口级别圆角
    static let windowRadius: CGFloat = 12

    /// 内容区域圆角
    static func contentRadius(padding: CGFloat) -> CGFloat {
        return childRadius(parent: windowRadius, padding: padding)
    }

    /// 卡片圆角
    static func cardRadius(contentPadding: CGFloat, cardPadding: CGFloat) -> CGFloat {
        let contentRadius = self.contentRadius(padding: contentPadding)
        return childRadius(parent: contentRadius, padding: cardPadding)
    }
}
```

### 2. Liquid Glass 材质层级

#### 材质选择矩阵

| 组件 | macOS 15+ | macOS 13-14 | 降级方案 |
|------|-----------|-------------|---------|
| 侧边栏 | `.sidebar` | `.sidebar` | 半透明背景 + 模糊 |
| 标题栏 | `.titlebar` | `.titlebar` | 半透明背景 |
| 内容区域 | `.windowBackground` | `.windowBackground` | 纯色背景 |
| 浮动面板 | `.ultraThinMaterial` | `.ultraThinMaterial` | 半透明背景 + 模糊 |

#### 向后兼容实现

```swift
struct AdaptiveMaterial: View {
    let preferredMaterial: Material
    let fallbackStyle: FallbackStyle

    enum FallbackStyle {
        case solid(Color)
        case translucent(Color, opacity: Double, blur: CGFloat)
    }

    var body: some View {
        if #available(macOS 15, *) {
            VisualEffectBackground(material: preferredMaterial, blendingMode: .withinWindow)
        } else if #available(macOS 13, *) {
            // macOS 13-14 使用基础材质
            VisualEffectBackground(material: preferredMaterial, blendingMode: .behindWindow)
        } else {
            // macOS 12 及以下使用降级方案
            fallbackBackground
        }
    }

    @ViewBuilder
    private var fallbackBackground: some View {
        switch fallbackStyle {
        case .solid(let color):
            color
        case .translucent(let color, let opacity, let blur):
            color.opacity(opacity).blur(radius: blur)
        }
    }
}
```

### 3. 滚动边缘效果（Scroll Edge Effect）

#### 效果样式

- **硬样式（macOS）**：不透明度更高，适合固定式文本和无背景控件
- **软样式（iOS/iPadOS）**：微妙过渡，适合交互式元素

#### 实现

```swift
struct ScrollEdgeModifier: ViewModifier {
    let edges: Edge.Set
    let style: Style

    enum Style {
        case soft(opacity: Double = 0.3, blur: CGFloat = 8)
        case hard(opacity: Double = 0.6, blur: CGFloat = 12)
    }

    func body(content: Content) -> some View {
        content
            .mask(
                LinearGradient(
                    gradient: Gradient(stops: gradientStops),
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
    }

    private var gradientStops: [Gradient.Stop] {
        switch style {
        case .soft(let opacity, _):
            return [
                .init(color: .clear, location: 0),
                .init(color: .white.opacity(opacity), location: 0.05),
                .init(color: .white, location: 0.1),
                .init(color: .white, location: 0.9),
                .init(color: .white.opacity(opacity), location: 0.95),
                .init(color: .clear, location: 1)
            ]
        case .hard(let opacity, _):
            return [
                .init(color: .clear, location: 0),
                .init(color: .white.opacity(opacity), location: 0.02),
                .init(color: .white, location: 0.05),
                .init(color: .white, location: 0.95),
                .init(color: .white.opacity(opacity), location: 0.98),
                .init(color: .clear, location: 1)
            ]
        }
    }
}
```

### 4. 布局层次系统

#### 层次表现方式

1. **间距和分组**（优先）：
   - 使用 `DesignTokens.Spacing` 定义垂直间距
   - 主要部分间距：`.lg` (24pt)
   - 次要部分间距：`.md` (16pt)
   - 表单字段间距：`.sm` (8pt)

2. **着色**（次要）：
   - 主要操作：系统强调色（蓝色）
   - 次要操作：中性色（灰色）
   - 警告操作：红色/橙色

3. **字体层次**：
   - 标题：`.title` (20pt, semibold)
   - 正文：`.body` (14pt, regular)
   - 辅助文字：`.caption` (12pt, regular)

#### 禁止的装饰元素

- ❌ 硬边框（除非表单输入框）
- ❌ 装饰性阴影（除非浮动层）
- ❌ 冗余背景色（除非功能需要）
- ❌ 多余的分隔线（用间距代替）

## Component Architecture

### 窗口层级结构

```
NSWindow
├── fullSizeContentView: true
├── titlebarAppearsTransparent: true
└── Content (SwiftUI)
    ├── Layer 0: Background (Liquid Glass 或纯色)
    ├── Layer 1: Main Content (HStack)
    │   ├── Left Spacer (200pt)
    │   └── Right Content Area
    │       ├── Concentric Shape (12pt - 0pt = 12pt radius)
    │       ├── Title Bar (功能层)
    │       └── Tab Content (Providers, Routing, etc.)
    └── Layer 2: Floating Sidebar
        ├── Concentric Shape (12pt - 12pt = 0pt, 使用固定 10pt)
        ├── Liquid Glass Material
        └── Subtle Shadow (非硬阴影)
```

### 侧边栏内容延伸策略

#### 延伸区域

1. **完全延伸**（适用标签页）：
   - General
   - Memory
   - Behavior

2. **部分延伸**（适用标签页）：
   - Providers（左侧卡片列表延伸，右侧编辑面板不延伸）
   - Routing（规则列表延伸，编辑器不延伸）

3. **不延伸**（适用标签页）：
   - Shortcuts（需要清晰的视觉边界）

#### 实现

```swift
enum ContentExtensionStyle {
    case full           // 内容完全延伸到侧边栏下方
    case partial(CGFloat) // 内容部分延伸（指定宽度）
    case none           // 内容不延伸
}

struct ContentAreaView: View {
    let extensionStyle: ContentExtensionStyle

    var body: some View {
        HStack(spacing: 0) {
            switch extensionStyle {
            case .full:
                content
            case .partial(let width):
                HStack(spacing: 0) {
                    extendedContent.frame(width: width)
                    regularContent
                }
            case .none:
                Color.clear.frame(width: sidebarWidth)
                content
            }
        }
    }
}
```

## Data Flow

### 设计系统配置流程

1. **应用启动**：
   - 检测 macOS 版本
   - 加载对应的设计令牌（Design Tokens）
   - 初始化 Liquid Glass 材质或降级方案

2. **窗口创建**：
   - 应用同心几何计算
   - 设置材质和背景
   - 配置标题栏样式

3. **内容渲染**：
   - 根据标签页选择内容延伸策略
   - 应用滚动边缘效果
   - 应用布局层次规则

## Performance Considerations

### 材质渲染优化

1. **减少材质层级**：
   - 最多 3 层材质嵌套
   - 避免在滚动视图内使用复杂材质

2. **动画性能**：
   - 使用 CALayer 动画而非 SwiftUI 动画（关键路径）
   - 材质过渡使用 `.easing` 而非 `.spring`

3. **内存管理**：
   - 惰性加载内容区域
   - 释放不可见标签页的视图

### 降级策略

| macOS 版本 | 材质支持 | 降级方案 |
|-----------|---------|---------|
| 15+ | 完整 Liquid Glass | - |
| 13-14 | 基础材质 | 使用 `.sidebar`, `.titlebar` |
| 12 及以下 | 无 | 半透明背景 + 模糊 |

## Testing Strategy

### 视觉回归测试

1. **快照测试**：
   - 捕获各标签页截图
   - 对比设计稿和实现
   - 自动化检测视觉差异

2. **同心几何验证**：
   - 单元测试 `ConcentricGeometry` 计算
   - 边界条件测试（最小圆角、负内边距）

3. **材质回退测试**：
   - 模拟不同 macOS 版本
   - 验证降级方案正确渲染

### 性能测试

1. **帧率测试**：
   - 使用 Instruments (Core Animation)
   - 目标：窗口缩放和滚动保持 60fps

2. **GPU 使用率**：
   - 监控材质渲染 GPU 负载
   - 目标：< 30% GPU 使用率

3. **内存占用**：
   - 监控窗口创建和销毁内存变化
   - 目标：无内存泄漏，峰值 < 50MB

## Security and Privacy

无额外安全或隐私影响。设计更新仅涉及视觉呈现，不改变数据处理逻辑。

## Rollout Plan

### 阶段 1：基础设施（2 天）

- 创建 `ConcentricGeometry` 工具类
- 创建 `AdaptiveMaterial` 组件
- 创建 `ScrollEdgeModifier`
- 更新 `DesignTokens` 添加 Liquid Glass 令牌

### 阶段 2：窗口和侧边栏改造（2 天）

- 重构 `SettingsView` 窗口结构
- 更新 `ModernSidebarView` 使用 Liquid Glass 材质
- 实现内容延伸策略

### 阶段 3：内容区域优化（2 天）

- 移除硬边框和装饰性阴影
- 应用滚动边缘效果
- 优化布局和间距

### 阶段 4：测试和调优（1 天）

- 执行视觉回归测试
- 执行性能测试
- 在不同 macOS 版本上验证

### 阶段 5：文档和发布（1 天）

- 更新设计系统文档
- 编写迁移指南
- 创建 PR 和发布说明

**总计**：8 个工作日

## Future Enhancements

1. **动画过渡**：
   - 标签页切换时的流畅过渡
   - 侧边栏展开/收起动画

2. **深色模式优化**：
   - 深色模式下的材质调整
   - 高对比度模式支持

3. **辅助功能**：
   - 减少透明度选项
   - 增加对比度选项
   - VoiceOver 优化

4. **自定义主题**：
   - 允许用户自定义材质和圆角
   - 预设主题（简洁、经典、科技）

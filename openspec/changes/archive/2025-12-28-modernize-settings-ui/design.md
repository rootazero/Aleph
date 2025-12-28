# Design: Modern Settings UI Architecture

## Context

Aether当前使用SwiftUI构建设置界面，功能完整但视觉呈现较为基础。本次改造参考uisample.png的现代化设计风格，目标是在保持现有功能的前提下，全面提升视觉体验和交互流畅度。

**技术背景:**
- SwiftUI作为主UI框架
- 需要兼容macOS 13+ (Ventura)
- 无需修改Rust Core和UniFFI绑定
- 设置窗口大小：最小800x600，理想1200x800

**设计约束:**
- 不能破坏现有配置加载/保存逻辑
- 必须保持所有现有功能可访问
- 性能不能下降（渲染帧率保持60fps）
- 支持三种主题模式：白天（Light）、夜晚（Dark）、跟随系统（Auto）
- 主题选择需持久化到用户配置

## Goals / Non-Goals

### Goals
1. **视觉现代化**: 实现uisample.png所示的精致视觉效果
2. **提升可用性**: 添加搜索、筛选等高级交互功能
3. **组件化设计**: 创建可复用的UI组件库
4. **无缝迁移**: 不影响现有用户配置和工作流

### Non-Goals
1. **功能扩展**: 本次仅改造UI，不添加新的配置项
2. **跨平台**: 仅针对macOS优化，不考虑Windows/Linux
3. **后端修改**: 不修改Rust Core或配置文件格式
4. **动画复杂化**: 避免过度动画，保持性能优先

## Decisions

### Decision 1: Design Token System (设计规范系统)

**What:** 创建`DesignTokens.swift`文件，集中管理所有视觉参数

```swift
enum DesignTokens {
    // Colors
    enum Colors {
        static let sidebarBackground = Color(nsColor: .controlBackgroundColor)
        static let cardBackground = Color(nsColor: .controlBackgroundColor).opacity(0.5)
        static let accentBlue = Color(red: 0.0, green: 0.48, blue: 1.0)
        static let providerActive = Color.green
        static let providerInactive = Color.gray
    }

    // Spacing
    enum Spacing {
        static let xs: CGFloat = 4
        static let sm: CGFloat = 8
        static let md: CGFloat = 16
        static let lg: CGFloat = 24
        static let xl: CGFloat = 32
    }

    // Corner Radius
    enum CornerRadius {
        static let small: CGFloat = 6
        static let medium: CGFloat = 10
        static let large: CGFloat = 16
    }

    // Typography
    enum Typography {
        static let title = Font.system(size: 22, weight: .semibold)
        static let heading = Font.system(size: 17, weight: .medium)
        static let body = Font.system(size: 14)
        static let caption = Font.system(size: 12)
    }
}
```

**Why:**
- 保证设计一致性
- 方便后续主题切换
- 易于维护和调整

**Alternatives Considered:**
- ❌ 硬编码颜色和间距：难以维护，不一致
- ❌ 使用第三方设计系统库：增加依赖，过度复杂

### Decision 2: Component Architecture (组件架构)

**What:** 采用原子设计（Atomic Design）模式，创建三层组件结构

```
Components/
├── Atoms/           # 原子组件（最小单位）
│   ├── SearchBar.swift
│   ├── StatusIndicator.swift
│   └── ActionButton.swift
├── Molecules/       # 分子组件（组合原子）
│   ├── ProviderCard.swift
│   ├── SidebarItem.swift
│   └── DetailPanel.swift
└── Organisms/       # 有机组件（完整模块）
    ├── ModernSidebar.swift
    ├── ProviderListView.swift
    └── ProviderDetailView.swift
```

**Why:**
- 组件可复用性强
- 测试更容易
- 符合SwiftUI最佳实践

**Alternatives Considered:**
- ❌ 单一文件包含所有UI：难以维护，文件过大
- ❌ 按页面划分：组件重复，不利于一致性

### Decision 3: Layout Strategy (布局策略)

**What:** 使用三栏布局 (Sidebar | Content | Detail Panel)

```
┌─────────────────────────────────────────────────────────┐
│  Sidebar    │     Content Area        │  Detail Panel   │
│  (200pt)    │     (flexible)          │  (350pt)        │
│             │                         │                 │
│  General    │  ┌──────────────────┐   │  DeepSeek       │
│  Providers  │  │  Provider Card   │   │  Active         │
│  Routing    │  └──────────────────┘   │                 │
│  Shortcuts  │  ┌──────────────────┐   │  Description... │
│  Behavior   │  │  Provider Card   │   │                 │
│  Memory     │  └──────────────────┘   │  [Config Panel] │
│             │                         │                 │
└─────────────────────────────────────────────────────────┘
```

**Implementation:**
```swift
NavigationSplitView(columnVisibility: $columnVisibility) {
    // Sidebar: 200pt fixed width
    ModernSidebarView(selectedTab: $selectedTab)
        .navigationSplitViewColumnWidth(200)
} content: {
    // Content: Flexible width
    ContentView(selectedTab: selectedTab, selectedProvider: $selectedProvider)
} detail: {
    // Detail Panel: 350pt ideal width (collapsible)
    if let provider = selectedProvider {
        ProviderDetailPanel(provider: provider)
            .navigationSplitViewColumnWidth(ideal: 350, max: 500)
    }
}
```

**Why:**
- 符合macOS三栏布局规范
- 提供更多信息展示空间
- 支持窗口大小自适应

**Alternatives Considered:**
- ❌ 二栏布局（无Detail Panel）：信息展示受限，需要弹窗
- ❌ 模态弹窗展示详情：打断用户流程，体验不连贯

### Decision 4: Search & Filter Implementation (搜索和筛选实现)

**What:** 在ContentView顶部添加SearchBar，实时过滤Provider列表

```swift
struct ProviderListView: View {
    @State private var searchText = ""
    let providers: [ProviderConfigEntry]

    var filteredProviders: [ProviderConfigEntry] {
        if searchText.isEmpty {
            return providers
        }
        return providers.filter { provider in
            provider.name.localizedCaseInsensitiveContains(searchText) ||
            provider.providerType.localizedCaseInsensitiveContains(searchText)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            SearchBar(text: $searchText, placeholder: "Search providers...")
                .padding(.horizontal, 16)
                .padding(.vertical, 12)

            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(filteredProviders) { provider in
                        ProviderCard(provider: provider)
                    }
                }
                .padding(16)
            }
        }
    }
}
```

**Why:**
- 实时搜索响应快速
- 符合用户预期（类似macOS Finder搜索）
- 代码简单，易于维护

**Alternatives Considered:**
- ❌ 延迟搜索（debounce）：对小数据集不必要
- ❌ 后端搜索：配置项少，前端搜索足够

### Decision 5: Theme System with Manual Control (主题系统与手动控制)

**What:** 提供三种主题模式，并在右上角添加精美的主题切换器

**主题模式:**
1. **白天模式 (Light)**: 明亮配色，浅色背景
2. **夜晚模式 (Dark)**: 深色配色，深色背景
3. **跟随系统 (Auto)**: 自动跟随macOS系统外观设置

**主题切换器设计:**
```swift
struct ThemeSwitcher: View {
    @Binding var selectedTheme: ThemeMode

    var body: some View {
        HStack(spacing: 0) {
            ThemeButton(
                icon: "sun.max.fill",
                theme: .light,
                isSelected: selectedTheme == .light
            )
            ThemeButton(
                icon: "moon.fill",
                theme: .dark,
                isSelected: selectedTheme == .dark
            )
            ThemeButton(
                icon: "circle.lefthalf.filled",
                theme: .auto,
                isSelected: selectedTheme == .auto
            )
        }
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                .fill(Color(nsColor: .controlBackgroundColor))
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                .stroke(Color.gray.opacity(0.2), lineWidth: 1)
        )
    }
}

struct ThemeButton: View {
    let icon: String
    let theme: ThemeMode
    let isSelected: Bool

    var body: some View {
        Button(action: { /* switch theme */ }) {
            Image(systemName: icon)
                .font(.system(size: 14))
                .foregroundColor(isSelected ? .white : .secondary)
                .frame(width: 32, height: 28)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small - 1)
                        .fill(isSelected ? Color.accentColor : Color.clear)
                )
        }
        .buttonStyle(.plain)
    }
}

enum ThemeMode: String, Codable {
    case light = "light"
    case dark = "dark"
    case auto = "auto"
}
```

**位置:** 设置窗口右上角（toolbar区域）

**持久化:**
```swift
class ThemeManager: ObservableObject {
    @Published var currentTheme: ThemeMode {
        didSet {
            UserDefaults.standard.set(currentTheme.rawValue, forKey: "app_theme")
            applyTheme()
        }
    }

    init() {
        let saved = UserDefaults.standard.string(forKey: "app_theme") ?? "auto"
        currentTheme = ThemeMode(rawValue: saved) ?? .auto
        applyTheme()
    }

    func applyTheme() {
        switch currentTheme {
        case .light:
            NSApp.appearance = NSAppearance(named: .aqua)
        case .dark:
            NSApp.appearance = NSAppearance(named: .darkAqua)
        case .auto:
            NSApp.appearance = nil // Follow system
        }
    }
}
```

**Why:**
- 用户有明确的主题偏好控制权
- 不依赖系统设置，提供独立选择
- 符合现代应用趋势（Slack、VSCode等都有类似设计）
- 切换器视觉精美，三个图标按钮形成统一组件

**Alternatives Considered:**
- ❌ 仅跟随系统：限制用户自主性，无法满足"白天用深色模式"等需求
- ❌ 在GeneralSettings中放下拉菜单：不够直观，需要多次点击
- ❌ 使用Toggle开关：只能支持两种模式，无法表达"跟随系统"

### Decision 6: Visual Effects (视觉效果)

**What:** 使用以下视觉技术提升现代感

1. **毛玻璃背景** (NSVisualEffectView wrapper)
```swift
struct VisualEffectBackground: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .sidebar
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {}
}
```

2. **卡片阴影和悬停效果**
```swift
.shadow(color: .black.opacity(0.1), radius: 4, x: 0, y: 2)
.onHover { isHovered in
    withAnimation(.easeInOut(duration: 0.2)) {
        hoverScale = isHovered ? 1.02 : 1.0
    }
}
```

3. **微妙的过渡动画**
```swift
.animation(.spring(response: 0.3, dampingFraction: 0.8), value: selectedProvider)
```

**Why:**
- 符合macOS原生应用视觉规范
- 提供触觉反馈，提升交互愉悦度
- 性能开销可接受（GPU加速）

**Alternatives Considered:**
- ❌ 不使用毛玻璃：显得过时，不够精致
- ❌ 过度动画：分散注意力，影响性能

## Risks / Trade-offs

### Risk 1: 性能下降
**风险:** 复杂的视觉效果可能降低渲染性能

**Mitigation:**
- 使用`LazyVStack`而非`VStack`延迟加载
- 限制同时显示的阴影效果数量
- 使用Instruments性能分析工具验证帧率
- 在低端Mac上测试（2020 Intel MacBook Air）

### Risk 2: 兼容性问题
**风险:** 新的SwiftUI API可能在macOS 13上不可用

**Mitigation:**
- 查阅SwiftUI API可用性文档
- 使用`@available(macOS 13, *)`条件编译
- 为旧系统提供降级UI方案
- 在macOS 13、14、15上完整测试

### Risk 3: 代码复杂度增加
**风险:** 组件拆分过细可能导致代码导航困难

**Mitigation:**
- 遵循单一职责原则，每个组件不超过200行
- 使用清晰的命名规范和注释
- 创建`ComponentsIndex.md`文档说明组件层级
- 使用Xcode的Symbol Navigator快速跳转

### Risk 4: 破坏现有功能
**风险:** UI重构可能意外改变业务逻辑

**Mitigation:**
- 先写测试，验证现有功能（Provider CRUD、配置保存）
- 逐个视图重构，每次重构后回归测试
- 保留旧视图文件作为备份（命名为`*View.legacy.swift`）
- 使用Git分支隔离开发，充分测试后再合并

## Migration Plan

### Phase 1: 设计系统搭建（2天）
1. 创建`DesignTokens.swift`
2. 创建基础原子组件（SearchBar, StatusIndicator, ActionButton）
3. 在`GeneralSettingsView`中试用新组件，验证可行性

### Phase 2: ProvidersView重构（3天）
1. 创建`ProviderCard.swift`组件
2. 创建`ProviderDetailPanel.swift`组件
3. 重构`ProvidersView.swift`使用新组件
4. 添加搜索和筛选功能
5. 测试Provider CRUD功能

### Phase 3: 其他视图迁移（3天）
1. 重构`RoutingView.swift`
2. 重构`ShortcutsView.swift`
3. 重构`BehaviorSettingsView.swift`
4. 重构`MemoryView.swift`

### Phase 4: 侧边栏重构（2天）
1. 创建`ModernSidebarView.swift`
2. 集成到`SettingsView.swift`
3. 添加侧边栏底部操作区（导入/导出/重置）

### Phase 5: 测试和优化（2天）
1. 完整回归测试（所有功能）
2. 性能测试（Instruments）
3. 多屏幕尺寸测试
4. 亮色/深色主题测试
5. Bug修复和视觉调整

### Rollback Plan
- 如果重构后发现重大问题，恢复`*.legacy.swift`备份文件
- 使用Git revert回滚到重构前commit
- 提供紧急补丁修复关键功能

## Open Questions

1. **是否支持自定义主题颜色？**
   - 当前计划：跟随系统亮色/深色模式
   - 可选扩展：允许用户自定义主题色（Phase 7+）

2. **是否需要添加键盘快捷键？**
   - 例如：Cmd+F 聚焦搜索框，Cmd+N 新建Provider
   - 建议：在本次实现基础快捷键，详细快捷键留待后续

3. **Detail Panel是否应该支持折叠？**
   - 当前计划：支持隐藏/显示（类似Xcode Inspector）
   - 实现方式：使用`NavigationSplitView`的`columnVisibility`

4. **是否需要添加导入/导出Provider配置？**
   - 当前计划：在侧边栏底部添加"导入/导出设置"按钮
   - 格式：JSON格式导出Provider配置和路由规则

## Dependencies

**Swift Packages:**
- 无需新增依赖，全部使用SwiftUI标准库

**macOS Version:**
- 最低支持：macOS 13 (Ventura)
- 推荐版本：macOS 14+ (Sonoma)

**Xcode Version:**
- Xcode 15+
- Swift 5.9+

## Success Criteria

### Functional Requirements
- ✅ 所有现有配置功能正常工作
- ✅ Provider CRUD操作无回退
- ✅ 配置文件加载/保存正确
- ✅ 搜索功能实时响应，无延迟

### Visual Requirements
- ✅ 界面风格与uisample.png一致
- ✅ 支持亮色/深色主题自动切换
- ✅ 卡片、阴影、圆角等视觉细节到位
- ✅ 在不同屏幕尺寸下布局合理

### Performance Requirements
- ✅ 渲染帧率稳定在60fps
- ✅ 搜索响应时间 < 50ms
- ✅ 窗口打开时间 < 500ms
- ✅ 内存占用无显著增加（< +10MB）

### UX Requirements
- ✅ 用户无需重新学习界面
- ✅ 所有功能可通过鼠标和键盘访问
- ✅ 错误状态有清晰提示
- ✅ 交互反馈及时（按钮点击、悬停效果）

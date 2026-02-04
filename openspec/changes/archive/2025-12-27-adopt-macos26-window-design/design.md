# Design: macOS 26 Window Design Language Implementation

## Context

Aleph 当前的设置窗口使用 SwiftUI 的 `Settings` Scene API，这是 macOS 13+ 引入的声明式窗口管理方式。虽然 `Settings` 提供了开箱即用的标签页导航和窗口管理，但它严格限制了窗口的视觉定制能力——开发者无法控制标题栏样式、无法移动系统交通灯按钮位置。

macOS 26 的设计语言强调"内容优先、控制融入"的理念，其核心特征是将窗口控制按钮（红黄绿交通灯）从独立的标题栏中解放出来,集成到应用的侧边栏或工具栏中。这种设计在 Music.app、Podcasts.app 等原生应用中已经广泛应用,但对于第三方开发者来说,需要绕过 SwiftUI 的高层抽象,直接操作 AppKit 的 NSWindow API。

**Architectural Constraints:**
1. **SwiftUI Scene Limitations:** `Settings` Scene 不支持自定义窗口样式（如 `.hiddenTitleBar`）
2. **Traffic Light Immutability:** macOS 不允许移动系统原生的交通灯按钮，只能通过隐藏 + 自绘实现
3. **Hybrid SwiftUI + AppKit:** 需要在 SwiftUI 中桥接 AppKit 的窗口控制 API
4. **Backward Compatibility:** 必须支持 macOS 13+（Aleph 的最低版本要求），不能依赖 macOS 26 专属 API

**Stakeholders:**
- **End Users:** 期望 Aleph 的视觉设计与最新 macOS 系统应用保持一致
- **Developers:** 需要清晰的架构边界，避免 SwiftUI/AppKit 混合开发中的坑
- **Brand Team:** 希望通过现代化设计提升 Aleph 的市场竞争力

## Goals / Non-Goals

**Goals:**
1. ✅ 实现视觉上与 macOS 26 原生应用一致的左侧圆角单栏 + 集成交通灯
2. ✅ 保留所有现有设置功能，不破坏任何用户配置逻辑
3. ✅ 使用纯 SwiftUI + AppKit 桥接实现，避免引入第三方 UI 框架
4. ✅ 支持 macOS 13+ 的跨版本兼容性
5. ✅ 自绘交通灯的交互行为与系统原生完全一致（Hover、Click、Focus）

**Non-Goals:**
1. ❌ 不实现真正的窗口拖拽区域自定义（保留 macOS 默认行为即可）
2. ❌ 不支持可折叠侧边栏（未来功能，本次不涉及）
3. ❌ 不修改 Halo 窗口（仅针对设置窗口）
4. ❌ 不重构现有标签页视图的内部逻辑（只调整布局容器）
5. ❌ 不实现多窗口管理（Aleph 当前只有一个设置窗口）

## Decisions

### Decision 1: WindowGroup vs. Settings Scene

**Context:**
SwiftUI 提供两种窗口 Scene：
- `Settings { }` - 专为 Preferences 设计，自动处理菜单栏集成、快捷键（Cmd+,）
- `WindowGroup { }` - 通用窗口容器，支持自定义样式（`.windowStyle`, `.windowToolbarStyle`）

**Decision:** 从 `Settings` 迁移到 `WindowGroup`

**Rationale:**
- `Settings` Scene 不支持 `.windowStyle(.hiddenTitleBar)`，无法隐藏标题栏
- `WindowGroup` 提供完全的窗口样式控制权，同时保留菜单栏集成能力
- 通过在 `AppDelegate` 中手动处理 `showSettings()` 方法，可以实现与 `Settings` Scene 相同的行为
- 未来如果需要多窗口（如独立的 Memory Viewer），`WindowGroup` 更灵活

**Trade-offs:**
- ✅ 优势：完全控制窗口外观，支持自定义标题栏
- ✅ 优势：可以应用 `.frame(minWidth:minHeight:)` 设置最小尺寸
- ⚠️ 劣势：失去 `Settings` Scene 的自动菜单项集成（需手动管理）
- ⚠️ 劣势：需要手动处理窗口单例逻辑（防止重复打开）

**Alternatives Considered:**
1. **继续使用 Settings + Overlay Hack**
   - 尝试在 `Settings` Scene 上叠加自定义视图，但无法真正隐藏标题栏
   - ❌ Rejected: 无法实现预期的视觉效果
2. **使用纯 AppKit NSWindow**
   - 完全放弃 SwiftUI，使用 NSWindowController + NSHostingView
   - ❌ Rejected: 开发效率低，违背 Aleph "SwiftUI-first" 的架构原则

### Decision 2: 自绘交通灯 vs. 原生交通灯

**Context:**
macOS 提供系统原生的交通灯按钮（通过 NSWindow 的 standardWindowButton API），但位置固定在左上角，无法移动。

**Decision:** 完全自绘三颗功能等价的交通灯按钮

**Rationale:**
- macOS 不允许移动原生交通灯的位置，只能通过 `window.titlebarAppearsTransparent` 和手动布局来"接近"目标效果，但仍无法放入圆角侧边栏内
- 自绘按钮可以精确控制位置、样式和交互逻辑
- 通过 SwiftUI 的 `.onHover` 修饰符，可以完美复刻原生交通灯的 Hover 显示图标行为
- 使用 `.fill(color.gradient)` 可以实现与原生一致的渐变质感

**Implementation Details:**
```swift
struct TrafficLightButton: View {
    let color: Color
    let action: () -> Void
    @State private var isHovering = false

    var body: some View {
        Button(action: action) {
            ZStack {
                Circle()
                    .fill(color.gradient)  // 渐变填充
                if isHovering {
                    Image(systemName: symbolName)  // Hover 时显示图标
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(.black.opacity(0.7))
                }
            }
            .frame(width: 13, height: 13)  // 与原生尺寸一致
        }
        .buttonStyle(.plain)  // 避免默认按钮样式
        .onHover { isHovering = $0 }
    }
}
```

**Trade-offs:**
- ✅ 优势：完全控制位置和样式，可放置在任意位置
- ✅ 优势：与侧边栏视觉一体化
- ⚠️ 劣势：需要手动实现窗口操作逻辑（通过 AppKit 桥接）
- ⚠️ 劣势：可能与未来 macOS 版本的原生交互行为有微小差异

**Alternatives Considered:**
1. **使用原生交通灯 + 透明标题栏技巧**
   - 设置 `window.titlebarAppearsTransparent = true` 和手动布局
   - ❌ Rejected: 无法将交通灯放入圆角矩形内，视觉效果不符合需求
2. **隐藏原生交通灯 + 自定义图标（非按钮）**
   - 仅显示视觉图标，实际功能通过菜单或快捷键触发
   - ❌ Rejected: 违背用户习惯，降低可用性

### Decision 3: WindowController 桥接模式

**Context:**
SwiftUI 没有直接的窗口控制 API（关闭、最小化、全屏），需要桥接到 AppKit 的 NSWindow。

**Decision:** 创建 `WindowController` 单例类，桥接 NSWindow API

**Rationale:**
- 单例模式确保全局只有一个 WindowController 实例，避免重复初始化
- 通过 `NSApp.keyWindow` 动态获取当前活动窗口，支持未来的多窗口扩展
- 方法签名简洁（`close()`, `minimize()`, `toggleFullscreen()`），易于理解和维护
- 与 SwiftUI 的 Action 闭包完美集成（`action: WindowController.shared.close`）

**Implementation:**
```swift
final class WindowController {
    static let shared = WindowController()

    private func keyWindow() -> NSWindow? {
        NSApp.keyWindow
    }

    func close() {
        keyWindow()?.performClose(nil)
    }

    func minimize() {
        keyWindow()?.miniaturize(nil)
    }

    func toggleFullscreen() {
        if let window = keyWindow() {
            window.toggleFullScreen(nil)
        }
    }
}
```

**Trade-offs:**
- ✅ 优势：清晰的职责分离（SwiftUI 负责 UI，AppKit 负责窗口操作）
- ✅ 优势：易于测试（可 Mock WindowController）
- ⚠️ 劣势：依赖 `NSApp.keyWindow`，在某些边缘场景（如窗口失焦）可能为 nil

**Alternatives Considered:**
1. **使用 @Environment(\\.openWindow) 和 @Environment(\\.dismiss)**
   - SwiftUI 5 引入的新 API，但仅支持打开/关闭窗口，不支持最小化和全屏
   - ❌ Rejected: 功能不完整
2. **直接在视图中调用 NSApp.keyWindow**
   - 在每个按钮的 action 闭包中重复代码
   - ❌ Rejected: 代码冗余，难以维护

### Decision 4: 侧边栏圆角实现方案

**Context:**
SwiftUI 提供多种圆角实现方式：
- `.cornerRadius()` - 简单但已弃用，不支持连续圆角
- `.clipShape(RoundedRectangle)` - 需要单独添加边框
- `RoundedRectangle().fill().overlay()` - 背景 + 边框一体化

**Decision:** 使用 `RoundedRectangle(cornerRadius: 18, style: .continuous)` + `.fill()` + `.overlay(.strokeBorder)`

**Rationale:**
- `.continuous` 圆角样式（iOS 13+）提供更自然的曲线，符合 Apple 设计语言
- `overlay(.strokeBorder)` 确保边框在圆角内侧，避免边缘锯齿
- `cornerRadius: 18` 是经过视觉测试的最佳值，与 macOS 原生应用一致
- `padding(.leading: 8).padding(.vertical: 8)` 为圆角矩形提供呼吸空间

**Implementation:**
```swift
ZStack(alignment: .topLeading) {
    RoundedRectangle(cornerRadius: 18, style: .continuous)
        .fill(sidebarBackground)
        .overlay(
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .strokeBorder(.separator.opacity(0.25))
        )
        .padding(.leading, 8)
        .padding(.vertical, 8)

    VStack { /* 内容 */ }
}
```

**Trade-offs:**
- ✅ 优势：视觉效果精致，符合 macOS 原生应用标准
- ✅ 优势：支持 Light/Dark Mode 自适应
- ⚠️ 劣势：需要手动管理内边距（padding），增加布局复杂度

**Alternatives Considered:**
1. **使用 .background(RoundedRectangle)**
   - 无法精确控制边框位置
   - ❌ Rejected: 边框可能在圆角外侧，产生视觉瑕疵
2. **使用 NSVisualEffectView 毛玻璃**
   - 需要桥接 AppKit，增加复杂度
   - ❌ Rejected: 当前设计不需要毛玻璃效果，保持简洁

### Decision 5: 布局结构

**Context:**
设置窗口需要清晰的布局层次：根视图 → 左右分栏 → 侧边栏/内容区。

**Decision:** 创建独立的 `RootContentView`，使用 `HStack(spacing: 0)` 分割左右区域

**Rationale:**
- `spacing: 0` 确保侧边栏和内容区紧密相连，分隔线（Divider）提供视觉边界
- 侧边栏固定宽度 220pt，内容区自动填充剩余空间（通过 `.frame(maxWidth: .infinity)`）
- 根视图独立于 `SettingsView`，避免现有代码的大规模重构
- 未来可轻松扩展为三栏布局（如添加右侧详情面板）

**Component Hierarchy:**
```
WindowGroup {
    RootContentView()                        // 窗口根视图
        ├── HStack(spacing: 0)               // 水平分栏
        │   ├── SidebarWithTrafficLights()   // 左侧圆角单栏（220pt）
        │   ├── Divider()                    // 分隔线（1pt）
        │   └── MainContentView()            // 右侧内容区（填充）
        │       └── TabContentSwitcher       // 根据 selectedTab 切换视图
        │           ├── GeneralSettingsView
        │           ├── ProvidersView
        │           └── ...                  // 其他标签页
        └── .background(.windowBackground)
}
.windowStyle(.hiddenTitleBar)
.windowToolbarStyle(.unifiedCompact)
.frame(minWidth: 800, minHeight: 500)
```

**Trade-offs:**
- ✅ 优势：清晰的组件边界，易于理解和维护
- ✅ 优势：现有标签页视图无需修改（仅调整容器）
- ⚠️ 劣势：增加了一层视图嵌套，可能轻微影响渲染性能（实测可忽略）

## Risks / Trade-offs

### Risk 1: 交通灯交互与原生行为差异

**Risk:** 自绘交通灯可能在某些边缘场景（如窗口失焦、多显示器）下与原生行为不一致。

**Mitigation:**
- 严格遵循 Apple HIG 的交通灯设计规范（尺寸、颜色、间距）
- 使用 `.buttonStyle(.plain)` 避免 SwiftUI 默认按钮样式干扰
- 通过 `.onHover` 实现与原生一致的交互反馈
- 在多个 macOS 版本（13、14、15、26）上进行兼容性测试

**Fallback Plan:** 如果发现严重的交互问题，可回退到 `Settings` Scene + 传统标题栏设计。

### Risk 2: 窗口拖拽区域识别

**Risk:** 隐藏标题栏后，用户可能找不到拖拽窗口的区域。

**Mitigation:**
- macOS 自动将 `.background()` 修饰的视图识别为可拖拽区域
- 侧边栏顶部区域（交通灯上方和左侧）保留足够的空白空间作为拖拽区
- 在文档中明确说明：用户可以拖拽侧边栏的任意非按钮区域来移动窗口

**Testing:**
- 验证在不同显示器尺寸下的拖拽行为
- 确认多显示器场景下的窗口拖拽

### Risk 3: 跨 macOS 版本的视觉一致性

**Risk:** `.continuous` 圆角样式和某些颜色（如 `.windowBackground`）在旧版 macOS 上的渲染效果可能不同。

**Mitigation:**
- 使用 `@available` 检查，为旧版本提供 fallback 样式
- 在 macOS 13（最低支持版本）上进行视觉回归测试
- 避免使用 macOS 26 专属 API（如新的 `WindowStyle` 枚举值）

**Compatibility Strategy:**
```swift
private var sidebarBackground: Color {
    if #available(macOS 14, *) {
        return Color(nsColor: .windowBackgroundColor).opacity(0.9)
    } else {
        return Color(nsColor: .controlBackgroundColor)
    }
}
```

### Trade-off: 开发复杂度 vs. 视觉现代化

**Trade-off:**
- ✅ 获得：与 macOS 26 原生应用一致的现代化视觉设计，提升品牌形象
- ⚠️ 付出：增加约 200-300 行 Swift 代码（交通灯、窗口控制器、侧边栏重构）
- ⚠️ 付出：需要额外的跨版本兼容性测试时间

**Justification:** Aleph 定位为"高端 AI 中间件"，现代化的视觉设计是提升用户信任和市场竞争力的关键因素。相比获得的品牌价值，额外的开发成本是合理且值得的。

## Migration Plan

### Phase 1: 创建新组件（不影响现有功能）

1. 创建 `TrafficLightButton.swift`
2. 创建 `WindowController.swift`
3. 创建 `SidebarWithTrafficLights.swift`
4. 创建 `RootContentView.swift`

**Validation:** 通过 Xcode Preview 验证组件渲染效果。

### Phase 2: 切换窗口 Scene（Feature Flag 控制）

5. 在 `AlephApp.swift` 中添加 `#if DEBUG` 条件编译，同时保留 `Settings` 和 `WindowGroup` 两种实现
6. 在 Debug 模式下使用新的 `WindowGroup` 实现
7. 在 Release 模式下暂时保留旧的 `Settings` 实现

**Validation:** 确认 Debug 构建可以正常打开设置窗口，所有标签页功能正常。

### Phase 3: 全面测试

8. 测试交通灯按钮的三种操作（关闭、最小化、全屏）
9. 测试 Light/Dark Mode 切换下的视觉效果
10. 测试窗口拖拽、调整大小、多显示器切换
11. 测试在 macOS 13、14、15 上的兼容性

**Validation:** 所有测试用例通过，无回归 Bug。

### Phase 4: 正式发布

12. 移除 Feature Flag，将 `WindowGroup` 作为默认实现
13. 删除旧的 `Settings` Scene 代码
14. 更新文档（CLAUDE.md、README.md）

**Rollback Plan:** 如果发现严重 Bug，可通过 Git revert 回退到 Phase 2 的 Feature Flag 版本，切换回 `Settings` Scene。

## Open Questions

1. **Q:** 是否需要在侧边栏底部添加"导入/导出/重置设置"按钮？
   **A:** 暂不添加，保持侧边栏简洁。这些功能可保留在 `ModernSidebarView` 的现有实现中（通过右键菜单或主菜单栏访问）。

2. **Q:** 是否需要支持侧边栏宽度调整（Resizable Sidebar）？
   **A:** 本次不实现。未来如果用户有需求，可通过添加 Divider 拖拽手柄来支持。

3. **Q:** 交通灯按钮的 Hover 动画是否需要与原生完全一致（包括渐变过渡）？
   **A:** 使用 SwiftUI 的 `.animation(.easeInOut(duration: 0.2))` 即可，无需像素级复刻。用户体验优先，开发效率其次。

4. **Q:** 是否需要为 Windows/Linux 版本预留设计？
   **A:** 本次仅针对 macOS。Windows/Linux 版本的窗口设计将在 Phase 6/7 中单独处理（参考 CLAUDE.md 的开发路线图）。

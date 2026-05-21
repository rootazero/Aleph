# macOS 26 Window Design Architecture

**Status**: ✅ Implemented (Phase 1, 2 & 3 Complete)

Aleph 采用 macOS 26 最新设计语言，包括 Liquid Glass 效果、自定义交通灯按钮，以及将系统控制按钮集成到左侧圆角侧边栏中，实现现代化、沉浸式的窗口体验。

## Design Philosophy

**macOS 26 设计语言特征:**
- **Content-First**: 隐藏原生标题栏，内容紧贴窗口上沿
- **Unified Controls**: 交通灯按钮融入侧边栏，而非独立的标题栏
- **Continuous Curves**: 使用 18pt continuous 圆角（Apple 标准）
- **Adaptive Materials**: 自适应 Dark/Light Mode 背景材质
- **Liquid Glass**: 原生玻璃效果，保持前后台一致外观

---

## Liquid Glass Implementation

### Overview

macOS 26 引入了 Liquid Glass 设计语言，Aleph 通过 `.glassEffect()` API 实现原生玻璃效果，并使用环境变量覆盖确保窗口在前台/后台切换时保持一致的视觉外观。

### Core Implementation

```swift
// AdaptiveGlassModifier.swift
func body(content: Content) -> some View {
    if #available(macOS 26, *) {
        content
            .glassEffect(.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .environment(\.controlActiveState, .active)  // 保持一致外观
    } else {
        // Fallback: NSVisualEffectView
        content
            .background(VisualEffectBackground(material: .hudWindow, blendingMode: .behindWindow))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}
```

### Design Decision: Consistent Appearance

**问题**: 默认情况下，Liquid Glass 在窗口失去焦点时会自动变淡（系统行为），导致视觉跳跃。

**解决方案**: 使用 `.environment(\.controlActiveState, .active)` 强制保持一致的玻璃外观。

**为什么选择 `.active` 而非 `.key`**:

| 值 | 含义 | 适用场景 |
|---|------|---------|
| `.active` | 窗口属于前台应用 | 通用活跃状态（推荐） |
| `.key` | 窗口接收键盘输入 | 多窗口场景区分焦点 |
| `.inactive` | 窗口完全非活跃 | 后台状态 |

**最终效果**: 稍微淡化的玻璃效果，提供更好的文字可读性，同时保持前后台视觉一致性。

### Glass Effect Components

| 组件 | API | 用途 |
|------|-----|------|
| `GlassModifier` | `.glassEffect(.clear)` | 通用玻璃背景（轻量边框） |
| `GlassProminentButtonModifier` | `.glassEffect(.clear.interactive())` | 主要按钮 |
| `GlassButtonModifier` | `.glassEffect(.clear)` | 次要按钮 |
| `AdaptiveGlassContainer` | `GlassEffectContainer` | 玻璃元素分组/融合 |
| `GlassMessageBubbleModifier` | `.glassEffect(.clear)` | 消息气泡 |

### Usage

```swift
// 应用玻璃效果
VStack {
    Text("Content")
}
.adaptiveGlass()  // 自动适配 macOS 26+ / 旧版本

// 玻璃按钮
Button("Action") {}
    .adaptiveGlassButton()

// 玻璃容器（支持元素融合）
AdaptiveGlassContainer(spacing: 8) {
    // 子元素会自动共享玻璃上下文
}
```

### File Location

`Aleph/Sources/Components/Atoms/AdaptiveGlassModifier.swift`

---

## Window Architecture

### From Settings Scene to WindowGroup

**旧设计（已弃用）:**
```swift
Settings {
    SettingsView()
}
```

**新设计（macOS 26 风格）:**
```swift
WindowGroup {
    RootContentView(core: appDelegate.core, keychainManager: appDelegate.keychainManager)
        .frame(minWidth: 800, minHeight: 500)
}
.windowStyle(.hiddenTitleBar)          // 隐藏原生标题栏
.windowToolbarStyle(.unifiedCompact)   // 内容紧贴窗口上沿
.defaultSize(width: 1200, height: 800)
```

**关键优势:**
- 完全控制窗口顶部区域的视觉设计
- 为自绘交通灯提供空间
- 实现与原生应用一致的沉浸式体验

---

## Component Architecture

### 1. TrafficLightButton Component

自定义交通灯按钮，完美复刻 macOS 原生外观：

```swift
struct TrafficLightButton: View {
    let color: Color  // .red, .yellow, .green
    let action: () -> Void
    @State private var isHovering = false

    var body: some View {
        Button(action: action) {
            ZStack {
                Circle()
                    .fill(color.gradient)  // 渐变填充
                    .frame(width: 13, height: 13)

                if isHovering {
                    Image(systemName: symbolName)  // xmark, minus, arrow
                        .font(.system(size: 7, weight: .bold))
                }
            }
        }
        .buttonStyle(.plain)
        .onHover { isHovering = $0 }
    }
}
```

**规格:**
- 直径: 13pt（与原生一致）
- 间距: 8pt（按钮之间）
- 位置: top 14pt, leading 18pt（相对侧边栏）
- Hover 状态: 显示操作图标（✕、−、全屏箭头）

**文件位置:** `Aleph/Sources/Components/Window/TrafficLightButton.swift`

---

### 2. WindowController Bridge

AppKit 窗口控制桥接，连接 SwiftUI 和 NSWindow API：

```swift
final class WindowController {
    static let shared = WindowController()

    func close() {
        NSApp.keyWindow?.performClose(nil)
    }

    func minimize() {
        NSApp.keyWindow?.miniaturize(nil)
    }

    func toggleFullscreen() {
        NSApp.keyWindow?.toggleFullScreen(nil)
    }
}
```

**设计模式:**
- 单例模式确保全局唯一
- 动态获取 keyWindow（支持多窗口扩展）
- 优雅处理 nil window（Debug 日志）

**文件位置:** `Aleph/Sources/Components/Window/WindowController.swift`

---

### 3. WindowConfigurator (Native Traffic Lights Hiding)

AppKit 窗口配置器，隐藏原生交通灯按钮：

```swift
struct WindowConfigurator: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            self.configureWindow(window)
        }
        return view
    }

    private func configureWindow(_ window: NSWindow) {
        // Make titlebar transparent
        window.titlebarAppearsTransparent = true

        // Hide standard window buttons (native traffic lights)
        window.standardWindowButton(.closeButton)?.isHidden = true
        window.standardWindowButton(.miniaturizeButton)?.isHidden = true
        window.standardWindowButton(.zoomButton)?.isHidden = true

        // Hide titlebar but keep window resizable
        window.titleVisibility = .hidden

        // Allow dragging from content area
        window.isMovableByWindowBackground = true
    }
}
```

**关键功能:**
- `titlebarAppearsTransparent`: 标题栏透明化
- `standardWindowButton().isHidden`: 隐藏原生红绿灯
- `titleVisibility = .hidden`: 隐藏标题文字
- `isMovableByWindowBackground`: 允许从内容区域拖动窗口

**使用方式:**
```swift
RootContentView()
    .hideNativeTrafficLights()  // 应用修饰符
```

**重要性:**
- `.windowStyle(.hiddenTitleBar)` 只隐藏标题文字，不隐藏交通灯
- 必须通过 AppKit API 显式隐藏原生交通灯
- 否则会同时显示原生和自定义两套交通灯

**文件位置:** `Aleph/Sources/Components/Window/WindowConfigurator.swift`

---

### 4. SidebarWithTrafficLights Component

圆角侧边栏，集成交通灯和导航：

```swift
struct SidebarWithTrafficLights: View {
    @Binding var selectedTab: SettingsTab
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        ZStack(alignment: .topLeading) {
            // 圆角背景
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .fill(sidebarBackground)
                .strokeBorder(.separator.opacity(0.25))
                .padding(.leading: 8, .vertical: 8)

            VStack(alignment: .leading, spacing: 12) {
                // 交通灯按钮
                HStack(spacing: 8) {
                    TrafficLightButton(color: .red, action: WindowController.shared.close)
                    TrafficLightButton(color: .yellow, action: WindowController.shared.minimize)
                    TrafficLightButton(color: .green, action: WindowController.shared.toggleFullscreen)
                }
                .padding(.top: 14, .leading: 18)

                // 导航项目...
            }
        }
        .frame(width: 220)
    }
}
```

**视觉规格:**
- 宽度: 220pt（固定）
- 圆角: 18pt continuous（Apple 标准）
- 边框: .separator.opacity(0.25)
- 背景:
  - Dark Mode: `windowBackgroundColor.opacity(0.9)`
  - Light Mode: `underPageBackgroundColor`

**文件位置:** `Aleph/Sources/Components/Window/SidebarWithTrafficLights.swift`

---

### 5. RootContentView Component

窗口根布局，两栏设计：

```swift
struct RootContentView: View {
    let core: AlephCore?
    let keychainManager: KeychainManagerImpl
    @State private var selectedTab: SettingsTab = .general

    var body: some View {
        HStack(spacing: 0) {
            SidebarWithTrafficLights(selectedTab: $selectedTab)  // 220pt
            Divider()                                             // 1pt
            ContentArea(selectedTab: selectedTab)                 // 填充剩余空间
        }
        .background(.windowBackground)
    }
}
```

**布局比例:**
- 左侧: 220pt 圆角侧边栏
- 分隔线: 1pt Divider
- 右侧: 内容区（.frame(maxWidth: .infinity)）

**文件位置:** `Aleph/Sources/Components/Window/RootContentView.swift`

---

## Migration from Settings Scene

### Why Migrate from Settings to WindowGroup?

| 特性 | Settings Scene | WindowGroup |
|------|---------------|-------------|
| 窗口样式控制 | ❌ 不支持自定义 | ✅ 完全控制 |
| 标题栏隐藏 | ❌ 无法隐藏 | ✅ `.hiddenTitleBar` |
| 交通灯位置 | ❌ 固定左上角 | ✅ 可通过自绘实现 |
| 最小尺寸设置 | ⚠️ 有限支持 | ✅ `.frame(minWidth:minHeight:)` |
| 菜单栏集成 | ✅ 自动处理 | ⚠️ 需手动管理 |

### Migration Strategy

- 使用 `#if DEBUG` Feature Flag 控制
- Debug 构建: 使用新 WindowGroup（测试）
- Release 构建: 保留旧 Settings（稳定回退）
- 测试通过后移除 Flag

---

## Window Activation Logic

**AppDelegate.showSettings() 实现:**

```swift
@objc private func showSettings() {
    #if DEBUG
    // 新设计: 查找或激活 WindowGroup 窗口
    if let window = NSApp.windows.first(where: { $0.isVisible }) {
        window.makeKeyAndOrderFront(nil)
    } else {
        NSApp.activate(ignoringOtherApps: true)  // SwiftUI 自动创建窗口
    }
    #else
    // 旧设计: 手动创建 NSWindow
    let window = NSWindow(contentViewController: NSHostingController(rootView: SettingsView()))
    window.makeKeyAndOrderFront(nil)
    #endif
}
```

---

## Technical Constraints

### macOS 限制

1. **交通灯不可移动**: 系统不允许移动原生交通灯位置，只能通过隐藏 + 自绘实现
2. **标题栏透明技巧**: `.hiddenTitleBar` 仅隐藏视觉，拖拽区域仍保留
3. **Focus 管理**: 自绘交通灯不触发窗口激活（使用 `.buttonStyle(.plain)`）

### 兼容性

- 最低支持: macOS 13 (Ventura)
- 最佳体验: macOS 26+ (Tahoe) - 完整 Liquid Glass 支持
- macOS 14/15: NSVisualEffectView fallback
- `.continuous` 圆角在所有版本正确渲染

---

## Event Flow Example

```
用户点击菜单栏"Settings..."
    ↓
AppDelegate.showSettings() 调用
    ↓
查找现有 WindowGroup 窗口
    ↓ (如果存在)
window.makeKeyAndOrderFront(nil) - 激活现有窗口
    ↓ (如果不存在)
NSApp.activate() - SwiftUI 自动创建 WindowGroup
    ↓
RootContentView 渲染
    ↓
SidebarWithTrafficLights 显示（含交通灯）
    ↓
用户点击红色交通灯
    ↓
WindowController.shared.close()
    ↓
NSApp.keyWindow?.performClose(nil)
    ↓
窗口关闭
```

---

## Files Changed

### 新增组件 (5 个文件)

- `Aleph/Sources/Components/Window/TrafficLightButton.swift` - 自定义交通灯按钮
- `Aleph/Sources/Components/Window/WindowController.swift` - AppKit 桥接
- `Aleph/Sources/Components/Window/WindowConfigurator.swift` - 隐藏原生交通灯
- `Aleph/Sources/Components/Window/SidebarWithTrafficLights.swift` - 圆角侧边栏
- `Aleph/Sources/Components/Window/RootContentView.swift` - 根布局容器

### 修改文件 (3 个文件)

- `Aleph/Sources/AlephApp.swift` - 添加 WindowGroup（Feature Flag 控制）
- `Aleph/Sources/AppDelegate.swift` - 暴露 core/keychainManager，更新 showSettings()
- `Aleph/Sources/Components/Atoms/AdaptiveGlassModifier.swift` - Liquid Glass 实现

### 代码统计

- 新增代码: ~525 行（含 WindowConfigurator）
- Liquid Glass: ~280 行（含 fallback 和 Preview）
- Preview 数量: 8 个
- 文档注释: 完整的 MARK 和 DocC 注释

---

## Testing Checklist

### 视觉验证

- [ ] 交通灯尺寸和位置符合规范（13pt, top:14pt, leading:18pt）
- [ ] 圆角半径正确（18pt continuous）
- [ ] Dark/Light Mode 背景色正确切换
- [ ] Hover 状态显示图标

### 功能验证

- [ ] 红色按钮关闭窗口
- [ ] 黄色按钮最小化到 Dock
- [ ] 绿色按钮切换全屏
- [ ] 窗口调整大小（最小 800x500）
- [ ] 所有设置标签页正常工作

### 跨版本验证

- [ ] macOS 13 (Ventura) 兼容性
- [ ] macOS 14 (Sonoma) 兼容性
- [ ] macOS 15 (Sequoia) 兼容性
- [ ] macOS 26 (Tahoe) Liquid Glass 兼容性

### Liquid Glass 验证

- [ ] Liquid Glass 效果正确显示（macOS 26+）
- [ ] 窗口失焦时玻璃效果保持一致（不变淡/变暗）
- [ ] 文字在玻璃背景上清晰可读
- [ ] NSVisualEffectView fallback 在旧版本正常工作
- [ ] GlassEffectContainer 元素正确融合

---

## Related Documentation

- See `docs/ui-design-guide.md` for general UI design principles
- See `docs/visual-testing-guide.md` for visual testing procedures
- See `Aleph/Sources/Components/Window/` for implementation code

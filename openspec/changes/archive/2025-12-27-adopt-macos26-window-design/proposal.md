# Change: Adopt macOS 26 Window Design Language with Sidebar Traffic Lights

## Why

macOS 26 引入了全新的窗口设计语言，其标志性特征是将系统控制按钮（红黄绿"交通灯"）集成到左侧圆角侧边栏中，打破了传统标题栏的设计范式。这种设计语言体现了 Apple 对于现代化应用界面的最新思考——更加沉浸、统一和简洁。

**Current Limitations:**
- Aleph 当前的设置窗口使用传统的 `Settings` Scene API，窗口样式为默认标题栏
- 交通灯按钮固定在窗口左上角的系统标题栏内，与侧边栏分离
- 无法实现与最新 macOS 系统应用（如 Music、Podcasts）一致的视觉语言
- 侧边栏与窗口顶部存在视觉断层，缺乏一体化设计感

**Opportunity:**
- 遵循 macOS 26 最新设计规范，提升 Aleph 的现代感和专业性
- 通过左侧圆角单栏 + 自绘交通灯实现"伪原生"效果，视觉上更加统一
- 为未来的 UI 扩展（如可折叠侧边栏、紧凑模式）奠定基础
- 增强品牌识别度，与竞品（如 Raycast、Arc Browser）拉开差距

**Technical Context:**
- macOS 并不允许开发者真正移动系统自带的交通灯按钮位置
- 必须通过 `windowStyle(.hiddenTitleBar)` 隐藏原生标题栏
- 然后自绘三颗功能等价的交通灯按钮，放置在左侧圆角单栏内部
- 通过 AppKit 桥接（`NSWindow` API）实现关闭、最小化、全屏等窗口操作

## What Changes

### 1. Window Style Migration

**从 Settings Scene 迁移到 WindowGroup Scene:**
- 当前：`Settings { SettingsView() }` - 使用系统默认窗口样式
- 新设计：`WindowGroup { RootContentView() }` - 使用自定义窗口样式
- 应用 `.windowStyle(.hiddenTitleBar)` 隐藏原生标题栏
- 应用 `.windowToolbarStyle(.unifiedCompact)` 使内容紧贴窗口上沿

**Benefits:**
- 完全控制窗口顶部区域的视觉设计
- 为自绘交通灯提供空间
- 实现与原生应用一致的沉浸式体验

### 2. Custom Traffic Light Buttons

**创建 `TrafficLightButton` 组件:**
- 圆形按钮，直径 13pt（与原生尺寸一致）
- 三种颜色：`.red`（关闭）、`.yellow`（最小化）、`.green`（全屏切换）
- Hover 状态显示操作图标：
  - 红色：`xmark`（✕）
  - 黄色：`minus`（−）
  - 绿色：`arrow.up.left.and.arrow.down.right`（全屏切换箭头）
- 使用渐变填充（`.fill(color.gradient)`）增强视觉质感

**创建 `WindowController` 单例:**
- 桥接 AppKit `NSWindow` API
- 提供三个方法：
  - `close()` - 调用 `window.performClose(nil)`
  - `minimize()` - 调用 `window.miniaturize(nil)`
  - `toggleFullscreen()` - 调用 `window.toggleFullScreen(nil)`
- 通过 `NSApp.keyWindow` 获取当前活动窗口

### 3. Sidebar Redesign with Rounded Corners

**创建 `SidebarWithTrafficLights` 组件:**
- 左侧固定宽度：220pt
- 使用 `RoundedRectangle(cornerRadius: 18, style: .continuous)` 实现大圆角背景
- 顶部放置三颗交通灯按钮：
  - 位置：距顶部 14pt，距左侧 18pt
  - 按钮间距：8pt
- 背景材质：
  - Dark Mode：`windowBackgroundColor.opacity(0.9)`
  - Light Mode：`underPageBackgroundColor`
- 边框：使用 `.strokeBorder(.separator.opacity(0.25))` 增加细腻的边界感

**侧边栏内容布局:**
- 顶部：交通灯按钮区域（占 14pt + 13pt + 部分间距）
- 中间：导航项目列表（使用 `SidebarItem` 组件）
- 底部：`Spacer()` 占位（为未来的底部操作区预留空间）

### 4. Root Layout Structure

**创建 `RootContentView` 作为窗口根视图:**
```swift
HStack(spacing: 0) {
    SidebarWithTrafficLights()  // 左侧圆角单栏（220pt）
    Divider()                   // 分隔线
    MainContentView()           // 右侧内容区域（填充剩余空间）
}
.background(.windowBackground)
```

**Benefits:**
- 清晰的左右分栏结构
- 左侧专注于导航 + 窗口控制
- 右侧专注于内容展示
- 分隔线提供视觉边界

### 5. Integration with Existing Settings Tabs

**MainContentView 负责渲染标签页内容:**
- 根据侧边栏选中的 `SettingsTab` 枚举值切换视图
- 支持所有现有标签页：General, Providers, Routing, Shortcuts, Behavior, Memory
- 保持现有的 `SettingsView` 逻辑不变（配置加载、保存、热重载）
- 内容区域可独立滚动，与侧边栏解耦

**Tab Navigation Logic:**
```swift
@Binding var selectedTab: SettingsTab  // 传递给侧边栏和内容区

switch selectedTab {
case .general:   GeneralSettingsView()
case .providers: ProvidersView()
// ... 其他标签页
}
```

### 6. Window Dimensions and Behavior

**窗口尺寸:**
- 最小尺寸：`800x500` pt（确保内容不会被压缩）
- 默认尺寸：`1200x800` pt（与现有设置窗口保持一致）
- 支持用户调整大小（`resizable: true`）

**窗口定位:**
- 首次打开居中显示
- 后续打开恢复上次位置（macOS 自动行为）

**Focus & Activation:**
- 点击菜单栏"Settings..."时激活窗口
- 交通灯按钮不触发窗口激活（使用 `.buttonStyle(.plain)` 避免焦点变化）

## Impact

**Affected Specs:**
- `macos-client` - 添加窗口样式和自定义交通灯的需求
- `settings-ui-layout` - 更新布局比例和侧边栏结构要求

**Affected Code:**
- `Aleph/Sources/AlephApp.swift` - 从 `Settings` 迁移到 `WindowGroup`
- `Aleph/Sources/SettingsView.swift` - 拆分为 `RootContentView` + 侧边栏 + 内容区
- `Aleph/Sources/AppDelegate.swift` - 更新 `showSettings()` 方法以适配新窗口
- New files:
  - `Aleph/Sources/Components/Window/TrafficLightButton.swift`
  - `Aleph/Sources/Components/Window/WindowController.swift`
  - `Aleph/Sources/Components/Window/SidebarWithTrafficLights.swift`
  - `Aleph/Sources/Components/Window/RootContentView.swift`

**Breaking Changes:**
- **NONE** - 纯视觉层改造，所有功能逻辑保持不变
- 用户配置文件格式不变
- UniFFI 接口不受影响
- 现有标签页视图无需修改（仅调整布局容器）

**Migration Plan:**
1. 创建新组件（交通灯、侧边栏、根视图）
2. 在 `AlephApp.swift` 中保留 `Settings` Scene 作为 fallback（可通过 feature flag 切换）
3. 测试新窗口在各种场景下的行为（最小化、全屏、多显示器）
4. 确认交通灯按钮在 Light/Dark Mode 下的视觉效果
5. 移除旧的 `Settings` Scene 代码

**Risks & Mitigations:**
- **风险：** 自绘交通灯可能与系统原生行为不完全一致
  - **缓解：** 严格遵循 macOS 交通灯的视觉规范和交互逻辑
  - **测试：** 在多个 macOS 版本（13-26）上验证兼容性
- **风险：** 用户可能习惯了传统标题栏的拖拽区域
  - **缓解：** 保留侧边栏顶部区域作为可拖拽区域（`.background()` 自动支持）
- **风险：** 窗口焦点管理可能出现问题
  - **缓解：** 使用 `NSApp.keyWindow` 而非硬编码窗口引用
  - **测试：** 验证多窗口场景（虽然 Aleph 目前只有一个设置窗口）

**Benefits:**
- 视觉现代化，符合 macOS 26 设计语言
- 提升品牌专业性和用户信任度
- 为未来的 UI 创新提供更灵活的基础架构
- 学习和实践 SwiftUI + AppKit 混合开发模式

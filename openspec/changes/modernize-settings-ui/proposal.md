# Change: Modernize Settings UI with Contemporary macOS Design

## Why

当前的Aether设置界面使用了基础的SwiftUI组件，虽然功能完整，但视觉呈现相对传统，缺乏现代化macOS应用的精致感和专业性。参考uisample.png中展示的现代化UI设计，我们需要对整个设置界面进行视觉升级，以提供更优秀的用户体验。

**Current Problems:**
- 侧边栏使用默认样式，缺乏视觉层次感
- 内容区域布局平铺，没有卡片化的现代设计语言
- 缺少搜索、筛选等高级交互功能
- 颜色和间距未优化，视觉密度不够平衡
- 缺少细节面板，信息展示不够直观

**Opportunity:**
- 提升品牌形象，展现Aether作为高端AI中间件的专业性
- 改善用户体验，使配置流程更直观、高效
- 遵循最新macOS设计规范，与系统原生应用保持一致性

## What Changes

### 1. Visual Design System
- 引入三种主题模式：白天（Light）、夜晚（Dark）、跟随系统（Auto）
- 在设置窗口右上角添加精美的主题切换器（图标按钮组）
- 实现毛玻璃背景效果（NSVisualEffectView集成）
- 统一圆角、阴影、间距等视觉参数
- 优化字体层级和颜色对比度，确保三种模式下都有良好的可读性

### 2. Sidebar Redesign
- 重新设计侧边栏，使用图标+文字组合
- 添加选中状态的视觉反馈（背景高亮、图标颜色变化）
- 优化图标设计，使用SF Symbols 5最新图标集
- 添加侧边栏底部操作区（导入/导出/重置设置）

### 3. Provider List Modernization
- 将Provider列表改为卡片化设计
- 每个Provider卡片显示：图标、名称、状态指示器、简要描述
- 添加搜索栏，支持实时过滤Provider
- 实现选中Provider后右侧显示详细信息面板
- 添加快速操作按钮（测试连接、复制配置等）

### 4. Enhanced Interactions
- 添加搜索功能，支持跨所有Provider搜索
- 实现拖拽排序Provider优先级
- 添加快捷操作（右键菜单、键盘快捷键）
- 优化按钮样式（主要/次要按钮区分明确）
- 添加状态指示器（在线/离线、配置完整性检查）

### 5. Detail Panel
- 右侧添加详细信息面板，显示选中Provider的完整配置
- 包含：API端点URL、模型选择、使用示例代码
- 添加"Use with Claude Code"集成指引
- 显示环境变量配置说明

### 6. Layout & Spacing
- 重新调整所有视图的padding和spacing
- 优化内容区域宽度比例（侧边栏:内容:详情 = 1:2:1.5）
- 使用弹性布局，支持窗口调整大小时自适应

### Affected Components
- `SettingsView.swift` - 主设置视图框架
- `ProvidersView.swift` - Provider管理视图（重点改造）
- `RoutingView.swift` - 路由规则视图
- `ShortcutsView.swift` - 快捷键配置视图
- `BehaviorSettingsView.swift` - 行为设置视图
- `GeneralSettingsView.swift` - 通用设置视图
- `MemoryView.swift` - 内存管理视图

### New Components
- `ModernSidebarView.swift` - 现代化侧边栏组件
- `ProviderCardView.swift` - Provider卡片组件
- `DetailPanelView.swift` - 详细信息面板组件
- `SearchBarView.swift` - 搜索栏组件
- `ThemeSwitcher.swift` - 主题切换器组件（白天/夜晚/跟随系统）
- `DesignTokens.swift` - 设计规范常量（颜色、间距、圆角等）
- `ThemeManager.swift` - 主题管理器（持久化用户选择）

## Impact

**Affected Specs:**
- `macos-client` - Settings window UI implementation
- New capability needed: `settings-ui-design-system`

**Affected Code:**
- `Aether/Sources/SettingsView.swift`
- `Aether/Sources/ProvidersView.swift`
- `Aether/Sources/RoutingView.swift`
- `Aether/Sources/ShortcutsView.swift`
- `Aether/Sources/BehaviorSettingsView.swift`
- `Aether/Sources/GeneralSettingsView.swift`
- `Aether/Sources/MemoryView.swift`
- New files in `Aether/Sources/Components/` directory

**Breaking Changes:**
- **NONE** - This is a pure UI refactor with no API or behavior changes
- All existing functionality remains intact
- Configuration loading/saving logic unchanged
- UniFFI bindings not affected

**Benefits:**
- 提升用户体验和应用专业度
- 降低配置学习成本（更直观的界面）
- 为未来功能扩展提供更好的UI基础
- 符合macOS Human Interface Guidelines最新规范

**Risks:**
- 开发工作量较大，涉及多个视图文件重构
- 需要仔细测试以确保现有功能不受影响
- 需要在多种屏幕尺寸和macOS版本上测试兼容性

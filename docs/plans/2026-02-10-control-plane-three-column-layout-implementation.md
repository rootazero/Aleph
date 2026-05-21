# Control Plane 三栏布局架构重构 - 实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 Control Plane 的三栏布局架构，解决导航上下文丢失问题，建立持久化的设置导航体验。

**Architecture:** 采用嵌套路由 + Layout 模式，通过 SettingsLayout 管理第二栏（SettingsSidebar）和第三栏（内容区）。主 Sidebar 根据路由自动切换窄/宽模式，并支持状态总线订阅系统告警。

**Tech Stack:** Leptos (WASM UI), leptos_router (嵌套路由), Tailwind CSS (样式), Rust (类型系统)

**Design Document:** `docs/plans/2026-02-10-control-plane-three-column-layout-design.md`

---

## Phase 1: 基础架构

### Task 1: 创建 Sidebar 类型定义

**Files:**
- Create: `core/ui/control_plane/src/components/sidebar/types.rs`
- Create: `core/ui/control_plane/src/components/sidebar/mod.rs`

**Step 1: 创建 sidebar 目录**

```bash
mkdir -p core/ui/control_plane/src/components/sidebar
```

**Step 2: 创建 types.rs 定义核心类型**

```rust
// core/ui/control_plane/src/components/sidebar/types.rs
use std::collections::HashMap;

/// Sidebar 显示模式
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SidebarMode {
    /// 宽模式 (w-64) - 显示图标 + 文字
    Wide,
    /// 窄模式 (w-16) - 仅显示图标
    Narrow,
}

/// 告警级别
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlertLevel {
    /// 无告警
    None,
    /// 信息提示（蓝色徽章）
    Info,
    /// 警告（黄色徽章）
    Warning,
    /// 严重错误（红色徽章 + 呼吸动画）
    Critical,
}

/// 系统告警
#[derive(Clone, Debug)]
pub struct SystemAlert {
    /// 告警 key（如 "system.health"）
    pub key: &'static str,
    /// 告警级别
    pub level: AlertLevel,
    /// 可选的数字徽章（如 "3 个错误"）
    pub count: Option<u32>,
    /// Tooltip 中显示的详细信息
    pub message: Option<String>,
}
```

**Step 3: 创建 mod.rs 导出类型**

```rust
// core/ui/control_plane/src/components/sidebar/mod.rs
pub mod types;

pub use types::{SidebarMode, AlertLevel, SystemAlert};
```

**Step 4: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功，无错误

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/components/sidebar/
git commit -m "feat(control-plane): add sidebar types (SidebarMode, AlertLevel, SystemAlert)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 2: 更新 DashboardState 添加状态总线

**Files:**
- Modify: `core/ui/control_plane/src/context.rs`

**Step 1: 读取当前 context.rs**

Run: `cat core/ui/control_plane/src/context.rs`

**Step 2: 在 DashboardState 中添加状态总线字段**

在 `DashboardState` 结构体中添加：

```rust
use std::collections::HashMap;
use crate::components::sidebar::{SidebarMode, SystemAlert};

#[derive(Clone)]
pub struct DashboardState {
    // ... 现有字段

    /// 系统告警状态总线
    pub alerts: RwSignal<HashMap<&'static str, SystemAlert>>,

    /// Sidebar 模式覆盖（用户手动设置）
    pub sidebar_mode_override: RwSignal<Option<SidebarMode>>,
}
```

**Step 3: 在 DashboardState 的 new() 或 default() 中初始化**

```rust
impl DashboardState {
    pub fn new(/* ... */) -> Self {
        Self {
            // ... 现有字段初始化
            alerts: RwSignal::new(HashMap::new()),
            sidebar_mode_override: RwSignal::new(None),
        }
    }

    /// 更新告警状态
    pub fn update_alert(&self, key: &'static str, alert: SystemAlert) {
        self.alerts.update(|map| {
            map.insert(key, alert);
        });
    }

    /// 获取告警状态
    pub fn get_alert(&self, key: &'static str) -> Option<SystemAlert> {
        self.alerts.with(|map| map.get(key).cloned())
    }

    /// 清除告警状态
    pub fn clear_alert(&self, key: &'static str) {
        self.alerts.update(|map| {
            map.remove(key);
        });
    }
}
```

**Step 4: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/context.rs
git commit -m "feat(control-plane): add alert bus and sidebar mode override to DashboardState

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 3: 创建 SettingsLayout 组件

**Files:**
- Create: `core/ui/control_plane/src/components/layouts/settings_layout.rs`
- Create: `core/ui/control_plane/src/components/layouts/mod.rs`

**Step 1: 创建 layouts 目录**

```bash
mkdir -p core/ui/control_plane/src/components/layouts
```

**Step 2: 创建 settings_layout.rs**

```rust
// core/ui/control_plane/src/components/layouts/settings_layout.rs
use leptos::prelude::*;
use leptos_router::components::Outlet;
use crate::components::SettingsSidebar;

/// Settings 域的布局容器
///
/// 管理第二栏（SettingsSidebar）和第三栏（内容区）的渲染。
/// 通过 Outlet 渲染子路由内容。
#[component]
pub fn SettingsLayout() -> impl IntoView {
    view! {
        <div class="flex h-full">
            // 第二栏：Settings 专用侧边栏
            <SettingsSidebar />

            // 第三栏：内容区（通过 Outlet 渲染子路由）
            <div class="flex-1 overflow-y-auto">
                <Outlet />
            </div>
        </div>
    }
}
```

**Step 3: 创建 mod.rs 导出**

```rust
// core/ui/control_plane/src/components/layouts/mod.rs
pub mod settings_layout;

pub use settings_layout::SettingsLayout;
```

**Step 4: 在 components/mod.rs 中导出 layouts**

在 `core/ui/control_plane/src/components/mod.rs` 中添加：

```rust
pub mod layouts;
pub use layouts::SettingsLayout;
```

**Step 5: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功

**Step 6: Commit**

```bash
git add core/ui/control_plane/src/components/layouts/
git add core/ui/control_plane/src/components/mod.rs
git commit -m "feat(control-plane): add SettingsLayout for nested routing

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 4: 重构 app.rs 使用嵌套路由

**Files:**
- Modify: `core/ui/control_plane/src/app.rs`

**Step 1: 读取当前 app.rs**

Run: `cat core/ui/control_plane/src/app.rs`

**Step 2: 导入 SettingsLayout**

在文件顶部添加：

```rust
use crate::components::SettingsLayout;
```

**Step 3: 修改路由结构**

将 Settings 相关路由从平铺结构改为嵌套结构。

**原有结构**：
```rust
<Route path="/settings" view=Settings />
<Route path="/settings/general" view=GeneralView />
<Route path="/settings/shortcuts" view=ShortcutsView />
// ... 其他设置路由
```

**新结构**（使用 ParentRoute）：
```rust
<ParentRoute path="/settings" view=SettingsLayout>
    <Route path="/" view=Settings />
    <Route path="/general" view=GeneralView />
    <Route path="/shortcuts" view=ShortcutsView />
    <Route path="/behavior" view=BehaviorView />
    <Route path="/generation" view=GenerationView />
    <Route path="/search" view=SearchView />
    <Route path="/providers" view=ProvidersView />
    <Route path="/generation-providers" view=GenerationProvidersView />
    <Route path="/agent" view=AgentView />
    <Route path="/routing" view=RoutingRulesView />
    <Route path="/mcp" view=McpView />
    <Route path="/plugins" view=PluginsView />
    <Route path="/skills" view=SkillsView />
    <Route path="/memory" view=MemoryView />
    <Route path="/security" view=SecurityView />
    <Route path="/policies" view=PoliciesView />
</ParentRoute>
```

**注意**：如果 Leptos 不支持 `ParentRoute`，使用嵌套 `<Route>` 的方式：

```rust
<Route path="/settings" view=SettingsLayout>
    <Route path="" view=Settings />
    <Route path="general" view=GeneralView />
    // ... 其他路由
</Route>
```

**Step 4: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功

**Step 5: 测试路由**

Run: `cd core/ui/control_plane && trunk serve`
手动测试：
1. 访问 `http://localhost:8080/settings` - 应显示 Settings 首页 + SettingsSidebar
2. 访问 `http://localhost:8080/settings/general` - 应显示 GeneralView + SettingsSidebar（持久化）
3. 在 Settings 子页面间切换 - SettingsSidebar 不应重新挂载（无闪烁）

**Step 6: Commit**

```bash
git add core/ui/control_plane/src/app.rs
git commit -m "refactor(control-plane): use nested routes for Settings with SettingsLayout

- Settings routes now use ParentRoute/nested Route
- SettingsSidebar persists across all Settings sub-pages
- Fixes navigation context loss issue

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 5: 拆分 Sidebar 组件（准备工作）

**Files:**
- Read: `core/ui/control_plane/src/components/sidebar.rs`
- Create: `core/ui/control_plane/src/components/sidebar/sidebar.rs`
- Create: `core/ui/control_plane/src/components/sidebar/sidebar_item.rs`

**Step 1: 备份原有 sidebar.rs**

```bash
cp core/ui/control_plane/src/components/sidebar.rs core/ui/control_plane/src/components/sidebar_backup.rs
```

**Step 2: 读取原有 sidebar.rs 内容**

Run: `cat core/ui/control_plane/src/components/sidebar.rs`

**Step 3: 将 Sidebar 组件移动到 sidebar/sidebar.rs**

创建 `core/ui/control_plane/src/components/sidebar/sidebar.rs`，将原有的 `Sidebar` 组件代码复制过来（暂时保持原样，下一个 Task 会修改）。

**Step 4: 将 SidebarItem 组件移动到 sidebar/sidebar_item.rs**

创建 `core/ui/control_plane/src/components/sidebar/sidebar_item.rs`，将原有的 `SidebarItem` 组件代码复制过来。

**Step 5: 更新 sidebar/mod.rs**

```rust
// core/ui/control_plane/src/components/sidebar/mod.rs
pub mod types;
pub mod sidebar;
pub mod sidebar_item;

pub use types::{SidebarMode, AlertLevel, SystemAlert};
pub use sidebar::Sidebar;
pub use sidebar_item::SidebarItem;
```

**Step 6: 删除原有 sidebar.rs**

```bash
rm core/ui/control_plane/src/components/sidebar.rs
```

**Step 7: 更新 components/mod.rs**

将：
```rust
pub mod sidebar;
pub use sidebar::Sidebar;
```

改为：
```rust
pub mod sidebar;
pub use sidebar::{Sidebar, SidebarItem};
```

**Step 8: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功

**Step 9: Commit**

```bash
git add core/ui/control_plane/src/components/sidebar/
git rm core/ui/control_plane/src/components/sidebar.rs
git add core/ui/control_plane/src/components/mod.rs
git commit -m "refactor(control-plane): split Sidebar into sidebar/ directory

- Move Sidebar to sidebar/sidebar.rs
- Move SidebarItem to sidebar/sidebar_item.rs
- Prepare for narrow/wide mode implementation

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 6: 实现 Sidebar 窄/宽模式切换

**Files:**
- Modify: `core/ui/control_plane/src/components/sidebar/sidebar.rs`

**Step 1: 读取当前 sidebar.rs**

Run: `cat core/ui/control_plane/src/components/sidebar/sidebar.rs`

**Step 2: 添加必要的导入**

```rust
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use crate::context::DashboardState;
use crate::components::sidebar::{SidebarMode, SidebarItem};
```

**Step 3: 实现模式切换逻辑**

在 `Sidebar` 组件中添加：

```rust
#[component]
pub fn Sidebar() -> impl IntoView {
    let location = use_location();
    let state = expect_context::<DashboardState>();

    // 默认：根据路由自动判断
    let auto_mode = move || {
        if location.pathname.get().starts_with("/settings") {
            SidebarMode::Narrow
        } else {
            SidebarMode::Wide
        }
    };

    // 最终模式：用户覆盖 > 自动判断
    let mode = move || {
        state.sidebar_mode_override.get()
            .unwrap_or_else(|| auto_mode())
    };

    view! {
        <aside class=move || {
            let base = "border-r border-slate-800 bg-slate-900/50 backdrop-blur-xl flex flex-col transition-all duration-300";
            match mode() {
                SidebarMode::Narrow => format!("{} w-16 items-center", base),
                SidebarMode::Wide => format!("{} w-64", base),
            }
        }>
            // Logo 区域
            <LogoSection mode=mode />

            // 导航区域
            <nav class="flex-1 px-4 py-4 space-y-2">
                // 保持原有的 SidebarItem，但传入 mode
                // 暂时不修改 SidebarItem 的实现
            </nav>

            // 底部操作区域
            <div class="p-4 border-t border-slate-800">
                // Settings 链接
            </div>
        </aside>
    }
}
```

**Step 4: 实现 LogoSection 组件**

在同一文件中添加：

```rust
#[component]
fn LogoSection(mode: impl Fn() -> SidebarMode + 'static + Copy) -> impl IntoView {
    view! {
        <div class=move || {
            match mode() {
                SidebarMode::Wide => "p-6 flex items-center gap-3",
                SidebarMode::Narrow => "p-4 flex items-center justify-center",
            }
        }>
            <div class="w-8 h-8 bg-gradient-to-br from-indigo-500 to-purple-600 rounded-lg flex items-center justify-center shadow-lg shadow-indigo-500/20">
                <span class="text-white font-bold text-xl">"A"</span>
            </div>
            {move || match mode() {
                SidebarMode::Wide => Some(view! {
                    <h1 class="text-xl font-semibold tracking-tight">"Aleph Hub"</h1>
                }),
                SidebarMode::Narrow => None,
            }}
        </div>
    }
}
```

**Step 5: 验证编译**

Run: `cd core/ui/control_plane && cargo check`
Expected: 编译成功

**Step 6: 测试模式切换**

Run: `cd core/ui/control_plane && trunk serve`
手动测试：
1. 访问 `/` - Sidebar 应为宽模式（w-64），显示 "Aleph Hub" 文字
2. 访问 `/settings` - Sidebar 应自动收窄为 w-16，只显示 "A" 图标
3. 返回 `/` - Sidebar 应自动展开为 w-64
4. 观察过渡动画是否平滑（300ms）

**Step 7: Commit**

```bash
git add core/ui/control_plane/src/components/sidebar/sidebar.rs
git commit -m "feat(control-plane): implement Sidebar narrow/wide mode switching

- Auto-switch to narrow mode when entering /settings/*
- Support sidebar_mode_override for manual control
- Add smooth transition animation (300ms)
- Logo adapts to mode (full text vs icon only)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 7: 验证 Phase 1 完成

**验收标准检查清单**：

**Step 1: 测试导航持久化**

```bash
cd core/ui/control_plane && trunk serve
```

手动测试：
- [ ] 访问 `/settings/general` - SettingsSidebar 应可见
- [ ] 点击 SettingsSidebar 中的其他分类（如 Shortcuts） - 侧边栏不应闪烁
- [ ] 在 Settings 子页面间快速切换 - 体验应流畅

**Step 2: 测试 Sidebar 模式切换**

- [ ] 从 `/` 进入 `/settings` - 主 Sidebar 应收窄为 w-16
- [ ] 从 `/settings` 返回 `/` - 主 Sidebar 应展开为 w-64
- [ ] 过渡动画应平滑（无跳变）

**Step 3: 测试状态总线（基础）**

在浏览器控制台中测试：

```javascript
// 假设 DashboardState 可通过某种方式访问
// 这一步主要验证 API 存在，Phase 2 会实际使用
```

**Step 4: 代码审查**

- [ ] 所有新文件都有适当的注释
- [ ] 类型定义清晰（SidebarMode, AlertLevel, SystemAlert）
- [ ] 路由结构使用嵌套模式
- [ ] 无编译警告

**Step 5: 文档更新**

如果需要，更新 `docs/ARCHITECTURE.md` 或相关文档，说明新的路由结构。

**Step 6: 最终 Commit**

```bash
git add -A
git commit -m "milestone: complete Phase 1 - Control Plane three-column layout foundation

Achievements:
- ✅ Persistent navigation in Settings sub-pages
- ✅ Auto narrow/wide Sidebar mode switching
- ✅ Nested routing with SettingsLayout
- ✅ Alert bus infrastructure in DashboardState

Next: Phase 2 - UI enhancements (Tooltip, Badge, subscriptions)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Phase 2: UI 增强（预览）

Phase 2 将在 Phase 1 完成后开始，主要任务包括：

1. **Task 8**: 实现 Tooltip 组件（`components/ui/tooltip.rs`）
2. **Task 9**: 实现 StatusBadge 组件（`components/ui/badge.rs`）
3. **Task 10**: 修改 SidebarItem 集成 Tooltip 和 Badge
4. **Task 11**: 实现告警状态订阅逻辑
5. **Task 12**: 测试窄模式下的 Tooltip 显示
6. **Task 13**: 测试 Badge 的不同告警级别样式

详细步骤将在 Phase 1 完成后补充。

---

## Phase 3: 数据集成（预览）

Phase 3 将在 Phase 2 完成后开始，主要任务包括：

1. **Task 14**: 在 `shared_ui_logic` 中定义告警相关的 RPC 方法
2. **Task 15**: 实现 WebSocket 订阅机制
3. **Task 16**: 移除 Mock 数据
4. **Task 17**: 集成真实的 Gateway RPC 调用
5. **Task 18**: 测试实时告警更新

详细步骤将在 Phase 2 完成后补充。

---

## 技术注意事项

### Leptos 路由 API 兼容性

如果 `ParentRoute` 不可用，使用以下替代方案：

```rust
// 方案 1：嵌套 Route
<Route path="/settings" view=SettingsLayout>
    <Route path="" view=Settings />
    <Route path="general" view=GeneralView />
</Route>

// 方案 2：使用 Routes 嵌套
<Route path="/settings/*">
    <SettingsLayout>
        <Routes>
            <Route path="/" view=Settings />
            <Route path="/general" view=GeneralView />
        </Routes>
    </SettingsLayout>
</Route>
```

### 状态管理最佳实践

- 使用 `RwSignal` 而不是 `Signal` 来管理 `HashMap`，避免不必要的克隆
- 告警状态应该是 `Option<SystemAlert>`，而不是直接存储 `SystemAlert`
- 考虑使用 `create_memo` 来缓存计算结果（如 `auto_mode`）

### 性能优化

- 如果告警数量超过 10 个，考虑使用 `IndexMap` 替代 `HashMap`
- Sidebar 模式切换应该使用 CSS `transition`，而不是 JavaScript 动画
- Tooltip 应该使用 CSS `:hover` 伪类，而不是 JavaScript 事件

---

## 故障排查

### 问题 1：SettingsSidebar 在子页面消失

**症状**：进入 `/settings/general` 后，SettingsSidebar 不可见。

**排查步骤**：
1. 检查 `app.rs` 中的路由结构是否正确嵌套
2. 检查 `SettingsLayout` 是否正确渲染 `<SettingsSidebar />`
3. 使用浏览器开发者工具检查 DOM 结构

**解决方案**：确保 Settings 相关路由都在 `SettingsLayout` 的 `<ParentRoute>` 或嵌套 `<Route>` 下。

### 问题 2：Sidebar 模式切换不生效

**症状**：进入 `/settings` 后，Sidebar 仍然是宽模式。

**排查步骤**：
1. 检查 `use_location()` 是否正确获取路径
2. 在 `auto_mode` 中添加 `console.log` 调试
3. 检查 CSS 类名是否正确应用

**解决方案**：确保 `location.pathname.get().starts_with("/settings")` 逻辑正确。

### 问题 3：编译错误 - 找不到 `ParentRoute`

**症状**：`cargo check` 报错 `cannot find ParentRoute in leptos_router`。

**解决方案**：使用嵌套 `<Route>` 的替代方案（见"技术注意事项"）。

---

**实施负责人**: 待定
**预计完成时间**: Phase 1 约 2-3 小时
**最后更新**: 2026-02-10

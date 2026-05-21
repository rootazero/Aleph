# Control Plane 三栏布局架构重构设计

> **设计日期**: 2026-02-10
> **状态**: 设计完成，待实施
> **优先级**: 高（用户体验核心改进）

## 背景与问题

### 当前架构问题

Aleph Control Plane 正处于从"独立 Dashboard"向"嵌入式控制平面"迁移的过渡阶段。经过全面架构审计，发现以下关键问题：

#### 1. 布局架构缺陷：导航上下文丢失（最高优先级）

**现状**：
- 采用全局 Sidebar + 内容区路由模式
- 进入 `/settings` 后，导航模式切换到"网格化首页"
- 进入二级设置页面（如 `GeneralView`）时，`SettingsSidebar` 消失
- 用户失去在不同设置分类间快速切换的能力，必须"后退-点击"

**影响**：
- 用户心智模型的连续性被破坏
- "死胡同"UI 模式增加认知负荷
- 违反"中心化管理"的控制面板设计原则

#### 2. 其他待解决问题（后续优先级）

- **设计系统完整性风险**：样式定义碎片化（`shadow-glass` 等未在 `tailwind.config.js` 定义）
- **信息架构与职责模糊**：顶级导航混淆"实时监控"与"持久化资产管理"
- **数据架构与有效性问题**：Mock 数据充斥，`control_plane` 与 `shared_ui_logic` 代码重复

### 优先级排序

**A > D > C > B**

1. **A (布局 - 骨架)**：解决导航持久化问题
2. **D (数据 - 灵魂)**：将 `shared_ui_logic` 注入已稳定的布局
3. **C (架构 - 大脑)**：进行功能域划分（Observability/Control/Assets）
4. **B (系统 - 皮肤)**：视觉规范化，样式沉淀到 `tailwind.config.js`

**本设计聚焦于 A（布局架构）**，为后续优化奠定基础。

---

## 设计目标

### 核心理念

采用 **"分形导航"（Fractal Navigation）** 模式，通过嵌套路由实现三栏布局的自动展开/收起。

### 三栏布局定义

```
┌────────┬──────────────┬─────────────────────────────┐
│ 第一层 │   第二层     │         第三层              │
│ 全局域 │  上下文导航  │        内容区               │
│ 导航   │  (条件显示)  │                             │
├────────┼──────────────┼─────────────────────────────┤
│ w-16   │   w-64       │        flex-1               │
│ 窄边栏 │  宽边栏      │                             │
│ 仅图标 │ Settings时   │   实际配置表单/视图         │
│        │ 展开         │                             │
└────────┴──────────────┴─────────────────────────────┘
```

### 关键改进

1. **状态一致性**：用户在深度配置时，依然可以从第一层看到系统健康状态（如闪烁的红色小点）
2. **路由解耦**：`SettingsSidebar` 的出现由路由状态触发，而不是由特定父组件渲染
3. **视觉稳定性**：Settings 子页面切换时，侧边栏保持静止，避免重绘闪烁

---

## 架构设计

### 1. 整体架构与路由结构

#### 路由树结构

```rust
// app.rs
<Router>
    <div class="flex h-screen bg-slate-950 text-slate-50">
        // 第一栏：全局主导航（始终显示）
        <Sidebar />

        // 第二、三栏：由路由决定
        <main class="flex-1 overflow-hidden relative">
            <Routes>
                // 普通路由：只有内容区（第三栏）
                <Route path="/" view=Home />
                <Route path="/trace" view=AgentTrace />
                <Route path="/status" view=SystemStatus />
                <Route path="/memory" view=Memory />

                // Settings 路由：使用 SettingsLayout（包含第二栏）
                <ParentRoute path="/settings" view=SettingsLayout>
                    <Route path="/" view=SettingsHome />
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
            </Routes>
        </main>
    </div>
</Router>
```

#### 关键特性

1. **确定性生命周期**：`SettingsLayout` 的存在由路由树保证，只要在 `/settings/*` 下就会渲染
2. **视觉稳定性**：Settings 子页面切换时，`SettingsSidebar` 不会重新挂载，避免闪烁
3. **关注点分离**：`App.rs` 只管理顶级路由，`SettingsLayout` 管理设置域的布局

---

### 2. 主 Sidebar 的窄/宽模式设计

#### 模式定义

```rust
// components/sidebar/types.rs
#[derive(Clone, Copy, PartialEq)]
pub enum SidebarMode {
    Wide,   // w-64 (256px) - 图标 + 文字
    Narrow, // w-16 (64px)  - 仅图标
}

#[derive(Clone, Copy, PartialEq)]
pub enum AlertLevel {
    None,
    Info,     // 蓝色徽章
    Warning,  // 黄色徽章
    Critical, // 红色徽章 + 呼吸动画
}
```

#### 模式切换逻辑（路由驱动 + 可选覆盖）

```rust
// components/sidebar/sidebar.rs
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
            if mode() == SidebarMode::Narrow {
                "w-16 border-r border-slate-800 bg-slate-900/50 backdrop-blur-xl flex flex-col items-center transition-all duration-300"
            } else {
                "w-64 border-r border-slate-800 bg-slate-900/50 backdrop-blur-xl flex flex-col transition-all duration-300"
            }
        }>
            // Logo 区域（窄模式下只显示图标）
            <LogoSection mode=mode />

            // 导航区域
            <nav class="flex-1 px-4 py-4 space-y-2">
                <SidebarItem
                    href="/"
                    label="Dashboard"
                    alert_key="dashboard.status"
                    mode=mode
                >
                    {/* Home 图标 SVG */}
                </SidebarItem>

                <SidebarItem
                    href="/trace"
                    label="Agent Trace"
                    alert_key="agent.trace"
                    mode=mode
                >
                    {/* Trace 图标 SVG */}
                </SidebarItem>

                <SidebarItem
                    href="/status"
                    label="System Health"
                    alert_key="system.health"
                    mode=mode
                >
                    {/* Health 图标 SVG */}
                </SidebarItem>

                <SidebarItem
                    href="/memory"
                    label="Memory Vault"
                    alert_key="memory.status"
                    mode=mode
                >
                    {/* Memory 图标 SVG */}
                </SidebarItem>
            </nav>

            // 底部操作区域
            <div class="p-4 border-t border-slate-800">
                <SidebarItem
                    href="/settings"
                    label="Settings"
                    mode=mode
                >
                    {/* Settings 图标 SVG */}
                </SidebarItem>
            </div>
        </aside>
    }
}
```

#### 关键特性

1. **自动化**：进入 `/settings/*` 自动收窄，离开自动展开
2. **可覆盖**：为未来的用户偏好、响应式布局预留扩展点（`sidebar_mode_override`）
3. **平滑过渡**：使用 CSS `transition-all duration-300` 实现动画

---

### 3. SettingsLayout 的实现

#### 核心职责

作为 Settings 域的布局容器，管理 `SettingsSidebar`（第二栏）和内容区（第三栏）的渲染。

#### 实现方案

```rust
// components/layouts/settings_layout.rs
use leptos::prelude::*;
use leptos_router::components::Outlet;
use crate::components::SettingsSidebar;

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

#### 关键改进

1. **生命周期绑定**：`SettingsLayout` 只在 `/settings/*` 路由下存在，离开时自动卸载
2. **Outlet 机制**：子路由（如 `GeneralView`）通过 `<Outlet />` 渲染到第三栏，无需手动管理
3. **视觉连续性**：Settings 子页面切换时，`SettingsSidebar` 保持挂载，只有 Outlet 内容重新渲染

---

### 4. 状态指示器与订阅模式

#### 核心理念

通过"状态总线"实现解耦的告警系统，让 Sidebar 的每个 Item 能够订阅相关的系统状态，实现"环境感知"（Ambient Awareness）。

#### 状态总线设计

```rust
// context.rs 或 shared_ui_logic
#[derive(Clone, Debug)]
pub struct SystemAlert {
    pub key: &'static str,      // "system.health", "memory.usage"
    pub level: AlertLevel,       // None/Info/Warning/Critical
    pub count: Option<u32>,      // 可选的数字徽章（如 "3 个错误"）
    pub message: Option<String>, // Tooltip 中显示的详细信息
}

#[derive(Clone)]
pub struct DashboardState {
    // ... 其他状态
    pub alerts: RwSignal<HashMap<&'static str, SystemAlert>>,
    pub sidebar_mode_override: RwSignal<Option<SidebarMode>>,
}

impl DashboardState {
    pub fn update_alert(&self, key: &'static str, alert: SystemAlert) {
        self.alerts.update(|map| {
            map.insert(key, alert);
        });
    }

    pub fn get_alert(&self, key: &'static str) -> Option<SystemAlert> {
        self.alerts.with(|map| map.get(key).cloned())
    }
}
```

#### SidebarItem 的订阅实现

```rust
// components/sidebar/sidebar_item.rs
#[component]
pub fn SidebarItem(
    href: &'static str,
    label: &'static str,
    icon: Children,
    mode: ReadSignal<SidebarMode>,
    #[prop(optional)] alert_key: Option<&'static str>, // 订阅的状态 key
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // 订阅告警状态
    let alert = move || {
        alert_key.and_then(|key| state.get_alert(key))
    };

    view! {
        <A href=href class="relative group flex items-center gap-3 px-3 py-2 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800/50 transition-all duration-200">
            // 图标容器
            <div class="relative flex-shrink-0">
                <svg
                    width="20"
                    height="20"
                    class="w-5 h-5 group-hover:scale-110 transition-transform duration-200"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    {icon()}
                </svg>

                // 状态徽章（右上角）
                {move || alert().map(|a| view! {
                    <StatusBadge level=a.level count=a.count />
                })}
            </div>

            // 文字标签（宽模式）或 Tooltip（窄模式）
            {move || match mode.get() {
                SidebarMode::Wide => view! {
                    <span class="text-sm font-medium">{label}</span>
                }.into_any(),
                SidebarMode::Narrow => view! {
                    <Tooltip text=label alert=alert() position="right" />
                }.into_any(),
            }}
        </A>
    }
}
```

#### StatusBadge 组件

```rust
// components/ui/badge.rs
#[component]
pub fn StatusBadge(
    level: AlertLevel,
    #[prop(optional)] count: Option<u32>,
) -> impl IntoView {
    let (bg_class, animation_class) = match level {
        AlertLevel::None => return view! {}.into_any(),
        AlertLevel::Info => ("bg-blue-500", ""),
        AlertLevel::Warning => ("bg-yellow-500", ""),
        AlertLevel::Critical => ("bg-red-500", "animate-pulse"),
    };

    view! {
        <div class=format!(
            "absolute -top-1 -right-1 {} {} rounded-full min-w-[16px] h-4 flex items-center justify-center text-[10px] font-bold text-white px-1",
            bg_class, animation_class
        )>
            {count.map(|c| c.to_string()).unwrap_or_default()}
        </div>
    }.into_any()
}
```

#### Tooltip 组件

```rust
// components/ui/tooltip.rs
#[component]
pub fn Tooltip(
    text: &'static str,
    #[prop(optional)] alert: Option<SystemAlert>,
    #[prop(default = "right")] position: &'static str,
) -> impl IntoView {
    view! {
        <div class="absolute left-full ml-2 px-3 py-2 bg-slate-800 border border-slate-700 rounded-lg shadow-xl opacity-0 group-hover:opacity-100 transition-opacity duration-200 pointer-events-none whitespace-nowrap z-50">
            <div class="text-sm font-medium text-slate-200">{text}</div>
            {alert.and_then(|a| a.message).map(|msg| view! {
                <div class="text-xs text-slate-400 mt-1">{msg}</div>
            })}
        </div>
    }
}
```

#### 使用示例

```rust
<SidebarItem
    href="/status"
    label="System Health"
    alert_key="system.health"  // 订阅系统健康状态
    mode=mode
>
    {/* 图标 SVG */}
</SidebarItem>
```

#### 关键特性

1. **解耦设计**：告警状态由 Gateway RPC 更新，Sidebar 只负责订阅和显示
2. **实时响应**：使用 Signal，状态变化自动触发 UI 更新
3. **灵活扩展**：未来可以轻松添加新的告警源（如 Memory 使用率、Agent 错误数）

---

### 5. 组件拆分与文件结构

#### 目标文件结构

```
core/ui/control_plane/src/
├── components/
│   ├── layouts/
│   │   ├── mod.rs
│   │   ├── main_layout.rs      // 未来可能需要的主布局
│   │   └── settings_layout.rs  // Settings 域布局
│   ├── sidebar/
│   │   ├── mod.rs               // 导出 Sidebar, SidebarItem, types
│   │   ├── sidebar.rs           // Sidebar 主容器
│   │   ├── sidebar_item.rs      // SidebarItem + Badge + Tooltip
│   │   └── types.rs             // SidebarMode, AlertLevel, SystemAlert
│   ├── settings_sidebar.rs      // Settings 二级侧边栏（保持现状）
│   ├── ui/                      // 原子组件（Badge, Tooltip, Card 等）
│   │   ├── mod.rs
│   │   ├── badge.rs             // StatusBadge 组件
│   │   ├── tooltip.rs           // Tooltip 组件（新增）
│   │   ├── card.rs
│   │   └── button.rs
│   └── mod.rs
├── views/
│   ├── home.rs
│   ├── agent_trace.rs
│   ├── settings/
│   │   ├── mod.rs
│   │   ├── general.rs
│   │   └── ...
│   └── mod.rs
├── context.rs                   // DashboardState + 状态总线
├── app.rs                       // 路由配置
└── lib.rs
```

---

## 实施计划

### Phase 1：基础架构（高优先级）

**目标**：建立三栏布局的骨架，实现导航持久化。

**任务清单**：

1. **创建 Layout 组件**
   - [ ] 创建 `components/layouts/` 目录
   - [ ] 实现 `settings_layout.rs`
   - [ ] 在 `components/layouts/mod.rs` 中导出

2. **重构 Sidebar**
   - [ ] 创建 `components/sidebar/` 目录
   - [ ] 创建 `types.rs`（定义 `SidebarMode`, `AlertLevel`）
   - [ ] 拆分 `sidebar.rs`（主容器）和 `sidebar_item.rs`（单项）
   - [ ] 实现窄/宽模式切换逻辑
   - [ ] 在 `components/sidebar/mod.rs` 中导出

3. **修改路由结构**
   - [ ] 修改 `app.rs`，引入 `ParentRoute`
   - [ ] 将 Settings 相关路由嵌套到 `SettingsLayout` 下
   - [ ] 测试路由切换，确保 `SettingsSidebar` 持久化

4. **更新 Context**
   - [ ] 在 `context.rs` 的 `DashboardState` 中添加：
     - `alerts: RwSignal<HashMap<&'static str, SystemAlert>>`
     - `sidebar_mode_override: RwSignal<Option<SidebarMode>>`
   - [ ] 实现 `update_alert()` 和 `get_alert()` 方法

**验收标准**：
- ✅ 进入 `/settings/general` 时，`SettingsSidebar` 保持可见
- ✅ 主 Sidebar 在 `/settings/*` 下自动收窄为 w-16
- ✅ 离开 Settings 时，主 Sidebar 自动展开为 w-64

---

### Phase 2：UI 增强（中优先级）

**目标**：实现 Tooltip、Badge 等交互组件，提升可用性。

**任务清单**：

5. **实现 Tooltip 组件**
   - [ ] 创建 `components/ui/tooltip.rs`
   - [ ] 支持 `position` 参数（right/left/top/bottom）
   - [ ] 支持显示 `SystemAlert` 的详细信息
   - [ ] 在 `components/ui/mod.rs` 中导出

6. **实现 StatusBadge 组件**
   - [ ] 在 `components/ui/badge.rs` 中添加 `StatusBadge`
   - [ ] 支持 `AlertLevel` 的不同样式（颜色、动画）
   - [ ] 支持可选的数字徽章（`count`）

7. **集成到 SidebarItem**
   - [ ] 在 `SidebarItem` 中集成 Tooltip（窄模式）
   - [ ] 在 `SidebarItem` 中集成 StatusBadge（图标右上角）
   - [ ] 实现告警状态订阅逻辑

**验收标准**：
- ✅ 窄模式下，鼠标悬停显示 Tooltip
- ✅ 当 `system.health` 状态为 Critical 时，图标显示红色呼吸徽章
- ✅ Tooltip 中显示告警的详细信息

---

### Phase 3：数据集成（后续）

**目标**：将 Mock 数据替换为真实的 Gateway RPC 调用，实现状态总线的自动更新。

**任务清单**：

8. **Gateway RPC 集成**
   - [ ] 在 `shared_ui_logic` 中定义告警相关的 RPC 方法
   - [ ] 在 `control_plane` 中调用 `shared_ui_logic` 的 API
   - [ ] 移除 `control_plane` 内部的重复 API 定义

9. **WebSocket 订阅**
   - [ ] 实现 WebSocket 订阅机制，监听系统状态变化
   - [ ] 当收到状态更新时，调用 `DashboardState::update_alert()`
   - [ ] 测试实时更新（如手动触发系统错误，观察徽章变化）

10. **Mock 数据清理**
    - [ ] 移除 `mock_data.rs` 中的硬编码数据
    - [ ] 确保所有视图使用真实数据

**验收标准**：
- ✅ System Health 视图显示真实的系统指标
- ✅ 当系统出现错误时，主 Sidebar 的 "System Health" 图标自动显示红色徽章
- ✅ 无 Mock 数据残留

---

## 技术风险与缓解

### 风险 1：Leptos ParentRoute 兼容性

**风险描述**：Leptos 的嵌套路由 API 可能与文档中的示例不完全一致。

**缓解措施**：
- 在 Phase 1 开始前，先创建一个最小化的嵌套路由示例，验证 API 可用性
- 如果 `ParentRoute` 不可用，使用 `<Route>` 嵌套或其他 Leptos 推荐的方式

### 风险 2：状态总线性能

**风险描述**：如果告警数量过多，`HashMap` 的频繁更新可能影响性能。

**缓解措施**：
- 初期只订阅少量关键状态（4-5 个）
- 如果性能成为问题，考虑使用 `IndexMap` 或分片存储

### 风险 3：Tooltip 在窄边栏中的定位

**风险描述**：窄边栏靠近屏幕左侧，Tooltip 可能被遮挡或溢出。

**缓解措施**：
- Tooltip 默认显示在右侧（`position="right"`）
- 使用 `z-50` 确保 Tooltip 在最上层
- 测试不同屏幕尺寸下的显示效果

---

## 后续优化方向

完成 Phase 1-3 后，可以考虑以下优化：

### 1. 设计系统完整性（优先级 B）

- 将 `shadow-glass`、`shadow-neon-indigo` 等自定义样式沉淀到 `tailwind.config.js`
- 提取原子组件（`CardSection`、`FormGroup`）
- 建立设计令牌（Design Tokens）规范

### 2. 信息架构重组（优先级 C）

- 重新梳理功能域：
  - **观察域 (Observability)**：Home, Trace, Health
  - **控制域 (Control)**：Settings, Providers, Security
  - **资产域 (Assets)**：Memory, Files, Skills
- 调整顶级导航的分类和顺序

### 3. 智能化控制平面（未来特性）

- **可视化配置**：针对 Routing Rules，引入可视化连线界面
- **主动诊断**：基于日志自动提出优化建议
- **多环境快照**：支持配置环境的快照与回滚功能

---

## 总结

本设计通过 **嵌套路由 + Layout 模式** 实现了三栏布局架构，解决了 Control Plane 的核心用户体验问题：

1. **导航持久化**：用户在任何 Settings 子页面都能看到分类列表
2. **状态一致性**：通过状态总线和徽章系统，实现"环境感知"
3. **架构清晰**：路由即真相，生命周期确定，关注点分离

这是构建"工业级生产工具"的黄金标准，为后续的数据集成和功能扩展奠定了坚实的基础。

---

**设计者**: Claude Sonnet 4.5
**审核者**: 待定
**实施负责人**: 待定

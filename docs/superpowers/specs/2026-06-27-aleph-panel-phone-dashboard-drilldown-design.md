# 手机端 Dashboard 下钻屏 设计 (Phone Dashboard Drill-Down)

> 批次 #3/4。四个还在左右分屏 / 手机不可达的 tab(Agents/Teams/Dashboard/Extensions)套同一「手机不分屏下钻法则」,各自独立 spec。顺序:Agents(#1,已完成)→ More 入口(#2,已完成)→ **Dashboard(本 spec)** → Teams → Extensions。

## §1 背景与问题

桌面 Dashboard 是「256px 左栏 `DashboardSidebar`(6 项次级菜单)+ `MainContent` 内容」两列。手机上 `ModeSidebar`(含 `DashboardSidebar`)被各模式的 `PhoneShell`(`fixed h-dvh z-[70]`)覆盖 —— 但 Dashboard 此前**没有手机专屏**,所以一旦进入 `/dashboard`,左栏 + 内容会并排挤在手机宽 = 左右分屏。

批次 #2(More 入口)已让 Dashboard 在手机上**可达**(••• → More 菜单 → 点 Dashboard 行 → 导航 `/dashboard`),但当时约定的过渡期行为是「先导航、暂仍显示桌面布局」。本 spec 用「不分屏下钻法则」把 `/dashboard` 重做成手机原生菜单 + 下钻,闭合这个过渡。

`DashboardSidebar`(`components/dashboard_sidebar.rs`)的 6 项:

| 次序 | 路径 | 视图 | 桌面图标 |
|---|---|---|---|
| 1 | `/dashboard` | Overview(`Home`) | house |
| 2 | `/dashboard/trace` | Agent Trace(`AgentTrace`) | pulse(`alert_key="agent.trace"`) |
| 3 | `/dashboard/tasks` | Scheduled Tasks(`TasksView`) | clock |
| 4 | `/dashboard/logs` | Server Logs(`Logs`) | file-lines |
| 5 | `/dashboard/runtimes` | Runtimes(`RuntimesView`) | monitor |
| 6 | `/dashboard/usage` | Usage(`UsageView`) | bar-chart |

`DashboardRouter`(`app.rs`)另映射两条不在 sidebar 的 legacy 路径:`/dashboard/memory`(→ MemoryVaultRedirect)、`/dashboard/cron`(→ CronView)。手机菜单**不暴露**这两条。

## §2 导航法则(复用既有约束)

- 手机屏**坚决不做左右分屏**。
- 底部 `PhoneTabBar` = 顶层模式切换;Dashboard 经 ••• More tab 进入(批次 #2),••• 在 Dashboard 全程高亮(`under_more()` 已含 Dashboard)。
- 模式的手机**着陆页**(全屏)= 该模式桌面版「左侧次级菜单」内容 = `DashboardSidebar` 的 6 项作为菜单。
- 点菜单项 → 下钻到全屏内容页,内容页带 `‹ Dashboard` 返回回菜单。
- 路由驱动:`PanelMode::from_path` 已用 `starts_with("/dashboard")` 把所有 `/dashboard*` 归类为 Dashboard mode,无需注册 `<Routes>`。

## §3 范围

**做**:手机 `/dashboard` 菜单着陆页(6 行)+ 6 个叶子的全屏下钻(各带 ‹ 返回)。

**叶子策略(user 已定:复用桌面视图 + 全屏挂载)**:每个叶子 = 把现有桌面视图(`Home`/`AgentTrace`/`TasksView`/`Logs`/`RuntimesView`/`UsageView`)原样挂进 `PhoneShell` 全屏 body。这些视图桌面端天生宽(Home 卡片墙、Tasks 表、Usage 图表)→ 同 Canvas/Teams/Extensions 先例:**全屏挂载、宽交互延后**(可能横滚,作为过渡可接受)。本批次只做导航外壳,不重写叶子内部。

**不做**:重写任一叶子视图的内部布局;暴露 legacy `/dashboard/cron`、`/dashboard/memory`;镜像 Agent Trace 告警徽标(见 §6 延后)。

## §4 路由机制(零枚举改动)

与 More(#2)的最大不同:**无需新 `PanelMode` 变体**。`from_path` 已把 `/dashboard`、`/dashboard/overview`、`/dashboard/trace` 等全部经 `starts_with("/dashboard")` 归为 `Dashboard`。故**不碰** `mode_sidebar.rs`、`nav_menu.rs`、`PanelMode` 枚举、`under_more()`。

`MainContent`(`app.rs`)的 Dashboard 臂当前是裸 `<DashboardRouter/>`(无 form-factor swap)。改为按 form-factor 分支:

```rust
<div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
    {move || if form_factor.form_factor.get() == FormFactor::Phone {
        view! { <PhoneDashboard /> }.into_any()
    } else {
        view! { <DashboardRouter /> }.into_any()
    }}
</div>
```

桌面分支仍是原 `DashboardRouter`(**字节级不变**);phone 分支走新 `PhoneDashboard`。手机端 `DashboardRouter` 根本不运行。

## §5 PhoneDashboard 路由器

**新文件** `interfaces/webchat/src/platform/phone/dashboard/mod.rs`。

**无 state struct** —— 叶子视图各自拥有数据订阅(它们在桌面就是自携数据的独立组件),菜单是静态的。路由器是纯路径分发(比 Agents 路由器更薄,无 loader / 无 nonce)。

```rust
/// Which phone Dashboard screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashScreen {
    Menu,
    Overview,
    Trace,
    Tasks,
    Logs,
    Runtimes,
    Usage,
}

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> DashScreen {
    match path.trim_end_matches('/') {
        "/dashboard" | "" => DashScreen::Menu,
        "/dashboard/overview" => DashScreen::Overview,
        "/dashboard/trace" => DashScreen::Trace,
        "/dashboard/tasks" => DashScreen::Tasks,
        "/dashboard/logs" => DashScreen::Logs,
        "/dashboard/runtimes" => DashScreen::Runtimes,
        "/dashboard/usage" => DashScreen::Usage,
        // legacy /dashboard/cron, /dashboard/memory, or anything else under
        // /dashboard → fall back to the menu (phone doesn't surface them).
        _ => DashScreen::Menu,
    }
}
```

> 注:`trim_end_matches('/')` 把 `"/dashboard/"` 归一成 `"/dashboard"`;裸根 `""`(理论不达,Dashboard mode 必有 `/dashboard` 前缀)也兜底 Menu。

`PhoneDashboard` 组件按 `screen_for_path(location.pathname)` 渲染:`Menu` → `<PhoneDashboardMenu/>`;其余 → 对应叶子(§7 全屏挂载)。

## §6 菜单着陆页

**新文件** `interfaces/webchat/src/platform/phone/dashboard/menu.rs`,单组件 `PhoneDashboardMenu`。1:1 套 PhoneMore / PhoneSettings 着陆页结构:

- `PhoneShell title="Dashboard"`(无 `back`,着陆页 → 左对齐大标题,无返回箭头)。
- 一个 `.list`,6 行 `.cell`(顺序固定,**镜像 `DashboardSidebar`**):

| 行 | `.cell-leading` 图标(复用仓库现成 SVG path) | `.cell-title` | 点击 navigate |
|---|---|---|---|
| Overview | `DashboardSidebar` overview(house+polyline) | `"Overview"` | `/dashboard/overview` |
| Agent Trace | `DashboardSidebar` agent_trace(pulse polyline) | `"Agent Trace"` | `/dashboard/trace` |
| Scheduled Tasks | `DashboardSidebar` scheduled_tasks(circle+clock) | `"Scheduled Tasks"` | `/dashboard/tasks` |
| Server Logs | `DashboardSidebar` server_logs(file-lines) | `"Server Logs"` | `/dashboard/logs` |
| Runtimes | `DashboardSidebar` runtimes(monitor) | `"Runtimes"` | `/dashboard/runtimes` |
| Usage | `DashboardSidebar` usage(bar-chart) | `"Usage"` | `/dashboard/usage` |

- 每行结构与 PhoneMore 一致:`<div class="cell" on:click=…>` → `<span class="cell-leading">{icon svg}</span>` + `<div class="cell-body"><div class="cell-title">{label}</div></div>` + `<svg class="cell-chevron" …>`。
- 标签用**字面英文**(同 PhoneMore/PhoneSettings/PhoneAgents 既有手机约定,非 i18n;桌面 sidebar 用 `t_string!` 是桌面约定,手机屏到目前为止一律字面英文)。
- 无 `.cell-value`(无状态副文本)。
- 导航用 `use_navigate()` + `NavigateOptions::default()`,每个 handler 各拿一份 clone(仿 PhoneMore `go`)。
- **零新 CSS**:全复用 `ios.css` 现有 `.list` / `.cell` / `.cell-leading` / `.cell-body` / `.cell-title` / `.cell-chevron`。
- **延后 Minor**:桌面 `DashboardSidebar` 的 Agent Trace 行带 `alert_key="agent.trace"` 告警徽标(读 `DashboardState.alerts`);**v1 手机菜单保持静态、暂不镜像徽标**(留 future,折入未来 `panel: cleanup` 或 Dashboard 二期)。

## §7 叶子全屏挂载

`PhoneDashboard`(`mod.rs`)对非 Menu 的每个叶子,渲染:

```rust
view! {
    <PhoneShell title="Agent Trace" back="/dashboard" back_label="Dashboard">
        <AgentTrace />
    </PhoneShell>
}.into_any()
```

> `PhoneShell` 签名:`title: &'static str`、`back: Option<&'static str>`、`back_label: Option<&'static str>`(均 `#[prop(optional)]`)→ 全部传字面量,**不加 `.to_string()`**。

各叶子复用现有桌面组件(从 `crate::views::…` 导入,与 `app.rs` 的 `DashboardRouter` 同源):

| DashScreen | title | 复用桌面组件 |
|---|---|---|
| Overview | `"Overview"` | `Home` |
| Trace | `"Agent Trace"` | `AgentTrace` |
| Tasks | `"Scheduled Tasks"` | `TasksView` |
| Logs | `"Server Logs"` | `Logs` |
| Runtimes | `"Runtimes"` | `RuntimesView` |
| Usage | `"Usage"` | `UsageView` |

精确导入(与 `app.rs` 同源):`crate::views::home::Home`、`crate::views::agent_trace::AgentTrace`、`crate::views::tasks::TasksView`、`crate::views::logs::Logs`、`crate::views::runtimes::RuntimesView`、`crate::views::usage::UsageView`。

- 所有叶子的 `back="/dashboard"`、`back_label="Dashboard"`,`‹` 返回回菜单。
- 叶子内部不改:它们的数据来自 app-wide context(`DashboardState` 等),在 phone 挂载点同样可用。
- **宽交互延后**:密集视图在 390px 全屏 body 内可能横向溢出/横滚,这是 Canvas 先例下可接受的过渡态,不在本批次优化。

`platform/phone/mod.rs` 加 `pub mod dashboard;`(按字母序,在 `chat` 之后、`memory` 之前)。

## §8 Overview 路由小结

桌面 `/dashboard` = `Home`(Overview);手机 `/dashboard` = 菜单。故 Overview 在手机走**新 phone-only 路径 `/dashboard/overview`**(镜像 Memory 的 `/memory/graph`、`/memory/list` 等 phone-only 子路径)。

- 该路径**仅手机使用**:桌面经 form-factor swap 后跑 `DashboardRouter`,其 `match` 不含 `/dashboard/overview`(落 `_ => ().into_any()` 空渲染),但桌面**从不导航到它**(`DashboardSidebar` 的 Overview 行链接 `/dashboard`)→ 无可见影响。
- 桌面 `/dashboard` 在桌面端仍渲染 `Home`(`DashboardRouter` 不动)。
- 桌面功能字节级不变。

## §9 变更清单

| 文件 | 改动 |
|---|---|
| `platform/phone/dashboard/mod.rs` | **新建** `PhoneDashboard` 路由器 + `DashScreen` 枚举 + `screen_for_path` 纯函数 + 6 叶子 PhoneShell 全屏挂载 + `#[cfg(test)]` 测 `screen_for_path` |
| `platform/phone/dashboard/menu.rs` | **新建** `PhoneDashboardMenu`(6 行 `.cell` 列表) |
| `platform/phone/mod.rs` | +`pub mod dashboard;` |
| `app.rs` | +`use …dashboard::PhoneDashboard;`;`MainContent` Dashboard 臂改 form-factor swap(phone→PhoneDashboard,桌面→DashboardRouter) |

零 core / 零 IPC / 零依赖 / 零新 CSS。桌面功能字节级不变(`DashboardRouter` 不动,Dashboard 臂仅新增 phone 分支)。R4(I/O-only:菜单行只导航,叶子复用既有自携数据视图)。不碰 `PanelMode` / `mode_sidebar.rs` / `nav_menu.rs`。

## §10 测试

**单测**(`mod.rs` 的 `#[cfg(test)]`):`screen_for_path` 真值表 —
- `/dashboard` → Menu;`/dashboard/` → Menu。
- `/dashboard/overview` → Overview;`/dashboard/trace` → Trace;`/dashboard/tasks` → Tasks;`/dashboard/logs` → Logs;`/dashboard/runtimes` → Runtimes;`/dashboard/usage` → Usage。
- `/dashboard/cron` → Menu;`/dashboard/memory` → Menu(legacy 兜底)。

**iOS-sim QA(权威运行时门,user-driven)**:按 [[feedback-ios-panel-test-via-full-macos-app]] 重编完整版 app 重嵌 dist → sim 连本地 core →
1. ••• → More 菜单 → 点 Dashboard → 进入手机 Dashboard 菜单:6 行(Overview/Agent Trace/Scheduled Tasks/Server Logs/Runtimes/Usage),**无左右分屏**。
2. 点任一行 → 全屏叶子视图,顶部 `‹ Dashboard` 返回;点返回回菜单。
3. 全程底部 ••• tab 保持高亮(`under_more()` 含 Dashboard)。
4. Overview 行进入 `/dashboard/overview` 显示 Home overview(全屏,允许横滚)。

## §11 成功标准

- [ ] 手机 `/dashboard` 渲染全屏 `PhoneDashboardMenu`(无左右分屏),6 行导航正确。
- [ ] 6 个叶子各自全屏挂载现有桌面视图,带 `‹ Dashboard` 返回。
- [ ] Overview 经 phone-only `/dashboard/overview` 进入;桌面 `/dashboard` 仍 = Home。
- [ ] 桌面功能字节级不变(`DashboardRouter`、`PanelMode`、`mode_sidebar.rs`、`nav_menu.rs` 不动);`just wasm` 编译通过。
- [ ] 单测覆盖 `screen_for_path` 全映射 + legacy 兜底。
- [ ] ••• tab 在 Dashboard 全程高亮(沿用 `under_more()`,无新改动)。

## §12 关联

续 [[feedback-phone-no-split-drilldown-law]]、[[project-aleph-panel-phone-more-entry]]、[[project-aleph-panel-phone-agents-drilldown]]、[[project-aleph-panel-phone-memory-drilldown]]、[[reference-leptos-phoneshell-dynamic-child-footgun]]、[[feedback-ios-panel-test-via-full-macos-app]]。

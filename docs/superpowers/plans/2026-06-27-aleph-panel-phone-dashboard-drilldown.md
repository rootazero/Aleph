# 手机端 Dashboard 下钻屏 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 `/dashboard` 从「桌面分屏」重做成原生菜单着陆页(6 行,镜像 `DashboardSidebar`)+ 各叶子全屏下钻(复用现有桌面视图,带 ‹ 返回),消除左右分屏。

**Architecture:** 新增 `platform/phone/dashboard/` 模块:`mod.rs` = 纯路径分发路由器(`screen_for_path` + `PhoneDashboard`)+ 6 个叶子的 `PhoneShell` 全屏挂载(复用 `Home`/`AgentTrace`/`TasksView`/`Logs`/`RuntimesView`/`UsageView`);`menu.rs` = 6 行 `.cell` 菜单(套 `PhoneMore` 结构)。`app.rs` 的 Dashboard 臂改 form-factor swap(phone→`PhoneDashboard`,桌面→不变的 `DashboardRouter`)。`from_path` 已用 `starts_with("/dashboard")` 归类 → 零 `PanelMode`/sidebar/nav_menu 改动。

**Tech Stack:** Rust + Leptos/WASM(crate `aleph-panel`,目录 `interfaces/webchat`);`leptos_router` `use_location`/`use_navigate`;复用 `ios.css` 既有 `.list`/`.cell` 类。

## Global Constraints

- **Build policy(项目 cargo 节制)**:实现者**只转写计划里的完整代码 + 自审 + commit,绝不跑 `cargo build`/`cargo test`/`just`**。构建由控制器在每个 task 后统一跑一次 `just wasm`(退出 0 = `✓ WASM dist OK`)。
- **单测不可宿主运行**:crate `aleph-panel` 的 web-sys 未 gate → 宿主 `cargo test -p aleph-panel --lib` 不保证可编译。`screen_for_path` 是纯函数,其真值表由 reviewer 对照 spec §10 逐行核验(不靠 `cargo test`)。
- **桌面字节级不变**:**不碰** `DashboardRouter`、`PanelMode` 枚举、`components/mode_sidebar.rs`、`components/nav_menu.rs`。`app.rs` 的 Dashboard 臂仅**新增 phone 分支**,桌面分支保持原 `<DashboardRouter />`。
- **零 core / 零 IPC / 零依赖 / 零新 CSS**:全复用 `ios.css` 现有 `.list`/`.cell`/`.cell-leading`/`.cell-body`/`.cell-title`/`.cell-chevron`。
- **R4(I/O-only)**:菜单行只导航;叶子复用各自自携数据(app-wide context)的视图,本批次不在叶子内做业务逻辑。
- **标签用字面英文**(非 i18n),同 `PhoneMore`/`PhoneSettings`/`PhoneAgents` 既有手机约定。
- **PhoneShell 签名**:`title: &'static str`、`back: Option<&'static str>`、`back_label: Option<&'static str>`(均 `#[prop(optional)]`)→ 全部传字面量,**不加 `.to_string()`**。
- **Overview 走 phone-only `/dashboard/overview`**(桌面从不导航到它;手机 form-swap 后桌面 `DashboardRouter` 不运行)。
- **Agent Trace 告警徽标 v1 延后**:手机菜单保持静态,不镜像桌面 sidebar 的 `alert_key="agent.trace"`。
- **PhoneShell footgun**([[reference-leptos-phoneshell-dynamic-child-footgun]]):勿给 `PhoneShell` 传裸 `{move||…}` dynamic block 紧挨 static 兄弟。本计划每个分支的 `PhoneShell` children 都是**单个**元素/组件(menu 是单个 `<div class="list">`,叶子是单个视图组件)→ 天然规避。

---

### Task 1: `platform/phone/dashboard/` 模块(菜单 + 路由器 + 叶子挂载 + 单测)

**Files:**
- Create: `interfaces/webchat/src/platform/phone/dashboard/menu.rs`
- Create: `interfaces/webchat/src/platform/phone/dashboard/mod.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`(加 `pub mod dashboard;`)

**Interfaces:**
- Consumes:
  - `crate::platform::phone::shell::PhoneShell`(`title: &'static str`, `back`/`back_label: Option<&'static str>`, `children`)
  - `crate::views::home::Home` / `crate::views::agent_trace::AgentTrace` / `crate::views::tasks::TasksView` / `crate::views::logs::Logs` / `crate::views::runtimes::RuntimesView` / `crate::views::usage::UsageView`(均 `#[component] pub fn … -> impl IntoView`,无参)
  - `leptos_router::hooks::{use_location, use_navigate}`、`leptos_router::NavigateOptions`
- Produces(供 Task 2):
  - `crate::platform::phone::dashboard::PhoneDashboard`(`#[component] pub fn PhoneDashboard() -> impl IntoView`,无参)
  - `crate::platform::phone::dashboard::screen_for_path(&str) -> DashScreen`(`pub(crate)`)、`pub enum DashScreen { Menu, Overview, Trace, Tasks, Logs, Runtimes, Usage }`

- [ ] **Step 1: 创建 `menu.rs`(PhoneDashboardMenu — 6 行 `.cell`)**

Create `interfaces/webchat/src/platform/phone/dashboard/menu.rs`:

```rust
//! Phone Dashboard menu landing (`/dashboard`): a full-screen sections menu
//! whose rows mirror the desktop `DashboardSidebar` (Overview / Agent Trace /
//! Scheduled Tasks / Server Logs / Runtimes / Usage). Each row drills into a
//! full-screen leaf. Mirrors the `PhoneMore` landing structure. I/O-only (R4):
//! rows only navigate.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;

#[component]
#[must_use]
pub fn PhoneDashboardMenu() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="Dashboard">
            <div class="list">
                <div class="cell" on:click=go("/dashboard/overview")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path>
                            <polyline points="9 22 9 12 15 12 15 22"></polyline>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Overview"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/dashboard/trace")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12"></polyline>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Agent Trace"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/dashboard/tasks")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <circle cx="12" cy="12" r="10"></circle>
                            <polyline points="12 6 12 12 16 14"></polyline>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Scheduled Tasks"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/dashboard/logs")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
                            <polyline points="14 2 14 8 20 8"></polyline>
                            <line x1="16" y1="13" x2="8" y2="13"></line>
                            <line x1="16" y1="17" x2="8" y2="17"></line>
                            <polyline points="10 9 9 9 8 9"></polyline>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Server Logs"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/dashboard/runtimes")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
                            <line x1="8" y1="21" x2="16" y2="21"></line>
                            <line x1="12" y1="17" x2="12" y2="21"></line>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Runtimes"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/dashboard/usage")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M3 3v18h18"></path>
                            <path d="M18 17V9"></path>
                            <path d="M13 17V5"></path>
                            <path d="M8 17v-3"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Usage"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </PhoneShell>
    }
}
```

> 图标 SVG path 逐字取自 `components/dashboard_sidebar.rs` 的 6 个 `SidebarItem`(顺序一致)。SVG 子元素用 `></tag>` 显式闭合形式,与 sibling `more.rs` 一致。

- [ ] **Step 2: 创建 `mod.rs`(DashScreen + screen_for_path + 单测 + PhoneDashboard 路由器 + 叶子全屏挂载)**

Create `interfaces/webchat/src/platform/phone/dashboard/mod.rs`:

```rust
//! Native iPhone Dashboard screens. Mirrors the phone Chat/Memory/Agents
//! drill-down pattern: a menu landing (`/dashboard`) whose rows mirror the
//! desktop `DashboardSidebar`, each drilling into a full-screen leaf that
//! reuses the existing desktop view (Home / AgentTrace / TasksView / Logs /
//! RuntimesView / UsageView) mounted inside a `PhoneShell` with a back button.
//! Wide interaction on those dense views is deferred (Canvas precedent); this
//! batch only builds the no-split navigation chrome. I/O-only (R4): the menu
//! navigates; leaves reuse the views' own (app-wide context) data.

pub mod menu;

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::platform::phone::shell::PhoneShell;
use crate::views::agent_trace::AgentTrace;
use crate::views::home::Home;
use crate::views::logs::Logs;
use crate::views::runtimes::RuntimesView;
use crate::views::tasks::TasksView;
use crate::views::usage::UsageView;

use self::menu::PhoneDashboardMenu;

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

/// Map a `/dashboard…` path to its phone screen. Trailing slashes are
/// normalized; legacy/unknown sub-paths (`/dashboard/cron`, `/dashboard/memory`)
/// fall back to the menu since the phone doesn't surface them.
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
        _ => DashScreen::Menu,
    }
}

/// Phone Dashboard router. Pure path dispatch — no owned state, since each leaf
/// view carries its own data subscriptions from app-wide context. Renders the
/// menu at `/dashboard` or a full-screen leaf at `/dashboard/{leaf}`.
#[component]
#[must_use]
pub fn PhoneDashboard() -> impl IntoView {
    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        DashScreen::Menu => view! { <PhoneDashboardMenu/> }.into_any(),
        DashScreen::Overview => view! {
            <PhoneShell title="Overview" back="/dashboard" back_label="Dashboard">
                <Home/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Trace => view! {
            <PhoneShell title="Agent Trace" back="/dashboard" back_label="Dashboard">
                <AgentTrace/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Tasks => view! {
            <PhoneShell title="Scheduled Tasks" back="/dashboard" back_label="Dashboard">
                <TasksView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Logs => view! {
            <PhoneShell title="Server Logs" back="/dashboard" back_label="Dashboard">
                <Logs/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Runtimes => view! {
            <PhoneShell title="Runtimes" back="/dashboard" back_label="Dashboard">
                <RuntimesView/>
            </PhoneShell>
        }
        .into_any(),
        DashScreen::Usage => view! {
            <PhoneShell title="Usage" back="/dashboard" back_label="Dashboard">
                <UsageView/>
            </PhoneShell>
        }
        .into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_for_path_maps_all_leaves() {
        assert_eq!(screen_for_path("/dashboard"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/overview"), DashScreen::Overview);
        assert_eq!(screen_for_path("/dashboard/trace"), DashScreen::Trace);
        assert_eq!(screen_for_path("/dashboard/tasks"), DashScreen::Tasks);
        assert_eq!(screen_for_path("/dashboard/logs"), DashScreen::Logs);
        assert_eq!(screen_for_path("/dashboard/runtimes"), DashScreen::Runtimes);
        assert_eq!(screen_for_path("/dashboard/usage"), DashScreen::Usage);
    }

    #[test]
    fn screen_for_path_legacy_and_unknown_fall_back_to_menu() {
        assert_eq!(screen_for_path("/dashboard/cron"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/memory"), DashScreen::Menu);
        assert_eq!(screen_for_path("/dashboard/bogus"), DashScreen::Menu);
    }
}
```

- [ ] **Step 3: 注册模块**

Modify `interfaces/webchat/src/platform/phone/mod.rs` — 在 `pub mod chat;` 与 `pub mod memory;` 之间插入一行(保持字母序):

```rust
pub mod agents;
pub mod chat;
pub mod dashboard;
pub mod memory;
pub mod more;
pub mod settings;
pub mod shell;
```

- [ ] **Step 4: 自审(不跑 cargo — 遵守 Build policy)**

逐项核对(无需运行命令):
1. **screen_for_path 真值表**对照 spec §10:`/dashboard`→Menu、`/dashboard/`→Menu、6 个叶子各自映射、`/dashboard/cron|memory|bogus`→Menu。与 Step 2 的两个 `#[test]` 一致。
2. **PhoneShell 调用**:全部传字面量(`title="Overview"`、`back="/dashboard"`、`back_label="Dashboard"`),无 `.to_string()`。
3. **导入路径**:`crate::views::{home,agent_trace,tasks,logs,runtimes,usage}::*` 与 `app.rs` 现有 `DashboardRouter` 同源;`PhoneShell` 来自 `crate::platform::phone::shell`。
4. **footgun**:每个 `PhoneShell` 的 children 是单个元素(menu = 单 `<div class="list">`;叶子 = 单个视图组件),无裸 dynamic block 紧挨 static 兄弟。
5. **菜单图标/标签/导航**对照 spec §6 表逐行一致(6 行,顺序 Overview→Trace→Tasks→Logs→Runtimes→Usage)。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/dashboard/menu.rs \
        interfaces/webchat/src/platform/phone/dashboard/mod.rs \
        interfaces/webchat/src/platform/phone/mod.rs
git commit -m "panel: phone Dashboard menu + drill-down router (split-tab batch #3)"
```

> 控制器在 review 前跑一次 `just wasm`(预期 `✓ WASM dist OK`)。`PhoneDashboard` 此刻 pub-but-unused(Task 2 接线)→ 可能有 unused 警告,非错误。

---

### Task 2: `app.rs` 接线(import + Dashboard 臂 form-factor swap)

**Files:**
- Modify: `interfaces/webchat/src/app.rs`(phone imports 区 + `MainContent` 的 Dashboard 臂)

**Interfaces:**
- Consumes:
  - `crate::platform::phone::dashboard::PhoneDashboard`(Task 1 产出)
  - 已在 `MainContent` 作用域内:`form_factor: FormFactorState`(`app.rs:389`)、`FormFactor`(已 import,Chat 臂在用)、`PanelMode`、`DashboardRouter`(同文件 `#[component]`)
- Produces: 手机 `/dashboard` 渲染 `PhoneDashboard`;桌面渲染不变的 `DashboardRouter`。

- [ ] **Step 1: 加 import**

Modify `interfaces/webchat/src/app.rs` — 在 `use crate::platform::phone::chat::PhoneChat;`(line 36)之后插入(保持字母序):

```rust
use crate::platform::phone::dashboard::PhoneDashboard;
```

插入后该区应为:

```rust
use crate::platform::phone::agents::PhoneAgents;
use crate::platform::phone::chat::PhoneChat;
use crate::platform::phone::dashboard::PhoneDashboard;
use crate::platform::phone::memory::PhoneMemory;
use crate::platform::phone::more::PhoneMore;
```

- [ ] **Step 2: Dashboard 臂改 form-factor swap**

Modify `interfaces/webchat/src/app.rs` — 把 `MainContent` 里现有的 Dashboard 臂:

```rust
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            <DashboardRouter />
        </div>
```

替换为:

```rust
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneDashboard /> }.into_any()
            } else {
                view! { <DashboardRouter /> }.into_any()
            }}
        </div>
```

> 桌面分支仍是原 `<DashboardRouter />`(行为字节级不变);phone 分支走新 `PhoneDashboard`。`form_factor`/`FormFactor`/`PanelMode` 均已在作用域。

- [ ] **Step 3: 自审(不跑 cargo)**

1. 桌面分支 `view! { <DashboardRouter /> }.into_any()` 与原渲染等价;`DashboardRouter` 本体未改。
2. 模式同 Chat/Memory/Agents 臂的 form-factor swap 写法一致(`form_factor.form_factor.get() == FormFactor::Phone`)。
3. import 字母序正确,无重复。
4. 无其它臂被改动。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: wire PhoneDashboard into MainContent Dashboard arm"
```

> 控制器跑 `just wasm`(预期 `✓ WASM dist OK`,Task 1 的 unused 警告消失)。

---

## 完成后(控制器职责,非实现者)

- 最终全分支审查(opus):range = Task 1 父提交 .. HEAD。
- 重建 dist:`just wasm`(已在每 task 后跑;最终确认 dist 已含新模块字节)并 commit dist。
- **不推送、不部署、不跑 iOS-sim QA**(均 user-driven,spec §10 权威运行时门)。

## 测试总览

- **单测**:`screen_for_path` 真值表(Task 1 Step 2 的两个 `#[test]`),reviewer 对照 spec §10 核验(宿主 `cargo test` 不在 Build policy 内)。
- **iOS-sim QA(user-driven)**:见 spec §10 —— ••• → More → Dashboard → 菜单 6 行无分屏 → 点行进全屏叶子带 ‹ Dashboard 返回 → ••• tab 全程高亮。

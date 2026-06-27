# 手机 Teams 下钻屏 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 `/teams` 从桌面左右分屏重做成「原生菜单 → 全屏下钻」,套手机不分屏下钻法则(split-tab 批次 #4/4)。

**Architecture:** 新增 `platform/phone/teams/` 模块:`menu.rs`(`PhoneTeamsMenu` = 复用桌面 `TeamSelector` + 5 行 `.cell`)+ `mod.rs`(`TeamScreen` 枚举 + `screen_for_path` 纯函数 + `PhoneTeams` 路由器,自持 `TeamsTabState` 并复刻桌面 `TeamsView` 的 connect-gated 团队加载,按 `use_location().pathname` 分发菜单或全屏叶子)。app.rs 的 Teams 臂从裸 `<TeamsView/>` 改 form-factor swap。零 `PanelMode`/sidebar/nav_menu 改动(`from_path` 已 `starts_with("/teams")`)。

**Tech Stack:** Rust + Leptos/WASM(crate `aleph-panel`,`interfaces/webchat`);复用既有 `TeamsApi`、`DashboardState`、`TeamsTabState`、桌面 5 子视图 + `TeamSelector`。

## Global Constraints

[每个 task 的要求隐含包含本节。逐字遵守。]

- **Build policy = controller-only `just wasm`**:implementer 只转写计划代码 + 自审 + commit,**不跑任何 cargo/just/构建/测试**。controller 每 task 后跑 `just wasm`(绿 = "✓ WASM dist OK")再 review。host `cargo test -p aleph-panel --lib` 不保证可跑(web-sys 未 gate)→ `screen_for_path` 是纯函数,靠 reviewer 逐行核对真值表。
- **桌面字节级不变**:app.rs 桌面分支只把现有 `<TeamsView/>` 调用点包进 form-factor swap 的 else 臂,`<TeamsView/>` 调用本身 + `use crate::views::teams::TeamsView;`(app.rs:25)不删不改。
- **零 core/IPC、零新依赖、零新 CSS**:复用既有 ios.css 类(`.list`/`.cell`/`.cell-leading`/`.cell-body`/`.cell-title`/`.cell-chevron`)+ tailwind(`px-4`/`py-3`)。
- **R4(Interface 纯 I/O)**:菜单行只 `navigate`;团队加载复用既有 `TeamsApi::list`,不新写持久化/检索/规划逻辑。
- **英文字面标签**:菜单/叶子标题英文字面(同既有手机 More/Settings/Dashboard 约定),不接 i18n(future 债)。
- **PhoneShell 字面参数 + footgun**:`PhoneShell` 接 `&'static str` 字面(`title`/`back`/`back_label`),**不要** `.to_string()`。菜单的 `PhoneShell` children 必须是**单个静态 `<div>`** 包住(选择器 + 列表),避开 [[reference-leptos-phoneshell-dynamic-child-footgun]](勿传裸 `{move||}` 紧挨 static 兄弟)。
- **phone-only `/teams/*` 路径**:`/teams/{overview,kanban,plan,replay,workers}` 仅手机用;桌面 `TeamsView` 用内存信号切子视图、不读这些路径,form-swap 后桌面不挂载 `PhoneTeams`。
- **`PhoneTeams` 自持 `TeamsTabState`**:`mod.rs` 里复刻桌面 `TeamsView`(`platform/wide/views/teams/mod.rs:53-80`)的 state 创建 + `provide_context` + connect-gated load **逐字**;state 在 router 级 → 选择跨菜单↔叶子持久。无独立 Retry 按钮(镜像桌面 reconnect 自动重载语义)。
- **模块注册字母序**:`platform/phone/mod.rs` 的 `pub mod teams;` 放在 `pub mod shell;` 后(`t > s`);app.rs import 放在 `use crate::platform::phone::settings::PhoneSettings;`(app.rs:45)后。

---

## File Structure

| 文件 | 职责 |
|------|------|
| `interfaces/webchat/src/platform/phone/teams/menu.rs` | **新建** — `PhoneTeamsMenu`:`PhoneShell title="Teams"` + 单 `<div>`(`TeamSelector` + `.list` 5 行 `.cell`) |
| `interfaces/webchat/src/platform/phone/teams/mod.rs` | **新建** — `TeamScreen` + `screen_for_path` + `PhoneTeams`(自持 state + load + 6 路由臂) + 单测 + `pub mod menu;` |
| `interfaces/webchat/src/platform/phone/mod.rs` | 改 — 加 `pub mod teams;` |
| `interfaces/webchat/src/app.rs` | 改 — 加 `PhoneTeams` import + Teams 臂 form-factor swap |

---

## Task 1: `platform/phone/teams/` 模块(菜单 + 路由器 + 单测)+ 注册

**Files:**
- Create: `interfaces/webchat/src/platform/phone/teams/menu.rs`
- Create: `interfaces/webchat/src/platform/phone/teams/mod.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`(加 `pub mod teams;`)
- Test: 单测内嵌 `mod.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes(既有,均已验证 pub 可达):
  - `crate::platform::phone::shell::PhoneShell`(props:`title: &'static str`,`#[prop(optional)] back: Option<&'static str>`,`#[prop(optional)] back_label: Option<&'static str>`,`children`)
  - `crate::api::teams::TeamsApi`,方法 `TeamsApi::list(state: &DashboardState) -> Result<Vec<TeamSummary>, String>`
  - `crate::context::DashboardState`,字段 `is_connected: RwSignal<bool>`
  - `crate::views::teams::{TeamsTabState, TeamsSubTab}`(`TeamsTabState { sub_tab: RwSignal<TeamsSubTab>, teams: RwSignal<Vec<TeamSummary>>, selected_team_id: RwSignal<Option<String>> }`)
  - `crate::views::teams::{overview::OverviewView, kanban::KanbanView, plan_dag::PlanDagView, replay::ReplayView, workers::WorkersView}`
  - `crate::views::teams::components::team_selector::TeamSelector`
  - `leptos_router::hooks::{use_location, use_navigate}`,`leptos_router::NavigateOptions`,`leptos::task::spawn_local`
- Produces(供 Task 2):
  - `crate::platform::phone::teams::PhoneTeams`(`#[component] pub fn PhoneTeams() -> impl IntoView`)
  - `pub(crate) fn screen_for_path(path: &str) -> TeamScreen`、`pub enum TeamScreen`

- [ ] **Step 1: 新建 `menu.rs`(逐字转写)**

写 `interfaces/webchat/src/platform/phone/teams/menu.rs`:

```rust
//! Phone Teams menu landing (`/teams`): a full-screen sections menu whose rows
//! mirror the desktop `TeamsSidebar` (team selector + Overview / Kanban / Plan /
//! Replay / Workers). Each row drills into a full-screen leaf. Mirrors the
//! `PhoneDashboardMenu` structure. I/O-only (R4): rows only navigate; the team
//! selector reuses the desktop component reading `TeamsTabState` from context.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;
use crate::views::teams::components::team_selector::TeamSelector;

#[component]
#[must_use]
pub fn PhoneTeamsMenu() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="Teams">
            <div>
                <div class="px-4 py-3">
                    <TeamSelector/>
                </div>
                <div class="list">
                    <div class="cell" on:click=go("/teams/overview")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <line x1="8" y1="6" x2="21" y2="6"></line>
                                <line x1="8" y1="12" x2="21" y2="12"></line>
                                <line x1="8" y1="18" x2="21" y2="18"></line>
                                <line x1="3" y1="6" x2="3.01" y2="6"></line>
                                <line x1="3" y1="12" x2="3.01" y2="12"></line>
                                <line x1="3" y1="18" x2="3.01" y2="18"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Overview"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/kanban")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                                <line x1="9" y1="3" x2="9" y2="21"></line>
                                <line x1="15" y1="3" x2="15" y2="21"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Kanban"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/plan")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="18" cy="5" r="3"></circle>
                                <circle cx="6" cy="12" r="3"></circle>
                                <circle cx="18" cy="19" r="3"></circle>
                                <line x1="8.59" y1="13.51" x2="15.42" y2="17.49"></line>
                                <line x1="15.41" y1="6.51" x2="8.59" y2="10.49"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Plan"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/replay")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"></path>
                                <path d="M3 3v5h5"></path>
                                <path d="M12 7v5l4 2"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Replay"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/workers")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="4" y="4" width="16" height="16" rx="2"></rect>
                                <rect x="9" y="9" width="6" height="6"></rect>
                                <line x1="9" y1="2" x2="9" y2="4"></line>
                                <line x1="15" y1="2" x2="15" y2="4"></line>
                                <line x1="9" y1="20" x2="9" y2="22"></line>
                                <line x1="15" y1="20" x2="15" y2="22"></line>
                                <line x1="20" y1="9" x2="22" y2="9"></line>
                                <line x1="20" y1="14" x2="22" y2="14"></line>
                                <line x1="2" y1="9" x2="4" y2="9"></line>
                                <line x1="2" y1="14" x2="4" y2="14"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Workers"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                </div>
            </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 2: 新建 `mod.rs`(逐字转写,含单测)**

写 `interfaces/webchat/src/platform/phone/teams/mod.rs`:

```rust
//! Native iPhone Teams screens. Mirrors the phone Dashboard drill-down pattern:
//! a menu landing (`/teams`) whose rows mirror the desktop `TeamsSidebar`
//! (team selector + Overview / Kanban / Plan / Replay / Workers), each drilling
//! into a full-screen leaf that reuses the existing desktop sub-view mounted in
//! a `PhoneShell` with a back button. Wide interaction on the dense leaves
//! (Kanban board / Plan DAG / Replay split) is deferred (Canvas precedent);
//! this batch only builds the no-split navigation chrome.
//!
//! Unlike PhoneDashboard, this router OWNS `TeamsTabState` — the five leaves all
//! read `selected_team_id` from it — and mirrors the desktop `TeamsView`'s
//! connect-gated team-list load. State at the router level keeps the selection
//! alive across menu↔leaf navigation. I/O-only (R4): rows navigate; the load
//! reuses the existing `TeamsApi`.

pub mod menu;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::teams::{
    kanban::KanbanView, overview::OverviewView, plan_dag::PlanDagView, replay::ReplayView,
    workers::WorkersView, TeamsSubTab, TeamsTabState,
};

use self::menu::PhoneTeamsMenu;

/// Which phone Teams screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamScreen {
    Menu,
    Overview,
    Kanban,
    Plan,
    Replay,
    Workers,
}

/// Map a `/teams…` path to its phone screen. Trailing slashes are normalized;
/// the bare mode path and any unknown sub-path fall back to the menu.
#[must_use]
pub(crate) fn screen_for_path(path: &str) -> TeamScreen {
    match path.trim_end_matches('/') {
        "/teams" | "" => TeamScreen::Menu,
        "/teams/overview" => TeamScreen::Overview,
        "/teams/kanban" => TeamScreen::Kanban,
        "/teams/plan" => TeamScreen::Plan,
        "/teams/replay" => TeamScreen::Replay,
        "/teams/workers" => TeamScreen::Workers,
        _ => TeamScreen::Menu,
    }
}

/// Phone Teams router. Owns `TeamsTabState` (the five leaves read
/// `selected_team_id` from it) and mirrors the desktop `TeamsView` connect-gated
/// team-list load, then dispatches the menu (`/teams`) or a full-screen leaf
/// (`/teams/{leaf}`). State lives at the router so the selection survives
/// menu↔leaf navigation.
#[component]
#[must_use]
pub fn PhoneTeams() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let tab_state = TeamsTabState {
        sub_tab: RwSignal::new(TeamsSubTab::Overview),
        teams: RwSignal::new(Vec::new()),
        selected_team_id: RwSignal::new(None),
    };
    provide_context(tab_state);

    // Initial + reconnect load of the team list — verbatim from desktop TeamsView.
    Effect::new(move |_| {
        if dash.is_connected.get() {
            spawn_local(async move {
                if let Ok(list) = TeamsApi::list(&dash).await {
                    let keep = tab_state
                        .selected_team_id
                        .get_untracked()
                        .filter(|id| list.iter().any(|t| &t.id == id));
                    let new_sel = keep.or_else(|| list.first().map(|t| t.id.clone()));
                    tab_state.teams.set(list);
                    tab_state.selected_team_id.set(new_sel);
                }
            });
        } else {
            tab_state.teams.set(Vec::new());
            tab_state.selected_team_id.set(None);
        }
    });

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        TeamScreen::Menu => view! { <PhoneTeamsMenu/> }.into_any(),
        TeamScreen::Overview => view! {
            <PhoneShell title="Overview" back="/teams" back_label="Teams">
                <OverviewView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Kanban => view! {
            <PhoneShell title="Kanban" back="/teams" back_label="Teams">
                <KanbanView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Plan => view! {
            <PhoneShell title="Plan" back="/teams" back_label="Teams">
                <PlanDagView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Replay => view! {
            <PhoneShell title="Replay" back="/teams" back_label="Teams">
                <ReplayView/>
            </PhoneShell>
        }
        .into_any(),
        TeamScreen::Workers => view! {
            <PhoneShell title="Workers" back="/teams" back_label="Teams">
                <WorkersView/>
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
        assert_eq!(screen_for_path("/teams"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/overview"), TeamScreen::Overview);
        assert_eq!(screen_for_path("/teams/kanban"), TeamScreen::Kanban);
        assert_eq!(screen_for_path("/teams/plan"), TeamScreen::Plan);
        assert_eq!(screen_for_path("/teams/replay"), TeamScreen::Replay);
        assert_eq!(screen_for_path("/teams/workers"), TeamScreen::Workers);
    }

    #[test]
    fn screen_for_path_unknown_falls_back_to_menu() {
        assert_eq!(screen_for_path("/teams/bogus"), TeamScreen::Menu);
        assert_eq!(screen_for_path("/teams/overview/extra"), TeamScreen::Menu);
    }
}
```

- [ ] **Step 3: 注册模块 `platform/phone/mod.rs`**

在 `interfaces/webchat/src/platform/phone/mod.rs` 现有 `pub mod shell;` 行**后**加一行(字母序 `t > s`,是文件最后一个 `pub mod`):

```rust
pub mod shell;
pub mod teams;
```

(改后该 `pub mod` 块完整顺序:`agents` / `chat` / `dashboard` / `memory` / `more` / `settings` / `shell` / `teams`。)

- [ ] **Step 4: 自审(implementer,不构建)**

逐项核对:
- `menu.rs`:5 行 `.cell` 的 `on:click=go("/teams/…")` 路径与 `screen_for_path` 五个非 Menu 臂一一对应(overview/kanban/plan/replay/workers);children 是单个静态 `<div>` 包住选择器 + 列表(footgun 避开)。
- `mod.rs`:`PhoneTeams` 的状态创建 + `provide_context` + connect-gated `Effect` 与桌面 `TeamsView`(`platform/wide/views/teams/mod.rs:53-80`)逐字一致;6 个路由臂的 `PhoneShell` 用字面参数(无 `.to_string()`);叶子组件 import 路径正确。
- `screen_for_path` 真值表:`/teams`+`/teams/`→Menu、5 叶子各自、未知/超长→Menu。
- `pub mod teams;` 已加。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/teams/menu.rs \
        interfaces/webchat/src/platform/phone/teams/mod.rs \
        interfaces/webchat/src/platform/phone/mod.rs
git commit -m "panel: phone Teams menu + drill-down router (split-tab batch #4)"
```

> **Controller(非 implementer)**:本 task commit 后跑 `just wasm`,绿("✓ WASM dist OK")再 review;否则把编译错误回送 implementer 修。

---

## Task 2: app.rs 接线(import + Teams 臂 form-factor swap)

**Files:**
- Modify: `interfaces/webchat/src/app.rs`(加 import + 改 Teams 臂)

**Interfaces:**
- Consumes(Task 1 产出):`crate::platform::phone::teams::PhoneTeams`
- Consumes(既有,MainContent 作用域内):`form_factor`(`app.rs:390` `let form_factor = expect_context::<FormFactorState>();`)、`FormFactor`(`app.rs:50` import)、`PanelMode`、`TeamsView`(`app.rs:25` import)、`mode`
- Produces:无(终端接线)

- [ ] **Step 1: 加 import**

在 `interfaces/webchat/src/app.rs` 的 `use crate::platform::phone::settings::PhoneSettings;`(app.rs:45)**后**加一行(phone 段字母序,`teams > settings`):

```rust
use crate::platform::phone::settings::PhoneSettings;
use crate::platform::phone::teams::PhoneTeams;
```

- [ ] **Step 2: 改 Teams 臂为 form-factor swap**

把 `interfaces/webchat/src/app.rs:421-423` 现有 Teams 臂:

```rust
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            <TeamsView />
        </div>
```

改成(镜像同文件 Dashboard/Memory/Agents 臂的 swap 写法):

```rust
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneTeams /> }.into_any()
            } else {
                view! { <TeamsView /> }.into_any()
            }}
        </div>
```

> 桌面 else 臂保留原 `<TeamsView />` 调用;`use crate::views::teams::TeamsView;`(app.rs:25)不删。其余 mode 臂(Extensions/Settings/...)不动。

- [ ] **Step 3: 自审(implementer,不构建)**

- import 在 phone 段字母序正确位置(settings 后)。
- Teams 臂 phone→`PhoneTeams`、桌面→`TeamsView`,swap 写法与 Dashboard 臂(app.rs:400-406)逐字同构。
- 桌面分支字节不变(只新增 phone 臂 + 包裹)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: wire PhoneTeams into Teams arm (split-tab batch #4)"
```

> **Controller**:commit 后跑 `just wasm`,绿再 review。dist 重建(`just wasm` 产出的 `dist/aleph_panel.js` + `dist/aleph_panel_bg.wasm`)由 controller 在全部 task 完成、终审通过后单独一个 commit 提交(同 Dashboard:`cafd4d015`)。

---

## Self-Review(plan 作者自查)

**1. Spec coverage:**
- §4 路由(零枚举改 + phone-only 路径 + `screen_for_path`)→ Task 1 Step 2 ✅
- §5 `PhoneTeams` 自持 `TeamsTabState` + verbatim load → Task 1 Step 2 ✅
- §6 菜单(`TeamSelector` + 5 行)→ Task 1 Step 1 ✅
- §7 叶子全屏挂载 → Task 1 Step 2(6 路由臂)✅
- §8 app.rs 接线 → Task 2 ✅
- §9 变更清单 4 文件 → Task 1(3 文件)+ Task 2(1 文件)✅
- §10 单测 → Task 1 Step 2 `#[cfg(test)] mod tests` ✅
- 无遗漏需求。

**2. Placeholder scan:** 无 TBD/TODO/"similar to"/"handle edge cases";所有代码块完整逐字。✅

**3. Type consistency:**
- `TeamScreen` 五个非 Menu 变体(Overview/Kanban/Plan/Replay/Workers)= `screen_for_path` 五个路径臂 = `menu.rs` 五个 `go("/teams/…")` = `PhoneTeams` 五个 `PhoneShell` 叶子臂。四处一致。✅
- `screen_for_path: pub(crate) fn(&str) -> TeamScreen`、`PhoneTeams: pub fn() -> impl IntoView`(Task 1 Produces)= Task 2 Consumes。✅
- `TeamsTabState` 字段名(`sub_tab`/`teams`/`selected_team_id`)与桌面定义一致。✅

---

## Build / Verify(controller 总流程)

1. Task 1 完成 → `just wasm`(绿)→ review(`screen_for_path` 真值表逐行核对 + 菜单/路由臂一致性 + 桌面无触碰)。
2. Task 2 完成 → `just wasm`(绿)→ review(桌面字节不变 + swap 同构)。
3. 全 task 绿 → 终审(opus 全分支)→ 重建并提交 dist。
4. iOS-sim QA(权威运行时门,spec §10)+ push/部署 = **用户驱动**,不在本计划自动执行。

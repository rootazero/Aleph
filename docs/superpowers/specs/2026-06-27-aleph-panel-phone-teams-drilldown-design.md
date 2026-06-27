# 手机端 Teams 下钻屏设计 (Phone Teams Drill-down)

> 批次 #4/4 — 把桌面四个分屏/手机不可达的 tab(Agents/Teams/Dashboard/Extensions)套「手机不分屏下钻法则」。前序:Agents #1、More 入口 #2、Dashboard #3 全 done。本批次做 Teams,余 Extensions。

**Goal:** 把手机 `/teams`(经 #2 ••• More 可达)从「桌面左栏 `TeamsSidebar` + 内容」的左右分屏,重做成手机原生「菜单 → 全屏下钻」,闭合 #2 留的「先导航暂显桌面布局」过渡期。

**Status:** 设计已批准,准备写实现计划。

---

## §1. 背景:桌面 Teams 结构

桌面 Teams mode(`app.rs:421`)当前**无条件**渲染 `<TeamsView/>`(`platform/wide/views/teams/mod.rs`),手机端也照此渲染 → 256px 左栏 `TeamsSidebar` + 内容并排 = 手机宽度左右分屏。

`TeamsSidebar`(`mod.rs:98-136`)两段:
1. 顶部 `TeamSelector`(团队选择 pill + dropdown,`components/team_selector.rs`)— 选哪个团队。
2. 5 个**文字子标签**(`SubTabButton`,无图标):**Overview / Kanban / Plan / Replay / Workers**。

5 个叶子子视图(`mod.rs:84-90`,经**内存信号** `TeamsTabState.sub_tab` 切换,**非 URL 路由**):

| 子标签 | 组件 | 文件 | 手机宽度 |
|--------|------|------|----------|
| Overview | `OverviewView` | `overview.rs` | ✅ 单列卡片 |
| Kanban | `KanbanView` | `kanban.rs` | ❌ 5 列横向看板 |
| Plan | `PlanDagView` | `plan_dag.rs` | ❌ 1000px+ SVG DAG |
| Replay | `ReplayView` | `replay.rs` | ⚠️ 本身左列表+右详情分屏 |
| Workers | `WorkersView` | `workers.rs` | ✅ 单列卡片 |

全部 5 个子视图 scoped 到 `TeamsTabState.selected_team_id`(由 `TeamsView` 创建并 `provide_context`)。

`from_path`(`mode_sidebar.rs:45`)已 `starts_with("/teams")` 把所有 `/teams*` 归 `PanelMode::Teams`。

---

## §2. 手机不分屏下钻法则

- 底部 `PhoneTabBar` = 顶层模式切换;Teams 经第 5 个 ••• More tab 进入。
- 每模式手机**着陆页**(全屏)= 该模式桌面「左侧次级菜单」内容。
- 点菜单项 → 下钻全屏内容页,带 `‹` 返回回菜单。
- **绝不**在手机宽度并排两列(mode 级别)。

---

## §3. 范围与叶子策略(决策 A)

**叶子策略 = 复用桌面视图 + 全屏挂载,宽交互延后**(Canvas/Dashboard 先例,user 经 AskUserQuestion 选 A):

- 把现有 5 个桌面子视图原样挂进 `PhoneShell` 全屏 body,带 `‹ Teams` 返回。
- Overview/Workers 单列天然 OK。
- **Kanban/Plan 原样横向滚动**(看板/DAG 横向溢出 → 浏览器原生横滚)、**Replay 保留内部 list→detail 分屏**(叶子内分屏,过渡可接受)。
- 本批次**只做导航外壳**,不重写宽叶子。

**否决方案**:B(手机原生重写 6 个密集视图=工作量大)、C(Replay 单独拆真下钻=单叶子多一份工作量)。Replay 的叶子内分屏是已知延后债,非本批次目标。

---

## §4. 路由(零 `PanelMode`/sidebar/nav_menu 改动)

同 Dashboard:`from_path` 已 `starts_with("/teams")` → **无需碰枚举/`mode_sidebar`/`nav_menu`**。

手机引入 **phone-only 路径** `/teams/{overview,kanban,plan,replay,workers}`(桌面从不导航到它们;桌面 `TeamsView` 用内存信号切子视图,不读这些路径)。`screen_for_path(&str)->TeamScreen` 纯函数分发:`trim_end_matches('/')` 归一尾斜杠;`/teams`→`Menu`;未知/legacy 路径 → `_ => Menu` 兜底。

> **桌面字节不变保证**:桌面 `TeamsView` 不读 URL,phone-only 路径对它无意义;且 form-swap 后桌面 `TeamsView` 在手机不挂载、`PhoneTeams` 在桌面不挂载 → 互不影响。

---

## §5. 🔑 `PhoneTeams` 路由器自持 `TeamsTabState`(本批次唯一结构差异)

与 Dashboard 不同(Dashboard 叶子各自携 app-wide context,路由器无 state):**Teams 5 个子视图都依赖 `TeamsTabState.selected_team_id`**,而该 state 由桌面 `TeamsView` 创建+提供。手机 form-swap 后 `TeamsView` 不挂载 → `PhoneTeams` **必须自己复刻** `TeamsView` 的状态地基(`mod.rs:53-80` 逐字):

1. `expect_context::<DashboardState>()`(连接态,字段 `is_connected: RwSignal<bool>`)。
2. 建 `TeamsTabState { sub_tab, teams, selected_team_id }` 并 `provide_context`。
3. connect-gated `Effect`:`is_connected` 为真 → `spawn_local` 调 `TeamsApi::list(&dash)`,保留现选(仍存在)或兜底首个团队;断线则清空。**与桌面 reconnect 自动重载语义一致**,无独立 Retry 按钮(菜单/选择器不被 load 门控,5 行始终渲染;Effect 在重连时自动重跑)。

**state 在 router 级持有** → 跨「菜单 ↔ 叶子」导航时 `PhoneTeams` 组件不卸载(仅内层 `move||match` 重渲)→ `selected_team_id` 持久。= 比 Dashboard 重(自持 state)、比 Agents 轻(无 retry/无 tap-to-confirm)。

`PhoneTeams` 完整代码(`mod.rs`):

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

---

## §6. 菜单着陆页 `menu.rs`(`PhoneTeamsMenu`)

`PhoneShell title="Teams"`(无 back)。单个静态 `<div>` 子节点(避 PhoneShell dynamic-child footgun,镜像 Dashboard 单子节点)内含:
1. 顶部**复用桌面 `TeamSelector`**(读 `PhoneTeams` 提供的 `TeamsTabState` context,1:1 镜像桌面侧栏顶部团队选择)。
2. `.list` 5 行 `.cell` → navigate `/teams/{overview,kanban,plan,replay,workers}`。

> **图标说明**:桌面 Teams 子标签是**纯文字无图标**。为对齐既有手机 `.cell` 约定(More/Settings/Dashboard 菜单行都带 leading 图标),给每行配简单 inline SVG(phone-chosen,非镜像桌面;`width="17" height="17" viewBox="0 0 24 24" stroke-width="1.8"` 同 Dashboard)。标签文字英文字面(同既有手机约定,future i18n 债)。

`menu.rs` 完整代码:

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

---

## §7. 叶子(复用 5 桌面视图全屏挂载)

见 §5 `PhoneTeams` 的 `move||match`:每叶子 = `crate::views::teams::{overview::OverviewView, kanban::KanbanView, plan_dag::PlanDagView, replay::ReplayView, workers::WorkersView}` 原样挂进 `PhoneShell back="/teams" back_label="Teams"`。`TeamsTabState` 由 `PhoneTeams` 提供 → 叶子(descendant)`expect_context` 命中。Overview/Workers 单列;Kanban/Plan 横滚;Replay 内部分屏(延后)。

---

## §8. app.rs 接线

1. 加 import(phone 段字母序,在 `settings::PhoneSettings` 后):
   ```rust
   use crate::platform::phone::teams::PhoneTeams;
   ```
2. Teams 臂(当前 `app.rs:421-423` 裸 `<TeamsView/>`)改 form-factor swap(镜像 Dashboard/Memory/Agents 臂):
   ```rust
   <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
       {move || if form_factor.form_factor.get() == FormFactor::Phone {
           view! { <PhoneTeams /> }.into_any()
       } else {
           view! { <TeamsView /> }.into_any()
       }}
   </div>
   ```
   `form_factor`/`FormFactor`/`PanelMode`/`TeamsView` 均已在 MainContent 作用域(`app.rs:25` `use crate::views::teams::TeamsView;` 不删;`app.rs:390` `let form_factor = expect_context::<FormFactorState>();`)。桌面分支字节不变。
3. `platform/phone/mod.rs` 加 `pub mod teams;`(字母序,在 `pub mod shell;` 后,`t > s`)。

---

## §9. 变更清单

| 文件 | 动作 |
|------|------|
| `interfaces/webchat/src/platform/phone/teams/mod.rs` | **新建** — `TeamScreen` + `screen_for_path` + `PhoneTeams`(自持 `TeamsTabState` + connect-gated load + 6 路由臂)+ 单测 + `pub mod menu;` |
| `interfaces/webchat/src/platform/phone/teams/menu.rs` | **新建** — `PhoneTeamsMenu`(`TeamSelector` + 5 行 `.cell`) |
| `interfaces/webchat/src/platform/phone/mod.rs` | 改 — 加 `pub mod teams;` |
| `interfaces/webchat/src/app.rs` | 改 — 加 `PhoneTeams` import + Teams 臂 form-factor swap |

零 core/IPC、零新依赖、零新 CSS(复用 `.list`/`.cell`/`.cell-leading`/`.cell-body`/`.cell-title`/`.cell-chevron`/`px-4`/`py-3`)、桌面字节级不变、R4(纯 I/O,load 复用既有 `TeamsApi`)。

---

## §10. 测试

- **单测**:`screen_for_path` 真值表(`/teams`、`/teams/`、5 叶子、未知/超长路径兜底 Menu)。`screen_for_path` 是纯函数,reviewer 可逐行核对。
- **构建**:controller `just wasm`(绿即 dist OK)。implementer 只转写+自审+commit,不构建。host `cargo test -p aleph-panel --lib` 不保证可跑(web-sys 未 gate),故纯函数单测 + reviewer trace 为主。
- **iOS-sim QA(权威运行时门)**:按 [[feedback-ios-panel-test-via-full-macos-app]] 重编完整版 app 重嵌 dist → sim 连本地 core → 实测:
  1. ••• More → 点 Teams → 手机 Teams 菜单:顶部团队选择器 + 5 行,**无左右分屏**。
  2. 切换团队 → 选择持久。
  3. 点各行 → 全屏叶子带 `‹ Teams` 返回(Overview/Workers 单列正常;Kanban/Plan 横滚;Replay 内部分屏=已知延后)。
  4. 返回回菜单,选择仍在。
  5. ••• tab 全程高亮(`under_more()` 已含 Teams)。

---

## §11. 成功标准

- [ ] 手机 `/teams` 显原生菜单(团队选择器 + 5 行),无 mode 级左右分屏。
- [ ] 5 行各下钻全屏叶子,带 `‹ Teams` 返回回菜单。
- [ ] `selected_team_id` 跨菜单↔叶子导航持久。
- [ ] 桌面 Teams 字节级不变(form-swap 仅包裹 call site)。
- [ ] 零 `PanelMode`/`mode_sidebar`/`nav_menu` 改动。
- [ ] `screen_for_path` 单测全绿;`just wasm` 绿。
- [ ] 零 core/IPC/依赖/新 CSS,R4,PhoneShell footgun 避开。

---

## §12. 延后 / 关联

**延后(future `panel: cleanup`/i18n/phone-native pass)**:
- 宽叶子 phone-native 重写:Kanban 单列(一次一状态)、Plan 竖排树、**Replay 叶子内拆真下钻**(list→detail)。
- 菜单/叶子标签 i18n(现英文字面)。
- 菜单图标精修(现 phone-chosen 占位)。

**关联**:[[feedback-phone-no-split-drilldown-law]](法则)、[[project-aleph-panel-phone-dashboard-drilldown]](#3,最近模板)、[[project-aleph-panel-phone-more-entry]](#2,Teams 入口)、[[project-aleph-panel-phone-agents-drilldown]](#1,自持 state 先例)、[[feedback-ios-panel-test-via-full-macos-app]](运行时验证)、[[reference-leptos-phoneshell-dynamic-child-footgun]](footgun)。

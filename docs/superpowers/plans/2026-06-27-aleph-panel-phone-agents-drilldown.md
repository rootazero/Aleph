# Phone Agents 下钻 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 Agents tab 从「桌面侧栏+内容并排挤压（左右分屏）」改成「agent 列表着陆页 → 下钻单 agent 全屏详情（5 横滚 tab）」的原生下钻结构。

**Architecture:** 新增 `interfaces/webchat/src/platform/phone/agents/` 模块（`mod.rs` 路由器 + `list.rs` 着陆页 + `detail.rs` 详情），按 `use_location().pathname` 内部分发（无需注册路由）。详情屏复用桌面 5 个 tab 内容组件（`crate::views::agents::{overview,files,skills,channels,teams}::*Tab`），只换 PhoneShell 外壳 + 横滚 tab 条。`app.rs` 的 Agents 臂加 form-factor swap（Phone → `PhoneAgents`，否则现有 `AgentsRouter`），镜像 Chat/Memory/Settings。零 core/IPC、零依赖、桌面字节不变。

**Tech Stack:** Rust + Leptos（crate `aleph-panel`，WASM）；复用 `AgentsApi` / `WorkspaceApi` / `DashboardState`；iOS 组件类 `.cell` / `.chip` / `.field` / `.badge`（均已存在于 `styles/ios.css`）。

## Global Constraints

- **构建策略（项目 cargo 节制）**：implementer 只转写本计划的完整代码 + 自审 + commit，**不跑构建**。Controller 在每个 task 后跑 `just wasm`（WASM 编译是唯一真编译信号）作为门；纯函数单测可由 controller 用 `cargo test -p aleph-panel --lib platform::phone::agents` 跑一次（按节制裁量）。
- **桌面字节不变**：禁止修改任何 `platform/wide/` 文件、桌面 `AgentsView`、5 个 tab 组件、`agents_sidebar.rs`、`mode_sidebar.rs`、`nav_menu.rs`。详情屏**复用**这些组件，绝不改它们。
- **零新依赖**：不引入任何 crate（违 R3）。
- **零新 CSS 类**：只用已存在的 `.cell` / `.cell-leading` / `.cell-body` / `.cell-title` / `.cell-sub` / `.cell-chevron` / `.list` / `.list-header` / `.chip` / `.chip-active` / `.field` / `.badge` / `.badge-info` / `.badge-warning` / `.cc-hide-scroll`，其余用 inline style。`tailwind.css` / `ios.css` 不改。
- **PhoneShell footgun**：PhoneShell 的子节点必须是**单个元素**；混合 static + dynamic 兄弟必须包进一个 `<div>`（见 `reference-leptos-phoneshell-dynamic-child-footgun`）。
- **字面量文案，不加 i18n key**：phone 屏沿用字面量字符串（同 Memory phone 屏），不新增 i18n 键。
- **R4 复用**：不新增 core/IPC；只写表现层。
- **PhoneShell 签名**（`platform/phone/shell.rs`）：`PhoneShell(title: &'static str, #[prop(optional)] back: Option<&'static str>, #[prop(optional)] back_label: Option<&'static str>, children)`。
- **路由归类既成**：`PanelMode::from_path` 对 `starts_with("/agents")` 已判 Agents 模式；`/agents/{id}/{tab}` 自动归类，无需注册任何 `<Routes>`。

---

### Task 1: 着陆列表屏 `list.rs` + 路由状态 `PhoneAgentsState`

**Files:**
- Create: `interfaces/webchat/src/platform/phone/agents/mod.rs`
- Create: `interfaces/webchat/src/platform/phone/agents/list.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`（加 `pub mod agents;`）

**Interfaces:**
- Produces:
  - `pub struct PhoneAgentsState { agents: RwSignal<Vec<AgentSummary>>, bindings: RwSignal<HashMap<String, String>>, loaded: RwSignal<bool>, error: RwSignal<Option<String>>, reload_nonce: RwSignal<u32> }`（`Copy`，经 context 传递）—— Task 3 的路由器 `provide_context` 它、Task 3 的 loader 填它；Task 2 的 detail 也读它。
  - `pub fn PhoneAgentsList()` —— Task 3 路由器在 `/agents` 渲染它。
- Consumes:
  - `crate::api::agents::{AgentSummary, AgentsApi}`（`AgentsApi::create(&state, id:&str, name:Option<&str>, identity:Option<&AgentIdentity>, archetype:Option<&str>) -> Result<(),String>`）。
  - `crate::context::DashboardState`（`is_connected: RwSignal<bool>`）。
  - `crate::platform::phone::shell::PhoneShell`。

- [ ] **Step 1: 创建 `mod.rs` 基座（状态 + 模块声明）**

写 `interfaces/webchat/src/platform/phone/agents/mod.rs`：

```rust
//! Native iPhone Agents screens. Mirrors the phone Chat/Memory drill-down
//! pattern: a list landing (`/agents`) — filter + new-agent form + agent cells —
//! drilling into a full-screen single-agent detail (`/agents/{id}/{tab}`) with a
//! horizontally-scrollable 5-tab bar (Overview/Files/Skills/Channels/Teams).
//! Reuses the agents data layer + the desktop tab content components (R4); only
//! the navigation chrome is phone-specific.

pub mod list;

use std::collections::HashMap;

use leptos::prelude::*;

use crate::api::agents::AgentSummary;

/// Router-owned state for the phone Agents screens. Every field is an
/// `RwSignal` (Copy), so the struct is `Copy` and travels via context.
#[derive(Clone, Copy)]
pub struct PhoneAgentsState {
    /// All agents (one `agents.list` window).
    pub agents: RwSignal<Vec<AgentSummary>>,
    /// agent_id → channel_name bindings (for the channel badge + filter).
    pub bindings: RwSignal<HashMap<String, String>>,
    pub loaded: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// Bumping this re-triggers the agents loader (Retry / after create / set_default / delete).
    pub reload_nonce: RwSignal<u32>,
}
```

- [ ] **Step 2: 创建 `list.rs`（着陆页全部代码）**

写 `interfaces/webchat/src/platform/phone/agents/list.rs`：

```rust
//! Phone Agents list landing (`/agents`): mirrors the desktop `AgentsSidebar`
//! as a full-screen list — filter chips, an inline-expandable "+ New Agent"
//! form, and agent cells (emoji · name · channel badge · ★). Tapping a cell
//! drills into `/agents/{id}`. Reads the router-owned `PhoneAgentsState`;
//! reuses the agents data layer (R4).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::agents::{AgentSummary, AgentsApi};
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;

use super::PhoneAgentsState;

/// Filter chips: (label, filter key).
const FILTERS: [(&str, &str); 3] = [
    ("All", "all"),
    ("Channel", "channel"),
    ("Standalone", "standalone"),
];

#[component]
#[must_use]
pub fn PhoneAgentsList() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let st = expect_context::<PhoneAgentsState>();
    let navigate = use_navigate();

    // Filter + create-form state are list-local (ephemeral).
    let filter = RwSignal::new("all".to_string());
    let show_create = RwSignal::new(false);
    let new_id = RwSignal::new(String::new());
    let new_name = RwSignal::new(String::new());
    let new_archetype = RwSignal::new("assistant".to_string());
    let create_error = RwSignal::new(Option::<String>::None);

    // agents → filter (channel/standalone via the bindings map).
    let visible = move || {
        let binds = st.bindings.get();
        let f = filter.get();
        st.agents
            .get()
            .into_iter()
            .filter(|a| match f.as_str() {
                "channel" => binds.contains_key(&a.id),
                "standalone" => !binds.contains_key(&a.id),
                _ => true,
            })
            .collect::<Vec<AgentSummary>>()
    };

    let submit_create = move |_| {
        let id = new_id.get();
        if id.is_empty() {
            create_error.set(Some("Agent ID is required".to_string()));
            return;
        }
        create_error.set(None);
        let name_val = new_name.get();
        let name = (!name_val.is_empty()).then_some(name_val);
        let archetype = new_archetype.get();
        let dash = dashboard;
        spawn_local(async move {
            match AgentsApi::create(&dash, &id, name.as_deref(), None, Some(&archetype)).await {
                Ok(()) => {
                    show_create.set(false);
                    new_id.set(String::new());
                    new_name.set(String::new());
                    new_archetype.set("assistant".to_string());
                    st.reload_nonce.update(|n| *n += 1);
                }
                Err(e) => create_error.set(Some(e)),
            }
        });
    };

    view! {
        <PhoneShell title="Agents">
        // Single element child for PhoneShell (footgun).
        <div style="display:flex; flex-direction:column; gap:12px;">
            // ── Filter chips ──
            <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                {FILTERS.iter().map(|(label, key)| {
                    let key = *key;
                    view! {
                        <button
                            class="chip"
                            class:chip-active=move || filter.get() == key
                            style="flex:none;"
                            on:click=move |_| filter.set(key.to_string())
                        >
                            {*label}
                        </button>
                    }
                }).collect_view()}
            </div>

            // ── New Agent (inline-expandable) ──
            <div class="list">
                <div class="cell" on:click=move |_| show_create.update(|v| *v = !*v)>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">"New Agent"</div></div>
                </div>
                {move || show_create.get().then(|| view! {
                    <div style="display:flex; flex-direction:column; gap:8px; padding:12px;">
                        <input class="field" type="text" placeholder="Agent ID"
                            prop:value=move || new_id.get()
                            on:input=move |ev| new_id.set(event_target_value(&ev)) />
                        <input class="field" type="text" placeholder="Display name"
                            prop:value=move || new_name.get()
                            on:input=move |ev| new_name.set(event_target_value(&ev)) />
                        <select class="field"
                            prop:value=move || new_archetype.get()
                            on:change=move |ev| new_archetype.set(event_target_value(&ev))>
                            <option value="assistant">"Assistant"</option>
                            <option value="expert">"Expert"</option>
                            <option value="maker">"Maker"</option>
                            <option value="companion">"Companion"</option>
                        </select>
                        {move || create_error.get().map(|e| view! {
                            <div class="cell-sub" style="color:var(--color-danger);">{e}</div>
                        })}
                        <button class="chip" style="align-self:flex-start;" on:click=submit_create>"Create"</button>
                    </div>
                })}
            </div>

            // ── Agent list ──
            {move || {
                if !st.loaded.get() {
                    let label = if dashboard.is_connected.get() { "Loading…" } else { "Connecting…" };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = st.error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">"Couldn't load agents"</div><div class="cell-sub">{err}</div></div></div>
                            <div class="cell" on:click=move |_| st.reload_nonce.update(|n| *n += 1)>
                                <div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">"Retry"</div></div>
                            </div>
                        </div>
                    }.into_any();
                }
                let items = visible();
                if items.is_empty() {
                    return view! { <div class="list-header">"No agents"</div> }.into_any();
                }
                let binds = st.bindings.get();
                view! {
                    <div class="list">
                        {items.into_iter().map(|a| {
                            let navigate = navigate.clone();
                            let id_for_click = a.id.clone();
                            let emoji = a.emoji.clone().unwrap_or_default();
                            let name = a.name.clone().unwrap_or_else(|| a.id.clone());
                            let is_default = a.is_default;
                            let channel = binds.get(&a.id).cloned();
                            view! {
                                <div class="cell" on:click=move |_| navigate(&format!("/agents/{}", id_for_click), NavigateOptions::default())>
                                    <span class="cell-leading" style="font-size:18px;">{emoji}</span>
                                    <div class="cell-body"><div class="cell-title">{name}</div></div>
                                    {channel.map(|ch| view! { <span class="badge badge-info" style="flex:none;">{ch}</span> })}
                                    {is_default.then(|| view! { <span class="badge badge-warning" style="flex:none;">"★"</span> })}
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 3: 注册手机 agents 模块**

修改 `interfaces/webchat/src/platform/phone/mod.rs` —— 在模块声明区**最前**加 `pub mod agents;`（保持字母序）：

```rust
pub mod agents;
pub mod chat;
pub mod memory;
pub mod settings;
pub mod shell;
```

- [ ] **Step 4: 编译门（controller）**

Run: `just wasm`
Expected: 绿（成功生成 `interfaces/webchat/dist/aleph_panel_bg.wasm`）。`PhoneAgentsList` / `PhoneAgentsState` 此刻是未接线的 `pub` 项（被 Task 3 路由器接入），`pub` 项不触发 dead_code 警告。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/agents/mod.rs \
        interfaces/webchat/src/platform/phone/agents/list.rs \
        interfaces/webchat/src/platform/phone/mod.rs
git commit -m "panel: phone Agents list landing + router state"
```

---

### Task 2: 详情屏 `detail.rs` + tab 路由解析（纯函数 + 单测）

**Files:**
- Create: `interfaces/webchat/src/platform/phone/agents/detail.rs`
- Modify: `interfaces/webchat/src/platform/phone/agents/mod.rs`（加 `pub mod detail;`、`AgentDetailTab` 枚举、`DETAIL_TABS`、`parse_detail_path`、`#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes（来自 Task 1）：`PhoneAgentsState`（读 `agents` / `loaded`）。
- Consumes（桌面复用，禁止修改）：
  - `crate::views::agents::overview::OverviewTab(agent_id: String)`
  - `crate::views::agents::files::FilesTab(agent_id: String)`
  - `crate::views::agents::skills::SkillsTab(agent_id: String)`
  - `crate::views::agents::channels::ChannelsTab(agent_id: String)`
  - `crate::views::agents::teams::TeamsTab(agent_id: String)`
  - `crate::api::agents::AgentsApi`（`set_default(&state, id:&str)`、`delete(&state, id:&str)`，均 `-> Result<(),String>`）。
- Produces：
  - `pub(crate) fn parse_detail_path(path: &str) -> Option<(String, AgentDetailTab)>`、`pub enum AgentDetailTab { Overview, Files, Skills, Channels, Teams }`、`pub(crate) const DETAIL_TABS: [AgentDetailTab; 5]` —— Task 3 不直接用（仅 detail 用），但 enum/parse 是 Task 3 测试模块的邻居。
  - `pub fn PhoneAgentDetail()` —— Task 3 路由器在 `/agents/{id}/…` 渲染它。

- [ ] **Step 1: 写失败的单测（先红）**

在 `mod.rs` 末尾追加测试模块（此刻 `parse_detail_path` / `AgentDetailTab` 尚未定义 → 编译失败 = 红）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_detail_path_extracts_id_and_tab() {
        assert_eq!(parse_detail_path("/agents"), None);
        assert_eq!(parse_detail_path("/agents/"), None);
        assert_eq!(
            parse_detail_path("/agents/zoe"),
            Some(("zoe".to_string(), AgentDetailTab::Overview))
        );
        assert_eq!(
            parse_detail_path("/agents/zoe/skills"),
            Some(("zoe".to_string(), AgentDetailTab::Skills))
        );
        assert_eq!(
            parse_detail_path("/agents/zoe/bogus"),
            Some(("zoe".to_string(), AgentDetailTab::Overview))
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib platform::phone::agents`
Expected: FAIL —— `cannot find function parse_detail_path` / `cannot find type AgentDetailTab`。

- [ ] **Step 3: 在 `mod.rs` 加 `pub mod detail;` + tab 枚举 + 解析纯函数**

在 `mod.rs` 顶部模块声明改为：

```rust
pub mod detail;
pub mod list;
```

在 `PhoneAgentsState` 定义**之后**、`#[cfg(test)]` 之前插入：

```rust
/// The five per-agent detail tabs, mirroring the desktop `AgentsView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentDetailTab {
    Overview,
    Files,
    Skills,
    Channels,
    Teams,
}

impl AgentDetailTab {
    #[must_use]
    pub(crate) fn from_slug(s: &str) -> Self {
        match s {
            "files" => Self::Files,
            "skills" => Self::Skills,
            "channels" => Self::Channels,
            "teams" => Self::Teams,
            _ => Self::Overview,
        }
    }

    #[must_use]
    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Files => "files",
            Self::Skills => "skills",
            Self::Channels => "channels",
            Self::Teams => "teams",
        }
    }

    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Files => "Files",
            Self::Skills => "Skills",
            Self::Channels => "Channels",
            Self::Teams => "Teams",
        }
    }
}

pub(crate) const DETAIL_TABS: [AgentDetailTab; 5] = [
    AgentDetailTab::Overview,
    AgentDetailTab::Files,
    AgentDetailTab::Skills,
    AgentDetailTab::Channels,
    AgentDetailTab::Teams,
];

/// Parse `/agents/{id}` or `/agents/{id}/{tab}` → `(id, tab)`. Returns `None`
/// when no non-empty agent id is present (e.g. the bare `/agents` menu path).
#[must_use]
pub(crate) fn parse_detail_path(path: &str) -> Option<(String, AgentDetailTab)> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["agents", id, tab, ..] if !id.is_empty() => {
            Some(((*id).to_string(), AgentDetailTab::from_slug(tab)))
        }
        ["agents", id] if !id.is_empty() => Some(((*id).to_string(), AgentDetailTab::Overview)),
        _ => None,
    }
}
```

- [ ] **Step 4: 运行测试确认通过（绿）**

Run: `cargo test -p aleph-panel --lib platform::phone::agents`
Expected: PASS（`parse_detail_path_extracts_id_and_tab` 通过）。

- [ ] **Step 5: 创建 `detail.rs`（详情屏全部代码）**

写 `interfaces/webchat/src/platform/phone/agents/detail.rs`：

```rust
//! Phone single-agent detail (`/agents/{id}/{tab}`): a full-screen drill from
//! the Agents list. Reuses the desktop tab content components
//! (Overview/Files/Skills/Channels/Teams) under phone chrome — a header
//! (emoji · name · ★ · Set-default · Delete) and a horizontally-scrollable tab
//! bar. Reads the router-owned `PhoneAgentsState` to resolve the current agent.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::NavigateOptions;

use crate::api::agents::AgentsApi;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::agents::channels::ChannelsTab;
use crate::views::agents::files::FilesTab;
use crate::views::agents::overview::OverviewTab;
use crate::views::agents::skills::SkillsTab;
use crate::views::agents::teams::TeamsTab;

use super::{parse_detail_path, AgentDetailTab, PhoneAgentsState, DETAIL_TABS};

#[component]
#[must_use]
pub fn PhoneAgentDetail() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let st = expect_context::<PhoneAgentsState>();
    let location = use_location();
    let navigate = use_navigate();

    // (agent_id, active_tab) from the URL.
    let parsed = Memo::new(move |_| parse_detail_path(&location.pathname.get()));

    view! {
        <PhoneShell title="Agent" back="/agents" back_label="Agents">
        <div style="display:flex; flex-direction:column; gap:14px;">
            {move || {
                let Some((agent_id, tab)) = parsed.get() else {
                    return view! { <div class="list-header">"No agent selected"</div> }.into_any();
                };
                // Resolve the summary (emoji/name/default) from the loaded list.
                let Some(agent) = st.agents.get().into_iter().find(|a| a.id == agent_id) else {
                    let label = if st.loaded.get() { "Agent not found" } else { "Loading…" };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                };
                let emoji = agent.emoji.clone().unwrap_or_default();
                let name = agent.name.clone().unwrap_or_else(|| agent.id.clone());
                let is_default = agent.is_default;

                // Header actions. `delete_agent` is always rendered (disabled
                // when default); `set_default` is built only in the non-default
                // branch below to avoid an unused binding.
                let delete_agent = {
                    let id = agent_id.clone();
                    let navigate = navigate.clone();
                    move |_| {
                        let id = id.clone();
                        let navigate = navigate.clone();
                        let dash = dashboard;
                        spawn_local(async move {
                            if AgentsApi::delete(&dash, &id).await.is_ok() {
                                st.reload_nonce.update(|n| *n += 1);
                                navigate("/agents", NavigateOptions::default());
                            }
                        });
                    }
                };

                let id_for_tabs = agent_id.clone();
                let tab_content = match tab {
                    AgentDetailTab::Overview => view! { <OverviewTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Files => view! { <FilesTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Skills => view! { <SkillsTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Channels => view! { <ChannelsTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Teams => view! { <TeamsTab agent_id=agent_id.clone() /> }.into_any(),
                };

                // Default badge (when default) OR a "Set default" button (else).
                let default_action = if is_default {
                    view! { <span class="badge badge-warning" style="flex:none;">"★ Default"</span> }.into_any()
                } else {
                    let set_default = {
                        let id = agent_id.clone();
                        move |_| {
                            let id = id.clone();
                            let dash = dashboard;
                            spawn_local(async move {
                                if AgentsApi::set_default(&dash, &id).await.is_ok() {
                                    st.reload_nonce.update(|n| *n += 1);
                                }
                            });
                        }
                    };
                    view! { <button class="chip" style="flex:none;" on:click=set_default>"Set default"</button> }.into_any()
                };

                view! {
                    <div style="display:flex; flex-direction:column; gap:14px;">
                        // Header
                        <div style="display:flex; align-items:center; gap:10px;">
                            <span style="font-size:26px;">{emoji}</span>
                            <div style="flex:1; min-width:0;">
                                <div style="font-size:19px; font-weight:700; color:var(--color-text-primary); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">{name}</div>
                            </div>
                            {default_action}
                            <button
                                class="chip"
                                style=if is_default { "flex:none; opacity:0.4; pointer-events:none;" } else { "flex:none; color:var(--color-danger);" }
                                on:click=delete_agent
                            >"Delete"</button>
                        </div>

                        // Tab bar (horizontal scroll)
                        <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                            {DETAIL_TABS.iter().map(|t| {
                                let t = *t;
                                let navigate = navigate.clone();
                                let id = id_for_tabs.clone();
                                let is_active = t == tab;
                                view! {
                                    <button
                                        class="chip"
                                        class:chip-active=move || is_active
                                        style="flex:none;"
                                        on:click=move |_| navigate(&format!("/agents/{}/{}", id, t.slug()), NavigateOptions::default())
                                    >
                                        {t.label()}
                                    </button>
                                }
                            }).collect_view()}
                        </div>

                        // Tab content (reused desktop component)
                        <div>{tab_content}</div>
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 6: 编译门（controller）**

Run: `just wasm`
Expected: 绿。`PhoneAgentDetail` 是未接线 `pub` 项（Task 3 接入）。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/phone/agents/mod.rs \
        interfaces/webchat/src/platform/phone/agents/detail.rs
git commit -m "panel: phone Agent detail (reused tab components) + path parse"
```

---

### Task 3: 路由器 `PhoneAgents` + 分发 + 单测 + app.rs 接线（功能贯通）

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/agents/mod.rs`（加 `AgentsScreen` + `screen_for_path` + 测试一条、`PhoneAgents` 路由器组件 + loader）
- Modify: `interfaces/webchat/src/app.rs`（import `PhoneAgents` + Agents 臂 form-factor swap）

**Interfaces:**
- Consumes：`PhoneAgentsState`（Task 1）、`PhoneAgentsList`（Task 1）、`PhoneAgentDetail`（Task 2）、`crate::api::agents::AgentsApi::list`、`crate::api::workspace::WorkspaceApi::agent_bindings(&state) -> Result<HashMap<String,String>,String>`、`crate::context::DashboardState`。
- Produces：`pub fn PhoneAgents()`（app.rs 在 `FormFactor::Phone` 时渲染）。

- [ ] **Step 1: 写失败的单测（先红）**

在 `mod.rs` 的 `#[cfg(test)] mod tests` 内**追加**一条测试（此刻 `screen_for_path` / `AgentsScreen` 未定义 → 红）：

```rust
    #[test]
    fn screen_for_path_menu_only_for_bare_agents() {
        assert_eq!(screen_for_path("/agents"), AgentsScreen::Menu);
        assert_eq!(screen_for_path("/agents/"), AgentsScreen::Menu);
        assert_eq!(screen_for_path("/agents/abc"), AgentsScreen::Detail);
        assert_eq!(screen_for_path("/agents/abc/files"), AgentsScreen::Detail);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p aleph-panel --lib platform::phone::agents`
Expected: FAIL —— `cannot find value AgentsScreen` / `cannot find function screen_for_path`。

- [ ] **Step 3: 在 `mod.rs` 加 `screen_for_path` + `AgentsScreen`**

在 `parse_detail_path` 定义**之前**（即 tab 枚举区附近）插入：

```rust
/// Which phone Agents screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsScreen {
    Menu,
    Detail,
}

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> AgentsScreen {
    if path == "/agents" || path == "/agents/" {
        AgentsScreen::Menu
    } else {
        AgentsScreen::Detail
    }
}
```

- [ ] **Step 4: 运行测试确认通过（绿）**

Run: `cargo test -p aleph-panel --lib platform::phone::agents`
Expected: PASS（`parse_detail_path_extracts_id_and_tab` + `screen_for_path_menu_only_for_bare_agents` 两条均过）。

- [ ] **Step 5: 在 `mod.rs` 加路由器组件 + loader**

先把 `mod.rs` 顶部的 `use` 区补全（合并已有 import；最终顶部应为）：

```rust
pub mod detail;
pub mod list;

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::{AgentSummary, AgentsApi};
use crate::api::workspace::WorkspaceApi;
use crate::context::DashboardState;

use self::detail::PhoneAgentDetail;
use self::list::PhoneAgentsList;
```

然后在 `PhoneAgentsState` 定义**之后**插入路由器组件：

```rust
/// Phone Agents router. Owns `PhoneAgentsState`, connect-gated-loads the agent
/// list + bindings, and renders the list at `/agents` or the detail at
/// `/agents/{id}/…`. Request/response only (no streaming subscription).
#[component]
#[must_use]
pub fn PhoneAgents() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();

    let st = PhoneAgentsState {
        agents: RwSignal::new(Vec::new()),
        bindings: RwSignal::new(HashMap::new()),
        loaded: RwSignal::new(false),
        error: RwSignal::new(None),
        reload_nonce: RwSignal::new(0),
    };
    provide_context(st);

    // Agents loader — connect-gated (cold-boot lesson). Re-runs when
    // `reload_nonce` is bumped (Retry, or after create/set_default/delete).
    Effect::new(move || {
        st.reload_nonce.get();
        if dashboard.is_connected.get() {
            spawn_local(async move {
                st.loaded.set(false);
                st.error.set(None);
                match AgentsApi::list(&dashboard).await {
                    Ok(resp) => {
                        st.agents.set(resp.agents);
                        // Bindings are best-effort: a failure leaves the badge/
                        // filter empty but never blocks the list.
                        if let Ok(map) = WorkspaceApi::agent_bindings(&dashboard).await {
                            st.bindings.set(map);
                        }
                    }
                    Err(e) => st.error.set(Some(e)),
                }
                st.loaded.set(true);
            });
        } else {
            st.agents.set(Vec::new());
            st.loaded.set(false);
        }
    });

    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        AgentsScreen::Detail => view! { <PhoneAgentDetail/> }.into_any(),
        AgentsScreen::Menu => view! { <PhoneAgentsList/> }.into_any(),
    }
}
```

注意：原 Task 1 的 `mod.rs` 顶部只有 `use crate::api::agents::AgentSummary;` 与少量 import；本步把它替换为上面补全后的 `use` 区（新增 `AgentsApi` / `WorkspaceApi` / `DashboardState` / `spawn_local` / `use_location` / `HashMap` / `self::detail` / `self::list`）。

- [ ] **Step 6: 接线 `app.rs`（form-factor swap）**

在 `app.rs` 的 phone import 区（约 35–42 行，紧邻 `use crate::platform::phone::chat::PhoneChat;`）加：

```rust
use crate::platform::phone::agents::PhoneAgents;
```

把 `MainContent` 里 Agents 那一臂（约 407–409 行）：

```rust
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
```

改为：

```rust
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneAgents /> }.into_any()
            } else {
                view! { <AgentsRouter /> }.into_any()
            }}
        </div>
```

（`form_factor` 已在 `MainContent` 顶部 `let form_factor = expect_context::<FormFactorState>();` 取到，见 app.rs:387；`AgentsRouter` 保持不变继续给桌面用。）

- [ ] **Step 7: 编译门 + 测试（controller）**

Run: `just wasm`
Expected: 绿（dist 重新生成）。
Run: `cargo test -p aleph-panel --lib platform::phone::agents`
Expected: PASS（两条路由单测）。

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/platform/phone/agents/mod.rs \
        interfaces/webchat/src/app.rs
git commit -m "panel: wire phone Agents router + form-factor swap"
```

---

## 收尾（controller，非本计划任务步骤）

- 全部 task 绿后，重建并提交 dist：`just wasm` → `git add interfaces/webchat/dist && git commit -m "panel: rebuild dist with phone Agents drill-down"`（镜像 Memory 收尾）。
- **iOS-sim QA（权威运行时门，spec §9）**：按 `feedback-ios-panel-test-via-full-macos-app` 重编完整版 macOS app（重嵌 dist 到 :18790）→ sim 连本地 core → 实测：Agents tab 落在列表（非分屏）→ 点 agent 下钻详情 + `‹ Agents` 返回 → 5 tab 可滚 → 新建/设默认可用 → 全程无左右分屏。
- push / 部署：用户驱动。

## 已知次要项（延后，spec §8）

- 列表 `filter` 为屏内局部状态：从详情返回后重置为 "All"（list 重挂）。可接受 v1。
- 详情复用的桌面 tab 组件可能带桌面 padding/宽度假设：本轮先全屏挂载可用；窄宽精修（`max-sm:` 兜底、tab 条吸顶、Files 全屏编辑态）作 follow-up。
- `PhoneShell title` 为 `&'static str`，详情标题用静态 "Agent"，agent 名显示在 body 头部行。

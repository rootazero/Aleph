# Aleph Panel 移动端重型界面适配 (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph Panel 剩余重型界面(导航外壳 / Settings / Teams / 星系画布)适配到 `<640px` 手机,延续 Phase 0.5 骨架,纯 Leptos/WASM + CSS,无需 Mac。

**Architecture:** 四个工作流,按依赖排序——**① 导航外壳**(可复用 `MobileTopBar` + 通用抽屉 hamburger + 铃铛归位)gate **② Settings**(iOS 分组列表 landing + 1 列表单)与 **③ Teams**(只读为主);**④ Canvas**(双指缩放 / 底部 sheet / WebGL2 回退 / 性能信号)独立,可与 ①②③ 并行。最大化复用既有组件与状态(R2/R3/KISS),改动外科化,只加 `max-sm:` 分支,**不动桌面 ≥640px 渲染**。

**Tech Stack:** crate `aleph-panel`(`interfaces/webchat`)· Leptos 0.7/0.8(WASM)· Tailwind v4.2.2(`max-sm:` = `<640px`)· 目标 `wasm32-unknown-unknown` · 状态 `ViewportState{is_mobile,drawer_open}` / `MemoryState{memory_view:MemoryView}` / `TeamsTabState` · WebGL2(`canvas/gl/`)。

## Global Constraints

> 每个 task 的要求都隐含包含本节。逐条 verbatim:

- **纯 WASM/CSS,零新依赖**;守红线 R2(UI 唯一源)/ R3(核心轻量化, 复用优先)/ KISS;外科改动,每改一行可追溯到本计划。
- **只做移动端 reflow**:全部以 `max-sm:`(`<640px`,`MOBILE_BREAKPOINT_PX = 640.0`)或 `ViewportState.is_mobile` 分支;**严禁改变桌面 ≥640px 渲染**。
- **§11 钉死接口为权威**:`MobileTopBar(title: Signal<String>, #[prop(optional)] left/right: Option<Children>)` · `NotificationBell`(state 留 root)· `/settings`·`/teams` 根 + 显式 `navigate` 返回 · `TeamsTabState.task_status_filter: RwSignal<Option<TaskStatus>>` · `GalaxyCanvas { fallback: RwSignal<bool> }` · `bloom_level: RwSignal<f32>`。跨 task 名称/类型必须一致。
- **Cargo 极度节制**(用户硬约束):纯逻辑用**定向** `cargo test -p aleph-panel <test_name>`(RED→GREEN);UI/CSS 任务的门 = **每个 task 组一次** `cargo check --target wasm32-unknown-unknown -p aleph-panel` + chrome-devtools 390px 人工核验。**严禁** `cargo build` / 全量 `cargo test` / 每微步 check。
- **rust_embed STALE-EMBED**:Panel 经编译期嵌入 `aleph-server`;改动要在**运行中** server 看到效果须 `just wasm` + 重编 server。**per-task 门 = 编译 + 浏览器 QA**;每个工作流末尾做一次重编 + 真机/浏览器 390px 实测。验证 dist 嵌入用 served wasm size / `grep -a`,不用 `strings`。
- **提交规范**:English `panel: <description>`,**无** 归属 / `Co-Authored-By`;单分支直接在 main;**只 commit,push 由用户决定**。
- **测试现实**:本仓 Leptos/WASM **无 DOM 单测框架**——UI 组件不写假 jsdom 测试,验证靠编译 + 浏览器;只有纯 Rust 逻辑(pinch 数学 / `compute_depths` / status 过滤 / SETTINGS_GROUPS 数据映射)写真 `#[test]`。

## File Structure（全 Phase 概览）

**新建(4)**:
- `interfaces/webchat/src/components/mobile_top_bar.rs` — 可复用 3 槽 `MobileTopBar`(①)
- `interfaces/webchat/src/components/notification_bell.rs` — 从 `notification_center.rs` 抽出的铃铛 trigger(①)
- `interfaces/webchat/src/views/settings/mobile_landing.rs` — iOS 分组列表 landing(②)
- `interfaces/webchat/src/views/teams/components/status_filter.rs` — Kanban 状态筛选(③)

**改动**:`components/{mod,notification_center,mobile_tab_bar}.rs` · `views/chat/view.rs` · `app.rs` · `styles/tailwind.css`(①);`views/settings/{mod + 审计出的全部页}.rs`(②);`views/teams/{mod,kanban,plan_dag,replay}.rs` + `components/{board,task_drawer}.rs`(③);`views/canvas/{mod,galaxy_canvas}.rs` + `gl/{scene,context}.rs` + `views/memory_hub/mod.rs` + `state/memory.rs`(④);`locales/{en,zh}.json`(②③④ 新串)。

**依赖与排序**:
```
① 导航外壳 ──gate──▶ ② Settings（分组列表需 ‹返回 + 顶栏）
              └─gate──▶ ③ Teams（入口靠抽屉 hamburger）
④ Canvas（独立，可并行）
```
建议执行序:**① → ②/③(可并行) → ④**(或 ④ 全程并行)。每个工作流自带末尾 browser-QA task。

## Self-Review（writing-plans 自检结果）

- **Spec coverage**:spec §2→工作流①(T①.1–①.4)、§3→②(T②.0–②.4 + QA)、§4→③(T③.1–③.6)、§5→④(T④.1–④.6);§11 七项 pinned 全部落到对应 task(P-①②→①、P-③→②.3、P-④→③.2、P-⑤⑥⑦→④)。无遗漏。
- **Placeholder scan**:无 TODO/TBD/"add error handling"/"similar to Task N";唯一"同上"是显式"套用上一行同款 `grid-responsive`"(代码已给),非隐藏占位。
- **Type consistency**:`MobileTopBar`/`NotificationBell`(①定义→②消费)、`task_status_filter`/`compute_depths`(③)、`fallback`/`bloom_level`/`MAX_SETTLE_STEPS`/`MemoryView::Table`(④)跨 task 名称类型一致。
- **审计实跑**:T②.0 实际执行 grep,发现 spec 种子集漏 5 处网格 → reflow 目标由 ~13 扩到实测 **27 个**,已固化为 T②.2 硬清单。

---
## 工作流 ① — 导航外壳 (MobileTopBar + 通用抽屉入口 + 铃铛归位)

**File Structure (this workstream):**
- Create: `interfaces/webchat/src/components/mobile_top_bar.rs` — reusable 3-slot `MobileTopBar` (§11 P-①); auto-hamburger writes `viewport.drawer_open`, auto-`NotificationBell` in right slot, zero agent dependency.
- Create: `interfaces/webchat/src/components/notification_bell.rs` — `NotificationBell` trigger sub-component (§11 P-②); reads root `NotificationsState`, renders only the button (badge + open toggle), state stays at root.
- Modify: `interfaces/webchat/src/components/mod.rs` — register `pub mod mobile_top_bar;` + `pub mod notification_bell;`.
- Modify: `interfaces/webchat/src/components/notification_center.rs:66-94` — replace the inline bell `<button>` block with `<NotificationBell />`; popover/sheet + `NotificationsState` unchanged.
- Modify: `interfaces/webchat/src/views/chat/view.rs:234-260` — swap the inline Phase-0.5 pill top-bar for `<MobileTopBar left=… title=… />` (left = agent pill that opens drawer, title = agent name, right defaults to bell).
- Modify: `interfaces/webchat/src/app.rs:264` + `:397-424` — add `max-sm:hidden` to the floating `<NotificationCenter />` bell (kill mobile double-render) and mount `<MobileTopBar />` once for non-chat tabs inside `MainContent` (title = `label_of(mode)`, default hamburger + bell).
- Modify: `interfaces/webchat/styles/tailwind.css` (after `.aleph-content-top`, ~line 1987) — add `.mobile-top-bar` class (safe-area top padding + unified z-band).
- Test: `interfaces/webchat/src/components/notification_bell.rs` (compile + browser QA — no unit test); all other steps compile + 390px browser QA.

---

### Task ①.1: `MobileTopBar` component + `.mobile-top-bar` CSS

**Files:**
- Create: `interfaces/webchat/src/components/mobile_top_bar.rs`
- Modify: `interfaces/webchat/src/components/mod.rs:17-18` (insert `pub mod mobile_top_bar;` alphabetically)
- Modify: `interfaces/webchat/styles/tailwind.css` (insert after `.aleph-content-top { … }` block ending line 1987)
- Test: compile + browser QA — no unit test (pure Leptos view + CSS)

**Interfaces:**
- Consumes: `crate::state::viewport::ViewportState { drawer_open: RwSignal<bool> }` (`state/viewport.rs:22-28`); `crate::components::notification_bell::NotificationBell` (Task ①.2 — referenced but the bell sub-component lands first if you prefer; here we forward-reference, compile gate is at end of ①.2).
- Produces: `#[component] pub fn MobileTopBar(title: Signal<String>, #[prop(optional)] left: Option<Children>, #[prop(optional)] right: Option<Children>) -> impl IntoView` (§11 P-①) consumed by Tasks ①.3 and ①.4.

> Sequencing note: do Task ①.2 (NotificationBell) BEFORE the final compile gate of this group, since `MobileTopBar`'s default right slot references `NotificationBell`. Steps below write the file; the group's single `cargo check --target wasm32` runs at the end of ①.2.

- [ ] **Step 1: Create the component file.** Write `interfaces/webchat/src/components/mobile_top_bar.rs`:
```rust
//! `MobileTopBar` — the reusable mobile chrome band mounted atop every tab.
//!
//! Three slots: `left` (defaults to a hamburger that opens the nav drawer),
//! `title` (always a plain `String` signal rendered centered — the component
//! has ZERO agent / `MemoryState` dependency, so any tab can mount it), and
//! `right` (defaults to the `NotificationBell` trigger). Chat overrides `left`
//! with its agent pill; other tabs pass `title = label_of(mode)` and leave
//! `left` / `right` unset. Safe-area + z-band live in the `.mobile-top-bar`
//! design-system class so no tab re-derives them.

use crate::components::notification_bell::NotificationBell;
use crate::state::viewport::ViewportState;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn MobileTopBar(
    /// Center title — a plain string signal, no agent context required.
    title: Signal<String>,
    /// Left slot. `None` → auto hamburger that opens the nav drawer.
    #[prop(optional)] left: Option<Children>,
    /// Right slot. `None` → auto `NotificationBell` trigger.
    #[prop(optional)] right: Option<Children>,
) -> impl IntoView {
    let drawer_open = expect_context::<ViewportState>().drawer_open;

    let left_slot = match left {
        Some(children) => children().into_any(),
        None => view! {
            <button
                type="button"
                class="aleph-no-drag flex h-8 w-8 items-center justify-center \
                       rounded-full text-text-secondary hover:text-text-primary \
                       hover:bg-surface-raised transition-colors"
                on:click=move |_| drawer_open.set(true)
                aria-label="Open navigation"
            >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="1.8"
                     stroke-linecap="round" stroke-linejoin="round">
                    <line x1="3" y1="6" x2="21" y2="6" />
                    <line x1="3" y1="12" x2="21" y2="12" />
                    <line x1="3" y1="18" x2="21" y2="18" />
                </svg>
            </button>
        }
        .into_any(),
    };

    let right_slot = match right {
        Some(children) => children().into_any(),
        None => view! { <NotificationBell /> }.into_any(),
    };

    view! {
        <div class="mobile-top-bar hidden max-sm:flex items-center justify-between \
                    px-3 pb-2">
            {left_slot}
            <span class="flex-1 min-w-0 text-center text-sm font-semibold \
                         text-text-primary truncate px-2">
                {move || title.get()}
            </span>
            {right_slot}
        </div>
    }
}
```

- [ ] **Step 2: Register the module.** In `interfaces/webchat/src/components/mod.rs`, add `pub mod mobile_top_bar;` between line 17 (`pub mod mobile_tab_bar;`) and line 18 (`pub mod mode_sidebar;`):
```rust
pub mod mobile_tab_bar;
pub mod mobile_top_bar;
pub mod mode_sidebar;
```

- [ ] **Step 3: Add the `.mobile-top-bar` CSS class.** In `interfaces/webchat/styles/tailwind.css`, immediately after the `.aleph-content-top { padding-top: var(--aleph-content-top); }` block (ends line 1987), insert:
```css
/* Mobile top chrome band — the reusable `MobileTopBar` mounted atop every
   tab (`components/mobile_top_bar.rs`). One safe-area-aware band so each tab
   inherits notch clearance + a single z-layer instead of re-deriving them.
   Sits ABOVE tab content but BELOW the nav-drawer backdrop (z-65) / drawer
   (z-70), so opening the drawer covers it; the bottom tab bar is z-40, this
   band z-45 — the two never overlap (top vs bottom). env() resolves to 0 in
   the browser, so the web preview just hugs the top edge. */
.mobile-top-bar {
    position: relative;
    z-index: 45;
    padding-top: calc(var(--safe-area-top) + 0.5rem);
}
```

- [ ] **Step 4: (deferred) compile gate** — do NOT run `cargo check` yet; `MobileTopBar` references `NotificationBell`, which lands in Task ①.2. The group's single wasm compile runs at the end of ①.2.

- [ ] **Step 5: Commit**
  `git add interfaces/webchat/src/components/mobile_top_bar.rs interfaces/webchat/src/components/mod.rs interfaces/webchat/styles/tailwind.css`
  `git commit -m "panel: add reusable MobileTopBar chrome + .mobile-top-bar class"`

---

### Task ①.2: Extract `NotificationBell` trigger sub-component

**Files:**
- Create: `interfaces/webchat/src/components/notification_bell.rs`
- Modify: `interfaces/webchat/src/components/mod.rs` (add `pub mod notification_bell;`)
- Modify: `interfaces/webchat/src/components/notification_center.rs:66-94`
- Test: compile + browser QA — no unit test (the existing `contract_documented` sentinel in `notification_center.rs:351` already documents bell-visible ⇔ `has_connected_once` and badge ⇔ `unread_count`; we preserve it).

**Interfaces:**
- Consumes: `crate::context::DashboardState { alerts, pending_approvals, has_connected_once }`; `crate::state::notifications::{NotificationsState, unread_count}` (`notification_center.rs:13-22`).
- Produces: `#[component] pub fn NotificationBell() -> impl IntoView` — renders ONLY the bell button (visibility gate + badge + `is_open` toggle). Consumed by `MobileTopBar` (default right slot, Task ①.1) and re-used by `NotificationCenter` (Task ①.2 Step 3).

- [ ] **Step 1: Create the bell sub-component.** Write `interfaces/webchat/src/components/notification_bell.rs` — this is the exact button block lifted verbatim from `notification_center.rs:67-94`, with state read the same way (root `NotificationsState` + `DashboardState`). The popover/sheet stay in `notification_center.rs`:
```rust
//! `NotificationBell` — the bell *trigger* button, split out of
//! `notification_center.rs` (§11 P-②) so `MobileTopBar` can mount it in its
//! right slot. Reads the SAME root-provided `NotificationsState` +
//! `DashboardState` the popover does; toggles `NotificationsState::is_open`.
//! The popover/sheet + dismissed-set logic STAY at the root in
//! `NotificationCenter` — only the button moved, so there is no lifecycle
//! change (R-2).

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::notifications::{unread_count, NotificationsState};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn NotificationBell() -> impl IntoView {
    let dashboard = use_context::<DashboardState>().expect("DashboardState not provided");
    let notif = use_context::<NotificationsState>().expect("NotificationsState not provided");

    let alerts = dashboard.alerts;
    let pending_approvals = dashboard.pending_approvals;
    let is_open = notif.is_open;
    let dismissed = notif.dismissed;

    // Hide the bell until we've ever connected — otherwise first boot shows a
    // stray icon over the BootCheckGate spinner.
    let bell_visible = Memo::new(move |_| dashboard.has_connected_once.get());

    let badge_count = Memo::new(move |_| {
        let a = alerts.get();
        let d = dismissed.get();
        unread_count(&a, &d) + pending_approvals.get().len()
    });

    view! {
        <Show when=move || bell_visible.get() fallback=|| ()>
            <button
                type="button"
                class="aleph-no-drag relative flex items-center justify-center \
                       h-8 w-8 rounded-full text-text-secondary hover:text-text-primary \
                       hover:bg-surface-raised transition-colors"
                data-tauri-drag-region="false"
                on:click=move |_| is_open.update(|v| *v = !*v)
                aria-label=move || t_string!(use_i18n(), notifications.open_label).to_string()
                title=move || t_string!(use_i18n(), notifications.open_label).to_string()
            >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="1.8"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9" />
                    <path d="M13.73 21a2 2 0 0 1-3.46 0" />
                </svg>
                <Show when=move || { badge_count.get() > 0 } fallback=|| ()>
                    <span class="absolute -top-0.5 -right-0.5 min-w-[16px] h-[16px] \
                                 px-1 rounded-full bg-danger text-white text-[10px] \
                                 font-semibold flex items-center justify-center \
                                 border border-surface">
                        {move || badge_count.get().min(99).to_string()}
                    </span>
                </Show>
            </button>
        </Show>
    }
}
```

- [ ] **Step 2: Register the module.** In `interfaces/webchat/src/components/mod.rs`, add `pub mod notification_bell;` between line 20 (`pub mod nav_menu;`) and line 21 (`pub mod notification_center;`):
```rust
pub mod nav_menu;
pub mod notification_bell;
pub mod notification_center;
```

- [ ] **Step 3: Replace the inline bell in `NotificationCenter` with `<NotificationBell />`.** In `interfaces/webchat/src/components/notification_center.rs`, delete the entire `<Show when=move || bell_visible.get() …>` button block (lines 66-94, the first `<Show>` in the `view!`) and replace it with a single `<NotificationBell />`. After the edit the `view!` opens with:
```rust
    view! {
        <NotificationBell />

        <Show when=move || is_open.get() fallback=|| ()>
```
  Then remove the now-unused `bell_visible` memo and `badge_count` memo plus their now-orphaned imports. Specifically:
  - Delete `notification_center.rs:35-46` (the `bell_visible` memo comment+let and the `badge_count` memo) — they moved into `NotificationBell`.
  - Change the import line `notification_center.rs:17-19` from
    `use crate::state::notifications::{ unread_count, visible_alerts, NotificationsState, PendingApprovalView, };`
    to drop `unread_count` (now only used by the bell):
```rust
use crate::components::notification_bell::NotificationBell;
use crate::state::notifications::{visible_alerts, NotificationsState, PendingApprovalView};
```
  - Keep `alerts`, `pending_approvals`, `is_open`, `dismissed`, and the `list` memo — the popover still uses them. Leave the `contract_documented` test (`:340-355`) intact.

- [ ] **Step 4: Group compile gate (the single wasm check for ①.1 + ①.2).** Run:
```
cargo check --target wasm32-unknown-unknown -p aleph-panel
```
  Expect: clean compile. This is the ONE `cargo check --target wasm32` for this task group — do not run it per micro-step. (Resolves: `MobileTopBar` finds `NotificationBell`; `NotificationCenter` finds `NotificationBell`; no dead `unread_count`/`bell_visible`/`badge_count`.)

- [ ] **Step 5: Commit**
  `git add interfaces/webchat/src/components/notification_bell.rs interfaces/webchat/src/components/mod.rs interfaces/webchat/src/components/notification_center.rs`
  `git commit -m "panel: extract NotificationBell trigger from NotificationCenter"`

---

### Task ①.3: Refactor chat top-bar to use `MobileTopBar` (agent pill preserved)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/view.rs:234-260` (replace the inline Phase-0.5 mobile pill bar) + add the `MobileTopBar` import near `:12-21`.
- Test: compile + browser QA — no unit test (Leptos view change). Browser QA at 390px: chat tab still shows the agent emoji+name pill top-left; tapping it opens the drawer; bell appears top-right; behavior identical to Phase 0.5.

**Interfaces:**
- Consumes: `MobileTopBar` (§11 P-①, Task ①.1); existing `mobile_agent: Memo<(String, String)>` (`chat/view.rs:43-58`) and `viewport.drawer_open` (`:42`).
- Produces: nothing new — same visible behavior, now via the shared component.

- [ ] **Step 1: Add the import.** In `interfaces/webchat/src/views/chat/view.rs`, add to the `use crate::components::…` cluster (after `use crate::components::workspace_panel::WorkspacePanel;` at line 16):
```rust
use crate::components::mobile_top_bar::MobileTopBar;
```

- [ ] **Step 2: Replace the inline mobile pill bar with `MobileTopBar`.** In `chat/view.rs`, replace the block at lines 234-260 (the comment `// Mobile-only top bar — …` through the closing `</div>` of the `<div class="hidden max-sm:flex absolute …">`) with a `MobileTopBar` whose `left` slot is the agent pill (the pill itself opens the drawer) and whose `title` is the agent name. The pill keeps the exact Phase-0.5 visuals; `right` is left default (auto bell):
```rust
                    // Mobile-only top bar (Phase 1: shared MobileTopBar).
                    // `left` = the active-agent pill that opens the nav drawer
                    // (agent switch + sessions live in the reused ModeSidebar);
                    // `title` = the agent name; `right` defaults to the bell.
                    // Desktop hides this (`.mobile-top-bar` is `max-sm:` only):
                    // the left sidebar already owns agent + session switching.
                    <div class="absolute inset-x-0 top-0 z-20">
                        <MobileTopBar
                            title=Signal::derive(move || mobile_agent.get().1)
                            left=Box::new(move || view! {
                                <button
                                    type="button"
                                    class="aleph-no-drag flex items-center gap-1.5 \
                                           max-w-[64%] pl-1.5 pr-2.5 py-1 rounded-full glass \
                                           bg-surface-overlay/80 border border-border text-sm \
                                           text-text-primary"
                                    on:click=move |_| viewport.drawer_open.set(true)
                                    aria-label="Switch agent"
                                >
                                    <span class="flex h-6 w-6 items-center justify-center \
                                                 rounded-full bg-primary/15 text-primary text-xs \
                                                 flex-shrink-0">
                                        {move || mobile_agent.get().0}
                                    </span>
                                    <span class="truncate font-medium">
                                        {move || mobile_agent.get().1}
                                    </span>
                                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                                         stroke="currentColor" stroke-width="2.5"
                                         stroke-linecap="round" stroke-linejoin="round"
                                         class="flex-shrink-0 text-text-tertiary">
                                        <polyline points="6 9 12 15 18 9" />
                                    </svg>
                                </button>
                            }.into_any())
                        />
                    </div>
```
  Note: the chat pill carries its own `title` text already, so `MobileTopBar`'s centered `title` duplicates the name — that is intentional per §11 P-① (center is always `title`); the pill sits in `left`, the centered title is the same agent name. If the duplication reads poorly at 390px QA, pass `title=Signal::derive(|| String::new())` to blank the center (keep this as a QA-decision toggle, not a guess).

- [ ] **Step 3: Group compile gate.** Run the single wasm check for ①.3 + ①.4 AFTER Task ①.4's edits (they share `app.rs`/imports). Do not check here — see ①.4 Step 4.

- [ ] **Step 4: Commit**
  `git add interfaces/webchat/src/views/chat/view.rs`
  `git commit -m "panel: chat top-bar uses shared MobileTopBar (pill in left slot)"`

---

### Task ①.4: Mount `MobileTopBar` on non-chat tabs + de-dup the floating bell

**Files:**
- Modify: `interfaces/webchat/src/app.rs:397-424` (`MainContent`) — mount one `MobileTopBar` for the non-chat tabs, `title = label_of(mode)`, default hamburger + bell.
- Modify: `interfaces/webchat/src/app.rs:264` — gate the floating `<NotificationCenter />` bell `max-sm:hidden` so the bell does not double-render on mobile (it now lives in each MobileTopBar's right slot).
- Modify: `interfaces/webchat/src/components/notification_center.rs` — the bell button needs to be hidden on mobile *only when rendered from the root NotificationCenter* (the popover/sheet must still mount). Achieved via app.rs wrapper, see Step 2.
- Test: compile + browser QA — no unit test. Browser QA at 390px on Memory / Agents / Settings landing / Dashboard / Teams / Extensions tabs: each shows a hamburger top-left + tab title centered + bell top-right; tapping hamburger opens the drawer; exactly ONE bell on screen.

**Interfaces:**
- Consumes: `MobileTopBar` (§11 P-①); `label_of(mode, i18n)` (`nav_menu.rs:44`); `use_i18n` (`crate::i18n`); `PanelMode` + `mode` memo already in `MainContent` (`app.rs:399`).
- Produces: every non-chat tab gains a drawer-opening hamburger (closes Phase-1 gap a) + consistent bell.

- [ ] **Step 1: Mount one `MobileTopBar` for non-chat tabs in `MainContent`.** In `interfaces/webchat/src/app.rs`, edit `MainContent` (`:397-424`). Add imports at the top of `app.rs` (in the `use crate::components::…` cluster, after line 30):
```rust
use crate::components::mobile_top_bar::MobileTopBar;
use crate::components::nav_menu::label_of;
use crate::i18n::use_i18n;
```
  (`use_i18n` may already be imported via `crate::i18n::{t_string, use_i18n, …}` at line 1 — if so, do NOT re-add it; only add the two `components::` lines.) Then change `MainContent`'s body so a single mode-driven top bar floats over the non-chat tabs. The Chat `<div>` keeps its own bar (Task ①.3); the non-chat surfaces get the shared one. Replace the `view!` of `MainContent` (`:401-423`) with:
```rust
    let i18n = use_i18n();
    // Non-chat tabs share one mode-driven MobileTopBar (hamburger opens the
    // drawer → closes Phase-1 gap a; title = label_of(mode); right defaults to
    // the bell). Chat owns its own bar (agent pill) in chat/view.rs. Hidden
    // when Chat is active so it never stacks over the chat pill bar.
    let non_chat = Memo::new(move |_| mode.get() != PanelMode::Chat);
    let title = Signal::derive(move || label_of(mode.get(), i18n));

    view! {
        <Show when=move || non_chat.get()>
            <div class="absolute inset-x-0 top-0 z-20">
                <MobileTopBar title=title />
            </div>
        </Show>
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            <ChatView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            <DashboardRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <MemoryHub />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            <TeamsView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
            <ExtensionsView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Settings { "block" } else { "none" }>
            <SettingsRouter />
        </div>
    }
```
  (`mode` already exists at `app.rs:399`; `MainContent` is the right host because it is the single `relative`-ancestor child of `<main>` and is mode-aware, so we add the bar once instead of editing 7 separate tab-host files — surgical + KISS.)

- [ ] **Step 2: Gate the floating root bell `max-sm:hidden`.** The root `<NotificationCenter />` (`app.rs:264`) renders the bell `fixed right-3 z-[50]` globally — on mobile that now collides with the MobileTopBar bell. We hide ONLY the floating bell button on mobile while keeping the popover/sheet (which `NotificationCenter` still owns). Since the bell button now lives in `NotificationBell` (Task ①.2) and `NotificationCenter` renders `<NotificationBell />` at its top, wrap that one render site so it is desktop-only. In `interfaces/webchat/src/components/notification_center.rs`, change the `<NotificationBell />` line (added in ①.2 Step 3) to:
```rust
        // Floating bell — desktop only. On mobile the bell lives in each tab's
        // MobileTopBar right slot (components/mobile_top_bar.rs); rendering it
        // here too would double up. The popover/sheet below stays at root for
        // both form factors.
        <div class="contents max-sm:hidden"><NotificationBell /></div>
```
  This keeps the desktop `fixed right-3 z-[50]` bell exactly as before (the `.aleph-no-drag relative …` button inside `NotificationBell` is wrapped; on desktop `contents` is transparent so positioning is unaffected — but note: the OLD button carried `aleph-chrome-top fixed right-3 z-[50]` which `NotificationBell` dropped). **Reconcile the desktop position:** add those positioning classes back onto the desktop wrapper instead of the button, so desktop chrome is unchanged:
```rust
        <div class="aleph-chrome-top fixed right-3 z-[50] max-sm:hidden"><NotificationBell /></div>
```
  Rationale: `NotificationBell`'s button is `relative` (good for the MobileTopBar slot); the desktop wrapper re-supplies the window-anchored `fixed right-3 z-[50]` + `aleph-chrome-top` that the original inline bell had (`notification_center.rs:70-73`). The bell's own `-top-0.5 -right-0.5` badge stays correct because the button is still its badge's positioned ancestor.

- [ ] **Step 3: Verify z-band coherence (no code — inspection).** Confirm the documented band: bottom tab bar `z-40` (`mobile_tab_bar.rs:32`) / MobileTopBar `.mobile-top-bar z-45` (Task ①.1 CSS) + its absolute wrapper `z-20` within `<main>` / drawer backdrop `z-[65]` (`app.rs:224`) / drawer `z-[70]` (`mode_sidebar.rs:79`). Top bar (top) and tab bar (bottom) never overlap; backdrop+drawer correctly cover the top bar when open. Desktop bell `z-[50]` is `max-sm:hidden` so it no longer competes on mobile.

- [ ] **Step 4: Group compile gate (single wasm check for ①.3 + ①.4).** Run:
```
cargo check --target wasm32-unknown-unknown -p aleph-panel
```
  Expect: clean compile (no unused `label_of`/`use_i18n`, no missing `MobileTopBar`). This is the ONE wasm check covering both the chat refactor and the non-chat mount.

- [ ] **Step 5: Workstream browser QA (end-of-① gate, after `just wasm` + server rebuild).** Per spec §9 / R-15 (rust_embed stale-embed), run `just wasm` then rebuild the server so the running daemon serves the new WASM. Then chrome-devtools at 390px:
  - Each of Chat / Memory / Agents / Settings / Dashboard / Teams / Extensions shows exactly one top bar with a working drawer trigger (pill on Chat, hamburger elsewhere) and exactly one bell.
  - Tapping hamburger on Memory/Agents/Settings/Dashboard/Teams/Extensions opens the `ModeSidebar` drawer; selecting Teams/Dashboard/Extensions from the drawer navigates and auto-closes the drawer (`mode_sidebar.rs:71-74`).
  - Bell badge + popover/sheet still work (open from the top-bar bell on mobile, from the fixed bell on a ≥640px window).
  - No bell double-render at 390px; no z-index overlap glitch when the drawer is open (backdrop dims the top bar).

- [ ] **Step 6: Commit**
  `git add interfaces/webchat/src/app.rs interfaces/webchat/src/components/notification_center.rs`
  `git commit -m "panel: mount MobileTopBar on non-chat tabs and de-dup mobile bell"`


---

## 工作流 ② — Settings 分组列表 landing + 1 列表单

> 依赖工作流 ① (`MobileTopBar` + ‹back via `navigate`)。crate = `aleph-panel` (lib `aleph_panel`)。所有 cargo 命令用 `-p aleph-panel`。
> **R-6 已核验解除**：`.aleph-content-top` (`styles/tailwind.css:1985-1987`) 只设 `padding-top: var(--aleph-content-top)`，**无横向 padding** → 与 `px-8 max-sm:px-4` 不叠加冲突。该结论已折进 T②.2，无需运行时再验。

**File Structure (this workstream):**
- Create: `interfaces/webchat/src/views/settings/mobile_landing.rs` — 手机端 iOS 分组列表 landing，遍历 `SETTINGS_GROUPS` 渲染 cell。
- Create (优先复用方案): `.grid-responsive` 工具类 in `interfaces/webchat/styles/tailwind.css` — 统一 `grid-cols-1 sm:grid-cols-2`，降同步税。
- Modify: `interfaces/webchat/src/views/settings/mod.rs:1-22`(注册 module) + `:124`(Quick Setup 容器加 `max-sm:hidden` + 同级挂载 `MobileSettingsLanding`).
- Modify (reflow, T②.2): `appearance.rs:44` / `general.rs:68` / `route.rs:102,239` / `execution.rs:98` / `behavior.rs:40` / `browser.rs:87` / `memory.rs:66,178,249,381,561,709` / `mcp.rs:81` / `plugins.rs:68` / `skills.rs:192` / `policies.rs:147` / `security/mod.rs:158` / `security/sandbox.rs:46,104` / `reranking_providers/add_custom.rs:225` / `reranking_providers/detail_panel.rs:280` / `channels/overview.rs:67` / `channels/discord.rs:105` / `channels/platform_page.rs:64,304`.
- Modify (T②.3 ‹back): `channels/platform_page.rs` header — 新增 `navigate("/settings")` 返回；确认 `/settings` landing 路由 (`app.rs:460`，已存在，零改).
- Modify (T②.4 optional i18n): `components/settings_sidebar.rs:86,103,108,109,204` + `locales/en.json` + `locales/zh.json`.
- Test: `settings_sidebar.rs` `#[cfg(test)]` — 纯 Rust 单测验 landing 数据源 (group 数 / tab 计数 / 路径完整)。reflow 与 landing view = compile gate + browser QA（repo 无 DOM 测试）。

---

### Task ②.0: 全量 grep 审计 — 枚举所有 settings 网格 / padding / max-w（spec §3.3b T0）

**Files:**
- Test: `compile + audit record — no unit test`（这是审计任务，产出是下方已跑出的真实清单）

**Interfaces:**
- Consumes: 无（独立审计）。
- Produces: 经验证的 reflow 目标全集，T②.2 据此逐项打勾。**审计已实跑**，结果固化于本任务清单——发现 spec §3.3b 种子集**漏了 5 处网格**（`memory.rs:709`、`reranking_providers/add_custom.rs:225`、`reranking_providers/detail_panel.rs:280`、`security/sandbox.rs:46`、`security/sandbox.rs:104`）。

- [ ] **Step 1: 复跑权威 grep（确认无漂移）** — 在 `interfaces/webchat/src/views/settings`：
```bash
cd /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/settings && \
  find . -name '*.rs' | xargs grep -nE 'grid-cols-2|grid-cols-3|px-8|px-6|max-w-' | grep -v 'max-sm' | sort
```
- [ ] **Step 2: 与下表核对（actual audit output）** — 若 grep 行号与下表不符则以新 grep 为准更新 T②.2，否则照表执行。**审计全集**（已分类，✅=种子已含，🆕=spec 种子遗漏，⏭=容器内边距非内容宽度=低优先/可选）：

  **A. 最外层内容容器 `px-8/px-6 ... max-w-* mx-auto`（需 `max-sm:px-4 max-sm:max-w-none`）**
  - ✅ `mod.rs:124` `px-8 ... max-w-5xl mx-auto`
  - ✅ `general.rs:68` `px-8 ... max-w-4xl mx-auto`
  - ✅ `appearance.rs:44` `px-8 ... max-w-4xl mx-auto`
  - ✅ `route.rs:102` `px-8 ... max-w-5xl mx-auto`
  - ✅ `execution.rs:98` `px-8 ... max-w-5xl mx-auto`
  - ✅ `network/mod.rs:16` `px-8 ... max-w-5xl mx-auto`（Network 页，spec 未列但同形）

  **B. `flex-1 px-6 ... overflow-y-auto` 外层 + 内层 `max-w-*`（需 `max-sm:px-4` 外层 + `max-sm:max-w-none` 内层）**
  - ✅ `behavior.rs:40` `px-6 ... space-y-6`（无内 max-w，仅 `max-sm:px-4`）
  - 🆕 `browser.rs:87` `px-6 ... space-y-6`（spec 漏列；同 behavior，仅 `max-sm:px-4`）
  - ✅ `memory.rs:66` `px-6 ...` + `:67` 内 `max-w-4xl`
  - 🆕 `mcp.rs:81` `px-6 ...` + `:82` 内 `max-w-3xl`
  - 🆕 `plugins.rs:68` `px-6 ...` + `:69` 内 `max-w-3xl`
  - 🆕 `skills.rs:192` `px-6 ...` + `:193` 内 `max-w-3xl`
  - 🆕 `policies.rs:147` `px-6 ...` + `:148` 内 `max-w-2xl`
  - 🆕 `security/mod.rs:158` `px-6 ...` + `:159` 内 `max-w-4xl`
  - ✅ `channels/overview.rs:56` `px-6 ...` + `:57` 内 `max-w-5xl`
  - 🆕 `channels/platform_page.rs:64` `px-6 ...` + `:304` 内 `max-w-3xl`
  - 🆕 `channels/discord.rs:105` 内 `max-w-3xl`

  **C. 多列网格（需 `max-sm:grid-cols-1`）**
  - ✅ `memory.rs:178` `grid grid-cols-2 gap-4`
  - ✅ `memory.rs:249` `grid grid-cols-2 gap-4`
  - ✅ `memory.rs:381` `grid grid-cols-2 gap-4`
  - ✅ `memory.rs:561` `grid grid-cols-2 gap-4`
  - 🆕 `memory.rs:709` `grid grid-cols-2 gap-4`（spec §3.3b 写「6 处」但只列了 5 行号；这是第 6 行号）
  - ✅ `route.rs:239` `grid grid-cols-1 md:grid-cols-2 gap-4`
  - ✅ `channels/overview.rs:67` `grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3`（已响应，仅校验 gap）
  - 🆕 `reranking_providers/add_custom.rs:225` `grid grid-cols-2 gap-4`
  - 🆕 `reranking_providers/detail_panel.rs:280` `grid grid-cols-2 gap-4`
  - 🆕 `security/sandbox.rs:46` `grid grid-cols-2 gap-4`
  - 🆕 `security/sandbox.rs:104` `grid grid-cols-3 gap-2 text-xs`

  **D. ⏭ 排除项（panel/弹窗/卡片内边距，非主内容宽度，本 Phase 不动）**：`*/add_panel.rs` / `*/detail_panel.rs` / `*/mod.rs` 内 `px-6 py-4 border-b`（侧滑面板 header）、`mcp.rs:426` / `plugins.rs:368` / `skills.rs:1014,560` / `network/cluster.rs:145`（modal `max-w-md`，已 `w-full mx-4` 自适应）、`routing_rules.rs:344` (`p-8 max-w-3xl mx-auto`，二级抽屉内容，⏭ 可选)、`skills.rs:479`（文本截断 `max-w-full` 无关）、各 `px-6 py-2` 按钮。

- [ ] **Step 3: 落记录** — 把上表「A/B/C 共 27 个目标」作为 T②.2 的硬清单（spec 种子 ~13 → 实测 27）。无代码改动。
- [ ] **Step 4: Commit（仅审计无代码 → 跳过 commit）** — 本任务不产出代码 diff；审计结果直接进 T②.2 checklist，无独立 commit。

---

### Task ②.1: 新建 `mobile_landing.rs` — iOS 分组列表（手机 Settings landing）

**Files:**
- Create: `interfaces/webchat/src/views/settings/mobile_landing.rs`
- Modify: `interfaces/webchat/src/views/settings/mod.rs:1-22`（声明 `pub mod mobile_landing;` + `pub use`）, `mod.rs:124`（Quick Setup 容器 `max-sm:hidden` + 同级挂 `MobileSettingsLanding`）
- Test: `interfaces/webchat/src/views/settings/mobile_landing.rs` `#[cfg(test)]` — 纯 Rust 单测验数据源 + compile gate；view 本身 browser QA。

**Interfaces:**
- Consumes: `crate::components::settings_sidebar::{SETTINGS_GROUPS, SettingsGroup, SettingsTab}`（`path()`/`i18n_label()`/`icon_svg()`，`settings_sidebar.rs:50-208`）；`crate::i18n::use_i18n`；`leptos_router::components::A`。
- Produces: `#[component] pub fn MobileSettingsLanding() -> impl IntoView`（`mod.rs` `pub use` 后 T②.0/审计无依赖；`mod.rs:124` 挂载它）；pure fn `landing_group_count() -> usize` + `landing_tab_count() -> usize`（供单测，不进 pub API 外）。

- [ ] **Step 1（RED）: 写失败单测固定 landing 数据契约** — 在新文件底部加纯 Rust 测试（不渲染 DOM，验数据源正确）。先写 helper 占位让它编译失败/断言失败：
```rust
//! Mobile-only Settings landing — iOS grouped-list of `SETTINGS_GROUPS`.
//!
//! Rendered alongside the desktop Quick Setup checklist (`settings/mod.rs`);
//! visibility is CSS-gated (`max-sm:block` here / `max-sm:hidden` on Quick
//! Setup) so both mount unconditionally (no Leptos reactive-scope teardown).
//! Zero new data: every cell is driven straight from `SETTINGS_GROUPS`.

use crate::components::settings_sidebar::SETTINGS_GROUPS;
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos_router::components::A;

/// Number of groups rendered as iOS sections (data-source sanity).
#[must_use]
pub fn landing_group_count() -> usize {
    SETTINGS_GROUPS.len()
}

/// Total cells (= leaf settings entries) across all groups.
#[must_use]
pub fn landing_tab_count() -> usize {
    SETTINGS_GROUPS.iter().map(|g| g.tabs.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_renders_all_six_groups() {
        assert_eq!(landing_group_count(), 6, "iOS landing must mirror all 6 SETTINGS_GROUPS");
    }

    #[test]
    fn landing_cell_count_matches_metadata() {
        // 3 Basic + 8 AI + 1 Channels + 4 Extensions + 4 Advanced + 1 Network = 21.
        assert_eq!(landing_tab_count(), 21, "landing must surface every settings leaf as a cell");
    }
}
```
- [ ] **Step 2: 跑红/绿单测** — `cargo test -p aleph-panel landing_ --lib`（仅这两个测试；helper 已实现 → 应直接 GREEN，确认数据契约固定）。
- [ ] **Step 3（GREEN view）: 实现 `MobileSettingsLanding` 组件** — 在 helper 之上、`#[cfg(test)]` 之前插入组件。复用 `mode_sidebar.rs:252-272` 的 `<A>` + `inner_html=icon_svg` 惯用法，渲染为 iOS 分组卡（圆角分组 + cell 带 icon/label/›）：
```rust
#[component]
#[must_use]
pub fn MobileSettingsLanding() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        // Mobile-only: hidden ≥640px; the desktop Quick Setup covers wide.
        <div class="hidden max-sm:block px-4 pb-8 aleph-content-top space-y-6">
            {SETTINGS_GROUPS.iter().map(|group| {
                let group_label = group.i18n_label(i18n);
                view! {
                    <section class="space-y-2">
                        <h2 class="px-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group_label}
                        </h2>
                        <div class="rounded-xl overflow-hidden border border-border bg-surface-raised divide-y divide-border">
                            {group.tabs.iter().map(|tab| {
                                let path = tab.path();
                                let label = tab.i18n_label(i18n);
                                let icon_svg = tab.icon_svg();
                                view! {
                                    <A
                                        href=path
                                        attr:class="flex items-center gap-3 px-4 py-3 active:bg-surface-sunken transition-colors"
                                    >
                                        <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                                             stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                             stroke-linejoin="round"
                                             class="text-text-tertiary flex-shrink-0"
                                             inner_html=icon_svg
                                        />
                                        <span class="flex-1 text-sm text-text-primary">{label}</span>
                                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
                                             stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                             stroke-linejoin="round"
                                             class="text-text-tertiary flex-shrink-0"
                                        >
                                            <polyline points="9 18 15 12 9 6" />
                                        </svg>
                                    </A>
                                }
                            }).collect_view()}
                        </div>
                    </section>
                }
            }).collect_view()}
        </div>
    }
}
```
- [ ] **Step 4: 注册 module + 导出** — `views/settings/mod.rs`：在 `pub mod memory;`(`:12`) 后加 `pub mod mobile_landing;`，并在 `pub use memory::MemoryView;`(`:35`) 后加 `pub use mobile_landing::MobileSettingsLanding;`。
```rust
pub mod mobile_landing;
```
```rust
pub use mobile_landing::MobileSettingsLanding;
```
- [ ] **Step 5: 挂载 + Quick Setup 桌面专属** — `views/settings/mod.rs:124`，给 Quick Setup 外层 div 加 `max-sm:hidden`，并在其后同级无条件挂载 landing。改 `view!` 顶层为 fragment（`<>…</>`）以并列两个根：
```rust
    view! {
        <>
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10 max-sm:hidden">
```
  并在该 `</div>`（原 `:220` 收尾，对应 Quick Setup 容器结束）之后、`</>` 之前插入：
```rust
        <MobileSettingsLanding />
        </>
    }
```
  （注：`max-sm:hidden` 让整个 Quick Setup 桌面块在 <640 隐藏；landing 自带 `hidden max-sm:block` 反之。两者同时 mount，纯 CSS 切换，无条件卸载 → 规避 Leptos 响应作用域问题，符合 spec §3.3a。）
- [ ] **Step 6: compile gate** — `cargo check --target wasm32-unknown-unknown -p aleph-panel`（本任务组唯一一次 wasm check）。
- [ ] **Step 7: Commit** — `git add interfaces/webchat/src/views/settings/mobile_landing.rs interfaces/webchat/src/views/settings/mod.rs` + `git commit -m "panel: add iOS grouped-list mobile Settings landing"`

---

### Task ②.2: 表单 1 列化 + 安全内边距 reflow（27 目标，shared `.grid-responsive`）

**Files:**
- Create util: `interfaces/webchat/styles/tailwind.css`（`.grid-responsive` 类）
- Modify: T②.0 审计 A/B/C 全部 27 目标（路径行号见 ②.0 Step 2）
- Test: `compile + browser QA + grep lint — no unit test`（CSS reflow 无 DOM 单测）

**Interfaces:**
- Consumes: T②.0 审计清单（A/B/C）；R-6 结论（`aleph-content-top` 无横向 padding，安全叠加 `px-8 max-sm:px-4`）。
- Produces: `.grid-responsive`(`@apply grid grid-cols-1 sm:grid-cols-2 gap-4`) util — 后续/同步税降低；grep-lint 不变量（`grep 'grid-cols-2\|grid-cols-3' | grep -v 'max-sm\|grid-responsive'` 在 settings 内应为 0）。

- [ ] **Step 1: 加 `.grid-responsive` 工具类** — `styles/tailwind.css`，在 `.aleph-content-top`(`:1985`) 块**之前**插入（与现有自定义类同区）：
```css
/* Mobile-first responsive 2-col grid for settings forms. Collapses to a
   single column under the 640px breakpoint, expands to 2 columns on `sm`+.
   Replaces scattered `grid grid-cols-2` so each settings sub-grid stays in
   sync without per-call `max-sm:` duplication. */
.grid-responsive {
    @apply grid grid-cols-1 gap-4 sm:grid-cols-2;
}
```
- [ ] **Step 2: A 类 — 6 个外层容器加 `max-sm:px-4 max-sm:max-w-none`** — 逐文件 Edit（每处把 `px-8 pb-8 aleph-content-top max-w-5xl mx-auto` → 追加 `max-sm:px-4 max-sm:max-w-none`）。示例（`general.rs:68`，其余 `appearance.rs:44` / `route.rs:102` / `execution.rs:98` / `network/mod.rs:16` 同法，`mod.rs:124` 已在 ②.1 改为 `max-sm:hidden` → A 类不再动它）：
```rust
        <div class="px-8 pb-8 aleph-content-top max-w-4xl mx-auto max-sm:px-4 max-sm:max-w-none">
```
- [ ] **Step 3: B 类 — `px-6` 外层加 `max-sm:px-4`，内层 `max-w-*` 加 `max-sm:max-w-none`** — 逐文件。`behavior.rs:40` / `browser.rs:87`（仅外层 `max-sm:px-4`，无内 max-w）：
```rust
        <div class="px-6 pb-6 aleph-content-top space-y-6 max-sm:px-4">
```
  带内层 max-w 的（`memory.rs:66`+`:67` / `mcp.rs:81`+`:82` / `plugins.rs:68`+`:69` / `skills.rs:192`+`:193` / `policies.rs:147`+`:148` / `security/mod.rs:158`+`:159` / `channels/overview.rs:56`+`:57` / `channels/platform_page.rs:64`+`:304` / `channels/discord.rs:105`），外层加 `max-sm:px-4`、内层加 `max-sm:max-w-none`。示例 `memory.rs`：
```rust
        <div class="flex-1 px-6 pb-6 overflow-y-auto aleph-content-top max-sm:px-4">
            <div class="max-w-4xl max-sm:max-w-none">
```
  （`channels/discord.rs:105` 仅内层 `<div class="max-w-3xl space-y-6 max-sm:max-w-none">`；其外层 padding 来自 platform_page 容器，已在本步处理。）
- [ ] **Step 4: C 类 — 多列网格降单列** — 优先把裸 `grid grid-cols-2 gap-4` 换成 `grid-responsive`（消同步税）；已带 `md:`/`sm:` 断点的保留原样仅追加 `max-sm:grid-cols-1`。逐处：
  - `memory.rs:178/249/381/561/709`（5 处裸 `grid grid-cols-2 gap-4`）→ `grid-responsive`：
```rust
                <div class="grid-responsive">
```
  - `reranking_providers/add_custom.rs:225` / `reranking_providers/detail_panel.rs:280` / `security/sandbox.rs:46`（裸 `grid grid-cols-2 gap-4`）→ `grid-responsive`（同上）。
  - `security/sandbox.rs:104`（`grid grid-cols-3 gap-2 text-xs`，非标准 2 列、gap 不同 → 不套 util，追加 `max-sm:grid-cols-1`）：
```rust
            <div class="grid grid-cols-3 gap-2 text-xs max-sm:grid-cols-1">
```
  - `route.rs:239`（`grid grid-cols-1 md:grid-cols-2 gap-4` 已断点化 → 追加 `max-sm:grid-cols-1` 确保 <640 单列）：
```rust
                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4 max-sm:grid-cols-1">
```
  - `channels/overview.rs:67`（`grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3` 已响应 → 仅校验 gap，必要时追加 `max-sm:gap-2`；默认不改，browser QA 时定）。
- [ ] **Step 5: grep lint（不变量校验）** — settings 内不得残留未降级的 2/3 列网格：
```bash
cd /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/settings && \
  find . -name '*.rs' | xargs grep -nE 'grid-cols-2|grid-cols-3' \
  | grep -v 'max-sm' | grep -v 'grid-responsive'
```
  期望输出 = 空（C 类全部已转 `grid-responsive` 或带 `max-sm:grid-cols-1`）。
- [ ] **Step 6: compile gate** — `cargo check --target wasm32-unknown-unknown -p aleph-panel`（本任务组唯一一次）。
- [ ] **Step 7: Commit** — `git add interfaces/webchat/styles/tailwind.css interfaces/webchat/src/views/settings` + `git commit -m "panel: collapse settings forms to single column on mobile via .grid-responsive + safe padding"`

---

### Task ②.3: ‹back affordance — 子页返回 `/settings`（§11 P-③）

**Files:**
- Modify: `interfaces/webchat/src/views/settings/channels/platform_page.rs`（header 加 mobile-only ‹back，`navigate("/settings")`）
- Confirm: `interfaces/webchat/src/app.rs:460`（`"/settings" => <Settings/>` landing 路由，已存在 → 零改，仅核对）
- Test: `compile + browser QA — no unit test`

**Interfaces:**
- Consumes: `leptos_router::hooks::use_navigate`（SPA 显式导航，**不用 `history.back()`**，§11 P-③）；工作流 ① 的 `MobileTopBar` 不直接用于 settings 子页内容区（settings 子页有自己的 `aleph-content-top` 容器；back 内联在页面 header），故此处用页面内联 ‹back 而非顶栏 right slot。
- Produces: `platform_page` 顶部 mobile-only ‹back 行（`hidden max-sm:flex`）→ `navigate("/settings")`。其余设置子页因有抽屉快跳 + 顶栏 hamburger（工作流 ①）已可返回，本 Phase ‹back 仅补**最深层**的 channels 平台子页（`/settings/channels/telegram` 等，二级深度）。

- [ ] **Step 1: 确认 landing 路由存在** — 读 `app.rs:460` 已是 `"/settings" => view! { <Settings /> }.into_any()`。无需改路由；`navigate("/settings")` 直达桌面 Quick Setup（隐藏于手机）+ 手机分组列表（②.1）。记录确认，无代码。
- [ ] **Step 2: 在 `platform_page.rs` 引入 `use_navigate`** — 文件顶部 `use` 区加：
```rust
use leptos_router::hooks::use_navigate;
```
- [ ] **Step 3: 在 platform_page 主内容容器顶部加 mobile-only ‹back 行** — `platform_page.rs:64` 的 `<div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top max-sm:px-4">`（已在 ②.2 加 `max-sm:px-4`）内、紧接其后插入返回行。先在组件函数体内、`view!` 之前取 navigate：
```rust
    let navigate = use_navigate();
```
  在容器 `<div>` 开标签后第一个子节点插入：
```rust
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 mb-3 text-sm text-primary"
                    on:click={
                        let navigate = navigate.clone();
                        move |_| navigate("/settings", Default::default())
                    }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none"
                         stroke="currentColor" stroke-width="2" stroke-linecap="round"
                         stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                    <span>{t!(i18n, settings.back_to_settings)}</span>
                </button>
```
  （`t!(i18n, settings.back_to_settings)` 的 key 在 ②.4 补；若 ②.4 不做，临时用字面 `"‹ Settings"` 文本节点 `"‹ Settings"`。为避免跨任务硬依赖，本步先用字面串 `"‹ Settings"`，②.4 再换 i18n。）→ **采用字面串版本**：
```rust
                    <span>"Settings"</span>
```
- [ ] **Step 4: compile gate** — `cargo check --target wasm32-unknown-unknown -p aleph-panel`。
- [ ] **Step 5: Commit** — `git add interfaces/webchat/src/views/settings/channels/platform_page.rs` + `git commit -m "panel: add back-to-settings affordance on channel platform sub-pages"`

---

### Task ②.4 (optional): i18n keys — 4 个硬编码 label + Network 组名 + back

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs:86,103,108,109,204`（5 处硬编码 → `t_string!`）
- Modify: `interfaces/webchat/locales/en.json`（`settings.tabs.{appearance,browser,execution,network}` + `settings.groups.network` + `settings.back_to_settings`）
- Modify: `interfaces/webchat/locales/zh.json`（同 keys 中文）
- Modify: `platform_page.rs`（②.3 字面串 → `t!(i18n, settings.back_to_settings)`）
- Test: `cargo test -p aleph-panel --lib`（既有 settings_sidebar tests 须仍绿）+ compile gate

**Interfaces:**
- Consumes: `crate::i18n::t_string!`（`settings_sidebar.rs:6` 已 import）；现有 `settings.tabs`/`settings.groups` JSON 结构（`en.json:399-428`）。
- Produces: 新 i18n keys；`SettingsTab::i18n_label` / `SettingsGroup::i18n_label` 全部走 i18n（无硬编码串）。**风险**：`leptos_i18n` 编译期校验 key —— en/zh 必须**键集一致**否则 build fail。

- [ ] **Step 1: en.json 补 keys** — `locales/en.json`，`settings.tabs`(`:406-428`) 加 4 key、`settings.groups`(`:399-405`) 加 network、`settings` 顶层加 back：
```json
      "appearance": "Appearance",
      "browser": "Browser",
      "execution": "Execution",
      "network": "Service & Cluster",
```
```json
      "network": "Service & Cluster"
```
```json
    "back_to_settings": "Settings",
```
  （`tabs` 块末尾 `"model_route": "Model Routing"` 后补逗号 + 4 行；`groups` 块 `"advanced": "Advanced"` 后补逗号 + network；`back_to_settings` 加在 `settings` 对象内任意成员后，注意 JSON 逗号。）
- [ ] **Step 2: zh.json 补同名 keys** — `locales/zh.json` 对应位置加：
```json
      "appearance": "外观",
      "browser": "浏览器",
      "execution": "执行",
      "network": "服务与集群",
```
```json
      "network": "服务与集群"
```
```json
    "back_to_settings": "设置",
```
- [ ] **Step 3: settings_sidebar.rs 替换 5 处硬编码** — Edit：
  - `:86` `Self::Appearance => "外观".to_string(),` → `Self::Appearance => t_string!(i18n, settings.tabs.appearance).to_string(),`
  - `:103` `Self::Browser => "Browser".to_string(),` → `Self::Browser => t_string!(i18n, settings.tabs.browser).to_string(),`
  - `:108` `Self::Execution => "Execution".to_string(),` → `Self::Execution => t_string!(i18n, settings.tabs.execution).to_string(),`
  - `:109` `Self::Network => "服务与集群".to_string(),` → `Self::Network => t_string!(i18n, settings.tabs.network).to_string(),`
  - `:204` `"Network" => "服务与集群".to_string(),` → `"Network" => t_string!(i18n, settings.groups.network).to_string(),`
```rust
            Self::Appearance => t_string!(i18n, settings.tabs.appearance).to_string(),
```
```rust
            Self::Browser => t_string!(i18n, settings.tabs.browser).to_string(),
```
```rust
            Self::Execution => t_string!(i18n, settings.tabs.execution).to_string(),
```
```rust
            Self::Network => t_string!(i18n, settings.tabs.network).to_string(),
```
```rust
            "Network" => t_string!(i18n, settings.groups.network).to_string(),
```
- [ ] **Step 4: platform_page ‹back 文案改 i18n** — 把 ②.3 Step 3 的 `<span>"Settings"</span>` 换为 `<span>{t!(i18n, settings.back_to_settings)}</span>`（`platform_page.rs` 已有 `let i18n = use_i18n();`，确认存在；若无则在函数体加 `let i18n = use_i18n();`）：
```rust
                    <span>{t!(i18n, settings.back_to_settings)}</span>
```
- [ ] **Step 5: 单测 + compile gate** — `cargo test -p aleph-panel --lib clawhub_tab_is_removed mcp_plugins_skills_in_extensions_group`（确认 settings_sidebar 既有测试不回归）→ 一次 `cargo check --target wasm32-unknown-unknown -p aleph-panel`（i18n 编译期 key 校验在此触发）。
- [ ] **Step 6: Commit** — `git add interfaces/webchat/src/components/settings_sidebar.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json interfaces/webchat/src/views/settings/channels/platform_page.rs` + `git commit -m "panel: i18n the 4 hardcoded settings labels + Network group + back affordance"`

---

### 工作流 ② 收尾：浏览器 QA（391/390/375/320px）

> 单次 `just wasm` + 重编 server（rust_embed STALE-EMBED，spec §6/R-15）后，chrome-devtools 实测：

- [ ] **QA-1: landing 渲染** — 391/390px 访问 `/settings`：手机分组列表显示 6 组 / 21 cell（每 cell = icon + label + › 雪佛龙），桌面 Quick Setup checklist **不可见**；≥640px 反之（Quick Setup 显示、landing 隐藏）。
- [ ] **QA-2: 子页单列无横向溢出** — 逐验 `general`/`appearance`/`memory`(网格)/`route`/`channels/telegram`：内容全宽单列，**无水平滚动条**；`memory` 的 5 处 2 列网格在 <640 全部塌成单列。
- [ ] **QA-3: ‹back** — `/settings/channels/telegram` 顶部出现 ‹ Settings 行（仅 <640），点击 `navigate("/settings")` 回到分组列表。
- [ ] **QA-4: 窄屏边界（R-7）** — 切 375(iPhone SE) 与 320px：确认 `max-w-none` 下无溢出；若 320 仍溢出，回退该页 `max-sm:px-4` → `max-sm:px-3`（局部调整，记录于此）。
- [ ] **QA-5: R-6 复核（视觉）** — 确认 `aleph-content-top` 顶部 inset 正常、横向无双重 padding（已静态核验只 padding-top；此为视觉兜底）。


---

## 工作流 ③ — Teams 只读为主 MVP (经抽屉进入)

> **关键现实校正 (grounded, 偏离 §11 P-④)**: 仓库中**不存在 `TaskStatus` 枚举**——`CoordTaskDto.status` 是裸 `String`(`api/teams.rs:215`),board/kanban/plan_dag/replay 全部按 `&str` 比较状态。因此 P-④ 的 `RwSignal<Option<TaskStatus>>` 在本仓**落地为 `RwSignal<Option<String>>`**(语义等价:`None`=全部,`Some(s)`=该状态)。状态全集取自 `board.rs` 的 6 列:`pending / blocked / in_progress / completed / failed / cancelled`(`unsatisfiable` 归入 `blocked` 列,与 board 一致)。
>
> **`board.rs` 实为 6 列**(docstring 写 "five-column" 已过时,grid 在 `:35-36` 是**内联 style** `repeat(auto-fit, minmax(220px,1fr))`,非 Tailwind class)。
>
> **`compute_depths` 已有 8 个单测**(`plan_dag.rs:314-384`:empty/single/linear/diamond/unresolved/order-independent/cycle 全覆盖)。T③.4 **不重写**这些,只补 spec 点名但缺失的 **wide-DAG fan-out** 边界 + mobile 分层列表视图。

**File Structure (this workstream):**
- Modify: `interfaces/webchat/src/views/teams/mod.rs:107-133` — `TeamsSidebar` 的竖向 `<nav>` 加 `max-sm:` 横滑 segmented pills。
- Modify: `interfaces/webchat/src/views/teams/mod.rs:43-48` — `TeamsTabState` 加 `task_status_filter: RwSignal<Option<String>>` 字段 + `:54-58` 初始化。
- Create: `interfaces/webchat/src/views/teams/components/status_filter.rs` — `StatusFilter` 下拉组件 + 纯逻辑 `status_matches` 谓词 + 单测。
- Modify: `interfaces/webchat/src/views/teams/components/mod.rs` — 声明 `pub mod status_filter;`。
- Modify: `interfaces/webchat/src/views/teams/components/board.rs:34-36` — grid 加 `max-sm:` 单列 + 读 filter。
- Modify: `interfaces/webchat/src/views/teams/kanban.rs:131` — `KanbanBoard` 上方 `max-sm:` 挂 `StatusFilter`。
- Modify: `interfaces/webchat/src/views/teams/components/task_drawer.rs:191-193` — 右滑 overlay 加 `max-sm:` 底部 sheet CSS(kanban+plan_dag 共用)。
- Modify: `interfaces/webchat/src/views/teams/plan_dag.rs:86-104` — `max-sm:` 渲染只读分层列表(复用 `compute_depths`),桌面保留 SVG。
- Test: `interfaces/webchat/src/views/teams/plan_dag.rs` (`#[cfg(test)]`) — 补 `wide_dag_fanout_*` 边界单测。
- Modify: `interfaces/webchat/src/views/teams/replay.rs:110/143/233` — 两栏 `flex row` 加 `max-sm:` 上下单列堆叠。
- Test (status_filter): real `cargo test` for `status_matches` predicate.
- **No change**: `overview.rs` / `workers.rs`(spec §4.2:已接近响应式,本工作流零改动);`column.rs` / `task_card.rs`(可选密度微调留作末步顺手项,非阻塞)。

---

### Task ③.1: Sub-tab switcher → horizontal segmented pills on max-sm

**Files:**
- Modify: `interfaces/webchat/src/views/teams/mod.rs:107` (the `<nav>` element) + `:147-160` (`SubTabButton` class strings)
- Test: compile + browser QA — no unit test (pure CSS reflow)

**Interfaces:**
- Consumes: `TeamsTabState.sub_tab: RwSignal<TeamsSubTab>` (`mod.rs:45`, unchanged); `SubTabButton(label, current, target)` (`mod.rs:138-143`, unchanged signature).
- Produces: nothing new for downstream tasks (pure visual reflow of existing nav).

- [ ] **Step 1: Reflow the `<nav>` container to horizontal scroll on mobile.** The current `<nav>` (`mod.rs:107`) is `flex-1 overflow-y-auto px-3 space-y-1` (vertical). Add `max-sm:` variants so on `<640px` it becomes a single horizontal scrolling row, while desktop keeps the vertical list. Replace the `<nav>` opening tag:

```rust
            <nav class="flex-1 overflow-y-auto px-3 space-y-1 max-sm:flex-none max-sm:flex max-sm:flex-row max-sm:gap-1 max-sm:space-y-0 max-sm:overflow-x-auto max-sm:overflow-y-hidden max-sm:px-3 max-sm:py-2 max-sm:whitespace-nowrap">
```

- [ ] **Step 2: Make each pill not shrink + tighter on mobile.** The `SubTabButton` button (`mod.rs:147-160`) uses `w-full ... px-3 py-2 rounded-lg text-sm` for both active/inactive. `w-full` would collapse pills in a flex row. Add `max-sm:w-auto max-sm:flex-shrink-0 max-sm:px-3 max-sm:py-1.5 max-sm:rounded-full` to both branches. Replace the `class=move ||` closure body:

```rust
            class=move || {
                if is_active() {
                    "nav-tile-active w-full flex items-center px-3 py-2 rounded-lg text-sm max-sm:w-auto max-sm:flex-shrink-0 max-sm:px-3 max-sm:py-1.5 max-sm:rounded-full"
                } else {
                    "nav-tile w-full flex items-center px-3 py-2 rounded-lg text-sm max-sm:w-auto max-sm:flex-shrink-0 max-sm:px-3 max-sm:py-1.5 max-sm:rounded-full"
                }
            }
```

- [ ] **Step 3: Compile gate.** Run `cargo check --target wasm32-unknown-unknown -p aleph-panel` once — expect clean compile (pure class-string edits, no Rust API change).

- [ ] **Step 4: Commit.** `git add interfaces/webchat/src/views/teams/mod.rs` + `git commit -m "panel: teams sub-tab switcher horizontal pills on mobile"`

---

### Task ③.2: §11 P-④ status filter — state field + StatusFilter component + Kanban single-column on max-sm

**Files:**
- Modify: `interfaces/webchat/src/views/teams/mod.rs:43-48` (struct) + `:54-58` (init)
- Create: `interfaces/webchat/src/views/teams/components/status_filter.rs`
- Modify: `interfaces/webchat/src/views/teams/components/mod.rs` (add `pub mod status_filter;`)
- Modify: `interfaces/webchat/src/views/teams/components/board.rs:8-13` (props) + `:34-36` (grid) + `:16-30` (filter applied)
- Modify: `interfaces/webchat/src/views/teams/kanban.rs:131`
- Test: `interfaces/webchat/src/views/teams/components/status_filter.rs` — real `cargo test` on `status_matches`

**Interfaces:**
- Consumes: `TeamsTabState` (`mod.rs:43`); `CoordTaskDto.status: String` (`api/teams.rs:215`); `ViewportState.is_mobile` via `expect_context::<ViewportState>()`.
- Produces:
  - `TeamsTabState.task_status_filter: RwSignal<Option<String>>` (consumed by board.rs).
  - `pub fn status_matches(task_status: &str, filter: Option<&str>) -> bool` (pure; unit-tested).
  - `#[component] pub fn StatusFilter(value: RwSignal<Option<String>>)` (mounted by kanban.rs).
  - `STATUS_OPTIONS: &[&str]` constant (the 6 board statuses) reused by both component + board.

- [ ] **Step 1: Add `task_status_filter` field to `TeamsTabState`.** Edit `mod.rs:43-48` to add the field, and `:54-58` to initialize it `None`:

```rust
#[derive(Clone, Copy)]
pub struct TeamsTabState {
    pub sub_tab: RwSignal<TeamsSubTab>,
    pub teams: RwSignal<Vec<TeamSummary>>,
    pub selected_team_id: RwSignal<Option<String>>,
    /// Mobile Kanban single-column status filter. `None` = show all statuses.
    /// String (not an enum) because `CoordTaskDto.status` is a raw wire string.
    pub task_status_filter: RwSignal<Option<String>>,
}
```

And the constructor in `TeamsView` (`mod.rs:54-58`):

```rust
    let tab_state = TeamsTabState {
        sub_tab: RwSignal::new(TeamsSubTab::Overview),
        teams: RwSignal::new(Vec::new()),
        selected_team_id: RwSignal::new(None),
        task_status_filter: RwSignal::new(None),
    };
```

- [ ] **Step 2 (RED — write the failing predicate test first):** Create `interfaces/webchat/src/views/teams/components/status_filter.rs` with ONLY the test + an empty stub, so `cargo test` fails to compile/assert:

```rust
//! `StatusFilter` — mobile Kanban single-status selector (§11 P-④).
//!
//! Drives `TeamsTabState::task_status_filter`. `None` ⇒ show every status
//! (mobile renders the full single-column board); `Some(s)` ⇒ show only that
//! status' column. Status is a raw wire string (no `TaskStatus` enum exists in
//! this crate — `CoordTaskDto.status` is `String`).

/// The six derived Kanban statuses, in board column order. `unsatisfiable`
/// is intentionally absent: board groups it under `blocked`, and the filter
/// mirrors that by folding it into the `blocked` option (see `status_matches`).
pub const STATUS_OPTIONS: &[&str] = &[
    "pending",
    "blocked",
    "in_progress",
    "completed",
    "failed",
    "cancelled",
];

/// Pure predicate: does a task with `task_status` pass the active `filter`?
/// `None` filter passes everything. The `blocked` filter also matches
/// `unsatisfiable` (board.rs groups them in one column).
pub fn status_matches(task_status: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some("blocked") => task_status == "blocked" || task_status == "unsatisfiable",
        Some(s) => task_status == s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_filter_passes_every_status() {
        for s in STATUS_OPTIONS {
            assert!(status_matches(s, None), "{s} should pass under None");
        }
        assert!(status_matches("unsatisfiable", None));
    }

    #[test]
    fn some_filter_matches_only_that_status() {
        assert!(status_matches("completed", Some("completed")));
        assert!(!status_matches("pending", Some("completed")));
        assert!(!status_matches("failed", Some("completed")));
    }

    #[test]
    fn blocked_filter_also_matches_unsatisfiable() {
        assert!(status_matches("blocked", Some("blocked")));
        assert!(status_matches("unsatisfiable", Some("blocked")));
        assert!(!status_matches("pending", Some("blocked")));
    }

    #[test]
    fn unsatisfiable_does_not_leak_into_failed() {
        // unsatisfiable is grouped with blocked, NOT failed — guard the boundary.
        assert!(!status_matches("unsatisfiable", Some("failed")));
    }
}
```

- [ ] **Step 3: Register the module.** Add `pub mod status_filter;` to `interfaces/webchat/src/views/teams/components/mod.rs` (alongside the existing `board` / `column` / `task_card` / `task_drawer` / `create_form` / `team_selector` declarations — match their `pub mod` style).

- [ ] **Step 4 (GREEN — run the predicate test):** `cargo test -p aleph-panel status_matches` — the four `status_filter` tests must pass. (Targeted single-pattern run, not full suite.)

- [ ] **Step 5: Add the `StatusFilter` component below the test module's `pub fn`s** (same file, before `#[cfg(test)]`). A native `<select>` driving the signal; `aleph_panel` i18n optional — use plain labels via the status string + an "All" entry:

```rust
use leptos::prelude::*;
use crate::i18n::{t_string, use_i18n};

/// Mobile-only single-status selector. Reads/writes the shared
/// `task_status_filter` signal; an empty value ⇒ `None` (all statuses).
#[component]
#[must_use]
pub fn StatusFilter(value: RwSignal<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let all_label = move || t_string!(i18n, teams.kanban.filter.all).to_string();

    view! {
        <div class="max-sm:block hidden px-3 pb-2">
            <select
                class="w-full px-2 py-1.5 rounded bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-border-strong"
                on:change=move |ev| {
                    let v = event_target_value(&ev);
                    value.set(if v.is_empty() { None } else { Some(v) });
                }
                prop:value=move || value.get().unwrap_or_default()
            >
                <option value="">{all_label}</option>
                {STATUS_OPTIONS.iter().map(|s| {
                    let s = (*s).to_string();
                    let label = s.replace('_', " ");
                    view! { <option value=s.clone()>{label}</option> }
                }).collect_view()}
            </select>
        </div>
    }
}
```

- [ ] **Step 6: Add the i18n key `teams.kanban.filter.all`** to `interfaces/webchat/locales/en.json` (`"all": "All statuses"`) and `zh.json` (`"all": "全部状态"`) under the existing `teams.kanban.filter` object — create the `filter` object if absent. (If the i18n build fails on a missing key the compile gate in Step 9 catches it.)

- [ ] **Step 7: Make `KanbanBoard` accept + apply the filter, and collapse to single column on mobile.** Edit `board.rs`. Add a `filter` prop and a `display:none`-driven single column. The cleanest surgical approach: keep the 6 `KanbanColumn`s but on mobile hide every column whose status ≠ filter via a per-column wrapper, AND switch the grid to one column. Change the props (`board.rs:8-13`) to add the filter signal:

```rust
#[component]
#[must_use]
pub fn KanbanBoard(
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    #[prop(optional, into)] status_filter: Signal<Option<String>>,
) -> impl IntoView {
```

Change the grid container (`board.rs:34-36`) so mobile is a single column and desktop keeps auto-fit; and wrap each column so non-matching columns vanish on mobile. Replace the outer `<div ...>` open tag and the six `<KanbanColumn .../>` with `<FilterableColumn>` wrappers. Concretely, add this helper at the bottom of `board.rs`:

```rust
use super::status_filter::status_matches;
use super::column::KanbanColumn;

#[component]
fn FilterableColumn(
    status: &'static str,
    title: String,
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    empty_label: String,
    status_filter: Signal<Option<String>>,
) -> impl IntoView {
    // On mobile a column is shown iff it matches the active filter (None = all
    // columns visible, stacked). Desktop ignores the filter entirely.
    let hidden_on_mobile = move || {
        match status_filter.get() {
            None => false,
            Some(f) => !status_matches(status, Some(f.as_str())),
        }
    };
    view! {
        <div class=move || if hidden_on_mobile() { "max-sm:hidden" } else { "" }>
            <KanbanColumn
                title=title
                tasks=tasks
                on_card_click=on_card_click
                empty_label=empty_label
            />
        </div>
    }
}
```

Then change the grid `<div>` (`board.rs:35-36`) to single-column on mobile:

```rust
        <div class="grid gap-3 p-3 flex-1 overflow-auto max-sm:flex max-sm:flex-col"
             style="grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); align-items: stretch; min-height: 0;">
```

and replace each of the six `<KanbanColumn ... />` blocks with `<FilterableColumn status="pending" ... status_filter=status_filter />` (one per status: `pending`/`blocked`/`in_progress`/`completed`/`failed`/`cancelled`), passing the same `tasks=<signal>` / `on_card_click` / `empty_label` already wired, plus `status_filter=status_filter`. Example for the first:

```rust
            <FilterableColumn
                status="pending"
                title=t_string!(i18n, teams.kanban.columns.pending).to_string()
                tasks=pending
                on_card_click=on_card_click
                empty_label=empty_label()
                status_filter=status_filter
            />
```

(Repeat verbatim for blocked/in_progress/completed/failed/cancelled with their existing `tasks=` signals.)

- [ ] **Step 8: Mount `StatusFilter` + pass the filter into the board in `kanban.rs`.** Edit `kanban.rs:131`. Import `StatusFilter` (`use super::components::status_filter::StatusFilter;` near the top), then replace the single `<KanbanBoard .../>` line:

```rust
                            <StatusFilter value=state.task_status_filter />
                            <KanbanBoard
                                tasks=filtered
                                on_card_click=card_click
                                status_filter=state.task_status_filter.into()
                            />
```

- [ ] **Step 9: Compile gate (one wasm check for the whole task group).** `cargo check --target wasm32-unknown-unknown -p aleph-panel` — expect clean. This validates the new prop wiring + i18n key + module registration together.

- [ ] **Step 10: Commit.** `git add interfaces/webchat/src/views/teams/mod.rs interfaces/webchat/src/views/teams/components/status_filter.rs interfaces/webchat/src/views/teams/components/mod.rs interfaces/webchat/src/views/teams/components/board.rs interfaces/webchat/src/views/teams/kanban.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json` + `git commit -m "panel: teams kanban mobile single-column status filter"`

---

### Task ③.3: task_drawer.rs → bottom sheet on max-sm (shared by kanban + plan_dag)

**Files:**
- Modify: `interfaces/webchat/src/views/teams/components/task_drawer.rs:191-193`
- Test: compile + browser QA — no unit test (pure CSS reflow)

**Interfaces:**
- Consumes: `TaskDetailDrawer(open_for, on_changed)` — unchanged signature; both `kanban.rs:136` and `plan_dag.rs:102` already mount it, so this CSS change applies to both for free.
- Produces: nothing new (visual variant only).

- [ ] **Step 1: Reflow the overlay flex from right-justified to bottom-justified on mobile.** The container at `task_drawer.rs:191` is `fixed inset-0 z-40 flex justify-end`. Add `max-sm:items-end` so the sheet pins to the bottom edge on mobile. Replace line 191:

```rust
                    <div class="fixed inset-0 z-40 flex justify-end max-sm:items-end">
```

- [ ] **Step 2: Reflow the `<aside>` from right side-panel to bottom sheet.** Line 193 is `glass relative w-96 h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col`. On mobile: full width, capped height, rounded top, no left border, notch-aware bottom padding. Replace line 193:

```rust
                        <aside class="glass relative w-96 h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col max-sm:w-full max-sm:h-[90vh] max-sm:border-l-0 max-sm:rounded-t-2xl max-sm:pb-[env(safe-area-inset-bottom)]">
```

(The inner `<div class="flex-1 overflow-y-auto p-4 ...">` at `:203` already scrolls the body and the `<header>` at `:194` stays pinned — so R-12 "sticky header + scrolling body" is already satisfied; no further change.)

- [ ] **Step 3: Compile gate.** `cargo check --target wasm32-unknown-unknown -p aleph-panel`.

- [ ] **Step 4: Commit.** `git add interfaces/webchat/src/views/teams/components/task_drawer.rs` + `git commit -m "panel: teams task drawer bottom sheet on mobile"`

---

### Task ③.4: plan_dag mobile read-only layered list (reuse compute_depths) + wide-DAG edge tests

**Files:**
- Modify: `interfaces/webchat/src/views/teams/plan_dag.rs:86-104` (view branch) + add a `render_layered_list` fn near `render_dag`
- Test: `interfaces/webchat/src/views/teams/plan_dag.rs` — add `wide_dag_fanout_*` tests to the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `compute_depths(&[CoordTaskDto]) -> HashMap<String, usize>` (`plan_dag.rs:111`, pure, unchanged); `ViewportState.is_mobile`; `TaskDetailDrawer` (already mounted at `:102`); `status_fill(&str)` (`:143`, reuse).
- Produces: `render_layered_list(Vec<CoordTaskDto>, RwSignal<Option<CoordTaskDto>>) -> impl IntoView` (internal); two new tests guarding `compute_depths` fan-out.

- [ ] **Step 1 (RED — add the missing wide-DAG edge-case tests first):** The existing `#[cfg(test)] mod tests` already covers empty/single/linear/diamond/cycle. Spec T③.4 names "wide DAG" specifically — it is **not** yet covered. Add these two tests inside the existing `mod tests` (after `diamond_takes_max_dep_depth`):

```rust
    #[test]
    fn wide_dag_fanout_all_children_same_depth() {
        // One root → 5 independent children. All children sit at depth 1
        // (a wide single layer, the mobile list's worst horizontal case on
        // desktop SVG and the case the layered list must stack cleanly).
        let tasks = vec![
            task("root", &[]),
            task("c1", &["root"]),
            task("c2", &["root"]),
            task("c3", &["root"]),
            task("c4", &["root"]),
            task("c5", &["root"]),
        ];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("root"), Some(&0));
        for child in ["c1", "c2", "c3", "c4", "c5"] {
            assert_eq!(depths.get(child), Some(&1), "{child} should be depth 1");
        }
    }

    #[test]
    fn wide_dag_multi_parent_node_takes_deepest_parent() {
        // A node with parents at differing depths must land one below the
        // DEEPEST parent (its single layer in the list), not the shallowest.
        // root(0) → mid(1) → ...; sink depends on BOTH root and mid.
        let tasks = vec![
            task("root", &[]),
            task("mid", &["root"]),
            task("sink", &["root", "mid"]),
        ];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("root"), Some(&0));
        assert_eq!(depths.get("mid"), Some(&1));
        assert_eq!(depths.get("sink"), Some(&2));
    }
```

- [ ] **Step 2 (GREEN — confirm they pass against the existing implementation):** `cargo test -p aleph-panel compute_depths` plus the wide tests — run `cargo test -p aleph-panel wide_dag` (targeted). They should pass immediately (`compute_depths` is already correct); these tests **lock** the contract the mobile list relies on (deepest-parent layering). If a test fails, fix the test, not `compute_depths` (it is verified existing behaviour).

- [ ] **Step 3: Add `render_layered_list` reusing `compute_depths` + the existing layer grouping.** Add this fn after `render_dag` (after `plan_dag.rs:291`). It mirrors the SVG's grouping logic (`:159-169`) but emits an indented, dependency-annotated list:

```rust
/// Mobile read-only layered list: groups tasks by `compute_depths`, renders
/// each as an indented row (`pl-{depth}`), annotates dependencies as text
/// chips, and opens the same `TaskDetailDrawer` on tap. No edges, no zoom —
/// the SVG is desktop-only (R-11).
fn render_layered_list(
    tasks: Vec<CoordTaskDto>,
    drawer: RwSignal<Option<CoordTaskDto>>,
) -> impl IntoView {
    let depths = compute_depths(&tasks);
    let mut sorted = tasks.clone();
    sorted.sort_by_key(|t| (depths.get(&t.id).copied().unwrap_or(0), t.created_at));

    let rows = sorted
        .into_iter()
        .map(|t| {
            let depth = depths.get(&t.id).copied().unwrap_or(0);
            // Cap indent so deep chains don't run off a 390px screen.
            let pad = (depth.min(6) * 16) as i32;
            let fill = status_fill(&t.status);
            let task_for_click = t.clone();
            let subject = t.subject.clone();
            let status = t.status.clone();
            let dep_count = t.dependencies.len();
            view! {
                <button
                    class="w-full text-left flex items-center gap-2 py-2 pr-3 border-b border-border hover:bg-sidebar-active/40 cursor-pointer"
                    style=format!("padding-left: {pad}px")
                    on:click=move |_| drawer.set(Some(task_for_click.clone()))
                >
                    <span
                        class="inline-block w-2 h-2 rounded-full flex-shrink-0"
                        style=format!("background-color: {fill}")
                    />
                    <span class="text-sm text-text-primary truncate flex-1">{subject}</span>
                    {(dep_count > 0).then(|| view! {
                        <span class="text-[10px] px-1.5 py-0.5 rounded bg-surface-sunken text-text-tertiary flex-shrink-0">
                            {format!("↳{dep_count}")}
                        </span>
                    })}
                    <span class="text-[10px] text-text-tertiary flex-shrink-0">{status}</span>
                </button>
            }
        })
        .collect_view();

    view! {
        <div class="flex flex-col">
            {rows}
            <div class="px-3 py-3 text-xs text-text-tertiary">
                "完整 DAG 请在桌面查看 / View the full DAG on desktop."
            </div>
        </div>
    }
}
```

- [ ] **Step 4: Branch the view on `is_mobile` — SVG on desktop, list on mobile.** Import `ViewportState` (`use crate::state::viewport::ViewportState;` — confirm the path matches Phase 0.5; if it lives elsewhere adjust the `use`). In `PlanDagView` add `let viewport = expect_context::<ViewportState>();` after `:34`. Then replace the non-empty arm (`plan_dag.rs:97-99`, currently `render_dag(list, drawer).into_any()`) so it switches:

```rust
                    } else if viewport.is_mobile.get() {
                        render_layered_list(list, drawer).into_any()
                    } else {
                        render_dag(list, drawer).into_any()
                    }
```

(The reactive `move ||` block at `:89` already re-runs when `tasks` changes; reading `viewport.is_mobile.get()` inside it makes the branch reactive to viewport too — correct for resize.)

- [ ] **Step 5: Compile gate.** `cargo check --target wasm32-unknown-unknown -p aleph-panel`.

- [ ] **Step 6: Commit.** `git add interfaces/webchat/src/views/teams/plan_dag.rs` + `git commit -m "panel: teams plan DAG mobile layered list + wide-DAG tests"`

---

### Task ③.5: replay.rs single-column stack on max-sm

> **Actual two-pane structure (recorded from real code, per R-14):** `ReplayView` (`replay.rs:109-130`) renders an outer `<div class="flex-1 flex h-full overflow-hidden">` containing two siblings: `TaskListPane` (left) and `TracePane` (right). `TaskListPane` (`:142`) is `<div class="w-72 border-r border-border flex flex-col">` (fixed-width left rail). `TracePane` (`:232`) is `<div class="flex-1 flex flex-col overflow-hidden">` (fills the rest). So the reflow target = outer row → column, left rail → top capped-height block, right pane → bottom fill.

**Files:**
- Modify: `interfaces/webchat/src/views/teams/replay.rs:110` (outer) + `:143` (`TaskListPane` root) + `:233` (`TracePane` root)
- Test: compile + browser QA — no unit test (pure CSS reflow)

**Interfaces:**
- Consumes: existing `ReplayView` / `TaskListPane` / `TracePane` (signatures unchanged).
- Produces: nothing new (visual reflow only).

- [ ] **Step 1: Outer flex row → column on mobile.** Line 110 is `flex-1 flex h-full overflow-hidden`. Add `max-sm:flex-col`. Replace:

```rust
        <div class="flex-1 flex h-full overflow-hidden max-sm:flex-col">
```

- [ ] **Step 2: Left list rail → top block with capped height on mobile.** Line 143 is `w-72 border-r border-border flex flex-col`. On mobile: full width, bottom border instead of right, capped to ~45% height so the trace pane below stays usable (spec §4.3). Replace:

```rust
        <div class="w-72 border-r border-border flex flex-col max-sm:w-full max-sm:border-r-0 max-sm:border-b max-sm:max-h-[45%]">
```

- [ ] **Step 3: Right trace pane → bottom fill on mobile.** Line 233 is `flex-1 flex flex-col overflow-hidden`. `flex-1` already fills remaining vertical space once the parent is a column, and the inner body (`:253` `flex-1 overflow-y-auto p-6`) scrolls — so the only mobile concern is bottom safe-area padding. Add it:

```rust
        <div class="flex-1 flex flex-col overflow-hidden max-sm:pb-[env(safe-area-inset-bottom)]">
```

- [ ] **Step 4: Compile gate.** `cargo check --target wasm32-unknown-unknown -p aleph-panel`.

- [ ] **Step 5: Commit.** `git add interfaces/webchat/src/views/teams/replay.rs` + `git commit -m "panel: teams replay single-column stack on mobile"`

---

### Task ③.6: Workstream browser QA pass at 390px (rebuild + chrome-devtools)

> **`overview.rs` / `workers.rs` need NO change** (spec §4.2: already near-responsive). This task is the single end-of-workstream verification gate — NOT a unit test. It is the one place where `just wasm` + server rebuild is required (rust_embed STALE-EMBED, R-15).

**Files:**
- Modify: none (verification only)
- Test: browser QA at 390px — no unit test

- [ ] **Step 1: Rebuild WASM + server once.** Run `just wasm` then rebuild/replace the running `aleph-server` binary (per DESKTOP_SHELL.md refresh chain) so the embedded panel reflects all ③ commits. Verify served WASM changed via served-size check (NOT `strings`, per spec §9).

- [ ] **Step 2: 390px browser QA — Teams sub-tab pills (③.1).** Open the panel at 390px (chrome-devtools `resize_page` 390×844), enter Teams via the drawer hamburger. **Look for:** the 5 sub-tabs render as a single horizontal scrollable pill row (no vertical stack, no overflow clipping), active pill visually distinct, row scrolls if it exceeds width.

- [ ] **Step 3: 390px QA — Kanban single column + filter (③.2).** On the Kanban sub-tab with a team selected: **look for:** the status `<select>` appears above the board; columns stack vertically (single column, no horizontal grid); selecting a status (e.g. "in_progress") hides all other columns; selecting `unsatisfiable`'s parent "blocked" still shows unsatisfiable tasks; "All statuses" restores every column.

- [ ] **Step 4: 390px QA — task drawer bottom sheet (③.3).** Tap a task card (Kanban) AND a node row (Plan): **look for:** the detail panel slides up from the bottom (not the right), full width, rounded top corners, capped ~90vh, header pinned while body scrolls.

- [ ] **Step 5: 390px QA — Plan DAG layered list (③.4).** On the Plan sub-tab: **look for:** NO SVG; an indented list grouped by depth (children indented under parents), each row shows a status dot + subject + dependency count chip, the "View the full DAG on desktop" footer present; tapping a row opens the bottom sheet.

- [ ] **Step 6: 390px QA — Replay single column (③.5).** On the Replay sub-tab: **look for:** task list on top (capped ~45% height, scrollable), trace pane below filling the rest; selecting a task loads the trace in the lower pane without horizontal overflow.

- [ ] **Step 7: 320/375 narrow check.** Repeat the most overflow-prone screens (Kanban filter row, Plan deep-indent rows) at 320px and 375px — **look for:** no horizontal scrollbar on the page; deep DAG indentation capped (does not push rows off-screen, guarded by `depth.min(6)`).

- [ ] **Step 8: Commit (QA notes only, if any doc/screenshot artifacts are tracked; otherwise skip).** If no source changed during QA, no commit. If a tweak was needed, `git add <file>` + `git commit -m "panel: teams mobile QA fixups at 390px"`.


---

## 工作流 ④ — Canvas 触控强化 (Canvas touch: pinch-zoom · 44px pick · bottom-sheet · WebGL2 fallback · perf signal)

> 独立于 ①②③，可并行。仅依赖 Phase 0.5 已有的 `MemoryView{Graph,Table}` 切换 + `ViewportState.is_mobile`。纯 WASM/CSS。
> Crate 名 = `aleph-panel`（`interfaces/webchat/Cargo.toml:2`），但 `[lib] name = "aleph_panel"`（`:8`）。**cargo 命令用 package 名**：`cargo test -p aleph-panel <name>` / `cargo check --target wasm32-unknown-unknown -p aleph-panel`。

**File Structure (this workstream):**
- Create: `interfaces/webchat/src/views/canvas/gl/pinch.rs` — pure `pinch_zoom_factor(init_dist, cur_dist)` 数学 + 单元测试（T④.1 唯一真单测）。
- Modify: `interfaces/webchat/src/views/canvas/gl/mod.rs` — 加 `pub mod pinch;`（暴露纯函数给 galaxy_canvas）。
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs` — 加多指 pointer 追踪 map + pinch→`camera.zoom`；移动端 hover-pick ~75ms 节流；新增 `is_mobile`/`fallback` props。
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs` — `pick()` 接 `radius_px` 入参 + mobile 44px；`MAX_SETTLE_STEPS` 信号化（`new(settle_cap)`）；`bloom_level` 字段 + `render` 读取门控 `bloom.run`。
- Modify: `interfaces/webchat/src/views/canvas/gl/bloom.rs` — `run(gl, intensity)` 接 `bloom_level` 用作 composite `u_intensity`（已有钩子，仅参数化）。
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` — 详情面板 `max-sm:` 底部 sheet CSS；`fallback` RwSignal + watch Effect → `mem.memory_view.set(Table)`；把 `is_mobile`/`fallback` 传入 `GalaxyCanvas`。
- Modify: `interfaces/webchat/src/views/memory_hub/mod.rs` — Memory 顶部内联 fallback banner（「本设备不支持星系视图」）。
- Modify: `interfaces/webchat/locales/en.json` + `zh.json` — banner 文案 key `memory.galaxy_unsupported`。
- Test: 仅 `gl/pinch.rs`（真 `cargo test`）；其余 = compile gate + browser QA。

---

### Task ④.1: Pinch-zoom 纯数学 + 双指 Pointer Events（§11 P-⑤）

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/pinch.rs` (real `cargo test`)
- Modify: `interfaces/webchat/src/views/canvas/gl/mod.rs` (加 `pub mod pinch;`)
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs:12-14`（imports）, `:62-66`（state cells）, `:210-295`（pointer handlers）

**Interfaces:**
- Consumes: `OrbitCamera::zoom(factor: f32)`（`gl/camera.rs:51`，指数曲线由调用方算）；`Scene` 经 `scene.borrow_mut().as_mut()` 暴露 `pub camera`（`scene.rs:32`）→ 但 galaxy_canvas 通过 `Scene::on_wheel` 间接 zoom，本任务直接调 `s.camera.zoom(factor)`（camera 是 `pub`）。
- Produces: `pub fn pinch_zoom_factor(init_dist: f32, cur_dist: f32) -> f32`（供测试 + handler 复用）；`galaxy_canvas` 内 `active_ptrs: Rc<RefCell<Vec<(i32, f32, f32)>>>` + `pinch_base_dist: Rc<Cell<f32>>`（私有，无下游消费）。

- [ ] **Step 1: RED — 写 pinch 数学纯函数测试（先建文件含 `todo!()` 失败实现）。** 选用 wheel 同款指数曲线（`scene.rs:25/258-259`：`exp(normalized * 0.05)`），把 `cur/init` 距离比对数化喂同一曲线，使捏合幅度与滚轮手感一致、且方向对称（拉开=放大→distance 缩小？注意：`camera.zoom(factor)` 乘 `tgt_distance`，factor<1 = 拉近放大）。捏合**拉开两指 = 放大 = 相机靠近 = factor<1**。完整初始文件：

```rust
//! Pinch-to-zoom factor math (pure, unit-tested on native).
//!
//! Two-finger pinch maps the distance ratio `cur/init` to a `camera.zoom`
//! factor using the SAME exponential curve as the wheel (`scene.rs`
//! `ZOOM_SENSITIVITY`), so touch and trackpad feel identical. Spreading the
//! fingers apart (`cur > init`) zooms IN: it returns a factor `< 1.0`, which
//! `OrbitCamera::zoom` multiplies into `tgt_distance` to pull the camera
//! closer. Pinching together (`cur < init`) returns `> 1.0` (zoom out).

/// Wheel-zoom sensitivity, mirrored from `scene::ZOOM_SENSITIVITY` so pinch and
/// wheel share one curve. Kept here (not imported) to keep this module pure and
/// free of the GL-bound `scene` module.
const ZOOM_SENSITIVITY: f32 = 0.05;

/// Map a pinch distance ratio to a `camera.zoom` factor.
///
/// `init_dist` is the two-finger distance when the pinch began (the rebased
/// baseline); `cur_dist` is the live distance. Returns the multiplicative
/// factor to feed `OrbitCamera::zoom`. Degenerate inputs (`init_dist <= 0`)
/// return `1.0` (no-op) so a zero baseline never divides-by-zero or NaNs.
#[must_use]
pub fn pinch_zoom_factor(init_dist: f32, cur_dist: f32) -> f32 {
    if init_dist <= 0.0 || cur_dist <= 0.0 {
        return 1.0;
    }
    // ratio > 1 → fingers spread → want to zoom IN → factor < 1.
    // ln(ratio) gives a symmetric, signed magnitude; the same exp() curve as
    // the wheel keeps the feel consistent. The ±2 clamp caps a single jerky
    // frame (mirrors the wheel's normalized-delta clamp in scene::on_wheel).
    let ratio = cur_dist / init_dist;
    let normalized = ratio.ln().clamp(-2.0, 2.0);
    (-normalized * (ZOOM_SENSITIVITY * 20.0)).exp()
}

/// Euclidean distance between two screen points (helper for the host).
#[must_use]
pub fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_when_distance_equal() {
        // Same distance → factor 1.0 (no zoom).
        assert!((pinch_zoom_factor(100.0, 100.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spreading_fingers_zooms_in() {
        // cur > init → fingers spread → factor < 1 → camera pulls closer.
        let f = pinch_zoom_factor(100.0, 200.0);
        assert!(f < 1.0, "spread should zoom in (factor<1), got {f}");
    }

    #[test]
    fn pinching_together_zooms_out() {
        // cur < init → fingers together → factor > 1 → camera pushes back.
        let f = pinch_zoom_factor(200.0, 100.0);
        assert!(f > 1.0, "pinch should zoom out (factor>1), got {f}");
    }

    #[test]
    fn symmetric_about_unity() {
        // A 2x spread and a 2x pinch are inverse factors (within fp tolerance).
        let spread = pinch_zoom_factor(100.0, 200.0);
        let pinch = pinch_zoom_factor(200.0, 100.0);
        assert!((spread * pinch - 1.0).abs() < 1e-3, "{spread}*{pinch}");
    }

    #[test]
    fn degenerate_zero_baseline_is_noop() {
        assert_eq!(pinch_zoom_factor(0.0, 150.0), 1.0);
        assert_eq!(pinch_zoom_factor(-5.0, 150.0), 1.0);
        assert_eq!(pinch_zoom_factor(100.0, 0.0), 1.0);
    }

    #[test]
    fn extreme_ratio_clamped_not_runaway() {
        // A huge jump in one frame is clamped so the camera never teleports.
        let f = pinch_zoom_factor(1.0, 100000.0);
        assert!(f.is_finite() && f > 0.0);
        // ln clamp at -2 → factor = exp(2*1.0) ≈ 7.389 floor; assert bounded.
        assert!(f >= pinch_zoom_factor(1.0, 1e9) - 1e-3);
    }
}
```

  Run RED gate: `cargo test -p aleph-panel pinch_zoom_factor` → these compile-and-fail only if impl is `todo!()`; since this file ships the real impl, the test goes GREEN immediately. (This module is pure native — no wasm needed.) Per project frugality, run the single test once: `cargo test -p aleph-panel --lib pinch::`.

- [ ] **Step 2: GREEN — wire module into `gl/mod.rs`.** Add the `pub mod pinch;` declaration alongside the existing gl submodules. Open `gl/mod.rs`, find the module declaration list (e.g. `pub mod camera;` / `pub mod scene;` / `pub mod picking;`) and add:

```rust
pub mod pinch;
```

  Re-run `cargo test -p aleph-panel --lib pinch::` → 6 tests pass (GREEN). This is the only real unit-test gate in the workstream.

- [ ] **Step 3: Add multi-pointer tracking state in `galaxy_canvas.rs`.** After the existing `down_pos` cell (`galaxy_canvas.rs:66`), add two new shared cells for the active-pointer map and the pinch baseline. Insert immediately after line 66:

```rust
    // Multi-pointer pinch tracking (§11 P-⑤). Each entry = (pointerId, x, y) in
    // client pixels. ≥2 entries → pinch gesture active. Pointer Events ONLY (no
    // TouchEvent): galaxy_canvas already uses them with pointer-capture.
    let active_ptrs: Rc<RefCell<Vec<(i32, f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    // Baseline two-finger distance captured when the pinch began (or rebased
    // when a finger lifts mid-gesture). 0.0 = no active pinch.
    let pinch_base: Rc<Cell<f32>> = Rc::new(Cell::new(0.0));
```

- [ ] **Step 4: Track pointers on down — push into the map.** In `on_pointerdown` (`galaxy_canvas.rs:210-224`), after `down_pos_pd.set(pos);` (line 223), record this pointer. First clone the cells before the closure (add near line 209, before `let on_pointerdown`):

```rust
    let active_ptrs_pd = active_ptrs.clone();
    let pinch_base_pd = pinch_base.clone();
```

  Then change the closure body — replace the existing `on_pointerdown` (lines 210-224) with:

```rust
    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        // Capture so move/up fire even when pointer leaves canvas.
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                // Snapshot canvas origin for this gesture.
                let rect = el.get_bounding_client_rect();
                canvas_origin_pd.set((rect.left() as f32, rect.top() as f32));
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
        }
        ptr_down_pd.set(true);
        let pos = (ev.client_x() as f32, ev.client_y() as f32);
        last_ptr_pd.set(pos);
        down_pos_pd.set(pos);
        // Register this pointer for pinch tracking. When a second finger lands,
        // capture the baseline distance from the first two active pointers.
        {
            let mut ptrs = active_ptrs_pd.borrow_mut();
            ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
            ptrs.push((ev.pointer_id(), pos.0, pos.1));
            if ptrs.len() >= 2 {
                let a = (ptrs[0].1, ptrs[0].2);
                let b = (ptrs[1].1, ptrs[1].2);
                pinch_base_pd.set(super::gl::pinch::dist(a, b));
            }
        }
    };
```

- [ ] **Step 5: Pinch on move — when ≥2 pointers, zoom instead of orbit.** In `on_pointermove` (`galaxy_canvas.rs:232-259`), the pinch branch must take priority over the single-finger orbit. Add clones before the closure (near line 231):

```rust
    let active_ptrs_pm = active_ptrs.clone();
    let pinch_base_pm = pinch_base.clone();
```

  Replace the `on_pointermove` body (lines 232-259) with:

```rust
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let cx = ev.client_x() as f32;
        let cy = ev.client_y() as f32;

        // Update this pointer's tracked position.
        {
            let mut ptrs = active_ptrs_pm.borrow_mut();
            if let Some(p) = ptrs.iter_mut().find(|(id, _, _)| *id == ev.pointer_id()) {
                p.1 = cx;
                p.2 = cy;
            }
            // Pinch path: ≥2 active pointers → distance ratio drives zoom, NOT
            // orbit. Takes priority over the single-finger drag branch below.
            if ptrs.len() >= 2 {
                let a = (ptrs[0].1, ptrs[0].2);
                let b = (ptrs[1].1, ptrs[1].2);
                let cur = super::gl::pinch::dist(a, b);
                let base = pinch_base_pm.get();
                let factor = super::gl::pinch::pinch_zoom_factor(base, cur);
                let t_ms = perf_now();
                if let Some(s) = scene_pm.borrow_mut().as_mut() {
                    s.camera.zoom(factor);
                    s.camera.note_interaction(t_ms);
                }
                // Rebase the baseline each frame so zoom is incremental, not
                // absolute (mirrors the wheel's per-event accumulation).
                pinch_base_pm.set(cur);
                return;
            }
        }

        if ptr_down_pm.get() {
            // Drag: update orbit camera.
            let (lx, ly) = last_ptr_pm.get();
            let dx = cx - lx;
            let dy = cy - ly;
            last_ptr_pm.set((cx, cy));
            let t_ms = perf_now();
            if let Some(s) = scene_pm.borrow_mut().as_mut() {
                s.on_drag(dx, dy, t_ms);
            }
        } else {
            // Hover (no button down): pick and emit HoverNode on transition.
            // Use the cached canvas origin (refreshed on pointerdown and on resize)
            // to avoid get_bounding_client_rect() forced-reflow on every move (D).
            let (ox, oy) = canvas_origin_pm.get();
            let local = (cx - ox, cy - oy);
            let hit = scene_pm.borrow().as_ref().and_then(|s| s.pick(local));
            let mut lh = last_hover_pm.borrow_mut();
            if *lh != hit {
                *lh = hit.clone();
                on_event_pm.run(CanvasEvent::HoverNode(hit));
            }
        }
    };
```

  Note: this uses `s.camera.zoom(...)` directly (camera is `pub` on `Scene`, `scene.rs:32`); `s.pick(local)` becomes a 1-arg call in T④.2 — keep it 1-arg here for now.

- [ ] **Step 6: Remove pointers on up/cancel + rebase baseline.** In `on_pointerup` (`galaxy_canvas.rs:266-290`) and `on_pointercancel` (`:293-295`), drop the lifted pointer from the map; when fewer than 2 remain, clear the pinch baseline (and rebase if exactly 2 still remain — handled lazily by the move handler's per-frame rebase, so we only clear). Add clones before each closure (near line 265 and 292):

```rust
    let active_ptrs_pu = active_ptrs.clone();
    let pinch_base_pu = pinch_base.clone();
```
```rust
    let active_ptrs_pc = active_ptrs.clone();
    let pinch_base_pc = pinch_base.clone();
```

  In `on_pointerup`, after `ptr_down_pu.set(false);` (line 289), append before the closing brace:

```rust
        // Drop this pointer; below 2 active pointers ends the pinch gesture.
        {
            let mut ptrs = active_ptrs_pu.borrow_mut();
            ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
            if ptrs.len() < 2 {
                pinch_base_pu.set(0.0);
            }
        }
```

  Replace `on_pointercancel` (lines 293-295) with:

```rust
    let on_pointercancel = move |ev: web_sys::PointerEvent| {
        ptr_down_pc.set(false);
        let mut ptrs = active_ptrs_pc.borrow_mut();
        ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
        if ptrs.len() < 2 {
            pinch_base_pc.set(0.0);
        }
    };
```

- [ ] **Step 7: Commit.** `git add interfaces/webchat/src/views/canvas/gl/pinch.rs interfaces/webchat/src/views/canvas/gl/mod.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs` + `git commit -m "panel: add two-finger pinch-zoom to galaxy canvas via Pointer Events"`

---

### Task ④.2: 移动端 44px pick radius + ~75ms hover-pick 节流

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs:170-179`（`pick` 接 `radius_px`）, `:58-84`（`Scene::new` 暂不动签名，44 经新 field 或 `pick` 入参）
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs`（`is_mobile` prop + 节流时间戳 cell + pick 调用传半径）
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:321-330`（传 `is_mobile` 给 `GalaxyCanvas`）
- Test: compile + browser QA — no unit test（`pick_node(...,radius)` 已有半径单测覆盖纯逻辑，`picking.rs:59-82`）

**Interfaces:**
- Consumes: `Scene::pick(&self, cursor) -> Option<String>`（现 `scene.rs:170`，硬编码 18.0 在 `:176`）；`pick_node(..., radius_px)`（`picking.rs:6`，半径已是入参）。
- Produces: `Scene::pick(&self, cursor, radius_px: f32) -> Option<String>`（新签名，两个 caller：`galaxy_canvas.rs` move-hover + up-click）；`GalaxyCanvas` 新 prop `is_mobile: RwSignal<bool>`。

- [ ] **Step 1: 把 `Scene::pick` 的半径改为入参。** Replace `scene.rs:170-179` (`pick`):

```rust
    /// Screen-space picking: project all nodes through the last-frame view-proj
    /// and return the node id nearest the cursor (within `radius_px`), or `None`.
    /// The caller passes a larger radius on touch (≈44px WCAG target) to absorb
    /// fat-finger imprecision; mouse uses the tight 18px default.
    pub fn pick(&self, cursor: (f32, f32), radius_px: f32) -> Option<String> {
        super::picking::pick_node(
            &self.last_vp,
            &self.data.nodes,
            (self.width as f32, self.height as f32),
            cursor,
            radius_px,
        )
        .map(|i| self.data.nodes[i as usize].id.clone())
    }
```

- [ ] **Step 2: 加 `is_mobile` prop + 节流 cell 到 `GalaxyCanvas`.** Add the prop to the component signature in `galaxy_canvas.rs:37-55`. Insert after the `highlight_edges_request` prop (line 54), before the closing `)`:

```rust
    /// Mobile flag: widens the touch pick radius to ≈44px and throttles
    /// hover-picking to ~75ms (touch movement is coarse). Desktop stays at the
    /// tight 18px radius with per-move picking.
    is_mobile: RwSignal<bool>,
```

  Then add a throttle timestamp cell next to the other state cells — after the `pinch_base` cell from T④.1 (or after `down_pos` line 66 if T④.1 not yet merged; they're independent additions):

```rust
    // Last hover-pick time (ms) for mobile throttle. Desktop ignores it.
    let last_hover_pick_ms: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
```

- [ ] **Step 3: Use 44px radius + 75ms throttle in the hover/click pick calls.** Three pick sites must pass the radius and (hover only) honor the throttle. Define the constants near the top-level `CLICK_THRESHOLD_PX` const (`galaxy_canvas.rs:27`):

```rust
/// Touch pick radius (CSS px) — WCAG 2.5.5 minimum 44px target. High-DPI is
/// handled by the browser's client-pixel coordinate space (no manual scaling).
const TOUCH_PICK_RADIUS_PX: f32 = 44.0;
/// Mouse pick radius — tight, matches the previous hardcoded value.
const MOUSE_PICK_RADIUS_PX: f32 = 18.0;
/// Minimum ms between hover picks on mobile (coarse touch movement).
const MOBILE_HOVER_PICK_MS: f64 = 75.0;
```

  In `on_pointermove`'s hover branch (the `else` block — `galaxy_canvas.rs:246-258` pre-T④.1, or the equivalent block after T④.1), clone `is_mobile` + `last_hover_pick_ms` before the closure:

```rust
    let last_hover_pick_pm = last_hover_pick_ms.clone();
```
  (`is_mobile` is `Copy`, captured directly.) Replace the hover `else` branch body with:

```rust
        } else {
            // Hover (no button down): pick and emit HoverNode on transition.
            // On mobile, throttle picks to MOBILE_HOVER_PICK_MS and widen the
            // pick radius to absorb coarse touch movement.
            let mobile = is_mobile.get_untracked();
            let radius = if mobile { TOUCH_PICK_RADIUS_PX } else { MOUSE_PICK_RADIUS_PX };
            if mobile {
                let now = perf_now();
                if now - last_hover_pick_pm.get() < MOBILE_HOVER_PICK_MS {
                    return;
                }
                last_hover_pick_pm.set(now);
            }
            let (ox, oy) = canvas_origin_pm.get();
            let local = (cx - ox, cy - oy);
            let hit = scene_pm.borrow().as_ref().and_then(|s| s.pick(local, radius));
            let mut lh = last_hover_pm.borrow_mut();
            if *lh != hit {
                *lh = hit.clone();
                on_event_pm.run(CanvasEvent::HoverNode(hit));
            }
        }
```

  In `on_pointerup`'s click-pick (`galaxy_canvas.rs:279-287`), pass the radius — replace the `if dist < CLICK_THRESHOLD_PX { ... }` block:

```rust
            // Click (not drag): pick the node under the cursor. Touch uses the
            // wide 44px radius so fat-finger taps still land on a node.
            if dist < CLICK_THRESHOLD_PX {
                let radius = if is_mobile.get_untracked() {
                    TOUCH_PICK_RADIUS_PX
                } else {
                    MOUSE_PICK_RADIUS_PX
                };
                let (ox, oy) = canvas_origin_pu.get();
                let local = (cx - ox, cy - oy);
                let hit = scene_pu.borrow().as_ref().and_then(|s| s.pick(local, radius));
                match hit {
                    Some(id) => on_event_pu.run(CanvasEvent::SelectNode(id)),
                    None => on_event_pu.run(CanvasEvent::DeselectNode),
                }
            }
```

  (`is_mobile` `Copy` — captured directly; no clone needed in `on_pointerup`.)

- [ ] **Step 4: Pass `is_mobile` from `CanvasView` into `GalaxyCanvas`.** In `canvas/mod.rs`, get `ViewportState` and thread it. Add the import + context near the top of `RadialCanvasView` (after `let mem = expect_context::<MemoryState>();` — `mod.rs:37`):

```rust
    let viewport = expect_context::<crate::state::viewport::ViewportState>();
    let is_mobile = viewport.is_mobile;
```

  Then add the prop to the `<GalaxyCanvas ... />` call (`mod.rs:321-330`), after `highlight_edges_request=highlight_edges_request` (line 329):

```rust
                is_mobile=is_mobile
```

- [ ] **Step 5: Compile gate (group with later tasks if running together).** `cargo check --target wasm32-unknown-unknown -p aleph-panel` — verify the new `pick(local, radius)` signature compiles at all three call sites + the new prop wires. (Run ONCE for T④.2/④.3 together if doing them back-to-back, per cargo frugality.)

- [ ] **Step 6: Commit.** `git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs interfaces/webchat/src/views/canvas/mod.rs` + `git commit -m "panel: widen canvas pick radius to 44px and throttle hover-pick on mobile"`

---

### Task ④.3: 节点详情面板 → max-sm 底部 sheet（纯 CSS）

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:332-338`（详情面板 wrapper class）
- Test: compile + browser QA — no unit test（纯 CSS reflow）

**Interfaces:**
- Consumes: `NodeDetailPanel { excerpts: RwSignal<HashMap<String, NodeExcerpt>> }`（`node_detail_panel.rs:34-37`，无布局壳，由 wrapper 定位）。
- Produces: 无新接口（仅 class 变更）。

- [ ] **Step 1: 桌面右侧面板 → 手机底部全宽 sheet。** Replace the detail-panel wrapper view in `canvas/mod.rs:332-338`. Desktop keeps `bottom-0 right-0 w-72`; mobile (`max-sm:`) becomes a full-width bottom sheet with safe-area padding (R-20) and a rounded top:

```rust
            // NodeDetailPanel: overlay when a node is selected in the galaxy.
            // Desktop: docked bottom-right card. Mobile (max-sm): full-width
            // bottom sheet, notch-aware via safe-area-inset-bottom (R-20).
            {move || selected_node.get().map(|_| view! {
                <div class="absolute bottom-0 right-0 w-72 max-h-[60%] overflow-y-auto
                            bg-[#0d1120cc] border border-[#2a3060] rounded-tl-lg shadow-xl
                            backdrop-blur-sm
                            max-sm:left-0 max-sm:right-0 max-sm:w-full max-sm:max-h-[50%]
                            max-sm:rounded-tl-2xl max-sm:rounded-tr-2xl
                            max-sm:pb-[calc(env(safe-area-inset-bottom)+0.5rem)]">
                    <NodeDetailPanel excerpts=detail_panel_excerpts />
                </div>
            })}
```

- [ ] **Step 2: Compile gate.** Covered by the T④.2 `cargo check --target wasm32-unknown-unknown -p aleph-panel` if grouped; otherwise run it once here. (CSS-class-only change — compiles trivially; the gate confirms no `view!` macro typo.)

- [ ] **Step 3: Commit.** `git add interfaces/webchat/src/views/canvas/mod.rs` + `git commit -m "panel: render galaxy node detail as bottom sheet on mobile"`

---

### Task ④.4: WebGL2 不支持 → 回退列表 + 内联 banner（§11 P-⑥）

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs:43-55`（加 `fallback` prop）, `:106-112`（`Scene::new` Err → 置 true）
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`（`fallback` RwSignal + watch Effect → `mem.memory_view.set(Table)`，传 prop）
- Modify: `interfaces/webchat/src/views/memory_hub/mod.rs:48-63`（内联 banner）
- Modify: `interfaces/webchat/locales/en.json` + `zh.json`（`memory.galaxy_unsupported`）
- Test: compile + browser QA — no unit test（GL init 路径无 native 单测；浏览器禁 WebGL2 验回退）

**Interfaces:**
- Consumes: `Scene::new(&el) -> Result<Scene, String>`（`scene.rs:58`，经 `context::from_canvas` 的 `Err`，`context.rs:19-22` 在 WebGL2 缺失返 Err）；`MemoryState.memory_view: RwSignal<MemoryView>`（`memory.rs:53`）；`MemoryView::Table`（`memory.rs:21`）。
- Produces: `GalaxyCanvas` 新 prop `fallback: RwSignal<bool>`（`CanvasView` 提供，watch 驱动切表 + banner）。

- [ ] **Step 1: 加 `fallback` prop 到 `GalaxyCanvas`。** Insert after the `is_mobile` prop (added in T④.2) in `galaxy_canvas.rs`:

```rust
    /// WebGL2-unsupported flag (§11 P-⑥). Set to `true` when `Scene::new`
    /// (via `context::from_canvas`) errors on mount. `CanvasView` watches this
    /// and switches the Memory hub to the Table view permanently, with an
    /// inline banner explaining the galaxy is unavailable on this device.
    fallback: RwSignal<bool>,
```

- [ ] **Step 2: 挂载失败时置 `fallback = true`。** In the init Effect (`galaxy_canvas.rs:106-112`), the `Scene::new` `Err` arm currently only logs and returns. Set the fallback signal there. Replace the `match Scene::new(&el)` block:

```rust
        match Scene::new(&el) {
            Ok(s) => *scene_init.borrow_mut() = Some(s),
            Err(e) => {
                web_sys::console::error_1(&format!("GalaxyCanvas GL init failed: {e}").into());
                // §11 P-⑥: WebGL2 unavailable → signal the host to fall back to
                // the Table view. Permanent switch (CanvasView watches this).
                fallback.set(true);
                return;
            }
        }
```

- [ ] **Step 3: 在 `CanvasView` 建 `fallback` 信号 + watch → 切 Table，传 prop。** In `canvas/mod.rs`, after the existing intent-channel signals (`mod.rs:115-119`), add:

```rust
    // WebGL2-fallback signal (§11 P-⑥): GalaxyCanvas sets this true when GL init
    // fails on mount; the watch Effect below switches Memory to the Table view.
    let canvas_fallback: RwSignal<bool> = RwSignal::new(false);
```

  Add a watch Effect (place after the fold→LOD Effect, `mod.rs:314-316`):

```rust
    // Watch the WebGL2-fallback flag: on the first true, permanently switch the
    // Memory hub to the Table view (the galaxy can't render on this device).
    // The inline banner is rendered by MemoryHub (reads this via a shared signal
    // is not needed — switching view is sufficient; banner keyed off a separate
    // RwSignal threaded through context would be heavier, so MemoryHub re-derives
    // unsupported state from its own canvas_fallback prop — see Step 5).
    Effect::new(move || {
        if canvas_fallback.get() {
            mem.memory_view.set(MemoryView::Table);
        }
    });
```

  Thread the prop into `<GalaxyCanvas ... />` (after `is_mobile=is_mobile`):

```rust
                fallback=canvas_fallback
```

- [ ] **Step 4: 把 `canvas_fallback` 提升到 `MemoryState` 以便 banner 读取。** The banner lives in `MemoryHub` (sibling of `CanvasView`), so the flag must be reachable there. Add a field to `MemoryState` (`state/memory.rs:42-56`) — insert after `search_nonce` (line 55):

```rust
    /// WebGL2-unsupported flag, set by the galaxy canvas on GL-init failure
    /// (§11 P-⑥). MemoryHub reads it to show the "galaxy unsupported" banner;
    /// CanvasView's watch Effect uses it to force the Table view.
    pub galaxy_unsupported: RwSignal<bool>,
```

  Initialize it in `MemoryState::new` (`memory.rs:79-92`) — add after `search_nonce: RwSignal::new(0),` (line 91):

```rust
            galaxy_unsupported: RwSignal::new(false),
```

  Update `canvas/mod.rs` to use the shared signal instead of a local one: replace the `let canvas_fallback: RwSignal<bool> = RwSignal::new(false);` from Step 3 with:

```rust
    let canvas_fallback = mem.galaxy_unsupported;
```

  (The watch Effect and the `fallback=canvas_fallback` prop wiring from Step 3 stay unchanged — they now reference the shared signal.)

- [ ] **Step 5: Inline banner in `MemoryHub` over the Table view.** In `memory_hub/mod.rs`, read the flag and render a banner when set. Add after `let is_graph = Memo::new(...)` (`mod.rs:46`):

```rust
    let galaxy_unsupported = mem.galaxy_unsupported;
    let i18n = crate::i18n::use_i18n();
```

  Replace the Table-view `<div>` block (`mod.rs:56-61`) to prepend the banner:

```rust
            <div
                class="absolute inset-0 overflow-y-auto"
                style:display=move || if is_graph.get() { "none" } else { "block" }
            >
                {move || galaxy_unsupported.get().then(|| view! {
                    <div class="px-3 py-2 text-xs text-amber-200 bg-amber-900/30
                                border-b border-amber-700/40">
                        {crate::i18n::t!(i18n, memory.galaxy_unsupported)}
                    </div>
                })}
                <Memory />
            </div>
```

  (Confirm the `t!` macro path matches existing usage — `node_detail_panel.rs:9` imports `use crate::i18n::{t, use_i18n};`; if `t!` is the bracket form, match that local convention exactly. Grep `grep -rn 't!(' interfaces/webchat/src/views/memory_hub` is unnecessary — `node_detail_panel.rs` is the reference.)

- [ ] **Step 6: i18n keys.** Add to `interfaces/webchat/locales/en.json` under the `memory` object:

```json
    "galaxy_unsupported": "Galaxy view isn't supported on this device — showing the list instead.",
```

  And `interfaces/webchat/locales/zh.json` under `memory`:

```json
    "galaxy_unsupported": "本设备不支持星系视图，已切换为列表。",
```

  (Match the existing JSON nesting/trailing-comma style of the `memory` block — open both files and place the key alongside sibling memory keys.)

- [ ] **Step 7: Compile gate.** `cargo check --target wasm32-unknown-unknown -p aleph-panel` — confirms the new `MemoryState` field, the `fallback` prop, the watch Effect, and the i18n `t!` key all compile. (leptos_i18n generates the key accessor at build time, so a missing/mismatched JSON key fails HERE — this gate doubles as the i18n check.)

- [ ] **Step 8: Commit.** `git add interfaces/webchat/src/views/canvas/galaxy_canvas.rs interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/src/state/memory.rs interfaces/webchat/src/views/memory_hub/mod.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json` + `git commit -m "panel: fall back to memory table with banner when WebGL2 is unavailable"`

---

### Task ④.5: 手机性能护栏 — bloom_level 信号 + MAX_SETTLE_STEPS 钳制（§11 P-⑦）

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs:17`（`MAX_SETTLE_STEPS` → 实例字段）, `:27-84`（`Scene::new` 接 `settle_cap`+`bloom_level` 字段）, `:264-335`（`render` 读 `bloom_level` 门控 `bloom.run`）, `:279`（用字段而非 const）
- Modify: `interfaces/webchat/src/views/canvas/gl/bloom.rs:181-261`（`run(gl, intensity)`）
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs:106`（`Scene::new` 调用传 mobile 参数）
- Test: compile + browser QA — no unit test（GL-bound；bloom 纯逻辑 `gaussian_weights` 已有测，`bloom.rs:281-291`）

**Interfaces:**
- Consumes: `BloomPipeline::run(&self, gl)`（现 `bloom.rs:181`，composite `u_intensity` 硬编 0.9 在 `:251-252`）；`is_mobile`（galaxy_canvas prop，T④.2 已加）。
- Produces: `Scene::new(canvas, settle_cap: u32, bloom_level: f32) -> Result<Scene, String>`（新签名，唯一 caller = `galaxy_canvas.rs:106`）；`BloomPipeline::run(&self, gl, intensity: f32)`。

- [ ] **Step 1: `Scene` 加 `settle_cap` + `bloom_level` 字段 + 改 `new` 签名。** In `scene.rs`, replace the `MAX_SETTLE_STEPS` const (`:16-17`) with a mobile/desktop pair, and add two struct fields. Replace lines 16-17:

```rust
/// Maximum force-layout steps per new graph before switching to idle drift.
/// Desktop default; mobile clamps lower (§11 P-⑦) to settle faster on weaker GPUs.
const MAX_SETTLE_STEPS_DESKTOP: u32 = 400;
/// Mobile settle cap — fewer steps = faster convergence, less GPU churn.
pub const MAX_SETTLE_STEPS_MOBILE: u32 = 220;
```

  Add fields to the `Scene` struct — after `filtered_edges: Vec<(u32, u32)>,` (`scene.rs:54`):

```rust
    /// Force-layout step cap for this scene (desktop 400 / mobile ~220). §11 P-⑦.
    settle_cap: u32,
    /// Bloom intensity in [0,1]. 0 = bloom off (skip the pipeline entirely);
    /// >0 scales the composite intensity. Mobile defaults low/off (§11 P-⑦).
    bloom_level: f32,
```

  Change `Scene::new` (`scene.rs:58`) to accept the two params and store them. Replace the signature line + the struct-literal tail:

```rust
    pub fn new(canvas: &HtmlCanvasElement, settle_cap: u32, bloom_level: f32) -> Result<Scene, String> {
        let ctx = GlContext::from_canvas(canvas)?;
        let nodes = NodeRenderer::new(&ctx.gl)?;
        let edges = EdgeRenderer::new(&ctx.gl)?;
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;
        let bloom = BloomPipeline::new(&ctx.gl, w, h)?;
        Ok(Scene {
            ctx,
            nodes,
            edges,
            bloom,
            camera: OrbitCamera::new(800.0),
            data: GraphData::default(),
            width: w,
            height: h,
            last_t: 0.0,
            layout: None,
            settling: false,
            settle_steps: 0,
            last_vp: Mat4::identity(),
            highlight: None,
            highlight_edges: None,
            lod: 0.0,
            filtered_edges: Vec::new(),
            settle_cap,
            bloom_level: bloom_level.clamp(0.0, 1.0),
        })
    }
```

- [ ] **Step 2: `render` 用 `self.settle_cap` + 门控 bloom。** In `scene.rs`, replace the settle-cap comparison (`:279`) — change `self.settle_steps >= MAX_SETTLE_STEPS` to use the field:

```rust
                if layout.converged() || self.settle_steps >= self.settle_cap {
                    self.settling = false;
                }
```

  Replace the bloom call at the end of `render` (`scene.rs:332-334`):

```rust
        // --- Bloom pass: bright-pass → blur → composite to default FBO ---
        // §11 P-⑦: skip the whole pipeline when bloom is off (mobile default 0),
        // saving the 4-pass half-res gaussian on weak GPUs. Otherwise scale the
        // composite intensity by bloom_level.
        if self.bloom_level > 0.0 {
            self.bloom.run(gl, self.bloom_level);
        }
```

  ⚠️ When `bloom_level == 0.0`, the scene was rendered into `scene_fbo` (not the default framebuffer), so skipping `bloom.run` would leave the screen black. Add a direct blit/composite when bloom is off — bind the default framebuffer and run the composite pass with zero bloom contribution. Simplest correct approach: keep `bloom.run` always but parameterize intensity (it composites scene + bloom*intensity to the default FBO). So instead of the `if` skip above, replace `render`'s bloom tail with an **unconditional** call passing the level:

```rust
        // --- Bloom pass: bright-pass → blur → composite to default FBO ---
        // §11 P-⑦: bloom_level scales the bloom contribution in the composite.
        // The pipeline always runs (it also blits the scene FBO to the screen),
        // but at level 0 the composite adds zero bloom — cheap-ish, correct.
        self.bloom.run(gl, self.bloom_level);
```

  (Rationale: the composite pass is what copies scene→screen; fully skipping it leaves the default framebuffer unwritten = black. Parameterizing intensity is the surgical correct fix. True full-skip would need a separate scene-blit shader — out of scope, YAGNI.)

- [ ] **Step 3: `BloomPipeline::run` 接 `intensity`.** In `bloom.rs`, change `run(&self, gl)` (`:181`) to `run(&self, gl, intensity: f32)` and feed it to the composite `u_intensity` uniform (currently hardcoded `0.9` at `:251-252`). Replace the signature line:

```rust
    pub fn run(&self, gl: &Gl, intensity: f32) {
```

  Replace the composite intensity uniform (`bloom.rs:251-252`):

```rust
        let loc = gl.get_uniform_location(&self.prog_composite, "u_intensity");
        // §11 P-⑦: scale the bloom contribution. 0 = scene only (no glow).
        gl.uniform1f(loc.as_ref(), intensity.clamp(0.0, 1.0) * 0.9);
```

- [ ] **Step 4: `galaxy_canvas.rs` 调 `Scene::new` 传 mobile 参数。** In the init Effect (`galaxy_canvas.rs:106`), the `Scene::new(&el)` call must pass the settle cap and bloom level derived from `is_mobile`. Compute them just before the `match` (after line 104, before `match Scene::new`):

```rust
        // §11 P-⑦: mobile perf guardrails — fewer settle steps, bloom off.
        let mobile = is_mobile.get_untracked();
        let settle_cap = if mobile {
            super::gl::scene::MAX_SETTLE_STEPS_MOBILE
        } else {
            400
        };
        let bloom_level = if mobile { 0.0 } else { 1.0 };

        match Scene::new(&el, settle_cap, bloom_level) {
            Ok(s) => *scene_init.borrow_mut() = Some(s),
            Err(e) => {
                web_sys::console::error_1(&format!("GalaxyCanvas GL init failed: {e}").into());
                fallback.set(true);
                return;
            }
        }
```

  (`is_mobile` is the `RwSignal<bool>` prop from T④.2; this task depends on T④.2 having added it. `MAX_SETTLE_STEPS_MOBILE` is `pub` from Step 1; desktop `400` is inlined since `MAX_SETTLE_STEPS_DESKTOP` is private to scene.rs — alternatively make it `pub` and reference it. Use `pub const MAX_SETTLE_STEPS_DESKTOP` if you prefer referencing both; the `400` literal here keeps scene.rs's desktop const private. Pick one and be consistent.)

- [ ] **Step 5: Compile gate (group T④.5 alone — touches GL signatures).** `cargo check --target wasm32-unknown-unknown -p aleph-panel` — verifies the new `Scene::new(_, _, _)` and `BloomPipeline::run(_, _)` signatures propagate to their single call sites. The native bloom test (`gaussian_weights`) is unaffected; no need to re-run it.

- [ ] **Step 6: Commit.** `git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/gl/bloom.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs` + `git commit -m "panel: signal-gate galaxy bloom and clamp settle steps on mobile"`

---

### Task ④.6: 工作流末端 browser QA（390px 实测，含 STALE-EMBED 重编）

**Files:** none (verification only)

**Interfaces:** Consumes everything from T④.1–④.5.

- [ ] **Step 1: 一次性 WASM 重建 + server 重编（STALE-EMBED，R-15）。** Panel WASM 经 `rust_embed` 在 `aleph-server` **编译期**静态嵌入；改完源码后运行中的 daemon 仍 serve 旧 WASM。执行：`just wasm`（重建 dist），再重编 server binary（`cargo build --bin aleph-server` 或 `just build`），最后替换运行中的 binary。**不重编 = 看不到任何改动**。验证嵌入用 served wasm size 或 `grep -a`，**不**用 `strings`（rust_embed 压缩存储）。

- [ ] **Step 2: chrome-devtools 390px — pinch-zoom（T④.1）。** Resize page to 390px。打开 Memory → 切到「星系」视图（手机默认 Table，需手动切 Graph）。用合成双指 pointer 事件（或 trackpad pinch emulate）验证：两指拉开 = 放大（相机靠近），捏合 = 缩小；单指仍 orbit/pan；一指中途抬起不跳变。**Look for**: 双指距离变化时相机距离平滑变化，无跳变/无 NaN（节点不消失）。

- [ ] **Step 3: 390px — 44px tap + 节流（T④.2）。** 用合成 touch tap 点节点附近（≤44px 偏移）→ 应选中并出详情；桌面（resize 回 >640px）窄半径仍需精确点中。**Look for**: 手机端胖手指（偏离节点中心 ~30px）仍能选中；hover label 在快速拖动时不每帧闪烁（75ms 节流）。

- [ ] **Step 4: 390px — 底部 sheet（T④.3）。** 选中节点 → 详情面板从底部全宽弹出，圆角顶、不溢出右边、底部留 safe-area。**Look for**: `max-sm:` 下面板 `left-0 right-0 w-full`，桌面回 `w-72` 右下角。

- [ ] **Step 5: WebGL2 回退（T④.4）。** 在 chrome-devtools 禁用 WebGL2（`--disable-webgl2` 或 evaluate_script 覆盖 `getContext('webgl2')` 返 null）→ 重载 → Memory 应自动切 Table + 顶部琥珀色 banner「本设备不支持星系视图」。**Look for**: 不崩溃、不黑屏，banner 文案随 en/zh 语言切换。

- [ ] **Step 6: 性能护栏（T④.5）。** 390px 下星系视图 bloom 关闭（节点无柔光晕，仅清晰星核）；沉降更快（~220 步内停）。桌面仍有 bloom 柔光。**Look for**: 手机帧率不掉（performance trace 可选）；桌面视觉无回退。

- [ ] **Step 7: 无需 commit**（纯验证）。若 QA 暴露问题，回对应 task 修复并补提交。

# iPhone 原生 Settings 详情屏 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 Settings landing 能钻入的 5 条 route 重建为原生 iOS 详情屏(drill-in + `‹ Settings` 返回 + 保留 TabBar),并修复底部 TabBar 在真机被浏览器工具栏遮挡的问题。

**Architecture:** 抽共享 `PhoneShell`/`PhoneTabBar`(承载 `h-dvh` 全屏外壳 + 顶栏 + 底栏),landing 重构为用它;`SettingsRouter` 对 5 条 route 各加 `FormFactor::Phone` 分支渲染对应 `PhoneXxx` 屏;每屏调用与桌面页**同一套** `crate::api::*` / `appearance.rs`,只用 iOS 组件重建表现。

**Tech Stack:** Rust + Leptos 0.7 + WASM (`aleph-panel`),`leptos_router`,Tailwind v4。

## Global Constraints

- 只改 `interfaces/webchat/`,**零 core**;**不碰 `platform/wide/`**;phone 代码只落 `platform/phone/`;**零新依赖**。
- **绝不复用桌面表现**:用 iOS 组件(`.list`/`.cell`/选择 cell/iOS toggle/swatch)重建;只复用桌面页**数据层**(API/`appearance.rs`)。R2(UI 唯一源)/ R4(纯 I/O)。
- **cargo 极度节制**:Task 1–6 实现者**不跑任何 cargo/just/npm**,只做非 cargo 自检(grep/文件存在)。唯一一次构建(`just wasm` + 重 embed server)在 Task 7,由 controller 跑。移动文件/加模块后 RA 可能报 `unlinked-file`/`E0583`/`E0432` 等**陈旧假错** → 以 Task 7 实编为准,勿据 RA 增删。
- **提交**:未经用户明确要求**不 commit、不 push**(本计划"commit"步骤=保留工作树语义,跳过 `git commit`)。English `panel(phone): <desc>`,无 attribution。
- 全屏外壳根 = `fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col`(`h-dvh` 必须,替代 `inset-0`:移动浏览器布局视口底在工具栏后)。
- 详情屏顶栏左侧 `‹ Settings` 返回(`use_navigate("/settings")`),保留底部 TabBar(Settings `tabitem-active`)。
- 重型两屏(Providers/Embeddings)**v1 聚焦**:列出已配置 + 改 key/启用/设默认或活跃;完整 add/选型号/OAuth/test/presets/reembed **不做**。
- 接口跨任务一致:`PhoneShell{title:&'static str, back:Option<&'static str>, children}` · `PhoneTabBar()` · `PhoneConnection/PhoneAppearance/PhoneModelRoute/PhoneProviders/PhoneEmbeddings`(均 `#[component] pub fn ... -> impl IntoView`)。
- 回复中文,代码注释英文。

---

## 文件结构(改动地图)

| 文件 | 动作 | 职责 |
|------|------|------|
| `platform/phone/shell.rs` | **新建** | `PhoneShell`(全屏外壳 + 顶栏 + dvh)+ `PhoneTabBar`(共享底栏) |
| `platform/phone/mod.rs` | 改 | `pub mod shell;`(`settings` 仍登记) |
| `platform/phone/settings.rs` → `platform/phone/settings/mod.rs` | **移动+重构** | landing 改用 `PhoneShell` |
| `platform/phone/settings/connection.rs` | **新建** | `PhoneConnection`(只读连接状态) |
| `platform/phone/settings/appearance.rs` | **新建** | `PhoneAppearance`(6 项本地选择) |
| `platform/phone/settings/model_route.rs` | **新建** | `PhoneModelRoute`(`RouteConfigApi`) |
| `platform/phone/settings/providers.rs` | **新建** | `PhoneProviders`(focused) |
| `platform/phone/settings/embeddings.rs` | **新建** | `PhoneEmbeddings`(focused) |
| `styles/ios.css` | 改 | 加选择-cell 勾选 + iOS toggle 样式 |
| `src/app.rs` | 改 | `SettingsRouter` 5 条 route 加 Phone 分支 + import 5 屏 |

---

## Task 1: 共享 `PhoneShell`/`PhoneTabBar` + landing 重构 + dvh

**Files:**
- Create: `interfaces/webchat/src/platform/phone/shell.rs`
- Move+Modify: `interfaces/webchat/src/platform/phone/settings.rs` → `interfaces/webchat/src/platform/phone/settings/mod.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`
- Modify: `interfaces/webchat/styles/ios.css`

**Interfaces:**
- Produces: `crate::platform::phone::shell::{PhoneShell, PhoneTabBar}`;`PhoneShell(title: &'static str, #[prop(optional)] back: Option<&'static str>, children: Children)`;`PhoneTabBar()`. `crate::platform::phone::settings::PhoneSettings`(路径不变,因 mod.rs 仍是 `settings` 模块根)。

- [ ] **Step 1: 建 `shell.rs` —— `PhoneTabBar`(从现 settings.rs 抽,4 项全可导航)**

把现 `platform/phone/settings.rs` 末尾的 `<div class="tabbar glass" ...>…4 个 tabitem…</div>` 整块**原样**搬进新组件(SVG/文案逐字保留),唯一改动:给原本无 `on:click` 的 `Settings` tabitem 加上导航,且去掉 landing 里写死的 `padding-bottom:calc(0.4rem + 16px)` 内联(改用 `.tabbar` 类自带的安全区 padding):

```rust
//! platform/phone/shell.rs
//! Shared iOS chrome for phone screens: a full-screen `h-dvh` shell (top bar +
//! scroll body + bottom tab bar) and the tab bar itself. `h-dvh` (not inset-0)
//! keeps the tab bar above the mobile browser's bottom toolbar.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

/// Bottom tab bar shared by every phone screen (landing + detail). Settings is
/// the active tab on all settings screens. I/O-only: each item navigates.
#[component]
#[must_use]
pub fn PhoneTabBar() -> impl IntoView {
    let navigate = use_navigate();
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };
    view! {
        <div class="tabbar glass" style="flex:none;">
            <button class="tabitem" on:click=go("/")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.4 8.4 0 0 1-8.5 8.5 8.7 8.7 0 0 1-4-1L3 20l1-5.5a8.4 8.4 0 0 1-1-4A8.4 8.4 0 0 1 11.5 2 8.4 8.4 0 0 1 21 11.5z"></path></svg>
                "Chat"
            </button>
            <button class="tabitem" on:click=go("/memory")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="7" r="2.4"></circle><circle cx="18" cy="8" r="2.4"></circle><circle cx="11" cy="17" r="2.4"></circle><path d="M8 8.4l1.5 6.4M15.8 9.6L12.6 15.6"></path></svg>
                "Memory"
            </button>
            <button class="tabitem" on:click=go("/agents")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"></circle><path d="M5 21a7 7 0 0 1 14 0"></path></svg>
                "Agents"
            </button>
            <button class="tabitem tabitem-active" on:click=go("/settings")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 6.6 19l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 4 13.6H4a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 5 6.6l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 10.4 4V4a2 2 0 1 1 4 0v.1A1.6 1.6 0 0 0 17 5l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"></path></svg>
                "Settings"
            </button>
        </div>
    }
}
```

- [ ] **Step 2: 建 `shell.rs` —— `PhoneShell`(全屏外壳 + 顶栏 + dvh,在 PhoneTabBar 之上)**

追加到 `shell.rs`:

```rust
/// Full-screen iOS shell: gradient bg, glass top bar (optional `‹ Settings`
/// back + title), scroll body, shared bottom tab bar. `back=None` = landing
/// (left-aligned title, no back); `back=Some(route)` = detail (centered title +
/// back button). Root uses `h-dvh` so the tab bar clears the mobile browser bar.
#[component]
#[must_use]
pub fn PhoneShell(
    title: &'static str,
    #[prop(optional)] back: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    let navigate = use_navigate();
    let back_btn = back.map(|to| {
        let navigate = navigate.clone();
        view! {
            <button
                style="position:absolute; left:10px; top:50%; transform:translateY(-10%); display:flex; align-items:center; gap:2px; background:none; border:0; cursor:pointer; color:var(--color-primary); font:inherit; font-size:16px; padding:4px 6px 4px 0;"
                on:click=move |_| navigate(to, NavigateOptions::default())
            >
                <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 6 9 12 15 18"></polyline></svg>
                "Settings"
            </button>
        }
    });
    // Title: left-aligned on the landing; centered on detail screens (iOS nav).
    let title_style = if back.is_some() {
        "width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);"
    } else {
        "flex:1; font-size:20px; font-weight:700; letter-spacing:-0.02em; color:var(--color-text-primary);"
    };
    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:radial-gradient(120% 55% at 50% 0%, oklch(0.62 0.10 310 / 0.14), transparent 62%),radial-gradient(120% 45% at 50% 100%, oklch(0.60 0.09 250 / 0.10), transparent 60%),var(--color-surface);"
        >
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; gap:8px; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                {back_btn}
                <span style=title_style>{title}</span>
            </div>
            <div
                class="cc-hide-scroll"
                style="flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:20px; padding:16px 16px 18px;"
            >
                {children()}
            </div>
            <PhoneTabBar/>
        </div>
    }
}
```

- [ ] **Step 3: 移动并重构 landing 为 `settings/mod.rs`,改用 `PhoneShell`**

`git mv interfaces/webchat/src/platform/phone/settings.rs interfaces/webchat/src/platform/phone/settings/mod.rs`。然后在 `settings/mod.rs` 里:删掉外层根 `<div class="fixed ...">` + 顶栏 `<div class="glass">Settings</div>` + 底部 `<div class="tabbar glass">…</div>`(这三段由 `PhoneShell`/`PhoneTabBar` 接管),把**三组 `<div>`(连接/AI/外观)**作为 `PhoneShell` 的 children;`use_navigate` 的 `go` 保留给 cell 用。成品骨架:

```rust
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

#[component]
#[must_use]
pub fn PhoneSettings() -> impl IntoView {
    let navigate = use_navigate();
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };
    view! {
        <PhoneShell title="Settings">
            // 连接 / AI / 外观 三组 —— 原 settings.rs 里的三个 <div> 原样保留
            // (cell 的 on:click=go(...) 不变)。
            <div> <div class="list-header">"连接"</div> <div class="list"> /* Connection cell */ </div> </div>
            <div> <div class="list-header">"AI"</div> <div class="list"> /* Providers/Embeddings/Model route */ </div> </div>
            <div> <div class="list-header">"外观"</div> <div class="list"> /* Theme/Accent/Material */ </div> </div>
        </PhoneShell>
    }
}
```
> 三组内部的 cell（SVG/文案/`go(...)` 路由）**逐字保留**自原 settings.rs，不重画。

并在 `settings/mod.rs` 顶部加子屏模块登记(供后续任务):
```rust
pub mod appearance;
pub mod connection;
pub mod embeddings;
pub mod model_route;
pub mod providers;
```
(这些文件由后续任务创建；本任务先登记会触发 RA `unlinked-file` 假错——忽略，Task 7 实编为准。**或**在各自任务里再加登记行；二选一，本计划选"各任务自加",故本步**不**预登记,只建 landing。)

修正:本步**只**登记当前存在的——即不加上面 5 行;每个后续任务创建子屏时自己加 `pub mod xxx;`。

- [ ] **Step 4: `platform/phone/mod.rs` 加 `pub mod shell;`**

现 `mod.rs` 有文档注释 + `pub mod settings;`。追加:
```rust
pub mod shell;
```

- [ ] **Step 5: `ios.css` 加选择-cell 勾选 + iOS toggle 样式**

在 `ios.css` 末尾追加(后续 Appearance/Model route 用):
```css
/* iOS selectable list row: a .cell that shows a primary checkmark when chosen. */
.cell-check { color: var(--color-primary); flex: none; opacity: 0; }
.cell-selected .cell-check { opacity: 1; }
.cell-selected .cell-title { color: var(--color-primary); }

/* iOS switch — a CSS toggle for boolean rows. Markup:
   <button class="ios-switch" aria-pressed=..><span class="ios-knob"></span></button> */
.ios-switch { width: 44px; height: 27px; border-radius: 9999px; background: var(--color-border); border: 0; padding: 2px; cursor: pointer; flex: none; transition: background-color .18s ease; }
.ios-switch[aria-pressed="true"] { background: var(--color-primary); }
.ios-switch .ios-knob { display: block; width: 23px; height: 23px; border-radius: 9999px; background: #fff; box-shadow: var(--shadow-sm); transition: transform .18s ease; }
.ios-switch[aria-pressed="true"] .ios-knob { transform: translateX(17px); }
```

- [ ] **Step 6: 非 cargo 自检**

```bash
cd /Volumes/TBU4/Workspace/Aleph
test -f interfaces/webchat/src/platform/phone/shell.rs && echo "shell.rs OK"
test -f interfaces/webchat/src/platform/phone/settings/mod.rs && echo "settings/mod.rs OK"
test ! -f interfaces/webchat/src/platform/phone/settings.rs && echo "old settings.rs moved OK"
grep -n 'pub mod shell;' interfaces/webchat/src/platform/phone/mod.rs
grep -c 'PhoneShell' interfaces/webchat/src/platform/phone/settings/mod.rs   # 期望 ≥2 (use + 用)
grep -c 'fixed inset-x-0 top-0 h-dvh' interfaces/webchat/src/platform/phone/shell.rs  # 期望 1
grep -c 'ios-switch\|cell-check' interfaces/webchat/styles/ios.css            # 期望 ≥2
```
Expected: 所有 OK + grep 命中。

- [ ] **Step 7: Commit(保留工作树,不 `git commit`,除非用户要求)**

```bash
git add -A interfaces/webchat/src/platform/phone/ interfaces/webchat/styles/ios.css
# 用户要求时再: git commit -m "panel(phone): extract PhoneShell/PhoneTabBar + h-dvh fix + landing refactor"
```

---

## Task 2: `PhoneConnection` + 路由分支

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings/connection.rs`
- Modify: `interfaces/webchat/src/platform/phone/settings/mod.rs`(加 `pub mod connection;`)
- Modify: `interfaces/webchat/src/app.rs`(`/settings/network` 加 Phone 分支 + import)

**Interfaces:**
- Consumes: `PhoneShell`(Task 1)。
- Produces: `crate::platform::phone::settings::connection::PhoneConnection`。

- [ ] **Step 1: 建 `connection.rs`(纯展示,复用 host 逻辑)**

```rust
//! iPhone Connection detail screen — read-only connection status. Reuses the
//! same `location.host` + loopback logic as the wide ConnectionSection
//! (connection form is decided by build, not toggled here — R4 pure I/O).

use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;

fn current_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default()
}
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}
fn is_loopback_host(host: &str) -> bool {
    let h = host_only(host);
    h.eq_ignore_ascii_case("localhost") || h == "::1" || h.starts_with("127.")
}

#[component]
#[must_use]
pub fn PhoneConnection() -> impl IntoView {
    let host = current_host();
    let host_present = !host.is_empty();
    let remote = host_present && !is_loopback_host(&host);
    let badge = if remote { "remote" } else { "local" };
    let badge_style = if remote {
        "margin-left:auto; font-size:12px; padding:2px 8px; border-radius:9999px; background:color-mix(in oklch, var(--color-warning, oklch(0.7 0.15 70)) 15%, transparent); color:var(--color-warning, oklch(0.55 0.15 70)); flex:none;"
    } else {
        "margin-left:auto; font-size:12px; padding:2px 8px; border-radius:9999px; background:color-mix(in oklch, var(--color-success, oklch(0.6 0.13 150)) 15%, transparent); color:var(--color-success, oklch(0.5 0.13 150)); flex:none;"
    };
    view! {
        <PhoneShell title="Connection" back="/settings">
            <div>
                <div class="list-header">"连接"</div>
                <div class="list">
                    <div class="cell">
                        <div class="cell-body"><div class="cell-title">"Core"</div></div>
                        {if host_present {
                            view! { <span class="cell-value mono" style="font-size:13px;">{host.clone()}</span> }.into_any()
                        } else {
                            view! { <span class="cell-value">"—"</span> }.into_any()
                        }}
                        <span style=badge_style>{badge}</span>
                    </div>
                </div>
            </div>
        </PhoneShell>
    }
}
```
> 集群节点管理本轮不做(spec §非目标)。`--color-warning`/`--color-success` 若 tailwind.css 已定义则直接用(本 fallback 仅防御)。实现者可先 `grep -n 'color-warning\|color-success' interfaces/webchat/styles/tailwind.css` 确认,若有就用 `var(--color-warning)` 去掉 fallback。

- [ ] **Step 2: 登记模块**

`settings/mod.rs` 顶部加:`pub mod connection;`

- [ ] **Step 3: `app.rs` 加 `/settings/network` Phone 分支**

import 区加:`use crate::platform::phone::settings::connection::PhoneConnection;`
`SettingsRouter` 把 `"/settings/network" => view! { <NetworkView /> }.into_any(),` 改为:
```rust
"/settings/network" => {
    if form_factor.form_factor.get() == FormFactor::Phone {
        view! { <PhoneConnection /> }.into_any()
    } else {
        view! { <NetworkView /> }.into_any()
    }
}
```
(`form_factor` / `FormFactor` 已在 SettingsRouter 作用域,landing 任务引入。)

- [ ] **Step 4: 非 cargo 自检**
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n 'pub mod connection;' interfaces/webchat/src/platform/phone/settings/mod.rs
grep -n 'PhoneConnection' interfaces/webchat/src/app.rs   # import + 分支 共 2
grep -c 'PhoneShell title="Connection" back="/settings"' interfaces/webchat/src/platform/phone/settings/connection.rs  # 1
```

- [ ] **Step 5: Commit**(保留工作树)
```bash
git add interfaces/webchat/src/platform/phone/settings/connection.rs interfaces/webchat/src/platform/phone/settings/mod.rs interfaces/webchat/src/app.rs
```

---

## Task 3: `PhoneAppearance` + 路由分支

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings/appearance.rs`
- Modify: `settings/mod.rs`(`pub mod appearance;`)、`app.rs`(`/settings/appearance` 分支 + import)

**Interfaces:**
- Consumes: `PhoneShell`;`crate::appearance::{ThemeMode, Accent, Material, FontScale, Roundness, Density, read_mode, read_accent, read_material, read_font_scale, read_roundness, read_density, apply_mode, apply_accent, apply_material, apply_font_scale, apply_roundness, apply_density}`。每个枚举有 `const ALL: [Self; N]` 与 `fn label(self) -> &'static str`;`Accent` 另有 `fn swatch(self) -> &'static str`、`fn id`。
- Produces: `crate::platform::phone::settings::appearance::PhoneAppearance`。

- [ ] **Step 1: 建 `appearance.rs`(6 项本地选择,即时 apply)**

每项一个 `.list`,iterate `Enum::ALL`,当前值 `read_*()`,点击 `apply_*()` 并更新本地信号驱动勾选态。用一个泛型 helper 渲染"选择组"。Accent 用 swatch 行。

```rust
//! iPhone Appearance detail screen — Theme / Accent / Material / 字号 / 圆角 /
//! 紧凑度. Reuses crate::appearance read_*/apply_* (local, instant; no API).

use crate::appearance::{
    apply_accent, apply_density, apply_font_scale, apply_material, apply_mode, apply_roundness,
    read_accent, read_density, read_font_scale, read_material, read_mode, read_roundness, Accent,
    Density, FontScale, Material, Roundness, ThemeMode,
};
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn PhoneAppearance() -> impl IntoView {
    view! {
        <PhoneShell title="Appearance" back="/settings">
            <SelectGroup
                header="主题"
                items=ThemeMode::ALL.to_vec()
                current=read_mode()
                label=|m: ThemeMode| m.label()
                on_pick=|m| apply_mode(m)
            />
            <AccentGroup/>
            <SelectGroup
                header="材质"
                items=Material::ALL.to_vec()
                current=read_material()
                label=|m: Material| m.label()
                on_pick=|m| apply_material(m)
            />
            <SelectGroup
                header="字号"
                items=FontScale::ALL.to_vec()
                current=read_font_scale()
                label=|m: FontScale| m.label()
                on_pick=|m| apply_font_scale(m)
            />
            <SelectGroup
                header="圆角"
                items=Roundness::ALL.to_vec()
                current=read_roundness()
                label=|m: Roundness| m.label()
                on_pick=|m| apply_roundness(m)
            />
            <SelectGroup
                header="紧凑度"
                items=Density::ALL.to_vec()
                current=read_density()
                label=|m: Density| m.label()
                on_pick=|m| apply_density(m)
            />
        </PhoneShell>
    }
}

/// One iOS single-select section: a `.list` whose rows show a checkmark on the
/// chosen value. Generic over any `Copy + PartialEq` appearance enum.
#[component]
fn SelectGroup<T, L, P>(
    header: &'static str,
    items: Vec<T>,
    current: T,
    label: L,
    on_pick: P,
) -> impl IntoView
where
    T: Copy + PartialEq + 'static,
    L: Fn(T) -> &'static str + Copy + 'static,
    P: Fn(T) + Copy + 'static,
{
    let selected = RwSignal::new(current);
    view! {
        <div>
            <div class="list-header">{header}</div>
            <div class="list">
                {items.into_iter().map(|item| {
                    view! {
                        <div
                            class="cell"
                            class:cell-selected=move || selected.get() == item
                            on:click=move |_| { on_pick(item); selected.set(item); }
                        >
                            <div class="cell-body"><div class="cell-title">{label(item)}</div></div>
                            <svg class="cell-check" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

/// Accent picker — swatch row (mirrors the landing's Accent cell).
#[component]
fn AccentGroup() -> impl IntoView {
    let selected = RwSignal::new(read_accent());
    view! {
        <div>
            <div class="list-header">"主题色"</div>
            <div class="list">
                <div class="cell" style="align-items:center;">
                    <div class="cell-body"><div class="cell-title">"Accent"</div></div>
                    <div style="display:flex; align-items:center; gap:8px; flex:none;">
                        {Accent::ALL.into_iter().map(|a| {
                            let style = format!("width:26px; height:26px; background:{};", a.swatch());
                            view! {
                                <span
                                    class="swatch"
                                    class:swatch-active=move || selected.get() == a
                                    style=style
                                    title=a.label()
                                    on:click=move |_| { apply_accent(a); selected.set(a); }
                                ></span>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>
        </div>
    }
}
```
> 实现者先 `grep -nE 'pub enum Roundness|pub enum Density|impl Roundness|impl Density|const ALL|fn label' interfaces/webchat/src/appearance.rs` 确认 `Roundness`/`Density` 同样有 `ALL` + `label()`(其余枚举已确认有)。若某枚举无 `ALL`/`label`,按其实际 API 调整(仍用 SelectGroup 模式)。`apply_*` 即时改 `<html>` + 存 localStorage,无需保存按钮。

- [ ] **Step 2: 登记 + 路由**(同 Task 2 模式)

`settings/mod.rs` 加 `pub mod appearance;`;`app.rs` import `use crate::platform::phone::settings::appearance::PhoneAppearance;` + `/settings/appearance` 分支 `Phone ? <PhoneAppearance/> : <AppearanceView/>`。

- [ ] **Step 3: 非 cargo 自检**
```bash
grep -n 'pub mod appearance;' interfaces/webchat/src/platform/phone/settings/mod.rs
grep -n 'PhoneAppearance' interfaces/webchat/src/app.rs   # 2
grep -c 'SelectGroup' interfaces/webchat/src/platform/phone/settings/appearance.rs  # ≥6 调用 + 定义
```

- [ ] **Step 4: Commit**(保留工作树)

---

## Task 4: `PhoneModelRoute` + 路由分支

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings/model_route.rs`
- Modify: `settings/mod.rs`、`app.rs`

**Interfaces:**
- Consumes: `PhoneShell`;`crate::context::DashboardState`;`crate::api::{RouteConfigApi, RouteConfigUpdate, RouteProviderInfo, RateLimit}`(与 `platform/wide/views/settings/route.rs` 同款 —— 实现者**先读** `route.rs` 摸清 `RouteConfigApi::get` 返回字段与 `update` 入参)。
- Produces: `crate::platform::phone::settings::model_route::PhoneModelRoute`。

- [ ] **Step 1: 读桌面 `route.rs` 摸数据契约**

Run: 实现者打开 `interfaces/webchat/src/platform/wide/views/settings/route.rs`,记下:`RouteConfigApi::get(&state).await` 返回的 view 字段(`mode`/`allow_cloud_escalation`/`local_provider`/`cloud_provider`/`providers: Vec<RouteProviderInfo>`/`load_balance`/`rate_limits`),`RouteConfigUpdate` 字段,`MODE_KEYS=["auto","always_local","always_cloud"]`,`LB_KEYS`。**数据加载/保存逻辑照搬,仅表现换 iOS。**

- [ ] **Step 2: 建 `model_route.rs`(iOS 重建)**

结构(用 Task 1 的 `PhoneShell` + Task 3 的选择-cell 模式 + Task 1 的 `.ios-switch`):
- 顶栏右侧"应用"按钮(`position:absolute; right:14px`,color primary)触发 `RouteConfigApi::update`(沿用 route.rs 的 `save` 闭包逻辑)。
- 分组 ①"模式":`.list`,3 行(`auto`/`always_local`/`always_cloud`),label 用中文(照 route.rs 的 `mode_auto/local/cloud` 文案或直接 "Auto"/"Always Local"/"Always Cloud"),选中态用 `cell-selected` + `cell-check`,点击 `mode.set(key)`。
- 分组 ②"负载均衡":`.list` 单行,值显示当前 `load_balance`,点击在 `LB_KEYS` 间循环 **或** 展开为一个选择 `.list`(用 SelectGroup 模式,header="负载均衡",items=LB_KEYS)。v1 用 SelectGroup 模式(单选列表)。
- 分组 ③ 仅当 `mode=="always_local"`:`.list` 单行 iOS toggle 行("允许云升级",`allow_escalation`):
  ```rust
  <div class="cell">
      <div class="cell-body"><div class="cell-title">"允许云升级"</div></div>
      <button class="ios-switch" attr:aria-pressed=move || allow_escalation.get().to_string()
              on:click=move |_| allow_escalation.update(|v| *v = !*v)>
          <span class="ios-knob"></span>
      </button>
  </div>
  ```
- 分组 ④"偏好供应商":local/cloud 各一行,值显示当前 pin(空=配置顺序),点击进入该 tier 的选择 `.list`(items 来自 `providers` 按 tier 过滤);v1 可简化为单选 `.list`(SelectGroup 模式,含"配置顺序"=空选项)。
- 分组 ⑤"限流":每个 `providers` 一行,右侧两个内联 `<input type="number">`(rpm/tpm),`on:input` 更新 `rate_limits`(照搬 route.rs 的 `parse_limit` + update 逻辑)。

> 复用 route.rs 的信号集合(`mode`/`allow_escalation`/`local_provider`/`cloud_provider`/`providers`/`load_balance`/`rate_limits`/`saving`/`saved`/`error`)与 `spawn_local` 加载/保存。**只换 view 表现**(卡片/下拉/复选 → iOS list/选择-cell/toggle/内联输入)。`parse_limit` helper 一并复制过来。

- [ ] **Step 3: 登记 + 路由 + 自检**(同 Task 2 模式;`app.rs` `/settings/model-route` 分支 `Phone ? <PhoneModelRoute/> : <RouteView/>`)
```bash
grep -n 'pub mod model_route;' interfaces/webchat/src/platform/phone/settings/mod.rs
grep -n 'PhoneModelRoute' interfaces/webchat/src/app.rs   # 2
grep -c 'RouteConfigApi' interfaces/webchat/src/platform/phone/settings/model_route.rs  # ≥2 (get+update)
```

- [ ] **Step 4: Commit**(保留工作树)

---

## Task 5: `PhoneProviders`(focused v1)+ 路由分支

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings/providers.rs`
- Modify: `settings/mod.rs`、`app.rs`

**Interfaces:**
- Consumes: `PhoneShell`;`DashboardState`;`crate::api::ProvidersApi::{list, set_default, update}` + `ProviderInfo`(实现者**先读** `interfaces/webchat/src/api/providers.rs` 记下 `ProviderInfo` 字段:`name`、`enabled`、是否 default/active、key 字段名)与 `platform/wide/views/settings/providers/{list.rs,detail_panel.rs}` 摸"改 key/启用/设默认"的具体 update 入参)。
- Produces: `crate::platform::phone::settings::providers::PhoneProviders`。

- [ ] **Step 1: 读 `api/providers.rs` + 桌面 providers 视图,摸 focused-v1 字段**

记下:`ProvidersApi::list -> Vec<ProviderInfo>` 字段(name/enabled/default…),`set_default(name)`,`update(...)` 改 key/enable 的入参形状。**focused v1 只用 list/set_default/update;不碰 create/delete/catalog/oauth/test_connection。**

- [ ] **Step 2: 建 `providers.rs`(iOS 聚焦版)**

结构:
- `PhoneShell title="Providers" back="/settings"`。
- 加载:`spawn_local` 调 `ProvidersApi::list`,存 `RwSignal<Vec<ProviderInfo>>`;`loading`/`error` 信号。
- 一个 `.list` 列出每个 provider:`.cell` = 名(`cell-title`)+ 默认/启用徽章(`cell-value`)+ `cell-chevron`;点击展开**就地编辑**(同屏 `<Show>` 一个编辑区,或点击进入 `.list` 编辑分组):
  - "设为默认":一行,点击 `ProvidersApi::set_default(name)` 后 reload list。
  - "启用":`ios-switch` 行,切换调 `ProvidersApi::update`(enabled=…)。
  - "API Key":一行内联 `<input type="password">` + "保存"调 `ProvidersApi::update`(key=…)。复用既有 `components/provider_key_field` 若其表现适配窄屏;否则裸 `<input>`。
- **不做**:新增 provider、catalog 选型号、OAuth、test_connection(spec §非目标;v1 注明)。

> 数据编排照搬桌面 providers 视图的 list/set_default/update 调用;表现换 iOS list + toggle + 内联输入。保持 R4(纯 I/O)。

- [ ] **Step 3: 登记 + 路由 + 自检**(`app.rs` `/settings/providers` 分支 `Phone ? <PhoneProviders/> : <ProvidersView/>`)
```bash
grep -n 'pub mod providers;' interfaces/webchat/src/platform/phone/settings/mod.rs
grep -n 'PhoneProviders' interfaces/webchat/src/app.rs   # 2
grep -c 'ProvidersApi::list\|set_default\|ProvidersApi::update' interfaces/webchat/src/platform/phone/settings/providers.rs  # ≥1
grep -c 'create\|delete\|catalog\|oauth\|test_connection' interfaces/webchat/src/platform/phone/settings/providers.rs  # 期望 0 (focused v1)
```

- [ ] **Step 4: Commit**(保留工作树)

---

## Task 6: `PhoneEmbeddings`(focused v1)+ 路由分支

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings/embeddings.rs`
- Modify: `settings/mod.rs`、`app.rs`

**Interfaces:**
- Consumes: `PhoneShell`;`DashboardState`;`crate::api::EmbeddingProvidersApi::{list, set_active, update}` + `EmbeddingProviderEntry`(实现者**先读** `interfaces/webchat/src/api/embedding.rs` 记下 `EmbeddingProviderEntry` 字段 + `platform/wide/views/settings/embedding_providers.rs` 摸 update/set_active 入参)。
- Produces: `crate::platform::phone::settings::embeddings::PhoneEmbeddings`。

- [ ] **Step 1: 读 `api/embedding.rs` + 桌面 embedding 视图**

记下 `EmbeddingProviderEntry` 字段(id/name/active/enabled/key…)、`set_active(id)`、`update(...)` 入参。**focused v1 只用 list/set_active/update;不碰 add/remove/presets/reembed/test。**

- [ ] **Step 2: 建 `embeddings.rs`(iOS 聚焦版,镜像 Task 5 模式)**

结构同 `PhoneProviders`,但用 `EmbeddingProvidersApi`:
- `PhoneShell title="Embeddings" back="/settings"`。
- `spawn_local` → `EmbeddingProvidersApi::list` → `RwSignal<Vec<EmbeddingProviderEntry>>`。
- `.list` 列出已配置;每项就地编辑:"设为活跃"(`set_active(id)` + reload)、"启用"(`ios-switch` → `update`)、"API Key"(内联 input → `update`)。
- **不做**:add、presets、reembed、test(v1 注明)。

> 表现/控件与 Task 5 一致(iOS list + toggle + 内联输入),仅 API 与字段不同。

- [ ] **Step 3: 登记 + 路由 + 自检**(`app.rs` `/settings/embedding-providers` 分支 `Phone ? <PhoneEmbeddings/> : <EmbeddingProvidersView/>`)
```bash
grep -n 'pub mod embeddings;' interfaces/webchat/src/platform/phone/settings/mod.rs
grep -n 'PhoneEmbeddings' interfaces/webchat/src/app.rs   # 2
grep -c 'EmbeddingProvidersApi::list\|set_active\|EmbeddingProvidersApi::update' interfaces/webchat/src/platform/phone/settings/embeddings.rs  # ≥1
grep -c 'EmbeddingProvidersApi::add\|presets\|reembed\|::test' interfaces/webchat/src/platform/phone/settings/embeddings.rs  # 期望 0
```

- [ ] **Step 4: Commit**(保留工作树)

---

## Task 7: 构建 + 验证(唯一一次 cargo,controller 跑)

**Files:** 无新增;编译、重 embed、视觉验证。

- [ ] **Step 1: 单次编译/构建门**
```bash
cd /Volumes/TBU4/Workspace/Aleph
just wasm
```
Expected: Tailwind Done + `wasm-release ... Finished` 无错 + `✓ panel dist OK` + `✓ WASM`。
> 含 `cargo build --target wasm32`(= 编译门,覆盖全部 6 个屏 + app.rs 接线)。RA 在加模块/移动文件后的 `unlinked-file`/`E0432`/`E0583` 假错以本步实编为准;真错按编译信息修(常见:枚举无 `ALL`/`label` → 按实际 API 调整;`ProviderInfo`/`EmbeddingProviderEntry` 字段名 → 对齐 api 模块)。

- [ ] **Step 2: dist 含新类/屏**
```bash
grep -c 'ios-switch\|cell-check' interfaces/webchat/dist/tailwind.css  # ≥1
```

- [ ] **Step 3: 重 embed server(rust_embed 编译期嵌入,需重编)**
```bash
pkill -x aleph-server 2>/dev/null || true
cargo build -p alephcore --bin aleph-server   # core 未变 → 仅重嵌 dist + relink (~40-50s)
```

- [ ] **Step 4: 起 daemon + 验 served==disk**
```bash
( target/debug/aleph-server >/tmp/aleph-srv.log 2>&1 & )
code=$(curl -s -o /dev/null -w "%{http_code}" --retry 25 --retry-delay 1 --retry-all-errors --retry-connrefused --max-time 60 http://127.0.0.1:18790/)
echo "GET / → $code"
[ "$(curl -s http://127.0.0.1:18790/aleph_panel_bg.wasm | wc -c)" = "$(wc -c < interfaces/webchat/dist/aleph_panel_bg.wasm)" ] && echo "served==disk ✓"
```

- [ ] **Step 5: 视觉验证(优先 iOS Simulator;否则 Chrome 过渡)**

**若用户已装 Xcode 27 beta 2 + iOS 27 runtime**(`xcrun simctl list runtimes | grep iOS` 显 iOS 27 且 sim 能正常 boot 渲染):
```bash
UDID=$(xcrun simctl list devices available | grep -m1 'iPhone 1' | grep -oE '[0-9A-F-]{36}')
xcrun simctl boot "$UDID"; xcrun simctl bootstatus "$UDID" -b
for r in settings settings/network settings/appearance settings/model-route settings/providers settings/embedding-providers; do
  xcrun simctl openurl "$UDID" "http://localhost:18790/$r"
  # 等 WASM 加载后:
  xcrun simctl io "$UDID" screenshot "/Volumes/TBU4/Workspace/Aleph/.superpowers/sdd/sim-$(echo $r|tr / -).png"
done
```
逐屏对照:`‹ Settings` 返回 + 居中标题 + iOS list/控件 + **底部 TabBar 可见(dvh 生效)** + 无桌面左右分栏。验毕 `xcrun simctl shutdown "$UDID"`。

**否则(sim 未就绪)** chrome-devtools 过渡:`emulate 390x844x3,mobile,touch` 逐 route 截图审布局;`evaluate_script` 量 `.tabbar` 底边 ≤ viewport 高(验 dvh 逻辑);提示用户真机 Chrome 抽查底部 tab。

- [ ] **Step 6: 桌面回归**

chrome-devtools `emulate 1280x900x2`(非 mobile)→ 逐 route(`/settings/network` 等)确认仍是**原桌面视图**(ModeSidebar + 桌面内容),无 iOS 覆盖层。

- [ ] **Step 7: 收尾**

`pkill -x aleph-server`(停 controller 起的 daemon);向用户报告:逐屏 sim/Chrome 截图、桌面回归、served==disk;列出 §7 局限(重型两屏 focused、Cluster 延后、字体回退)与下一步。**未 commit/未 push**(除非用户要求)。

---

## Self-Review(plan vs spec)

- **spec §3 路由 drill-in + ‹back + 保留 TabBar** → Task 1(PhoneShell/PhoneTabBar)+ 各屏 `back="/settings"` + `app.rs` 分支。✓
- **spec §3 dvh 修复** → Task 1 Step2 `PhoneShell` 根 `h-dvh`(landing 经重构同享)。✓
- **spec §3 文件组织(settings.rs→settings/mod.rs + 子屏)** → Task 1 Step3。✓
- **spec §4.1–4.5 五屏** → Task 2(Connection)/3(Appearance)/4(Model route)/5(Providers focused)/6(Embeddings focused),各复用对应 API/appearance。✓
- **spec §2 复用数据不复用表现 / R4** → 各屏调既有 API/`appearance.rs`,iOS 重建 view;Task 4/5/6 明确"读桌面文件摸契约,只换表现"。✓
- **spec §1 非目标(focused、Cluster 延后、不碰 wide)** → Task 5/6 自检 grep 禁 create/delete/oauth/add/presets;Task 2 注 Cluster 延后;全程不碰 platform/wide。✓
- **spec §5 验证(sim 优先 / Chrome 过渡 / 桌面回归 / 单次 cargo)** → Task 7。✓
- **Placeholder scan**:Task 1–3 含完整代码;Task 4–6 含 iOS 结构 + 精确 API + 新控件代码 + "读桌面文件 X 摸数据契约"的具体指令(非含糊"加错误处理")。✓
- **Type consistency**:`PhoneShell(title,back,children)` / `PhoneTabBar()` / `Phone*` 组件名跨 Task 一致;`FormFactor`/`form_factor.form_factor.get()` 与 landing 任务一致;API 名(`ProvidersApi::{list,set_default,update}`/`EmbeddingProvidersApi::{list,set_active,update}`/`RouteConfigApi::{get,update}`)与已核对的 api 模块一致。✓
- **已知不确定**(实现者 Task 1 步首已 grep 确认):`Roundness`/`Density` 是否有 `ALL`/`label`(其余 4 枚举已确认有);`ProviderInfo`/`EmbeddingProviderEntry` 精确字段名 → Task 4/5/6 Step1 读源确认。

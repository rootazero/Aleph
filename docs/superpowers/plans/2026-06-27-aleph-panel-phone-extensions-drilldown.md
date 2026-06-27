# 手机 Extensions 下钻屏 Implementation Plan (batch #5/5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 `/extensions` 从桌面左右分屏(`CategoryNav` 左栏 + `BrowsePane` 主区)重做成 Browse 优先单列:横向分类 chip 条 + 复用 `BrowsePane` grid + 复用三个 fixed overlay,套「手机不分屏下钻法则」。

**Architecture:** 新建 `platform/phone/extensions/` 模块(`bar.rs` 横向 chip 条 + `mod.rs` `PhoneExtensions` 容器),复用桌面 `BrowsePane`/`ExtensionDetailDrawer`/`InstallFlow`/`InstalledPanel`(全部已手机响应式);`app.rs` Extensions 臂做 form-factor swap。`StoreState` 已在 app 根 `provide_context`,故本屏无 state、无子路由、无 `screen_for_path`(分类与 overlay 全是信号)。

**Tech Stack:** Rust / Leptos (crate `aleph-panel`,`interfaces/webchat`);Tailwind + ios.css(`.chip`/`.chip-active`/`.cc-hide-scroll` 已存在);build = `just wasm`。

## Global Constraints

- **零 core/IPC/依赖/新 CSS** — 只动 panel 表现层,复用既有 ios.css 类。
- **桌面字节级不变** — 只在 `app.rs` 加 import + Extensions 臂内分支;桌面 else 臂(`<ExtensionsView />`)字符不变;`platform/wide/**` 零触碰。
- **零 `PanelMode`/`mode_sidebar`/`nav_menu` 改动** — `PanelMode::from_path` 用 `starts_with` 已把 `/extensions` 归类。
- **无单元测试**(spec §9) — 本批次无 `screen_for_path` 之类纯函数;全是信号驱动 + 组件复用。每个 Task 的验证门 = controller 跑 `just wasm` 绿(`✓ WASM dist OK`)+ reviewer 逐行追溯。implementer 只转写 + 自审 + commit,不构建。
- **R4 I/O-only** — chip 只设 `store.category` 信号;`PhoneExtensions` 只渲染,不持业务逻辑。
- **PhoneShell footgun**(reference-leptos-phoneshell-dynamic-child-footgun) — `PhoneShell` 的 children 必须包在单个 `<div>` 内,不得传裸 `{move||}` dynamic block 紧邻 static 兄弟。
- **三个 overlay 必须在 `PhoneShell` 内**(children) — `PhoneShell` 根 `fixed z-[70]` 自成 stacking context;overlay 是 `z-[60]`/`z-50`,作壳后兄弟会被壳遮挡。放进 children 则在 z-70 context 内绘制于壳之上(`fixed` 不受祖先 overflow 裁剪,链上无 transform 祖先)。

---

## File Structure

| 文件 | 责任 |
|---|---|
| `interfaces/webchat/src/platform/phone/extensions/bar.rs`(新) | `PhoneCategoryBar`:横向分类 chip 条,设 `StoreState.category` |
| `interfaces/webchat/src/platform/phone/extensions/mod.rs`(新) | `pub mod bar;` + `PhoneExtensions` 容器(PhoneShell 包 chip 条 + BrowsePane + 3 overlay) |
| `interfaces/webchat/src/platform/phone/mod.rs`(改) | +`pub mod extensions;`(字母序) |
| `interfaces/webchat/src/app.rs`(改) | +import `PhoneExtensions`;Extensions 臂 form-swap |

---

## Task 1: `platform/phone/extensions/` 模块(chip 条 + 容器)

**Files:**
- Create: `interfaces/webchat/src/platform/phone/extensions/bar.rs`
- Create: `interfaces/webchat/src/platform/phone/extensions/mod.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`(在 `pub mod dashboard;` 与 `pub mod memory;` 之间加 `pub mod extensions;`)

**Interfaces:**
- Consumes(全部已存在,被桌面 `ExtensionsView`/`CategoryNav` 使用,故可达):
  - `crate::views::extensions::StoreState`(app 级 context;字段 `category: RwSignal<String>`)
  - `crate::views::extensions::model::CATEGORIES: &[CategoryFacet]`,`CategoryFacet { value: &'static str, label_key: &'static str, emoji: &'static str }`
  - `crate::components::extensions::labels::category_label(i18n: I18nContext<Locale>, value: &str) -> String`
  - `crate::i18n::use_i18n() -> I18nContext<Locale>`
  - `crate::views::extensions::browse::BrowsePane`(component)
  - `crate::components::extensions::detail_drawer::ExtensionDetailDrawer`(component)
  - `crate::components::extensions::install_flow::InstallFlow`(component)
  - `crate::views::extensions::installed::InstalledPanel`(component)
  - `crate::platform::phone::shell::PhoneShell`(props:`title: &'static str`,`back?`,`back_label?`,`children`)
- Produces(供 Task 2):
  - `crate::platform::phone::extensions::PhoneExtensions`(`#[component] pub fn PhoneExtensions() -> impl IntoView`,无参)

- [ ] **Step 1: 创建 `bar.rs`**

`interfaces/webchat/src/platform/phone/extensions/bar.rs`:

```rust
//! Phone Extensions category chip bar (`/extensions` landing top row): a
//! horizontal scrolling chip strip that replaces the desktop left-column
//! `CategoryNav`. Chips drive the shared app-level `StoreState.category`
//! signal — identical behavior to the desktop nav, restored to the historical
//! horizontal form. I/O-only (R4): chips only set the filter signal.

use leptos::prelude::*;

use crate::components::extensions::labels::category_label;
use crate::i18n::use_i18n;
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn PhoneCategoryBar() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    // One chip per category value. `store` and `value` are Copy, so the inner
    // class/click closures capture them by copy — mirrors desktop `CategoryNav`.
    let chip = move |value: &'static str, label: String, emoji: &'static str| {
        view! {
            <button
                class=move || if store.category.get() == value { "chip chip-active" } else { "chip" }
                on:click=move |_| store.category.set(value.to_string())
            >
                <span>{emoji}</span>
                <span class="whitespace-nowrap">{label}</span>
            </button>
        }
    };

    view! {
        <div class="flex gap-2 overflow-x-auto cc-hide-scroll py-1">
            {chip("featured", category_label(i18n, "featured"), "★")}
            {chip("all", category_label(i18n, "all"), "🗂")}
            {CATEGORIES
                .iter()
                .map(|c| chip(c.value, category_label(i18n, c.value), c.emoji))
                .collect_view()}
        </div>
    }
}
```

- [ ] **Step 2: 创建 `mod.rs`**

`interfaces/webchat/src/platform/phone/extensions/mod.rs`:

```rust
//! Phone Extensions screen (`/extensions`): Browse-first single-column store.
//! The desktop left-column `CategoryNav` is replaced by a horizontal chip bar
//! (`PhoneCategoryBar`); the responsive `BrowsePane` grid (already `grid-cols-1`
//! at phone width) and all three overlays (detail drawer / install flow /
//! installed panel, each `max-w-[94vw]` fixed) are reused verbatim. No
//! sub-routing — category + overlays are app-level `StoreState` signals, and
//! `StoreState` is provided at the app root (app.rs), so this screen holds no
//! state. The overlays sit INSIDE `PhoneShell` so its `z-[70]` stacking context
//! does not hide them (they are `z-[60]`/`z-50`). I/O-only (R4).

pub mod bar;

use leptos::prelude::*;

use crate::components::extensions::detail_drawer::ExtensionDetailDrawer;
use crate::components::extensions::install_flow::InstallFlow;
use crate::platform::phone::extensions::bar::PhoneCategoryBar;
use crate::platform::phone::shell::PhoneShell;
use crate::views::extensions::browse::BrowsePane;
use crate::views::extensions::installed::InstalledPanel;

#[component]
#[must_use]
pub fn PhoneExtensions() -> impl IntoView {
    view! {
        <PhoneShell title="Extensions">
            <div>
                <PhoneCategoryBar/>
                <BrowsePane/>
                <ExtensionDetailDrawer/>
                <InstallFlow/>
                <InstalledPanel/>
            </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 3: 注册模块**

`interfaces/webchat/src/platform/phone/mod.rs` — 在 `pub mod dashboard;` 与 `pub mod memory;` 之间插入一行(字母序):

```rust
pub mod dashboard;
pub mod extensions;
pub mod memory;
```

(原文件该段为 `pub mod agents; / pub mod chat; / pub mod dashboard; / pub mod memory; / pub mod more; / pub mod settings; / pub mod shell; / pub mod teams;`;只新增 `pub mod extensions;` 一行,其余不动。)

- [ ] **Step 4: 构建验证(controller 跑,implementer 不跑)**

Run: `just wasm`
Expected: 退出 0,末尾打印 `✓ WASM dist OK`(编译通过即证 `expect_context::<StoreState>()`、`CATEGORIES`/`category_label` 签名、4 个复用组件路径与可见性、`PhoneShell` props 全部正确 —— 任一错则编译失败)。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/extensions/bar.rs \
        interfaces/webchat/src/platform/phone/extensions/mod.rs \
        interfaces/webchat/src/platform/phone/mod.rs
git commit -m "panel: add phone Extensions browse screen (batch #5/5)"
```

---

## Task 2: `app.rs` 接线(import + Extensions 臂 form-swap)

**Files:**
- Modify: `interfaces/webchat/src/app.rs`(phone import 段 ~line 33-46;Extensions 臂 ~line 429-431)

**Interfaces:**
- Consumes:`crate::platform::phone::extensions::PhoneExtensions`(Task 1 产出)
- Produces:无(终端接线)

- [ ] **Step 1: 加 import**

`interfaces/webchat/src/app.rs` — 在 `use crate::platform::phone::dashboard::PhoneDashboard;` 与 `use crate::platform::phone::memory::PhoneMemory;` 之间插入一行(字母序):

```rust
use crate::platform::phone::dashboard::PhoneDashboard;
use crate::platform::phone::extensions::PhoneExtensions;
use crate::platform::phone::memory::PhoneMemory;
```

- [ ] **Step 2: Extensions 臂 form-swap**

`interfaces/webchat/src/app.rs` — 把 Extensions 臂(现为):

```rust
        <div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
            <ExtensionsView />
        </div>
```

改为(与 Teams/Agents/Dashboard/Memory/Chat 臂同构,桌面 else 分支 `<ExtensionsView />` 字符不变):

```rust
        <div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneExtensions /> }.into_any()
            } else {
                view! { <ExtensionsView /> }.into_any()
            }}
        </div>
```

(`form_factor` 已在 `MainContent` 作用域:`let form_factor = expect_context::<FormFactorState>();`,line ~391;`FormFactor` 已 import,Teams 臂在用。`use crate::views::extensions::{ExtensionsView, StoreState};` line 12 保留不动。)

- [ ] **Step 3: 构建验证(controller 跑)**

Run: `just wasm`
Expected: 退出 0,`✓ WASM dist OK`。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: wire PhoneExtensions into Extensions arm form-swap (batch #5/5)"
```

---

## Dist 重建(controller,最终审查后单独 commit)

两个 Task 合入后,controller 跑 `just wasm` 重建 `dist/aleph_panel.js` + `aleph_panel_bg.wasm`(rust_embed 编译期嵌入,生产 server 需此重建才能服务新 UI),单独 commit:

```bash
git add interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm
git commit -m "panel: rebuild dist with phone Extensions screen"
```

---

## Self-Review(写完即查,已在内联修正)

**1. Spec 覆盖:**
- §3 Browse 优先 + 横向 chip → Task 1 `PhoneCategoryBar` + `BrowsePane` 复用 ✓
- §4 无 state / 无子路由 / catalog load 由 BrowsePane 自带 → `PhoneExtensions` 无 state、无 Effect ✓
- §5 bar.rs / mod.rs / phone/mod.rs 注册 → Task 1 三步 ✓
- §5 🔑 overlay 在 PhoneShell 内 → Task 1 Step 2 结构 + Global Constraints ✓
- §6 app.rs import + 臂 swap → Task 2 ✓
- §9 无单测,build-gate → 每 Task 的 `just wasm` 步骤 ✓
- §10 成功标准 6 条 → 全覆盖 ✓

**2. 占位符扫描:** 无 TBD/TODO;每个 code step 含完整代码;构建步骤含确切命令与期望输出。✓

**3. 类型一致性:** `PhoneCategoryBar`(Task 1 bar.rs)↔ mod.rs 引用一致;`PhoneExtensions`(Task 1 产出)↔ Task 2 import 一致;`store.category`/`StoreState`/`CATEGORIES`/`category_label` 签名与桌面 `CategoryNav` 一致;`PhoneShell title=` 与 shell.rs 签名一致。✓

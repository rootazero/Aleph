# iPhone Settings 屏 (landing) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 <640px 宽度下,`/settings` 渲染一个全屏 iOS 原生 Settings 屏(glass 顶栏 + 三组 inset 卡片 + 底部 TabBar),1:1 照搬 `docs/design-system/aleph-mobile/screens/4-settings.png`;桌面/平板 (≥640px) 完全不变。

**Architecture:** `PhoneSettings` 是 `position:fixed; inset:0` 全屏覆盖层,在共享 `src/app.rs` 的 `SettingsRouter` 的 `/settings` 分支中、仅当 `FormFactor::Phone` 时替换桌面 `Settings`。iOS 组件样式集中在新文件 `styles/ios.css`,经 `@import` 进唯一被 serve 的 `tailwind.css`。phone 代码只在 `platform/phone/`。

**Tech Stack:** Rust + Leptos 0.7 + WASM (`aleph-panel`),Tailwind v4 (`@tailwindcss/cli`),`leptos_router`。

## Global Constraints

- 只改 `interfaces/webchat/`,**零 core 改动**。**不碰 `platform/wide/`**;phone 代码只落 `platform/phone/`。
- **零新依赖**。
- **cargo 极度节制**:全程**至多一次** cargo 调用(集中在 Task 5)。Task 1–4 不跑 cargo。移动文件后 rust-analyzer 可能报 `unlinked-file` / `E0583 views` 等**陈旧假错** —— 以 Task 5 的实编为准,不要据 RA 增删代码。
- iOS 组件类从 `docs/design-system/aleph-mobile/screens/exported/styles/aleph.css` **逐字复制**(数值不改)。SVG / 文案从 `Aleph Settings.dc.html` **逐字抄**,不自造。
- `.glass` / `.tabular-nums` 已在 `tailwind.css`,**复用、不复制**。
- 值为 **v1 静态占位**(`remote · 10.10.10.4` / `Anthropic` / `text-embedding-3` / `Opus 4.8` / `System` / `Luxe`,首个 Accent swatch active),接真实 config 留下一步。
- 提交规范 `<scope>: <description>`,English commit message,无 attribution。**未经用户明确要求不 commit、不 push**(本计划的 commit 步骤=本地暂存语义;若用户要求不提交,则跳过 `git commit`,改为不提交保留工作树)。
- 回复中文,代码注释英文。

---

## 文件结构(改动地图)

| 文件 | 动作 | 职责 |
|------|------|------|
| `interfaces/webchat/styles/ios.css` | **新建** | iOS 组件类(`.list`/`.cell`/`.cell-*`/`.tabbar`/`.tabitem`/`.swatch`/`.mono`/`.cc-hide-scroll`),逐字自 aleph.css |
| `interfaces/webchat/styles/tailwind.css` | 改 1 行 | 顶部 `@import "./ios.css";` 接入构建 |
| `interfaces/webchat/src/state/viewport.rs` | **新建** | `FormFactor` 枚举 + `FormFactorState`(reactive + resize) |
| `interfaces/webchat/src/state/mod.rs` | 改 1 行 | `pub mod viewport;` |
| `interfaces/webchat/src/platform/phone/settings.rs` | **新建** | `PhoneSettings` 全屏屏组件 |
| `interfaces/webchat/src/platform/phone/mod.rs` | 改 1 行 | `pub mod settings;` |
| `interfaces/webchat/src/app.rs` | 改 2 处 | `AppContent` 提供 `FormFactorState` context;`SettingsRouter` `/settings` 分支按 Phone 分流 |

---

## Task 1: iOS 组件样式 `ios.css` + 接入构建

**Files:**
- Create: `interfaces/webchat/styles/ios.css`
- Modify: `interfaces/webchat/styles/tailwind.css:1`

**Interfaces:**
- Produces: CSS 类 `.list .list-header .cell .cell-leading .cell-body .cell-title .cell-sub .cell-value .cell-chevron .tabbar .tabitem .tabitem-active .swatch .swatch-active .mono .cc-hide-scroll`,供 Task 3 的 `PhoneSettings` 使用。

- [ ] **Step 1: 创建 `interfaces/webchat/styles/ios.css`**

逐字复制自 `aleph.css`(`.cc-hide-scroll` 取自 `Aleph Settings.dc.html` 的内联 `<style>`,用于隐藏滚动条)。`.glass`/`.tabular-nums` 不在此文件(复用 `tailwind.css` 既有定义)。

```css
/* iOS-native component classes — ported verbatim from the aleph-mobile design
   system (docs/design-system/aleph-mobile/screens/exported/styles/aleph.css).
   Consumed only by platform::phone screens. Tokens (--color-*, --radius-*,
   --shadow-sm, --safe-area-bottom, --font-mono) are defined in tailwind.css.
   `.glass` / `.tabular-nums` are NOT redefined here — reused from tailwind.css. */

.mono { font-family: var(--font-mono); }

.list-header { padding: 0 0.875rem 0.375rem; font-size: 0.6875rem; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--color-text-tertiary); }

.list { background: var(--color-surface-raised); border: 1px solid var(--color-border-subtle); border-radius: var(--radius-xl); overflow: hidden; box-shadow: var(--shadow-sm); }

.cell { display: flex; align-items: center; gap: 0.75rem; min-height: 48px; padding: 0.625rem 0.875rem; border-bottom: 1px solid var(--color-border-subtle); color: var(--color-text-primary); }
.cell:last-child { border-bottom: 0; }
.cell-leading { display: inline-flex; width: 28px; height: 28px; align-items: center; justify-content: center; border-radius: var(--radius-md); background: var(--color-primary-subtle); color: var(--color-primary); flex: none; }
.cell-body { flex: 1; min-width: 0; }
.cell-title { font-size: 0.9375rem; }
.cell-sub { font-size: 0.8125rem; color: var(--color-text-secondary); }
.cell-value { font-size: 0.875rem; color: var(--color-text-tertiary); }
.cell-chevron { color: var(--color-text-tertiary); flex: none; }

.tabbar { display: flex; background: var(--color-surface-overlay); border-top: 1px solid var(--color-border-subtle); padding: 0.4rem 0 calc(0.4rem + var(--safe-area-bottom)); }
.tabbar.glass { background-color: color-mix(in oklch, var(--color-surface-overlay) 82%, transparent); }
.tabitem { flex: 1; display: flex; flex-direction: column; align-items: center; gap: 0.2rem; padding: 0.25rem; background: none; border: 0; cursor: pointer; color: var(--color-text-tertiary); font: inherit; font-size: 0.625rem; font-weight: 500; }
.tabitem-active { color: var(--color-primary); }

.swatch { width: 44px; height: 44px; border-radius: var(--radius-full); border: 2px solid var(--color-border); cursor: pointer; }
.swatch-active { box-shadow: 0 0 0 2px var(--color-surface-raised), 0 0 0 4px var(--color-primary); border-color: transparent; }

.cc-hide-scroll { scrollbar-width: none; -ms-overflow-style: none; }
.cc-hide-scroll::-webkit-scrollbar { display: none; }
```

- [ ] **Step 2: 在 `tailwind.css` 顶部接入 `ios.css`**

`interfaces/webchat/styles/tailwind.css` 当前第 1 行是 `@import "tailwindcss";`。在其**正下方**插入一行(`@import` 必须位于其它规则之前,这里紧跟现有的 import,合法):

```css
@import "tailwindcss";
@import "./ios.css";
```

- [ ] **Step 3: 非 cargo 自检 —— 类值与 aleph.css 字节一致**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
A="docs/design-system/aleph-mobile/screens/exported/styles/aleph.css"
B="interfaces/webchat/styles/ios.css"
for c in '\.list ' '\.cell ' '\.cell-leading' '\.tabbar ' '\.tabitem ' '\.swatch ' '\.mono'; do
  echo "--- $c ---"; diff <(grep -E "^$c" "$A") <(grep -E "^$c" "$B") && echo "OK(identical)";
done
grep -n '@import "./ios.css";' interfaces/webchat/styles/tailwind.css
```
Expected: 每个类的 `diff` 输出 `OK(identical)`(数值逐字一致);最后一行确认 import 已加。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/styles/ios.css interfaces/webchat/styles/tailwind.css
git commit -m "panel(phone): add iOS component styles (ios.css) + wire into tailwind build"
```

---

## Task 2: `FormFactor` viewport 状态

**Files:**
- Create: `interfaces/webchat/src/state/viewport.rs`
- Modify: `interfaces/webchat/src/state/mod.rs:6`

**Interfaces:**
- Produces:
  - `enum FormFactor { Wide, Phone, Tablet }`(`Copy + PartialEq`),`FormFactor::from_width(f64) -> FormFactor`(`<640→Phone`,`<1024→Tablet`,else `Wide`)。
  - `struct FormFactorState { pub form_factor: RwSignal<FormFactor> }`(`Copy`),`FormFactorState::new()` 注册 resize 监听。
  - 路径:`crate::state::viewport::{FormFactor, FormFactorState}`。

- [ ] **Step 1: 创建 `interfaces/webchat/src/state/viewport.rs`**

```rust
//! Reactive form-factor signal (Wide / Phone / Tablet).
//!
//! Phone (<640px) is currently the only factor that changes rendering — it
//! swaps the wide `/settings` page for the iOS-native `PhoneSettings` screen
//! (see `crate::platform::phone::settings`). Tablet is reserved for future
//! iPad screens and renders identically to Wide for now. The 640px line
//! matches Tailwind's `sm` breakpoint, so CSS and logic agree.

use leptos::prelude::*;

/// Upper bound (exclusive) of the Phone band. Matches Tailwind `sm`.
pub const PHONE_MAX_PX: f64 = 640.0;
/// Upper bound (exclusive) of the Tablet band.
pub const TABLET_MAX_PX: f64 = 1024.0;

/// Viewport class. Only `Phone` diverges in rendering today.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormFactor {
    Wide,
    Phone,
    Tablet,
}

impl FormFactor {
    /// Classify a viewport width. `<640 → Phone`, `<1024 → Tablet`, else `Wide`.
    #[must_use]
    pub fn from_width(width: f64) -> Self {
        if width < PHONE_MAX_PX {
            FormFactor::Phone
        } else if width < TABLET_MAX_PX {
            FormFactor::Tablet
        } else {
            FormFactor::Wide
        }
    }
}

/// Reactive form-factor, provided at the shell root via context. `Copy` (just a
/// signal handle), so it threads freely into router closures.
#[derive(Clone, Copy)]
pub struct FormFactorState {
    pub form_factor: RwSignal<FormFactor>,
}

impl FormFactorState {
    #[must_use]
    pub fn new() -> Self {
        let form_factor = RwSignal::new(FormFactor::from_width(measure_width()));
        // Keep in sync with resizes. Fire-and-forget for the app lifetime —
        // mirrors the shell-root listeners in app.rs (handle not retained).
        window_event_listener(leptos::ev::resize, move |_| {
            let now = FormFactor::from_width(measure_width());
            if form_factor.get_untracked() != now {
                form_factor.set(now);
            }
        });
        Self { form_factor }
    }
}

impl Default for FormFactorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Current window inner width; falls back to a wide width when unreadable
/// (e.g. during SSR / host-target tests where there is no `window`).
fn measure_width() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_widths_at_band_boundaries() {
        assert_eq!(FormFactor::from_width(0.0), FormFactor::Phone);
        assert_eq!(FormFactor::from_width(639.9), FormFactor::Phone);
        assert_eq!(FormFactor::from_width(640.0), FormFactor::Tablet);
        assert_eq!(FormFactor::from_width(1023.9), FormFactor::Tablet);
        assert_eq!(FormFactor::from_width(1024.0), FormFactor::Wide);
        assert_eq!(FormFactor::from_width(1920.0), FormFactor::Wide);
    }
}
```

- [ ] **Step 2: 在 `state/mod.rs` 注册模块**

`interfaces/webchat/src/state/mod.rs` 当前内容是 6 行 `pub mod …;`。按字母序在 `pub mod sessions;` 之后追加一行:

```rust
pub mod connection;
pub mod hotkey;
pub mod layout;
pub mod memory;
pub mod notifications;
pub mod sessions;
pub mod viewport;
```

- [ ] **Step 3: 非 cargo 自检 —— 文件就位、模块已登记**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
test -f interfaces/webchat/src/state/viewport.rs && echo "viewport.rs OK"
grep -n 'pub mod viewport;' interfaces/webchat/src/state/mod.rs
grep -n 'fn from_width' interfaces/webchat/src/state/viewport.rs
```
Expected: 打印 `viewport.rs OK` + 命中 `pub mod viewport;` + 命中 `fn from_width`。
(编译验证集中在 Task 5;此处不跑 cargo。)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/state/viewport.rs interfaces/webchat/src/state/mod.rs
git commit -m "panel(phone): add FormFactor viewport state (Wide/Phone/Tablet)"
```

---

## Task 3: `PhoneSettings` 屏组件

**Files:**
- Create: `interfaces/webchat/src/platform/phone/settings.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs`(末尾追加 `pub mod settings;`)

**Interfaces:**
- Consumes: Task 1 的 CSS 类;`leptos_router::hooks::use_navigate` + `leptos_router::NavigateOptions`(与 `app.rs` 现有用法一致)。
- Produces: `#[component] pub fn PhoneSettings() -> impl IntoView`,路径 `crate::platform::phone::settings::PhoneSettings`。

- [ ] **Step 1: 创建 `interfaces/webchat/src/platform/phone/settings.rs`**

逐字照 `Aleph Settings.dc.html`(SVG path / 文案 / 内联数值)。faux 状态栏 / 灵动岛 / home indicator **不渲染**(真机由 OS 绘制)。

```rust
//! iPhone Settings landing screen — 1:1 rebuild of the aleph-mobile design
//! (`docs/design-system/aleph-mobile/screens/exported/Aleph Settings.dc.html`).
//!
//! Rendered as a fixed full-screen overlay (`position:fixed; inset:0; z-50`) so
//! it covers the wide two-column shell beneath it; mounted only at <640px from
//! `app.rs`'s `SettingsRouter`. I/O-only (R4): cells/tabs navigate to existing
//! routes; displayed values are static placeholders for v1 (see spec §6). The
//! faux status bar / dynamic island / home indicator in the mockup are device
//! chrome (OS-drawn on real hardware) and are intentionally omitted.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

#[component]
#[must_use]
pub fn PhoneSettings() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <div
            class="fixed inset-0 z-50 flex flex-col"
            style="background:radial-gradient(120% 55% at 50% 0%, oklch(0.62 0.10 310 / 0.14), transparent 62%),radial-gradient(120% 45% at 50% 100%, oklch(0.60 0.09 250 / 0.10), transparent 60%),var(--color-surface);"
        >
            // ── Top bar (glass) ──
            <div
                class="glass"
                style="flex:none; display:flex; align-items:center; gap:8px; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                <span style="flex:1; font-size:20px; font-weight:700; letter-spacing:-0.02em; color:var(--color-text-primary);">
                    "Settings"
                </span>
            </div>

            // ── Grouped settings list (scroll) ──
            <div
                class="cc-hide-scroll"
                style="flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:20px; padding:16px 16px 18px;"
            >
                // Connection group
                <div>
                    <div class="list-header">"连接"</div>
                    <div class="list">
                        <div class="cell" on:click=go("/settings/network")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M5 12.5a7 7 0 0 1 14 0"></path>
                                    <path d="M2 9a11 11 0 0 1 20 0"></path>
                                    <path d="M8.5 16a4 4 0 0 1 7 0"></path>
                                    <circle cx="12" cy="19.5" r="1" fill="currentColor"></circle>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Connection"</div></div>
                            <span class="cell-value mono" style="font-size:13px;">"remote · 10.10.10.4"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                    </div>
                </div>

                // AI group
                <div>
                    <div class="list-header">"AI"</div>
                    <div class="list">
                        <div class="cell" on:click=go("/settings/providers")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12 3l1.6 3.8L17.5 8l-3.9 1.2L12 13l-1.6-3.8L6.5 8l3.9-1.2z"></path>
                                    <path d="M6 15l.8 2 .8-2 .8 2-.8-2zM18 16l.7 1.8.7-1.8-.7 1.8z"></path>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Providers"</div></div>
                            <span class="cell-value">"Anthropic"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                        <div class="cell" on:click=go("/settings/embedding-providers")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="12" cy="12" r="3"></circle>
                                    <circle cx="5" cy="6" r="1.6"></circle>
                                    <circle cx="19" cy="7" r="1.6"></circle>
                                    <circle cx="6" cy="18" r="1.6"></circle>
                                    <circle cx="18" cy="17" r="1.6"></circle>
                                    <path d="M9.6 10.4 6.4 7M14.4 10.6 17.6 8M9.8 14 6.9 16.6M14.2 13.8 17 16"></path>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Embeddings"</div></div>
                            <span class="cell-value mono" style="font-size:13px;">"text-embedding-3"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                        <div class="cell" on:click=go("/settings/model-route")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M6 3v6a6 6 0 0 0 12 0V3"></path>
                                    <path d="M6 21v-2a6 6 0 0 1 12 0v2"></path>
                                    <line x1="4" y1="3" x2="20" y2="3"></line>
                                    <line x1="4" y1="21" x2="20" y2="21"></line>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Model route"</div></div>
                            <span class="cell-value">"Opus 4.8"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                    </div>
                </div>

                // Appearance group
                <div>
                    <div class="list-header">"外观"</div>
                    <div class="list">
                        <div class="cell" on:click=go("/settings/appearance")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="12" cy="12" r="9"></circle>
                                    <path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor" stroke="none"></path>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Theme"</div></div>
                            <span class="cell-value">"System"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                        <div class="cell" style="align-items:center;" on:click=go("/settings/appearance")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="13.5" cy="6.5" r="1.3"></circle>
                                    <circle cx="17.5" cy="10.5" r="1.3"></circle>
                                    <circle cx="8.5" cy="7.5" r="1.3"></circle>
                                    <circle cx="6.5" cy="12.5" r="1.3"></circle>
                                    <path d="M12 3a9 9 0 1 0 0 18c1 0 1.5-.8 1.5-1.6 0-1.2-1-1.6-1-2.6 0-.8.7-1.3 1.6-1.3H16a5 5 0 0 0 5-5c0-4.4-4-7.5-9-7.5z"></path>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Accent"</div></div>
                            <div style="display:flex; align-items:center; gap:8px; flex:none;">
                                <span class="swatch swatch-active" style="width:26px; height:26px; background:oklch(0.55 0.120 310);" title="Mauve"></span>
                                <span class="swatch" style="width:26px; height:26px; background:oklch(0.55 0.130 250);" title="Ocean"></span>
                                <span class="swatch" style="width:26px; height:26px; background:oklch(0.53 0.115 150);" title="Forest"></span>
                                <span class="swatch" style="width:26px; height:26px; background:oklch(0.62 0.135 60);" title="Sunset"></span>
                                <span class="swatch" style="width:26px; height:26px; background:oklch(0.57 0.150 15);" title="Rose"></span>
                            </div>
                        </div>
                        <div class="cell" on:click=go("/settings/appearance")>
                            <span class="cell-leading">
                                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                    <rect x="3" y="3" width="18" height="18" rx="4"></rect>
                                    <path d="M3 9a9 6 0 0 0 18 0"></path>
                                    <path d="M3 14a9 5 0 0 0 18 0"></path>
                                </svg>
                            </span>
                            <div class="cell-body"><div class="cell-title">"Material"</div></div>
                            <span class="cell-value">"Luxe"</span>
                            <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                        </div>
                    </div>
                </div>
            </div>

            // ── Tab bar ──
            <div class="tabbar glass" style="flex:none; padding-bottom:calc(0.4rem + 16px);">
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
                <button class="tabitem tabitem-active">
                    <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 6.6 19l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 4 13.6H4a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 5 6.6l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 10.4 4V4a2 2 0 1 1 4 0v.1A1.6 1.6 0 0 0 17 5l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"></path></svg>
                    "Settings"
                </button>
            </div>
        </div>
    }
}
```

> 注:Leptos `view!` 里 SVG 子元素用显式闭合标签(`<path …></path>`)而非自闭合,以避免宏解析歧义;`viewBox` / `stroke-width` 等属性按 `app.rs` 既有 SVG 写法逐字保留。

- [ ] **Step 2: 在 `platform/phone/mod.rs` 注册子模块**

`interfaces/webchat/src/platform/phone/mod.rs` 当前只有文件级文档注释(`//! …`)、无 `pub mod`。在文件**末尾**追加:

```rust
pub mod settings;
```

- [ ] **Step 3: 非 cargo 自检 —— 文件就位、登记、SVG 数量对齐设计稿**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
test -f interfaces/webchat/src/platform/phone/settings.rs && echo "settings.rs OK"
grep -n 'pub mod settings;' interfaces/webchat/src/platform/phone/mod.rs
echo -n "leading icons (期望 7): "; grep -c 'class="cell-leading"' interfaces/webchat/src/platform/phone/settings.rs
echo -n "chevrons (期望 6): ";      grep -c 'class="cell-chevron"' interfaces/webchat/src/platform/phone/settings.rs
echo -n "swatches (期望 5): ";      grep -c 'class="swatch' interfaces/webchat/src/platform/phone/settings.rs
echo -n "tabitems (期望 4): ";      grep -c 'class="tabitem' interfaces/webchat/src/platform/phone/settings.rs
```
Expected: `settings.rs OK` + 命中 `pub mod settings;` + leading=7 / chevron=6 / swatch=5 / tabitem=4(与 `4-settings.png` 一致:7 cell,其中 Accent 无 chevron 故 6;5 色块;4 tab)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/settings.rs interfaces/webchat/src/platform/phone/mod.rs
git commit -m "panel(phone): add PhoneSettings landing screen (1:1 from design)"
```

---

## Task 4: 在 `app.rs` 接线(context + 路由分流)

**Files:**
- Modify: `interfaces/webchat/src/app.rs`(imports;`AppContent` 提供 context;`SettingsRouter` `/settings` 分支)

**Interfaces:**
- Consumes: `crate::state::viewport::{FormFactor, FormFactorState}`(Task 2)、`crate::platform::phone::settings::PhoneSettings`(Task 3)。
- Produces: 运行时 `<640px` 的 `/settings` 渲染 `PhoneSettings`,≥640px 渲染原 `Settings`。

- [ ] **Step 1: 增加两个 import**

在 `interfaces/webchat/src/app.rs` 顶部 import 区(紧接现有 `use crate::state::sessions::SessionMap;` 一带)加入:

```rust
use crate::platform::phone::settings::PhoneSettings;
use crate::state::viewport::{FormFactor, FormFactorState};
```

- [ ] **Step 2: 在 `AppContent` 提供 `FormFactorState` context**

`AppContent` 函数体内已有一串 `provide_context(...)`。在 `provide_context(SessionMap::new());` 之后追加一行(任意 provide_context 之间皆可,放此处便于阅读):

```rust
    provide_context(SessionMap::new());

    // Form-factor (Wide/Phone/Tablet) — read by SettingsRouter to swap the
    // wide `/settings` page for the iOS-native PhoneSettings at <640px.
    provide_context(FormFactorState::new());
```

- [ ] **Step 3: `SettingsRouter` 读取 form-factor 并分流 `/settings`**

在 `SettingsRouter` 组件里,`let location = use_location();` 之后加一行获取 context(Copy,移动进闭包):

```rust
fn SettingsRouter() -> impl IntoView {
    let location = use_location();
    let form_factor = expect_context::<FormFactorState>();

    move || {
        let path = location.pathname.get();
        match path.as_str() {
```

然后把现有的 `/settings` 分支:

```rust
            "/settings" => view! { <Settings /> }.into_any(),
```

替换为(读取 `form_factor.form_factor.get()` 使该分支同时跟踪 form-factor 信号,跨 640px 时自动重渲):

```rust
            "/settings" => {
                if form_factor.form_factor.get() == FormFactor::Phone {
                    view! { <PhoneSettings /> }.into_any()
                } else {
                    view! { <Settings /> }.into_any()
                }
            }
```

其余所有 `match` 分支与组件**保持不变**。

- [ ] **Step 4: 非 cargo 自检 —— 接线点齐全**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n 'use crate::platform::phone::settings::PhoneSettings;' interfaces/webchat/src/app.rs
grep -n 'use crate::state::viewport::{FormFactor, FormFactorState};' interfaces/webchat/src/app.rs
grep -n 'provide_context(FormFactorState::new());' interfaces/webchat/src/app.rs
grep -n 'expect_context::<FormFactorState>()' interfaces/webchat/src/app.rs
grep -n 'FormFactor::Phone' interfaces/webchat/src/app.rs
```
Expected: 5 条 grep 各命中一处。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel(phone): route /settings to PhoneSettings on Phone form-factor"
```

---

## Task 5: 构建 + 验证(唯一一次 cargo 调用)

**Files:** 无新增;本任务只编译、构建 dist、目视验证。

**Interfaces:** Consumes Task 1–4 的全部产物。

- [ ] **Step 1: 单次编译门(二选一,只跑一条)**

优先(符合"至多一次 `cargo check --lib`"):
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p aleph-panel --lib
```
Expected: `Finished`。若想同时运行 `from_width` 单测(`#[cfg(test)]`,`cargo check --lib` 不编译它),改跑等价的单次命令:
```bash
cargo test -p aleph-panel --lib viewport
```
Expected: `test classifies_widths_at_band_boundaries ... ok`。
> RA 此前可能报 `unlinked-file`/`E0583 views` 等**陈旧假错** —— 以本步实编结果为准。报错则按真实编译错误修复(常见:SVG 标签需显式闭合、属性引号)。

- [ ] **Step 2: 重建 dist(WASM + ios.css 内联进 tailwind.css)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
just wasm
```
Expected: `✓ WASM: …/dist/`。`npm run build:css` 会把 `@import "./ios.css"` 内联进 `dist/tailwind.css`。

- [ ] **Step 3: 确认 dist 含 iOS 类(无 cargo)**

```bash
grep -c '\.cell-leading' interfaces/webchat/dist/tailwind.css
grep -c '\.tabitem' interfaces/webchat/dist/tailwind.css
```
Expected: 两者均 ≥1(证明 ios.css 已编入被 serve 的样式表)。

- [ ] **Step 4: 确保 `aleph-server` 在 `:18790` 运行**

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:18790/ || true
```
若非 200:在另一终端起 debug server(dev daemon 从盘读 dist,无需为 panel 改动重编 server):
```bash
cargo run -p alephcore --bin aleph-server   # 或复用用户已有 daemon
```

- [ ] **Step 5: 验证 served wasm == 磁盘**

```bash
cd /Volumes/TBU4/Workspace/Aleph
echo "served: $(curl -s http://127.0.0.1:18790/aleph_panel_bg.wasm | wc -c)"
echo "disk  : $(wc -c < interfaces/webchat/dist/aleph_panel_bg.wasm)"
```
Expected: 两数相等。

- [ ] **Step 6: 390px 目视对照设计稿**

用 chrome-devtools MCP:`emulate 390x844, deviceScaleFactor 3, mobile, touch` → 导航 `http://127.0.0.1:18790/settings` → `take_screenshot` → 与 `docs/design-system/aleph-mobile/screens/4-settings.png` 内层(去掉手机外壳/状态栏/灵动岛/home indicator)对照:
  - glass 顶栏标题 `Settings`;
  - 三组 inset 卡片(连接 / AI / 外观),`.list-header` 大写灰字;
  - 每 cell:primary 圆角图标块 + 标题 + 值(mono 值用等宽)+ `›`;Accent 行 5 色块,首个 active 环;
  - 底部 `.tabbar` 4 项,Settings 高亮 primary 色。
- [ ] **Step 7: 1280px 桌面回归**

`emulate 1280x900, deviceScaleFactor 2`(关 mobile/touch)→ 导航 `/settings` → `take_screenshot`:确认仍是**原桌面双栏**(左 ModeSidebar + 右 Quick Setup 列表),**无** PhoneSettings 覆盖层。

- [ ] **Step 8: 交付小结(不提交、不 push)**

向用户报告:390px 截图 vs `4-settings.png` 对照结果、桌面回归结果、served==disk;列出 v1 占位项(spec §6)与下一步(spec §8:接真实 config / 其余 phone 屏 / 字体)。**保持未 commit/未 push**(除非用户已要求提交)。

---

## Self-Review(plan vs spec)

- **Spec §3 集成机制(fixed overlay)** → Task 3 Step1 根容器 `fixed inset-0 z-50` + Task 4 Step3 `/settings` 分流。✓
- **Spec §4.1/4.2 ios.css + @import** → Task 1。`.glass`/`.tabular-nums` 不复制已注明;`.cc-hide-scroll` 额外加入(设计稿内联 style 需要,用于隐藏滚动条),已在 Task 1 Step1 标注来源。✓
- **Spec §4.3 viewport.rs(删 drawer_open)** → Task 2,无 `drawer_open`。✓
- **Spec §4.4 app.rs context + 路由** → Task 4。✓
- **Spec §4.5 PhoneSettings + mod 登记** → Task 3。✓
- **Spec §5 交互路由表** → Task 3 `go(...)` 全部命中(Connection→/settings/network,Providers/Embeddings/Model route,Theme/Accent/Material→/settings/appearance,Tab→ / /memory /agents,Settings 不跳)。✓
- **Spec §6 静态占位** → Task 3 文案逐字硬编码。✓
- **Spec §7 验证(集中一次 cargo)** → Task 5,单条 cargo。✓
- **Spec §8 局限 + §9 DoD** → Task 5 Step8 交付小结覆盖。✓
- **Placeholder scan**:无 TBD/TODO;每个代码步骤含完整代码。✓
- **Type consistency**:`FormFactorState.form_factor`(Task2)== `app.rs` 读法 `form_factor.form_factor.get()`(Task4);`FormFactor::Phone`(Task2/4)一致;`PhoneSettings` 名称一致(Task3/4)。✓

# Phone More Entry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a 5th phone bottom-tab "More" (•••) that opens a full-screen sections menu (Dashboard / Teams / Extensions), each row navigating into that mode — the phone entry point for the management sections that aren't primary tabs.

**Architecture:** New `PanelMode::More` value classifies `/more` so the existing route-driven phone machinery (`from_path` → `MainContent` arm → tab active-state) carries it with no new mechanism. A new single-component screen `platform/phone/more.rs` (`PhoneMore`) reuses the `PhoneSettings` landing structure (`PhoneShell` + `.cell` rows). The ••• tab highlights via a new `PanelMode::under_more()` predicate (stays lit inside Dashboard/Teams/Extensions, iOS "More" convention).

**Tech Stack:** Rust + Leptos (crate `aleph-panel`, `interfaces/webchat`); WASM compiled via `just wasm`.

## Global Constraints

- **Scope = entry only.** Dashboard/Teams/Extensions phone screens are OUT (separate specs). Rows navigate; the target renders the current desktop layout until its own spec lands. This is the agreed transition behavior — not a regression.
- **Zero core / zero IPC / zero new deps / zero new CSS.** Reuse existing `ios.css` classes (`.list` `.cell` `.cell-leading` `.cell-body` `.cell-title` `.cell-chevron` `.tabbar` `.tabitem` `.tabitem-active`).
- **Desktop functionally byte-unchanged.** The desktop touch is additive: `PanelMode::More` + dead `match` arms that desktop never reaches (`/more` is only reachable from the phone ••• tab; `NavMenu::ALL_MODES` has no More, so no desktop link points at `/more`).
- **R4 (I/O-only):** the More menu only navigates; no persistence/business logic.
- **Build policy = controller-only `just wasm`.** Implementers transcribe the exact code below, self-review, and commit. They do NOT run builds/tests. The controller runs `just wasm` after each task as the compile gate (host `cargo test -p aleph-panel --lib` is optional at controller discretion — `aleph-panel` ungates `web-sys`, so host test builds are not guaranteed; the unit tests are pure and trace-verifiable by the reviewer).
- **Copy:** phone tab labels are literal English (no i18n), matching the existing tabs. The new label is exactly `"More"`.
- **Comments in English.**
- **PhoneShell footgun:** never pass a bare `{move||…}` dynamic block as a direct child of a component next to a static sibling. (`PhoneMore` passes a single static `<div class="list">` — no footgun. The `app.rs` More arm wraps its `{move||…}` inside a `<div>`, same as the existing arms.)
- **PhoneShell signature:** `PhoneShell(title: &'static str, #[prop(optional)] back: Option<&'static str>, #[prop(optional)] back_label: Option<&'static str>, children)`. The landing uses `title` only (no `back`).

---

### Task 1: `PanelMode::More` routing foundation

Adds the `More` mode value, classifies `/more`, the `under_more()` tab-active predicate, the dead desktop `match` arms required for exhaustiveness, and unit tests. After this task `/more` is a dormant, classified route (nothing navigates to it yet) and the crate compiles.

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs` (enum + `from_path` + `ModeSidebar` match + `under_more` + tests)
- Modify: `interfaces/webchat/src/components/nav_menu.rs` (`route_of` / `label_of` / `icon_of` dead arms)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `PanelMode::More` (new enum variant).
  - `PanelMode::from_path(path: &str) -> PanelMode` now returns `PanelMode::More` for any path starting `/more`.
  - `PanelMode::under_more(self) -> bool` — `const fn`, `true` for `More | Dashboard | Teams | Extensions`. (Task 2's `PhoneTabBar` calls this.)

- [ ] **Step 1: Add the `More` enum variant**

In `mode_sidebar.rs`, the enum currently ends:

```rust
pub enum PanelMode {
    Chat,
    Dashboard,
    Memory,
    Agents,
    Teams,
    Extensions,
    Settings,
}
```

Add `More` as the last variant:

```rust
pub enum PanelMode {
    Chat,
    Dashboard,
    Memory,
    Agents,
    Teams,
    Extensions,
    Settings,
    /// Phone-only ••• tab landing — a sections menu for the management modes
    /// that aren't primary phone tabs. Desktop never routes here.
    More,
}
```

- [ ] **Step 2: Classify `/more` in `from_path`**

`from_path` currently is:

```rust
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/memory") {
            Self::Memory
        } else if path.starts_with("/agents") {
            Self::Agents
        } else if path.starts_with("/teams") {
            Self::Teams
        } else if path.starts_with("/extensions") {
            Self::Extensions
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
    }
```

Insert a `/more` branch before the `/settings` branch (prefixes are disjoint, so position only needs to precede the `Chat` fallback):

```rust
        } else if path.starts_with("/extensions") {
            Self::Extensions
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/more") {
            Self::More
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat
        }
```

- [ ] **Step 3: Add the `under_more` predicate**

Inside `impl PanelMode`, directly after the `from_path` method's closing brace, add:

```rust
    /// True for the sections reached through the phone More (•••) tab. The
    /// ••• tab stays highlighted while inside any of them (iOS "More"
    /// convention). Phone-only concept; desktop never routes to these via More.
    #[must_use]
    pub const fn under_more(self) -> bool {
        matches!(
            self,
            Self::More | Self::Dashboard | Self::Teams | Self::Extensions
        )
    }
```

- [ ] **Step 4: Add the dead `ModeSidebar` match arm**

The `ModeSidebar` component's match currently is:

```rust
                {move || match mode.get() {
                    PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                    PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                    PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
                    PanelMode::Memory => view! { <MemorySidebar /> }.into_any(),
                    PanelMode::Teams => view! { <crate::views::teams::TeamsSidebar /> }.into_any(),
                    PanelMode::Extensions => view! { <crate::views::extensions::ExtensionsSidebar /> }.into_any(),
                    PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
                }}
```

Add a `More` arm (desktop `/more` is unreachable → empty secondary menu):

```rust
                    PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
                    // /more is a phone-only route; desktop never reaches it.
                    PanelMode::More => ().into_any(),
                }}
```

- [ ] **Step 5: Add the dead `nav_menu.rs` arms**

In `interfaces/webchat/src/components/nav_menu.rs`, three `match mode` helpers need a `More` arm for exhaustiveness. They are only read when `current == More`, which is unreachable on desktop (`NavMenu` renders inside the desktop `ModeSidebar`; `ALL_MODES` has no More), so these are dead arms.

`route_of` currently:

```rust
        PanelMode::Extensions => "/extensions",
        PanelMode::Settings => "/settings",
    }
```

→

```rust
        PanelMode::Extensions => "/extensions",
        PanelMode::More => "/more",
        PanelMode::Settings => "/settings",
    }
```

`label_of` currently:

```rust
        PanelMode::Extensions => t_string!(i18n, nav.extensions).to_string(),
        PanelMode::Settings => t_string!(i18n, nav.settings).to_string(),
    }
```

→ (literal `"More"`, consistent with the phone tab copy; no i18n key needed)

```rust
        PanelMode::Extensions => t_string!(i18n, nav.extensions).to_string(),
        PanelMode::More => "More".to_string(),
        PanelMode::Settings => t_string!(i18n, nav.settings).to_string(),
    }
```

`icon_of` currently ends:

```rust
        PanelMode::Settings => {
            r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 ... z"/>"#
        }
    }
}
```

Add a `More` arm (three horizontal dots) before the `Settings` arm — keep the existing `Settings` body exactly as-is:

```rust
        PanelMode::More => {
            r#"<circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/>"#
        }
        PanelMode::Settings => {
```

- [ ] **Step 6: Add the unit-test module**

At the END of `mode_sidebar.rs`, append a test module:

```rust
#[cfg(test)]
mod tests {
    use super::PanelMode;

    #[test]
    fn from_path_classifies_more() {
        assert_eq!(PanelMode::from_path("/more"), PanelMode::More);
        assert_eq!(PanelMode::from_path("/more/"), PanelMode::More);
        // /more must not shadow, nor be shadowed by, the other sections.
        assert_eq!(PanelMode::from_path("/memory"), PanelMode::Memory);
        assert_eq!(PanelMode::from_path("/dashboard"), PanelMode::Dashboard);
        assert_eq!(PanelMode::from_path("/settings"), PanelMode::Settings);
        assert_eq!(PanelMode::from_path("/"), PanelMode::Chat);
    }

    #[test]
    fn under_more_covers_more_sections() {
        for m in [
            PanelMode::More,
            PanelMode::Dashboard,
            PanelMode::Teams,
            PanelMode::Extensions,
        ] {
            assert!(m.under_more(), "{m:?} should be under More");
        }
        for m in [
            PanelMode::Chat,
            PanelMode::Memory,
            PanelMode::Agents,
            PanelMode::Settings,
        ] {
            assert!(!m.under_more(), "{m:?} should not be under More");
        }
    }
}
```

(`PanelMode` derives `Debug, Clone, Copy, PartialEq, Eq`, so `assert_eq!`, `{m:?}`, and by-value iteration all work.)

- [ ] **Step 7: Self-review + commit**

Self-review checklist: enum variant added; `from_path` `/more` branch present and before the `Chat` fallback; `under_more` is `const fn` returning the correct set; both dead arms (`ModeSidebar`, `nav_menu` ×3) added; the existing `Settings` arms are untouched; test module appended. Then commit:

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs interfaces/webchat/src/components/nav_menu.rs
git commit -m "panel: add PanelMode::More routing foundation (phone More entry)"
```

**Verification (controller, after the task):** run `just wasm`; expect exit 0 / "✓ WASM dist OK". The reviewer traces the two unit tests by hand (all cases must be correct); host `cargo test -p aleph-panel --lib` is optional at controller discretion.

---

### Task 2: `PhoneMore` screen + ••• tab + app wiring

Creates the sections-menu screen, registers its module, adds the 5th bottom tab, and wires the `MainContent` arm. After this task the ••• tab appears, navigates to `/more`, and renders the menu; tapping a row navigates into that mode.

**Files:**
- Create: `interfaces/webchat/src/platform/phone/more.rs` (`PhoneMore`)
- Modify: `interfaces/webchat/src/platform/phone/mod.rs` (`pub mod more;`)
- Modify: `interfaces/webchat/src/platform/phone/shell.rs` (`PhoneTabBar` 5th tab)
- Modify: `interfaces/webchat/src/app.rs` (import + `MainContent` More arm)

**Interfaces:**
- Consumes (from Task 1): `PanelMode::More`; `PanelMode::under_more(self) -> bool`.
- Consumes (existing): `crate::platform::phone::shell::PhoneShell`; `crate::state::viewport::{FormFactor, FormFactorState}` (already imported in `app.rs`).
- Produces: `crate::platform::phone::more::PhoneMore` — a `#[component]` taking no props.

- [ ] **Step 1: Create `platform/phone/more.rs`**

Create the file with exactly:

```rust
//! Phone More entry (`/more`): the 5th-tab landing — a full-screen sections
//! menu for the management modes that aren't primary phone tabs
//! (Dashboard / Teams / Extensions). Each row navigates into that mode; that
//! mode's own phone screen is a separate spec, so until then the target renders
//! the existing desktop layout. Mirrors the `PhoneSettings` landing structure.
//! I/O-only (R4): rows only navigate.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;

#[component]
#[must_use]
pub fn PhoneMore() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="More">
            <div class="list">
                <div class="cell" on:click=go("/dashboard")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="3" y="3" width="7" height="7"></rect>
                            <rect x="14" y="3" width="7" height="7"></rect>
                            <rect x="14" y="14" width="7" height="7"></rect>
                            <rect x="3" y="14" width="7" height="7"></rect>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Dashboard"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/teams")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path>
                            <circle cx="9" cy="7" r="4"></circle>
                            <path d="M23 21v-2a4 4 0 0 0-3-3.87"></path>
                            <path d="M16 3.13a4 4 0 0 1 0 7.75"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Teams"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/extensions")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.2 2.2 0 1 1 0 4.4H2V19a2 2 0 0 0 2 2h3.8v-1.5a2.2 2.2 0 1 1 4.4 0V21H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5z"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Extensions"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 2: Register the module**

In `interfaces/webchat/src/platform/phone/mod.rs`, the module list is:

```rust
pub mod agents;
pub mod chat;
pub mod memory;
pub mod settings;
pub mod shell;
```

Add `pub mod more;` in alphabetical position (after `memory`, before `settings`):

```rust
pub mod agents;
pub mod chat;
pub mod memory;
pub mod more;
pub mod settings;
pub mod shell;
```

- [ ] **Step 3: Add the 5th ••• tab to `PhoneTabBar`**

In `interfaces/webchat/src/platform/phone/shell.rs`, `PhoneTabBar` ends with the Settings button:

```rust
            <button class="tabitem" class:tabitem-active=move || mode.get() == PanelMode::Settings on:click=go("/settings")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 6.6 19l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 4 13.6H4a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 5 6.6l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 10.4 4V4a2 2 0 1 1 4 0v.1A1.6 1.6 0 0 0 17 5l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"></path></svg>
                "Settings"
            </button>
        </div>
    }
}
```

Add the More button immediately after the Settings `</button>`, before `</div>`. Its active predicate uses `under_more()` (not `== More`), so it stays lit on Dashboard/Teams/Extensions too. The icon is three filled dots (`fill="currentColor"` so they pick up the tab color, including `.tabitem-active`):

```rust
                "Settings"
            </button>
            <button class="tabitem" class:tabitem-active=move || mode.get().under_more() on:click=go("/more")>
                <svg width="23" height="23" viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.7"></circle><circle cx="12" cy="12" r="1.7"></circle><circle cx="19" cy="12" r="1.7"></circle></svg>
                "More"
            </button>
        </div>
    }
}
```

(`mode` is already defined in `PhoneTabBar` as `Memo::new(move |_| PanelMode::from_path(&location.pathname.get()))`; `go` is the existing `&'static str` → click-handler helper. No new imports — `under_more` is a method on the already-imported `PanelMode`.)

- [ ] **Step 4: Import `PhoneMore` in `app.rs`**

The phone imports in `app.rs` are:

```rust
use crate::platform::phone::agents::PhoneAgents;
use crate::platform::phone::chat::PhoneChat;
use crate::platform::phone::memory::PhoneMemory;
use crate::platform::phone::settings::appearance::PhoneAppearance;
```

Insert the `more` import after `memory` (alphabetical):

```rust
use crate::platform::phone::memory::PhoneMemory;
use crate::platform::phone::more::PhoneMore;
use crate::platform::phone::settings::appearance::PhoneAppearance;
```

- [ ] **Step 5: Add the `MainContent` More arm**

`MainContent` ends with the Settings arm:

```rust
        <div style:display=move || if mode.get() == PanelMode::Settings { "block" } else { "none" }>
            <SettingsRouter />
        </div>
    }
```

Add the More arm before the closing `}` of the `view!`. Phone renders `PhoneMore`; desktop (unreachable) renders nothing. `form_factor` and `FormFactor` are already in scope in `MainContent`:

```rust
        <div style:display=move || if mode.get() == PanelMode::Settings { "block" } else { "none" }>
            <SettingsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::More { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneMore /> }.into_any()
            } else {
                // /more is phone-only; desktop never routes here.
                ().into_any()
            }}
        </div>
    }
```

- [ ] **Step 6: Self-review + commit**

Self-review checklist: `more.rs` created with the single static `<div class="list">` child (no footgun); `pub mod more;` registered alphabetically; the ••• tab uses `under_more()` and navigates `/more`; the dots SVG uses `fill="currentColor"`; `PhoneMore` imported; `MainContent` More arm present with phone/desktop swap and the `()` desktop fallback; the existing Settings arm/button untouched. Then commit:

```bash
git add interfaces/webchat/src/platform/phone/more.rs interfaces/webchat/src/platform/phone/mod.rs interfaces/webchat/src/platform/phone/shell.rs interfaces/webchat/src/app.rs
git commit -m "panel: phone More tab + sections menu (Dashboard/Teams/Extensions)"
```

**Verification (controller, after the task):** run `just wasm`; expect exit 0 / "✓ WASM dist OK". (Runtime correctness — 5 tabs, menu, navigation, ••• active-state — is verified in the iOS-sim QA gate below, which is user-driven.)

---

## Post-implementation (controller)

1. Rebuild + commit the WASM dist (`just wasm` already does the build; commit the regenerated `dist/` per the established rust-embed embedding chain) so the embedded panel reflects the new screen.
2. **iOS-sim QA (authoritative runtime gate, user-driven)** — per `feedback-ios-panel-test-via-full-macos-app`: rebuild the full macOS app to re-embed the committed dist on `:18790`, connect the iOS sim to the same local core, then verify:
   - Bottom bar shows 5 tabs (Chat / Memory / Agents / Settings / More); layout not cramped.
   - Tap ••• → full-screen More menu, 3 rows (Dashboard / Teams / Extensions), **no left-right split**.
   - Tap a row → navigates to that mode (still desktop layout for now — agreed transition behavior).
   - ••• tab stays highlighted on `/dashboard`, `/teams`, `/extensions`, `/more`; on Chat/Memory/Agents/Settings the ••• is not highlighted and the matching tab is.
3. Push + deploy are user-driven (not part of this plan).

## Success Criteria (from spec §10)

- [ ] Phone bottom bar has 5 tabs; ••• is the 5th.
- [ ] `/more` renders the full-screen `PhoneMore` menu (no split); the 3 rows navigate correctly.
- [ ] ••• active-state follows `under_more()` (lit on Dashboard/Teams/Extensions/More).
- [ ] Desktop functionally byte-unchanged; `just wasm` compiles clean.
- [ ] Unit tests cover `from_path("/more")` and `under_more()`.
- [ ] The three target screens remain untouched (their own later specs).

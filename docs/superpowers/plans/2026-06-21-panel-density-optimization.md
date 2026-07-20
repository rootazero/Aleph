# Panel Density Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Leptos/WASM Panel ~12–15% denser (smaller body type, tighter spacing, weaker shadows, restructured chat-sidebar chrome) without altering the "Quiet Luxury" aesthetic, and expose a user-adjustable 「紧凑度」 (density) knob alongside the existing 字号/圆角 knobs.

**Architecture:** Three orthogonal runtime CSS axes — 字号 (`--control-ui-text-scale`), 圆角 (`--control-ui-radius-scale`), and the NEW 紧凑度 (`--control-ui-density`). The density knob drives Tailwind v4's single `--spacing` base unit, so every numeric padding/margin/gap/size utility re-scales from one value. A denser baseline is baked into the tokens; the knob's cleared-key default IS that compact baseline. Plus targeted surgery on the two named offenders (chat sidebar advanced-zone, chat bubble spacing/shadow).

**Tech Stack:** Rust + Leptos (WASM), Tailwind CSS v4 (`@theme` token system, OKLCH), `localStorage` persistence, `just wasm` build.

## Global Constraints

- **Do NOT touch the glass material system** — `--mat-*` primitives, `.glass`/`.msg-glass` sheen/grain/blur, OKLCH palette stay byte-identical. Density work touches ONLY whitespace, body font-size/line-height, and shadow strength.
- **Do NOT edit any `.dark` block or its `@media (prefers-color-scheme: dark)` mirror.** The `mirror_blocks_are_verbatim_copies` + `token_mirror_blocks_are_verbatim_copies` tests in `appearance.rs` enforce verbatim copies. All token edits in this plan land in single-definition regions (`@theme`, the a11y `:root`, `body`, the derived `:root` msg-glass block) — none of which are mirrored.
- **Magnitude = moderate ~12–15% ("克制").** Body 14→13px, `--spacing` 0.25→0.22rem baseline, shadow outer layers softened ~20–30%.
- **Appearance settings page is hardcoded Chinese** (no i18n macro) — the density knob follows suit: zh `&'static str` labels, no locale-file changes.
- **Code comments in English**; UI copy in Chinese (project convention).
- **Cargo frugality:** one host `cargo test -p aleph-panel --lib` run for the enum (TDD red+green), and ONE `just wasm` integration build at the end. CSS/view edits are verified by the final build, not per-edit cargo runs.
- **Shared checkout caution:** the working tree has unrelated in-flight edits from another session (`cron/mod.rs`, `settings/channels/platform_page.rs`, `settings/routing_rules.rs`, `teams/replay.rs`, `teams/workers.rs`, plus the pre-existing `dist/*`, `views/extensions/mod.rs`). Touch ONLY the files named in each task. Stage files explicitly by path in every commit — never `git add -A`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `interfaces/webchat/styles/tailwind.css` | Global density tokens: `--spacing`, `--control-ui-density`, body font/line-height, wide-screen step, shadow tokens, msg-glass-shadow | Task 1 |
| `interfaces/webchat/src/appearance.rs` | `Density` enum + `KEY_DENSITY` + read/apply/init + round-trip tests | Task 2 |
| `interfaces/webchat/src/views/settings/appearance.rs` | 「紧凑度」 SettingCard row + reset wiring | Task 3 |
| `interfaces/webchat/src/components/chat_sidebar.rs` | Advanced-zone 3 stacked buttons → 1 compact icon row; top padding/divider trim | Task 4 |
| `interfaces/webchat/src/views/chat/messages.rs` | Message list `space-y-3` → `space-y-2` | Task 5 |
| `interfaces/webchat/dist/*` | Rebuilt artifacts (`just wasm`) | Task 6 |

---

## Task 1: Global density tokens (CSS)

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css` (5 single-definition regions)

**Interfaces:**
- Produces: CSS custom property `--control-ui-density` (default `1`) consumed by `apply_density` in Task 2; tighter `--spacing` base consumed by all Tailwind numeric utilities panel-wide.

- [ ] **Step 1: Add the `--spacing` base inside the `@theme` block.** Insert immediately AFTER the `--radius-full: 9999px;` line (currently ~line 103), before the `=== Depth ===` shadow comment:

```css
  /* === Spacing base — Tailwind v4 keys every numeric `p-*/m-*/gap-*/w-*/h-*`
         utility off this one unit. 0.22rem (~12% under the 0.25rem Tailwind
         default) bakes a denser baseline; `--control-ui-density` (the Appearance
         「紧凑度」 knob, default 1) re-scales the whole panel's whitespace from
         here, exactly as `--control-ui-text-scale` re-scales every rem. === */
  --spacing: calc(0.22rem * var(--control-ui-density, 1));
```

- [ ] **Step 2: Declare the density knob default in the a11y `:root` block.** Find the `:root {` block containing `--control-ui-text-scale: 1;` and `--control-ui-radius-scale: 1;` (currently ~lines 155-156). Add a third line directly after `--control-ui-radius-scale: 1;`:

```css
  --control-ui-density: 1;
```

- [ ] **Step 3: Soften the body type.** In the `body {` rule, change these two lines:

```css
  font-size: 0.875rem;        /* 14px @ scale=1 */
  line-height: 1.55;
```

to:

```css
  font-size: 0.8125rem;       /* 13px @ scale=1 — denser baseline */
  line-height: 1.5;
```

- [ ] **Step 4: Soften the wide-screen step-up.** In the `@media (min-width: 1600px)` block, change:

```css
  body { font-size: 0.9375rem; }   /* 15px */
```

to:

```css
  body { font-size: 0.875rem; }    /* 14px */
```

- [ ] **Step 5: Weaken the elevation shadow tokens (outer layers only; xs/sm untouched).** In the `@theme` `=== Depth ===` block, replace the `--shadow-md`, `--shadow-lg`, `--shadow-xl` definitions:

```css
  --shadow-md: 0 1px 2px oklch(0.20 0.02 310 / 0.06),
               0 3px 6px oklch(0.20 0.02 310 / 0.08),
               0 8px 20px oklch(0.20 0.02 310 / 0.11);
  --shadow-lg: 0 2px 4px oklch(0.20 0.02 310 / 0.07),
               0 6px 12px oklch(0.20 0.02 310 / 0.10),
               0 18px 40px oklch(0.20 0.02 310 / 0.16);
  --shadow-xl: 0 4px 8px oklch(0.20 0.02 310 / 0.09),
               0 12px 24px oklch(0.20 0.02 310 / 0.12),
               0 32px 68px oklch(0.20 0.02 310 / 0.22);
```

with:

```css
  --shadow-md: 0 1px 2px oklch(0.20 0.02 310 / 0.05),
               0 3px 6px oklch(0.20 0.02 310 / 0.07),
               0 8px 18px oklch(0.20 0.02 310 / 0.09);
  --shadow-lg: 0 2px 4px oklch(0.20 0.02 310 / 0.06),
               0 6px 12px oklch(0.20 0.02 310 / 0.08),
               0 16px 34px oklch(0.20 0.02 310 / 0.12);
  --shadow-xl: 0 3px 6px oklch(0.20 0.02 310 / 0.07),
               0 10px 20px oklch(0.20 0.02 310 / 0.10),
               0 24px 52px oklch(0.20 0.02 310 / 0.16);
```

- [ ] **Step 6: Weaken the chat-bubble ambient shadow.** Find the derived `:root {` block (the one defining `--msg-glass-bg`, ~line 896-907) and change:

```css
  --msg-glass-shadow: 0 4px 16px var(--mat-shadow);
```

to:

```css
  --msg-glass-shadow: 0 2px 10px var(--mat-shadow);
```

- [ ] **Step 7: Sanity-check no mirror block was touched.** Run:

```bash
grep -n "control-ui-density\|--spacing:\|0.8125rem\|0 2px 10px var(--mat-shadow)" interfaces/webchat/styles/tailwind.css
```

Expected: matches only in `@theme`, the a11y `:root`, `body`, and the derived msg-glass `:root` — NOT inside any `.dark {` or `@media (prefers-color-scheme: dark)` block. (Verification that the mirror tests still pass happens in Task 2's test run, which recompiles the `include_str!`'d CSS.)

- [ ] **Step 8: Commit.**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: bake denser spacing/type/shadow tokens + --control-ui-density hook"
```

---

## Task 2: Density knob backend (`appearance.rs`) — TDD

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs` (module doc, `KEY_DENSITY` const, `Density` enum + impl, `read_density`/`apply_density`, `init_appearance`, tests)

**Interfaces:**
- Consumes: `--control-ui-density` CSS hook from Task 1.
- Produces:
  - `pub enum Density { Compact, Cozy, Spacious }` with `Density::ALL: [Density; 3]`, `label(self) -> &'static str`, `css_value(self) -> &'static str`, `storage_value(self) -> Option<&'static str>`, `from_storage(Option<&str>) -> Self`.
  - `pub fn read_density() -> Density`
  - `pub fn apply_density(density: Density)`
  - These are consumed by Task 3.

- [ ] **Step 1: Write the failing tests.** Add these two tests inside the existing `#[cfg(test)] mod tests { ... }` block (e.g. right after `roundness_round_trips_via_css_value`):

```rust
    #[test]
    fn density_round_trips_via_css_value() {
        for d in Density::ALL {
            assert_eq!(Density::from_storage(Some(d.css_value())), d);
        }
        // Compact is the cleared-key default (the new compact baseline).
        assert_eq!(Density::Compact.storage_value(), None);
        assert_eq!(Density::from_storage(None), Density::Compact);
        assert_eq!(Density::from_storage(Some("garbage")), Density::Compact);
    }

    #[test]
    fn density_non_default_values_persist_a_key() {
        assert!(Density::Cozy.storage_value().is_some());
        assert!(Density::Spacious.storage_value().is_some());
    }
```

- [ ] **Step 2: Run tests to verify they fail.** Run:

```bash
cargo test -p aleph-panel --lib density
```

Expected: FAIL — compile error `cannot find type/value Density in this scope` (the enum does not exist yet).

- [ ] **Step 3: Add the `KEY_DENSITY` const.** After the `const KEY_MATERIAL: &str = "aleph-material";` line (~line 26):

```rust
const KEY_DENSITY: &str = "aleph-density";
```

- [ ] **Step 4: Add the `Density` enum + impl.** Insert AFTER the `Roundness` impl block (after its closing `}`, ~line 348) and BEFORE the `// DOM / storage plumbing` banner:

```rust
// ---------------------------------------------------------------------------
// Density (whitespace compactness)
// ---------------------------------------------------------------------------

/// Whitespace compactness. Drives `--control-ui-density`, the multiplier that
/// Tailwind v4's `--spacing` base unit keys off of — so every numeric
/// padding/margin/gap/size utility re-scales from one value. `Compact` is the
/// cleared-key default: the baked baseline is already ~12% tighter than stock,
/// and the knob only adds breathing room from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,  // 1×    — the new compact baseline (default, clears the key)
    Cozy,     // 1.13× — restores the original ~0.25rem whitespace
    Spacious, // 1.25× — roomier
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Cozy, Self::Spacious];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "紧凑",
            Self::Cozy => "适中",
            Self::Spacious => "宽松",
        }
    }

    /// CSS multiplier applied to the `--spacing` base.
    #[must_use]
    pub const fn css_value(self) -> &'static str {
        match self {
            Self::Compact => "1",
            Self::Cozy => "1.13",
            Self::Spacious => "1.25",
        }
    }

    /// `localStorage` value, or `None` for the default (clears the key).
    #[must_use]
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Compact => None,
            other => Some(other.css_value()),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("1.13") => Self::Cozy,
            Some("1.25") => Self::Spacious,
            _ => Self::Compact,
        }
    }
}
```

- [ ] **Step 5: Add `read_density`.** After the `read_roundness` fn (~line 408):

```rust
#[must_use]
pub fn read_density() -> Density {
    Density::from_storage(read_key(KEY_DENSITY).as_deref())
}
```

- [ ] **Step 6: Add `apply_density`.** After the `apply_roundness` fn (~line 465):

```rust
pub fn apply_density(density: Density) {
    if let Some(html) = root() {
        let _ = html
            .style()
            .set_property("--control-ui-density", density.css_value());
    }
    persist(KEY_DENSITY, density.storage_value());
}
```

- [ ] **Step 7: Replay density on boot.** In `init_appearance`, after the `roundness` block (the `if roundness != Roundness::Default { apply_roundness(roundness); }`, ~line 514) and before the `material` block:

```rust
    let density = read_density();
    if density != Density::Compact {
        apply_density(density);
    }
```

- [ ] **Step 8: Update the module doc to list the new axis.** In the `//!` header, change `//! Five orthogonal, client-side axes` to `//! Six orthogonal, client-side axes`, and add a bullet after the `roundness` line:

```rust
//!   • density   — whitespace compactness           → `--control-ui-density`
```

- [ ] **Step 9: Run tests to verify they pass.** Run:

```bash
cargo test -p aleph-panel --lib
```

Expected: PASS — `density_round_trips_via_css_value`, `density_non_default_values_persist_a_key`, and the existing `mirror_blocks_are_verbatim_copies` / `token_mirror_blocks_are_verbatim_copies` (which recompile Task 1's edited CSS) all green.

- [ ] **Step 10: Commit.**

```bash
git add interfaces/webchat/src/appearance.rs
git commit -m "panel: add Density appearance axis (紧凑度 knob, default compact)"
```

---

## Task 3: Density knob UI (`settings/appearance.rs`)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/appearance.rs` (imports, reset closure, signal, SettingCard row, doc)

**Interfaces:**
- Consumes: `Density`, `read_density`, `apply_density` from Task 2; the existing `ChoiceButton` + `SettingCard` components in this file.

- [ ] **Step 1: Extend the imports.** Change the `use crate::appearance::{ ... };` block to add `apply_density`, `read_density`, `Density`:

```rust
use crate::appearance::{
    apply_accent, apply_density, apply_font_scale, apply_material, apply_mode, apply_roundness,
    read_accent, read_density, read_font_scale, read_material, read_mode, read_roundness, Accent,
    Density, FontScale, Material, Roundness, ThemeMode,
};
```

- [ ] **Step 2: Add the density signal.** After `let roundness = RwSignal::new(read_roundness());` (~line 24):

```rust
    let density = RwSignal::new(read_density());
```

- [ ] **Step 3: Wire density into the reset closure.** In the `reset` closure, add an apply call after `apply_roundness(Roundness::Default);` and a `density.set` after `roundness.set(Roundness::Default);`:

```rust
        apply_density(Density::Compact);
```

```rust
        density.set(Density::Compact);
```

- [ ] **Step 4: Add the 「紧凑度」 SettingCard.** Insert directly AFTER the `// --- Roundness ---` `SettingCard` (its closing `</SettingCard>`, ~line 135) and BEFORE the `// --- Live preview ---` block:

```rust
                // --- Density ----------------------------------------------------
                <SettingCard title="紧凑度" desc="界面留白与控件间距。「紧凑」更省空间，单屏显示更多内容。">
                    <div class="flex flex-wrap gap-2">
                        {Density::ALL.into_iter().map(|d| {
                            let active = move || density.get() == d;
                            view! {
                                <ChoiceButton
                                    label=d.label()
                                    active=Signal::derive(active)
                                    on_pick=move || { apply_density(d); density.set(d); }
                                />
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>
```

- [ ] **Step 5: Refresh the header copy + module doc to include density.** Change the page subtitle (`<p class="text-text-secondary">`) text from `"调整主题、强调色、字号与圆角。所有设置保存在本机浏览器，立即生效。"` to:

```rust
                    "调整主题、强调色、字号、圆角与紧凑度。所有设置保存在本机浏览器，立即生效。"
```

And in the `//!` module doc, change `four client-side appearance axes (theme mode, accent, font scale, roundness)` to `client-side appearance axes (theme mode, accent, material, font scale, roundness, density)`.

- [ ] **Step 6: Verify it compiles (folded into Task 6's `just wasm`).** No separate cargo run here — the view! macro expansion is checked by the integration build in Task 6. (Rationale: cargo frugality; a standalone `cargo check` here would duplicate Task 6's compile.)

- [ ] **Step 7: Commit.**

```bash
git add interfaces/webchat/src/views/settings/appearance.rs
git commit -m "panel: surface 紧凑度 knob in Appearance settings"
```

---

## Task 4: Chat sidebar — collapse advanced zone to a compact icon row

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs` (advanced-features zone ~lines 743-790, divider ~line 812)

**Interfaces:**
- Consumes: existing `show_compose` signal, `navigate` handle, `i18n` (`chat.team_chat`, `chat.project_management`, `nav.extensions`) — all already in scope in `ChatSidebar`.

- [ ] **Step 1: Tighten the top action container.** Change the opening tag (currently `<div class="p-3 space-y-2">`, ~line 743) to:

```rust
            <div class="p-2 space-y-2">
```

- [ ] **Step 2: Replace the three stacked full-width buttons with one icon row.** Replace the entire `<div class="flex flex-col gap-1.5"> ... </div>` block (the team-chat button, the disabled project-management button with its "coming soon" badge, and the Aleph Hub button — currently ~lines 748-790) with:

```rust
                // ── Advanced features zone ──────────────────────────────
                // Team chat / Project management (placeholder) / Aleph Hub,
                // collapsed into ONE compact icon row to reclaim vertical height
                // (was three stacked full-width rows). Each button keeps its own
                // click / disabled / navigation behavior; the text label is
                // dropped in favor of an emoji glyph + `title` tooltip.
                <div class="flex items-center gap-1.5">
                    <button
                        class="flex-1 inline-flex items-center justify-center px-2 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               hover:border-primary transition-colors"
                        title=move || t_string!(i18n, chat.team_chat).to_string()
                        on:click=move |_| show_compose.set(true)
                    >
                        "👥"
                    </button>
                    <button
                        class="flex-1 inline-flex items-center justify-center px-2 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               opacity-70 cursor-not-allowed"
                        title=move || t_string!(i18n, chat.project_management).to_string()
                        disabled=true
                    >
                        "📁"
                    </button>
                    <button
                        class="flex-1 inline-flex items-center justify-center px-2 py-1.5 rounded-lg
                               bg-surface-sunken border border-border text-sm
                               hover:border-primary transition-colors"
                        title=move || t_string!(i18n, nav.extensions).to_string()
                        on:click={
                            let navigate = navigate.clone();
                            move |_| navigate("/extensions", Default::default())
                        }
                    >
                        "🧩"
                    </button>
                </div>
```

- [ ] **Step 3: Soften the section divider.** Find the divider line (`<div class="border-t border-border/50"></div>`, ~line 812) and change `border-border/50` to `border-border/40`:

```rust
                <div class="border-t border-border/40"></div>
```

- [ ] **Step 4: Confirm no now-unused i18n reference remains in this file.** The removed "coming soon" badge was the only use of `chat.coming_soon` here. Run:

```bash
grep -n "chat.coming_soon" interfaces/webchat/src/components/chat_sidebar.rs
```

Expected: no matches (the locale key itself stays defined elsewhere — only this file's reference is gone; no Rust import is orphaned).

- [ ] **Step 5: Commit.**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: collapse chat sidebar advanced zone into a compact icon row"
```

---

## Task 5: Chat bubble — tighten inter-message spacing

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs` (message list container ~line 193)

**Interfaces:**
- Consumes: nothing new. Bubble padding (`px-4 py-3` etc.) shrinks automatically via Task 1's `--spacing`; this task only tightens the row gap.

- [ ] **Step 1: Reduce the message-list vertical rhythm.** Find the message list container class string (currently `"max-w-3xl mx-auto px-4 {} pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-3"`, ~line 193) and change `space-y-3` to `space-y-2`:

```rust
                            "max-w-3xl mx-auto px-4 {} pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-2",
```

- [ ] **Step 2: Commit.**

```bash
git add interfaces/webchat/src/views/chat/messages.rs
git commit -m "panel: tighten chat message list spacing (space-y-3 → space-y-2)"
```

---

## Task 6: Integration build + verification

**Files:**
- Modify (generated): `interfaces/webchat/dist/*` via `just wasm`

**Interfaces:**
- Consumes: all prior tasks.

- [ ] **Step 1: Rebuild WASM + Tailwind + dist.** From the repo root run:

```bash
just wasm
```

Expected: completes without error; Tailwind regenerates `dist/tailwind.css`, trunk/wasm-bindgen regenerate `dist/aleph_panel.js` + `dist/aleph_panel_bg.wasm`. A compile error here means a Rust/view! mistake in Tasks 2–5 — fix in the owning file and rerun.

- [ ] **Step 2: Confirm the density token reached the built CSS.** Run:

```bash
grep -c "control-ui-density" interfaces/webchat/dist/tailwind.css
```

Expected: ≥1 (the `--spacing` calc referencing the knob is present in the built stylesheet).

- [ ] **Step 3: Re-run the appearance test suite against the rebuilt tree (cheap, host).** Run:

```bash
cargo test -p aleph-panel --lib
```

Expected: PASS — density round-trips + both mirror-verbatim tests green.

- [ ] **Step 4: Manual verification checklist (optional redeploy).** If doing a visual pass (server rebuild + screenshot per user instruction): open the Panel and confirm —
  - Settings ▸ 外观 shows a new 「紧凑度」 row with 紧凑/适中/宽松; switching changes whitespace live; 恢复默认 returns to 紧凑.
  - Chat sidebar advanced zone is one icon row (👥/📁/🧩); 📁 is dimmed + non-clickable; 👥 opens team compose; 🧩 navigates to /extensions; all three show tooltips.
  - Chat bubbles read tighter with a lighter shadow; glass sheen/grain unchanged.
  - Toggle dark mode + each accent + each material: shadows/spacing render correctly, no material regression.

- [ ] **Step 5: Stage ONLY the regenerated dist artifacts and commit.**

```bash
git add interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm interfaces/webchat/dist/tailwind.css
git commit -m "panel: rebuild webchat dist artifacts (density optimization)"
```

> Note: the pre-existing `dist/*` modifications in the working tree predate this work; `just wasm` overwrites them with the current build. Staging the three dist files by explicit path (not `git add -A`) keeps the unrelated in-flight edits from other sessions out of these commits.

---

## Self-Review

**Spec coverage:**
- Block 1 (global token闸: `--spacing`, body font/line-height, wide step, `--msg-glass-shadow`, `--shadow-md/lg/xl`) → Task 1. ✓
- Block 2 (紧凑度 knob: `Density` enum + read/apply/init + tests; settings row; default compact) → Tasks 2 + 3. ✓ (Spec's "locales/*" line is superseded — the Appearance page is hardcoded zh, so no locale work; chat sidebar reuses existing i18n keys.)
- Block 3 (chat sidebar icon row; bubble spacing/shadow; other tabs via global闸) → Tasks 4 + 5; other tabs ride Task 1's `--spacing` automatically. ✓
- Block 4 (verification: host enum test; `just wasm`; optional redeploy) → Task 6. ✓
- Red lines (no material edits, no `.dark` mirror edits, single-definition regions) → Global Constraints + Task 1 Step 7. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to Task N". Every code step shows literal before/after. ✓

**Type consistency:** `Density` / `Density::ALL` / `label` / `css_value` / `storage_value` / `from_storage` / `read_density` / `apply_density` / `KEY_DENSITY` are defined in Task 2 and used verbatim in Task 3. `apply_density(Density::Compact)` (reset) and `Density::from_storage` keys (`"1.13"`/`"1.25"`) are internally consistent with `css_value`. ✓

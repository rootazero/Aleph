# Glass Material Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out the four recorded follow-ups from the glass-material-themes round (merged as `1b8348fe3`): WKWebView live measurements with the §6.2 fade verdict, `SwatchButton` dedup, `aria-pressed` accessibility, and the token-layer mirror parity test.

**Architecture:** No new architecture. Three small code tasks inside the existing Leptos panel (`interfaces/webchat/`) plus one controller-run measurement task against the real WebKit engine. All appearance logic stays in `crate::appearance` (single source); the new `SwatchButton` is a dumb view primitive in `components/ui/`.

**Tech Stack:** Leptos 0.7 (WASM), Tailwind v4 CSS, host-side `cargo test -p aleph-panel --lib`, Swift + WKWebView for measurements.

**Environment hard rules (every implementer):**

- Work ONLY in the worktree `/Volumes/TBU4/Workspace/aleph-glass-material` (branch `aleph-glass-material`). The shell cwd RESETS to the main repo between Bash calls — every command must be `cd /Volumes/TBU4/Workspace/aleph-glass-material && …` in the SAME call. Before claiming any test/build result, confirm with `pwd`.
- Test baseline in this worktree: **352 passed** (`cargo test -p aleph-panel --lib`). If you see 349, you are in the wrong directory — stop.
- Panel Rust changes MUST pass the wasm gate: `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown` (native check alone is insufficient — cfg(wasm32)-gated code exists).
- Never `git add -A` / `git add .` (untracked `node_modules` symlink + target/ artifacts). Stage explicit paths only.
- `cargo fmt -p aleph-panel` before committing; never bare `cargo fmt` for the whole workspace.
- New tests must not pull `web_sys` paths at host-test time (host-test redline).
- Builds share the main repo target dir via flock — parallel cargo invocations queue; that is expected, do not set CARGO_TARGET_DIR.

---

### Task 1: `SwatchButton` primitive + the three picker swatch sites

**Files:**
- Create: `interfaces/webchat/src/components/ui/swatch_button.rs`
- Modify: `interfaces/webchat/src/components/ui/mod.rs` (register + re-export)
- Modify: `interfaces/webchat/src/views/settings/appearance.rs` (Material + Accent cards)
- Modify: `interfaces/webchat/src/components/theme_toggle.rs` (popover Accent row)

The "swatch chip + active ring + optional caption" pattern is duplicated 3× (settings Material `:70-87`, settings Accent `:99-117`, popover Accent `:217-238`). The popover trigger's tiny accent dot (`theme_toggle.rs:121-123`) is a passive indicator, NOT a picker — leave it alone. `aria-pressed` is built into the component from birth (Task 2 covers the remaining text-pill buttons).

- [ ] **Step 1: Create the component**

```rust
//! Swatch picker chip — the shared "colour/material face + active ring"
//! primitive behind the appearance pickers (settings page + topbar popover).

use leptos::prelude::*;

/// A swatch chip button for appearance pickers: a colour/gradient face with
/// an active ring, an optional caption below, and `aria-pressed` toggle
/// semantics. The click handler receives the raw `MouseEvent` so popover
/// callers can feed the View-Transition reveal origin; settings callers
/// ignore it.
#[component]
#[must_use]
pub fn SwatchButton(
    /// CSS background of the chip face (any CSS color/gradient value).
    background: &'static str,
    /// Size/shape/hover classes for the face, e.g.
    /// "w-9 h-9 rounded-full transition-transform group-hover:scale-110".
    face: &'static str,
    /// `ring-offset-*` class matching the surface the picker sits on.
    ring_offset: &'static str,
    /// Tooltip + accessible name.
    title: &'static str,
    /// Caption below the chip; omit to render the chip alone.
    #[prop(optional, into)] label: Option<&'static str>,
    #[prop(into)] active: Signal<bool>,
    on_pick: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            on:click=move |ev: web_sys::MouseEvent| on_pick(ev)
            title=title
            aria-pressed=move || active.get().to_string()
            class="flex flex-col items-center gap-1.5 group"
        >
            <span
                class=move || {
                    if active.get() {
                        format!("{face} ring-2 ring-offset-2 {ring_offset} ring-text-primary")
                    } else {
                        format!("{face} ring-1 ring-border")
                    }
                }
                style=format!("background: {background}")
            />
            {label.map(|l| view! { <span class="text-xs text-text-secondary">{l}</span> })}
        </button>
    }
}
```

Note `face` carries the full base class set including `transition-transform` — the component only appends ring state. If `#[prop(optional, into)]` fights the macro for `Option<&'static str>`, fall back to `#[prop(optional)]` and pass `label=Some(m.label())` at the two labelled sites.

- [ ] **Step 2: Register in `components/ui/mod.rs`** — add `pub mod swatch_button;` to the module list (alphabetical) and `pub use swatch_button::SwatchButton;` next to the existing re-exports.

- [ ] **Step 3: Replace the settings Material card body** (`views/settings/appearance.rs`, the `<button …>` inside the Material `SettingCard`):

```rust
{Material::ALL.into_iter().map(|m| {
    let active = move || material.get() == m;
    view! {
        <SwatchButton
            background=m.preview()
            face="w-14 h-9 rounded-lg transition-transform group-hover:scale-105"
            ring_offset="ring-offset-surface-raised"
            title=m.label()
            label=m.label()
            active=Signal::derive(active)
            on_pick=move |_| { apply_material(m); material.set(m); }
        />
    }
}).collect::<Vec<_>>()}
```

- [ ] **Step 4: Replace the settings Accent card body** (same file):

```rust
{Accent::ALL.into_iter().map(|a| {
    let active = move || accent.get() == a;
    view! {
        <SwatchButton
            background=a.swatch()
            face="w-9 h-9 rounded-full transition-transform group-hover:scale-110"
            ring_offset="ring-offset-surface-raised"
            title=a.label()
            label=a.label()
            active=Signal::derive(active)
            on_pick=move |_| { apply_accent(a); accent.set(a); }
        />
    }
}).collect::<Vec<_>>()}
```

Add `use crate::components::ui::SwatchButton;` to the imports.

- [ ] **Step 5: Replace the popover Accent row body** (`components/theme_toggle.rs:214-240`, inside the existing `flex items-center justify-between px-1` wrapper):

```rust
{Accent::ALL.into_iter().map(|a| {
    let is_active = move || accent.get() == a;
    view! {
        <SwatchButton
            background=a.swatch()
            face="w-6 h-6 rounded-full transition-transform group-hover:scale-110"
            ring_offset="ring-offset-surface-overlay"
            title=a.label()
            active=Signal::derive(is_active)
            on_pick=move |ev: web_sys::MouseEvent| {
                let x = ev.client_x() as f64;
                let y = ev.client_y() as f64;
                animated_apply(x, y, move || apply_accent(a));
                accent.set(a);
            }
        />
    }
}).collect::<Vec<_>>()}
```

Add the `SwatchButton` import. Known accepted DOM deltas (document in the commit message): popover container gap `gap-1` → `gap-1.5` (single child without label — zero visual change) and new `aria-pressed` attribute everywhere.

- [ ] **Step 6: Verify**

```bash
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo build -p aleph-panel --lib --target wasm32-unknown-unknown
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo test -p aleph-panel --lib
```
Expected: build OK, **352 passed** (no new host tests — view-only change; host tests cannot render Leptos DOM).

- [ ] **Step 7: fmt + clippy spot + commit**

```bash
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo fmt -p aleph-panel && cargo clippy -p aleph-panel --lib -- -D warnings 2>&1 | tail -3
cd /Volumes/TBU4/Workspace/aleph-glass-material && git add interfaces/webchat/src/components/ui/swatch_button.rs interfaces/webchat/src/components/ui/mod.rs interfaces/webchat/src/views/settings/appearance.rs interfaces/webchat/src/components/theme_toggle.rs && git commit -m "panel: extract SwatchButton primitive for the three appearance pickers"
```

### Task 2: `aria-pressed` on the remaining toggle buttons

**Files:**
- Modify: `interfaces/webchat/src/views/settings/appearance.rs` (`ChoiceButton`)
- Modify: `interfaces/webchat/src/components/theme_toggle.rs` (popover Mode + Material text rows)

`SwatchButton` already carries `aria-pressed`; this closes the gap on the text-pill toggles so all five settings cards (主题模式/材质/强调色/字号/圆角) and all three popover rows expose toggle state to AT.

- [ ] **Step 1: `ChoiceButton`** — in its `view!`, add the attribute right after `on:click`:

```rust
aria-pressed=move || active.get().to_string()
```

- [ ] **Step 2: Popover Mode row button** (`theme_toggle.rs`, the button inside the Mode `grid grid-cols-3`) — add after its `on:click` closure:

```rust
aria-pressed=move || is_active().to_string()
```

- [ ] **Step 3: Popover Material row button** — same one-line attribute as Step 2.

- [ ] **Step 4: Verify + commit**

```bash
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo build -p aleph-panel --lib --target wasm32-unknown-unknown && cargo test -p aleph-panel --lib 2>&1 | tail -2
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo fmt -p aleph-panel && git add interfaces/webchat/src/views/settings/appearance.rs interfaces/webchat/src/components/theme_toggle.rs && git commit -m "panel: expose aria-pressed on every appearance toggle button"
```
Expected: 352 passed.

### Task 3: Token-layer mirror parity — re-align 4 accent mirrors + extend the test

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css` (the `@media (prefers-color-scheme: dark)` accent mirror block, ~line 389)
- Modify: `interfaces/webchat/src/appearance.rs` (test module)
- Possibly modify: `interfaces/webchat/dist/tailwind.css` (only if bytes change — expected NOT to)

Pre-checked facts (controller, 2026-06-12): in the token layer (everything ABOVE the "Material primitives" banner), `.dark {` ↔ `:root:not(.light) {` are already verbatim (24 lines); the four `html.dark[data-accent="…"] {` blocks drift from their `:root:not(.light)[data-accent="…"] {` mirrors in **interior whitespace only** (value-column alignment — property names and values identical). Fix direction: make each mirror line's content match the dark block verbatim (trimmed-line equality), i.e. copy the dark block's lines into the mirror, keeping the mirror's own leading indentation.

- [ ] **Step 1 (RED): Extract the shared assertion + add the token-layer test** in `appearance.rs`'s test module. Refactor `mirror_blocks_are_verbatim_copies` so both tests share one helper:

```rust
/// Assert every `(dark, mirror)` selector pair in `section` has a verbatim-
/// identical body (trimmed lines, comments included — mirrors are copies).
fn assert_mirror_pairs(section: &str, pairs: &[(&str, &str)]) {
    for (dark, mirror) in pairs {
        let dark_body = css_block_body(section, dark);
        let mirror_body = css_block_body(section, mirror);
        for (i, (d, m)) in dark_body.iter().zip(mirror_body.iter()).enumerate() {
            assert_eq!(
                d,
                m,
                "system-mode mirror {mirror:?} drifted from {dark:?} at body \
                 line {} — copy the `.dark` block verbatim",
                i + 1
            );
        }
        assert_eq!(
            dark_body.len(),
            mirror_body.len(),
            "system-mode mirror {mirror:?} and {dark:?} have different line \
             counts — copy the `.dark` block verbatim"
        );
    }
}
```

The existing material test becomes `assert_mirror_pairs(material_section, &pairs)`. New test:

```rust
#[test]
fn token_mirror_blocks_are_verbatim_copies() {
    // Same copy-verbatim discipline as the material primitives, applied to
    // the colour-token layer ABOVE the banner: the `.dark` token block and
    // the four dark accent overrides each keep a hand-synced
    // `@media (prefers-color-scheme: dark)` mirror for System-mode users.
    let css = include_str!("../styles/tailwind.css");
    let (token_section, _) = css
        .split_once("Material primitives")
        .expect("material primitives banner present in tailwind.css");
    assert_mirror_pairs(
        token_section,
        &[
            (".dark {", ":root:not(.light) {"),
            (
                r#"html.dark[data-accent="ocean"] {"#,
                r#":root:not(.light)[data-accent="ocean"] {"#,
            ),
            (
                r#"html.dark[data-accent="forest"] {"#,
                r#":root:not(.light)[data-accent="forest"] {"#,
            ),
            (
                r#"html.dark[data-accent="sunset"] {"#,
                r#":root:not(.light)[data-accent="sunset"] {"#,
            ),
            (
                r#"html.dark[data-accent="rose"] {"#,
                r#":root:not(.light)[data-accent="rose"] {"#,
            ),
        ],
    );
}
```

Also update the `css_block_body` doc comment's last sentence: callers pass one banner-delimited SECTION (token layer = before the banner, material primitives = after); the exactly-once assertion holds within each slice.

Run: `cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo test -p aleph-panel --lib token_mirror` — Expected: **FAIL** on the ocean pair (whitespace drift), proving the test sees what the pre-check saw.

- [ ] **Step 2 (GREEN): Re-align the four mirror blocks** in `styles/tailwind.css` — inside the `@media (prefers-color-scheme: dark)` block near line 389, pad each property's value column to match the dark block exactly (e.g. `--color-primary:          oklch(0.68 0.130 250);` — trimmed content identical to `html.dark[data-accent="ocean"]`'s line). 7 lines × 4 accents. Whitespace-only change; values must NOT change.

Run the test again — Expected: PASS, and the full suite `cargo test -p aleph-panel --lib` → **353 passed**.

- [ ] **Step 3: Confirm dist is byte-stable** (minifier swallows whitespace):

```bash
cd /Volumes/TBU4/Workspace/aleph-glass-material/interfaces/webchat && npm run build:css >/dev/null 2>&1; cd /Volumes/TBU4/Workspace/aleph-glass-material && git status --short interfaces/webchat/dist/
```
Expected: no output (dist unchanged). If `dist/tailwind.css` shows modified, inspect the diff — only whitespace-derived changes are acceptable; include it in the commit.

- [ ] **Step 4: fmt + commit**

```bash
cd /Volumes/TBU4/Workspace/aleph-glass-material && cargo fmt -p aleph-panel && git add interfaces/webchat/styles/tailwind.css interfaces/webchat/src/appearance.rs && git commit -m "panel: extend mirror parity test to the colour-token layer"
```
(Stage `interfaces/webchat/dist/tailwind.css` too only if Step 3 showed a diff.)

### Task 4 (controller-run): WKWebView 7-point live measurement + §6.2 verdict

Not a subagent task. Swift WKWebView harness (same WebKit engine as Aleph.app's Tauri shell) drives the acceptance fixture + the live panel; outputs JSON + PNGs under `target/wk-measure/` (gitignored). The seven points, from the round's final review:

1. **Async-scroll fast path** (the §6.2 drop-the-fade trigger): liquid+dark, tabs on, mask ON vs OFF — sustained scroll, rAF frame-time stats.
2. **Streaming re-blur**: simulated token stream under the composer face, luxe (16px) vs liquid (24px).
3. **Composer hover during streaming**: drive the 250ms translateY transition mid-stream; frame stability.
4. **Band dissolve visuals**: 3 materials × light/dark snapshots, content mid-scroll under the band.
5. **Reduced-transparency**: inject the extracted override block (`target/rt-block.json`); verify collapse via computed styles + snapshot.
6. **Overlay scrollbar**: snapshot during scroll; check thumb fade in top 26px (best-effort — overlay thumb may not appear on programmatic scroll).
7. **<2 tabs at top**: pt-6 path, first row at 92-100% mask alpha — snapshot, judge perceptibility.

**Verdict rule (§6.2):** drop the fade (keep the float) iff mask ON raises p95 frame time by >4ms or pushes dropped-frame share (>25ms frames) above 10% while mask OFF holds ~60fps at the same workload. Otherwise the fade stays. If dropped: remove `.chat-scroll-fade` rules + class, reduced-transparency lines, and re-run wasm/dist — as a follow-up task on this branch.

**RESULTS (measured 2026-06-12, real WKWebView via Swift harness, 1440×900@2x, 147-message fixture; artifacts in `target/wk-measure/`):**

| # | Point | Result |
|---|-------|--------|
| 1 | Async-scroll fast path | liquid mask ON avg 16.67ms / OFF 16.66ms (Δ≈0.01ms); p95 17ms both; luxe + aurora identical; 0% dropped, 60.0fps everywhere → **fade STAYS** |
| 2 | Streaming re-blur | luxe 59.9fps vs liquid 59.8fps, p95 21ms, 0 dropped — blur-tier delta invisible |
| 3 | Hover during streaming | 59.9fps, max frame 24ms, 0 dropped — no shimmer-class hitching |
| 4 | Band dissolve ×6 | clean in all snapshots — no full-alpha gap, no mask×backdrop-filter sibling artifact |
| 5 | Reduced-transparency | computed: composer/tabs backdrop `none`, scroller mask `none`, band opaque `oklch(0.15 0.02 310)`; visual collapse confirmed |
| 6 | Overlay scrollbar | thumb does not flash on programmatic scroll in WebKit — not capturable; arithmetic (fade top 26px) matches macOS thumb end-fade convention; non-blocking, eyeball on real use |
| 7 | <2 tabs top rows | first row at 92-100% mask alpha imperceptible in luxe·light AND liquid·dark — as the arithmetic said |

Live-panel pass: bootstrap-URL auth + real panel composition rendered correctly in WKWebView (`real_panel.png`); current session had zero scrollable depth, so fixture numbers govern. **§6.2 verdict: keep the fade.** Follow-up CSS task: none.

### Task 5 (controller-run): artifacts + final review + merge + deploy

1. Rebuild dist on the branch (worktree caveat: run wasm-bindgen with absolute paths if `just wasm` misroutes): wasm build → `wasm-bindgen` → `npm run build:css`; commit `dist/aleph_panel.js`, `dist/aleph_panel_bg.wasm` (tracked artifacts must ship with the branch).
2. Final whole-branch review (spec coverage + quality), fix loop if needed.
3. Verify zero-overlap vs main (`git diff` both sides from merge-base), then `git -C /Volumes/TBU4/Workspace/Aleph merge --no-ff aleph-glass-material`.
4. Rebuild `aleph-server` (rust_embed burn-in), redeploy to `/Applications/Aleph.app` (mv → .bak, cp, supervisor relaunch), live-verify aria-pressed + swatch visual parity via chrome-devtools.
5. NOT pushed (repo convention). Clean worktree in a later session or leave for user.

# Tablet-Specific Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the 640–1023px Tablet band usable by adapting the Wide views in place at the two points where Wide breaks — the nav sidebar becomes a slide-over overlay (default-collapsed), and the 4 floored provider-settings views stack vertically below 720px — with no parallel platform layer and no native/Rust-core change.

**Architecture:** Panel-only (`interfaces/webchat/`). Task 1 drives the existing `sidebar_collapsed` signal from the form factor and adds a `.ff-tablet` overlay CSS rule. Task 2 adds marker classes to the 4 views' shared master-detail skeleton plus one `@media (max-width: 720px)` stack rule. The 12 form-factor routing branches in `app.rs` stay Phone-vs-Wide; band thresholds and `Tablet == Wide` content routing are untouched.

**Tech Stack:** Rust + Leptos (WASM), Tailwind + hand-written `styles/tailwind.css`, `just wasm` build (regenerates `dist/aleph_panel.js`, `dist/aleph_panel_bg.wasm`, `dist/tailwind.css`).

## Global Constraints

Every task implicitly includes these (verbatim from `docs/superpowers/specs/2026-06-28-tablet-rendering-design.md`):

- **D1 — no parallel layer.** Do NOT build out `platform/tablet/` (the empty placeholder stays as-is). Adapt the Wide views in place.
- **No content-routing change.** Do NOT add `FormFactor::Tablet` branches to the 12 form-factor routing sites in `app.rs`; they stay Phone-vs-Wide. Do NOT change band thresholds or the `Tablet == Wide` mapping in `viewport.rs`.
- **Wide (≥1024px) stays byte-for-byte unchanged**, including the persisted `sidebar_collapsed` preference. The `~720px` stack rule and `.ff-tablet` overlay must not affect ≥1024px rendering.
- **No native / Rust-core change.** Changes are confined to `interfaces/webchat/src/` (Leptos) + `interfaces/webchat/styles/tailwind.css` + regenerated `dist/`.
- **Provider stack is CSS + marker-class strings only** — no Rust logic/state change in the 4 (+2) views; the stack must never touch their selection signals.
- **NEVER commit secrets**; commit the regenerated `dist/` artifacts alongside source. NEVER `git add` unrelated files.
- Commit style `panel: <description>`, no `Co-Authored-By` trailer.

---

### Task 1: Tablet sidebar as a default-collapsed slide-over overlay

**Files:**
- Modify: `interfaces/webchat/src/app.rs` (bind a `FormFactorState` handle; add `class:ff-tablet` on the shell root; add a form-factor → `sidebar_collapsed` Effect)
- Modify: `interfaces/webchat/styles/tailwind.css` (add the `.ff-tablet` overlay rules after the existing collapsible-sidebar block)
- Regenerated, commit: `interfaces/webchat/dist/aleph_panel.js`, `interfaces/webchat/dist/aleph_panel_bg.wasm`, `interfaces/webchat/dist/tailwind.css`
- Do NOT touch: `viewport.rs`, any `platform/` view, the 12 routing branches

**Interfaces:**
- Consumes: existing `MemoryState.sidebar_collapsed: RwSignal<bool>`, `FormFactorState.form_factor: RwSignal<FormFactor>`, the `.aleph-shell` / `.aleph-sidebar` / `.sidebar-collapsed` CSS.
- Produces: nothing other tasks rely on (Task 2 is independent).

**Context for the implementer:** `AppContent` (`app.rs:72`) provides `MemoryState::new()` (line 77) and `FormFactorState::new()` (line 101). The shell root `<div class="aleph-shell …">` (line 198) already carries `class:sidebar-collapsed=…` (line 200). Esc uncollapses (lines 152–160). CSS `.aleph-shell.sidebar-collapsed .aleph-sidebar { transform: translateX(-100%); width: 0; overflow: hidden; }` (`tailwind.css:1877`) hides the sidebar; `.aleph-shell` is already `position: relative; isolation: isolate` (`tailwind.css:1431`). `FormFactor` + `FormFactorState` are already imported (`app.rs:54`).

- [ ] **Step 1: Add the form-factor → sidebar Effect in `AppContent`**

Insert this block immediately after the Esc-key block (after `app.rs:160`, i.e. after its closing `}`):

```rust
    // Tablet auto-collapses the sidebar so the main content gets full width;
    // the `.ff-tablet` shell class (added below) makes a *revealed* sidebar a
    // slide-over overlay rather than an in-flow column. The Effect's prev-value
    // tracks the previous band: it skips its Wide branch on the initial run so
    // a Wide boot honors the persisted localStorage preference, while a Tablet
    // boot still collapses. Manual toggles within a band don't re-fire it (it
    // only depends on `form_factor`), so they are preserved.
    {
        let mem_for_ff = expect_context::<MemoryState>();
        let ff = expect_context::<FormFactorState>();
        Effect::new(move |prev_band: Option<FormFactor>| -> FormFactor {
            let now = ff.form_factor.get();
            match prev_band {
                None => {
                    if now == FormFactor::Tablet {
                        mem_for_ff.sidebar_collapsed.set(true);
                    }
                }
                Some(was) if was != now => match now {
                    FormFactor::Tablet => mem_for_ff.sidebar_collapsed.set(true),
                    FormFactor::Wide => mem_for_ff.sidebar_collapsed.set(false),
                    FormFactor::Phone => {}
                },
                Some(_) => {}
            }
            now
        });
    }
```

- [ ] **Step 2: Bind a `FormFactorState` handle for the shell and add the `class:ff-tablet` binding**

At `app.rs:194`, alongside `let mem_for_shell = expect_context::<MemoryState>();`, add:

```rust
    let mem_for_shell = expect_context::<MemoryState>();
    let ff_for_shell = expect_context::<FormFactorState>();
```

Then on the shell root `<div>` (currently lines 198–201), add the `class:ff-tablet` line so it reads:

```rust
        <div
            class="aleph-shell flex h-screen text-text-primary font-sans selection:bg-primary/30"
            class:sidebar-collapsed=move || mem_for_shell.sidebar_collapsed.get()
            class:ff-tablet=move || ff_for_shell.form_factor.get() == FormFactor::Tablet
        >
```

- [ ] **Step 3: Add the `.ff-tablet` overlay CSS**

In `interfaces/webchat/styles/tailwind.css`, immediately after the existing collapsible-sidebar block (after line 1881, the `}` closing `.aleph-shell.sidebar-collapsed .aleph-sidebar { … }`), add:

```css
/* Tablet: the sidebar is a slide-over overlay, not an in-flow column.
   Revealing it floats over content (absolute) instead of pushing the 256px
   column back into flow and re-cramping the main pane. `.aleph-shell` is
   already a positioning context. Width is kept at 16rem in both states so
   collapse/reveal is a clean horizontal slide, not a width grow. */
.aleph-shell.ff-tablet .aleph-sidebar {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: 60;
    width: 16rem;
}
.aleph-shell.ff-tablet.sidebar-collapsed .aleph-sidebar {
    width: 16rem;
    transform: translateX(-100%);
    overflow: hidden;
}
```

- [ ] **Step 4: Build & verify compilation**

Run from repo root:
```bash
just wasm
```
Expected: ends successfully (WASM compiled, `dist/` regenerated). If the Leptos `Effect::new(move |prev: Option<FormFactor>| -> FormFactor { … })` signature or any borrow fails to compile, fix until `just wasm` succeeds. (Editor/SourceKit-style diagnostics are not authoritative — the `just wasm` exit status is.)

- [ ] **Step 5: Commit (source + regenerated dist)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/app.rs interfaces/webchat/styles/tailwind.css \
        interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm interfaces/webchat/dist/tailwind.css
git status --short            # verify ONLY those paths are staged
git commit -m "panel: Tablet sidebar as default-collapsed slide-over overlay"
```

---

### Task 2: Provider-settings vertical stack below 720px

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/generation_providers/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/embedding_providers/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/reranking_providers/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/acp_harnesses/mod.rs`
- Modify: `interfaces/webchat/styles/tailwind.css` (add one `@media (max-width: 720px)` block)
- Optional (polish): `interfaces/webchat/src/platform/wide/views/settings/providers/mod.rs` + the search-settings view, if their skeleton matches
- Regenerated, commit: `dist/aleph_panel.js`, `dist/aleph_panel_bg.wasm`, `dist/tailwind.css`
- Do NOT touch: any view's selection signals / Rust logic; `app.rs`; `viewport.rs`

**Interfaces:**
- Consumes: nothing from Task 1 (independent).
- Produces: nothing other tasks rely on.

**Context for the implementer:** All 4 views share an identical master-detail skeleton — a container `<div class="flex h-full aleph-content-top">` wrapping a left `<div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">` and a right `<div class="w-7/12 min-w-[320px] bg-surface">`. The `min-w-[400px]` + `min-w-[320px]` = 720px floor is what overflows below 720px. The fix adds three marker classes (`aleph-md`, `aleph-md-list`, `aleph-md-detail`) that have NO effect ≥720px (they only carry behavior inside the `@media` block), so the desktop layout is byte-for-byte unchanged. The marker classes are added ALONGSIDE the existing Tailwind utilities — do not remove the `w-5/12` / `min-w-[400px]` / etc.

Known anchor lines (verify before editing — concurrent commits may shift them): generation `:159/:161/:339`, embedding `:81/:83/:277`, reranking `:112/:114/right`, acp `:100/:102/right`. For reranking and acp, locate the right pane with `grep -n 'w-7/12' <file>`.

- [ ] **Step 1: Add the `@media (max-width: 720px)` stack rule to `tailwind.css`**

Append to `interfaces/webchat/styles/tailwind.css` (end of file is fine; or near the other settings rules):

```css
/* Tablet narrow width: the settings master-detail (provider views) stacks
   vertically — list on top, detail below, both full-width, page scrolls —
   so the 720px side-by-side floor (min-w-[400px] + min-w-[320px]) can't
   overflow. Above 720px the side-by-side layout is unchanged. Child
   combinators give (0,2,0) specificity so width/min-width beat the Tailwind
   `w-5/12` / `min-w-[400px]` utilities without `!important`. */
@media (max-width: 720px) {
  .aleph-md { flex-direction: column; overflow-y: auto; }
  .aleph-md > .aleph-md-list,
  .aleph-md > .aleph-md-detail { width: 100%; min-width: 0; }
  .aleph-md > .aleph-md-list { border-right: 0; }
}
```

- [ ] **Step 2: Add marker classes in `generation_providers/mod.rs`**

- Container (`:159`): `class="flex h-full aleph-content-top"` → `class="flex h-full aleph-content-top aleph-md"`
- Left pane (`:161`): `class="flex flex-col w-5/12 min-w-[400px] border-r border-border"` → append ` aleph-md-list`
- Right pane (`:339`): `class="w-7/12 min-w-[320px] bg-surface"` → append ` aleph-md-detail`

- [ ] **Step 3: Add the same three marker classes in `embedding_providers/mod.rs`**

- Container (`:81`): append ` aleph-md` to `flex h-full aleph-content-top`
- Left pane (`:83`): append ` aleph-md-list`
- Right pane (`:277`): append ` aleph-md-detail`

- [ ] **Step 4: Add the same three marker classes in `reranking_providers/mod.rs`**

- Container (`:112`): append ` aleph-md`
- Left pane (`:114`): append ` aleph-md-list`
- Right pane (find via `grep -n 'w-7/12' reranking_providers/mod.rs`): append ` aleph-md-detail`

- [ ] **Step 5: Add the same three marker classes in `acp_harnesses/mod.rs`**

- Container (`:100`): append ` aleph-md`
- Left pane (`:102`): append ` aleph-md-list`
- Right pane (find via `grep -n 'w-7/12' acp_harnesses/mod.rs`): append ` aleph-md-detail`

- [ ] **Step 6: (Polish, optional) extend to the 2 proportional views**

`providers/mod.rs` shares the exact skeleton with `min-w-0` (container `:80`, left `:82`, right `:126`) — add the same three marker classes (its stack is free and keeps narrow-width behavior uniform). For the search-settings view, locate it (`grep -rn 'flex h-full aleph-content-top' src/platform/wide/views/settings/`); if it has the same skeleton, add the markers; if it differs, leave it and note that in the report. This step is non-blocking — skip rather than do bespoke work.

- [ ] **Step 7: Build & verify compilation**

```bash
just wasm
```
Expected: succeeds, `dist/` regenerated. (Marker-class string edits cannot break Rust compilation, but `just wasm` confirms the build and regenerates the CSS containing the new `@media` rule.)

- [ ] **Step 8: Commit (source + regenerated dist)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/settings/ interfaces/webchat/styles/tailwind.css \
        interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm interfaces/webchat/dist/tailwind.css
git status --short            # verify only the settings views + tailwind.css + dist are staged
git commit -m "panel: Tablet provider-settings vertical stack below 720px"
```

---

## Operator / QA Verification Gate (NOT subagent checkboxes)

Runtime verifications the user performs — the spec's §6 L1/L2/L3 gate. A subagent cannot eyeball a resized browser or drive Stage Manager.

**L1 — Browser (cheapest).** Serve the built panel; resize across `320 / 680 / 760 / 900 / 1100 px`. For each width walk every mode and confirm reflow with no hidden primary control. **Binding checks:** (a) at 680px the provider-settings master-detail is **stacked** (list on top / detail below, both full-width, page scrolls) and Save is on-screen; (b) at 760px the provider settings is **side-by-side** and revealing the sidebar **overlays** content (doesn't re-cramp it); (c) the stack ↔ side-by-side flip is exactly at 720px; (d) at 1100px desktop is unchanged, including the persisted collapse preference.

**L2 — iPad Simulator.** Split View (1/2, 1/3), Slide Over, Stage Manager resize → as the divider crosses 720px the provider settings flips between stacked and side-by-side live, the sidebar stays a collapsed overlay, no crash. Use `iPad Pro 11-inch (M5), OS=26.5` (27.0 `simctl` hangs).

**L3 — Real device (TestFlight).** Folds into the owed iPad real-device QA, via `just ios-testflight`.

**Conditional follow-ups (only if L1/L2/L3 surfaces a real problem):**
- If revealing the overlay sidebar mis-layers (z-index) or the slide animates a width-grow instead of a slide, tune the Task 1 `.ff-tablet` rules (z-index / width) — bounded to those CSS rules.
- If the stacked empty-detail placeholder reads as broken (not merely empty), consider hiding the detail pane when no item is selected — but that is per-view Rust state and was explicitly traded away in D3; defer to a follow-up rather than expand #2.

---

## Plan Self-Review

**1. Spec coverage:**
- §2 D1 (no parallel layer) / no-routing-change → Global Constraints.
- §2 D2 + §4.1 (sidebar overlay, default-collapse, no Wide regression) → Task 1 Steps 1–3 + the prev-value Effect's first-run skip.
- §2 D3 + §4.2 (provider vertical stack, CSS-only, 4 floored + 2 polish) → Task 2 Steps 1–6.
- §4.3 (unchanged views) → not touched (Global Constraints + no tasks for them).
- §6 (L1/L2/L3 + 720 flip + overlay binding checks) → Operator gate.
- §7 open questions → resolved inline (Effect prev-value wiring; marker classes added alongside utilities so ≥720px is byte-identical; providers confirmed shares skeleton, search located by grep).
- §8 success criteria → Task 1 + Task 2 + Operator gate.

No spec requirement is without a home.

**2. Placeholder scan:** No TBD/TODO. Every code step has the exact edit. The only deliberately-open items are the anchor line numbers (flagged "verify before editing — concurrent commits may shift them") and the optional Step 6 (explicitly non-blocking).

**3. Type consistency:** The marker classes `aleph-md` / `aleph-md-list` / `aleph-md-detail` are used identically in the CSS (Task 2 Step 1) and every view edit (Steps 2–6). The Effect returns `FormFactor` matching its `prev_band: Option<FormFactor>` parameter. `class:ff-tablet` (Task 1) matches the CSS selector `.aleph-shell.ff-tablet` (Task 1 Step 3). The `sidebar_collapsed` / `form_factor` signal names match `memory.rs` / `viewport.rs`.

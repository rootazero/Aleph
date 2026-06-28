# Tablet-Specific Rendering (FormFactor::Tablet) — Design Spec

**Date:** 2026-06-28
**Status:** Approved (ready for plan)
**Slice:** #2 of the three deferred iPad follow-ups (#1 multitasking ✅ merged, #2 Tablet-specific rendering, #3 touch ergonomics). Each is its own spec → plan → implementation cycle.
**Predecessor:** `2026-06-28-ipad-multitasking-design.md` (the #1 slice that enabled Split View / Stage Manager, producing the dynamic 640–1023px widths this slice now renders for).

---

## 1. Goal

Give the **640–1023px Tablet band** a usable layout. Today `FormFactor::Tablet` is dead code in every consumer — all 12 form-factor sites in `app.rs` branch only on `== FormFactor::Phone`, so Tablet renders the full Wide desktop layout, just cramped. This slice adapts the Wide views **in place** at the two points where Wide genuinely breaks at Tablet width; it does **not** build a parallel platform layer.

One sentence: *at Tablet width, stop the two layout-consuming secondary columns (the nav sidebar and the provider-settings detail pane) from squeezing content — the sidebar becomes a slide-over overlay, the provider detail stacks vertically — so primary content always gets full width.*

---

## 2. Locked Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| D1 | Architecture | **Targeted adaptation of the Wide views (定点适配 Wide)** | Reuse all Wide views; branch to Tablet behavior only where Wide breaks. The empty `platform/tablet/` placeholder is **not** built out — a parallel layer (like `platform/phone/`) would duplicate view logic and add a third layout to maintain (R3 / P6 / YAGNI). Rejected alternatives: a dedicated `platform/tablet/` layer; a "Wide content + Tablet shell chrome" middle path. |
| D2 | Sidebar at Tablet | **Overlay-on-reveal, default-collapsed** | At Tablet width the 256px `ModeSidebar` stops consuming layout width: default-collapsed (content full width), revealed on demand as a slide-over overlay via the **existing** fixed toggle + Esc. Reuses the existing `sidebar_collapsed` signal + `.sidebar-collapsed` CSS, driven from the form factor. At Wide it is unchanged (in-flow column, honors the persisted preference). |
| D3 | Provider master-detail at narrow width | **Vertical stack below ~720px width (CSS-only)** | Where the side-by-side master-detail can't fit even with the sidebar hidden, the two columns flip to a single stacked column — list on top, detail below, each full-width, page scrolls; Save always reachable. **CSS-only:** the 4 floored views share an identical container skeleton, so one shared class-swap + a `@media (max-width: 720px)` flips `flex` → `flex-col` and drops the `w-`/`min-w` floors — **no Rust logic/state changes** (only mechanical marker-class string edits). Chosen over the drawer (needs per-view Rust wiring for slide-on-selection + a back affordance, and the 4 views' models differ) and over drop-min-w (cramped side-by-side). At ≥720px the side-by-side is unchanged. |

---

## 3. Architecture & Approach

**Unifying principle:** at Tablet width, the two layout-consuming secondary columns stop squeezing primary content — the left `ModeSidebar` becomes a slide-over **overlay**, and the provider-settings detail column **stacks vertically** below the list. Both free the primary content pane to render at full width; the mechanism differs because the sidebar is shell chrome (a signal + CSS) while the provider columns share a uniform skeleton amenable to a pure-CSS stack.

This stays inside R1/R2/R4/R6: all changes live in the WASM panel (`interfaces/webchat/`) and its CSS. No native shell change, no Rust core change, no new platform layer.

**Crucially, the 12 form-factor routing branches in `app.rs` stay Phone-vs-Wide.** Tablet renders the Wide content views; the Tablet *adaptation* is carried by (a) a form-factor-driven shell-level class + an Effect that re-presents the sidebar as a default-collapsed overlay, and (b) a `@media` width breakpoint inside the provider-settings CSS. We do **not** add new `FormFactor::Tablet` content-routing consumers in `app.rs`.

**Band thresholds are unchanged:** `< 640 → Phone`, `640–1023 → Tablet`, `≥ 1024 → Wide`, and `Tablet` continues to render the same content components as `Wide`. The `~720px` stack breakpoint is a *local* responsive rule inside the settings CSS, not a global band-threshold change. (At Tablet the sidebar is collapsed, so viewport width ≈ content width and a plain `@media (max-width: 720px)` is a valid trigger — no container-query plugin is needed; the build has none today.)

---

## 4. Panel Changes

### 4.1 Sidebar overlay at Tablet (D2) — benefits every mode

The existing mechanism (read during planning):
- `MemoryState.sidebar_collapsed: RwSignal<bool>` (initial from `localStorage["aleph.sidebar.collapsed"]`, persisted via an Effect) drives a shell-root `.sidebar-collapsed` class (`app.rs:200`).
- CSS `.aleph-shell.sidebar-collapsed .aleph-sidebar { transform: translateX(-100%); width: 0; overflow: hidden; }` (`tailwind.css:1877`) slides the sidebar fully off-screen, reclaiming all 256px.
- A fixed top-left `.aleph-sidebar-toggle` (`app.rs:225`) + Esc (`app.rs:156`) flip `sidebar_collapsed` to reveal it.
- `FormFactorState.form_factor: RwSignal<FormFactor>` is provided in `AppContent` (`app.rs:101`).

The Tablet adaptation:
1. **Default-collapsed at Tablet.** An Effect in `AppContent` (where both `MemoryState` and `FormFactorState` are in scope) drives `sidebar_collapsed` from the form factor: collapse when the band is/【becomes】 Tablet; restore when it becomes Wide; do nothing on Phone (phone uses its own shell). The Effect must **not** clobber the localStorage-restored Wide preference on the initial mount run (track the previous band; skip the Wide branch on first run so a Wide boot keeps the persisted preference, while a Tablet boot still collapses).
2. **Reveal presents as an overlay.** A new form-factor-driven shell class (`.aleph-shell.ff-tablet`, bound from `form_factor.get() == FormFactor::Tablet`) makes the *revealed* sidebar an absolutely-positioned overlay sliding over content, instead of animating the 256px column back into flow and re-cramping content. A push-on-reveal (no `.ff-tablet` overlay rule) is an acceptable fallback — it is a transient deliberate action — but overlay is the target.
3. **Wide unchanged.** At ≥1024px the sidebar is the in-flow column it is today and honors the persisted `sidebar_collapsed` preference.

**Accepted minor behavior** (resolves spec open-question on persistence): the persist Effect lives in `MemoryState::new()` and has no form-factor context, so a collapse forced at Tablet is written to localStorage. The mount Effect skips its Wide branch on first run, so a later Wide reload still honors that persisted value — i.e. having been at Tablet can leave a fresh Wide session collapsed until one toggle/Esc reopens it. This is a benign, one-action-to-fix edge; guarding the persist write by form factor is **out of scope** (would require threading `FormFactorState` into `MemoryState::new()`).

### 4.2 Provider-settings vertical stack (D3) — the one genuinely-broken view family

Confirmed by grep: **4 settings views carry a hard `min-w-[400px]` + `min-w-[320px]` = 720px floor**, and all four share an **identical container skeleton**:

```
<div class="flex h-full aleph-content-top">                        ← container
  <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border"> … list … </div>
  <div class="w-7/12 min-w-[320px] bg-surface"> … detail / add form … </div>
</div>
```

- `platform/wide/views/settings/generation_providers/mod.rs` (container `:159`, left `:161`, right `:339`)
- `platform/wide/views/settings/embedding_providers/mod.rs` (`:81`, `:83`, `:277`)
- `platform/wide/views/settings/reranking_providers/mod.rs` (`:112`, `:114`, right pane below)
- `platform/wide/views/settings/acp_harnesses/mod.rs` (`:100`, `:102`, right pane below)

Two sibling master-detail views (`settings/providers/mod.rs`, `settings/search/mod.rs`) already use `min-w-0` (columns shrink freely, no overflow) but get cramped at the same narrow widths.

**The Tablet adaptation (CSS-only):** add a semantic class to the shared skeleton in each of the 4 views — e.g. container → `aleph-md`, left → `aleph-md-list`, right → `aleph-md-detail` (keeping the existing Tailwind utilities alongside for the ≥720px layout, OR moving the column widths into the CSS class) — then define in `tailwind.css`:

```css
@media (max-width: 720px) {
  .aleph-md { flex-direction: column; overflow-y: auto; }
  .aleph-md-list,
  .aleph-md-detail { width: 100%; min-width: 0; }   /* defeat w-5/12 / w-7/12 / min-w floors */
  .aleph-md-list { border-right: 0; }               /* the border-r is now a horizontal seam */
}
```

Result below 720px: list on top full-width, detail below full-width, the page scrolls vertically; Save and all detail controls are on-screen. At ≥720px the existing side-by-side is byte-for-byte unchanged. **No Rust logic/state changes** — only the mechanical marker-class string edits in each view's `class="…"` — robust to the 4 views' differing selection models (the stack never touches their state).

**Scope — required vs. polish:**
- **Required** (functional breakage): the **4 floored views**. They overflow and get the stack class.
- **Polish** (consistency): the **2 proportional views** (`providers`, `search`). They don't overflow, but adding the same class is nearly free and makes narrow-width behavior uniform. **Include them** unless their skeleton differs enough to need bespoke work, in which case defer.

**Known caveat:** when no provider is selected, the right pane shows an empty/placeholder detail; stacked, that empty placeholder appears below the list. Acceptable for this slice (the drawer would have hidden it, at the cost of per-view Rust wiring — explicitly traded away in D3).

### 4.3 Explicitly unchanged at Tablet

Verified during exploration to shrink/scroll gracefully — **no per-view work**, they only benefit from the reclaimed sidebar width:
- Memory hub (sidebar + canvas, both `flex-1 min-w-0`); the galaxy canvas is WebGL and already full-bleed.
- Teams kanban / plan DAG / tasks (horizontal scroll by design).
- Agents (`max-w-6xl mx-auto`), Dashboard (`max-w-5xl mx-auto`) — centered, full-width-capped.
- Chat (relative `max-w-[80%]` bubbles).
- Drawers/overlays already carrying `max-w-[Nvw]` viewport caps.

---

## 5. Out of Scope

- **Building `platform/tablet/`** into a real layer (D1 rejected it). The empty placeholder file stays as-is (not our dead code to remove this slice).
- **Touch ergonomics** — hover-fallbacks, 44pt tap targets, touch gestures, coarse-pointer media queries are **#3** (its own spec).
- **A slide-over drawer for provider settings** — traded away in D3 for the CSS-only stack.
- **Cosmetic polish** beyond the two functional fixes — typography scaling, spacing tokens, a bespoke Tablet aesthetic.
- **Band-threshold or `Tablet == Wide` content-routing change** in `app.rs` / `viewport.rs`.
- **Multi-window** (out of scope for #1 too).

---

## 6. Verification Gate (three tiers, cheapest first)

Panel reflow is pure web, so most verification needs no device.

### L1 — Browser (cheapest; no device)
Serve the built panel; resize across `320 / 680 / 760 / 900 / 1100 px`. For each width walk **every mode** (Chat / Memory / Agents / Teams / Dashboard / Settings / Extensions) and confirm reflow with no hidden primary control.

| Width | Band | Sidebar | Provider settings | Expected |
|-------|------|---------|-------------------|----------|
| 320px | Phone | n/a (PhoneSettings) | n/a (PhoneProviders) | phone drill-down |
| 680px | Tablet | collapsed overlay | **stacked** (<720) | list on top / detail below, both full-width, page scrolls; Save reachable |
| 760px | Tablet | collapsed overlay | **side-by-side** (≥720) | comfortable two-column; sidebar reveal slides over content |
| 900px | Tablet | collapsed overlay | side-by-side | same |
| 1100px | Wide | in-flow column (persisted pref) | side-by-side | regression — desktop unchanged |

**Binding checks:** (a) at 680px the provider-settings Save is on-screen with the panels stacked; (b) revealing the sidebar at 760px overlays content rather than re-cramping it; (c) at 1100px desktop is byte-for-byte the prior behavior including the persisted collapse preference; (d) the stack ↔ side-by-side flip happens exactly at 720px.

### L2 — iPad Simulator
Split View (1/2, 1/3), Slide Over, Stage Manager resize: as the divider drags across the 720 boundary, the provider settings flips between stacked and side-by-side live, the sidebar stays a collapsed overlay, nothing crashes. Use `iPad Pro 11-inch (M5), OS=26.5` (27.0 `simctl` hangs, per #1).

### L3 — Real device (TestFlight)
Folds into the owed iPad real-device QA. Same checks as L2 plus the feel of the sidebar overlay by touch. Reached via `just ios-testflight`.

### Unit
- `viewport.rs` band classification is already tested at 640/1024 — unchanged.
- The provider stack is a pure-CSS `@media` rule (no JS logic) — its gate is the L1 visual check, as in prior slices.
- If the sidebar default-collapse Effect warrants a guard test, add a small unit around the band-transition logic; otherwise the L1/L2 gate covers it.

---

## 7. Open Questions (resolved during planning, not blocking)

1. **Sidebar default-collapse wiring (§4.1):** the transition-tracking Effect that collapses on Tablet, restores on Wide, and skips the Wide branch on first run (so a Wide boot honors localStorage while a Tablet boot collapses). The accepted persistence edge is documented in §4.1.
2. **Semantic-class shape for the stack (§4.2):** whether to keep the existing Tailwind width utilities and add marker classes for the `@media` override, or move the column widths entirely into the CSS classes. The plan picks whichever keeps the ≥720px layout byte-for-byte identical.
3. **The 2 proportional views (§4.2):** include `providers` + `search` in the shared stack class for uniformity (near-free) unless their skeleton differs enough to need bespoke work. Resolved by reading those two modules during planning.

None blocks writing the plan.

---

## 8. Success Criteria

- At Tablet width (640–1023px), the nav sidebar does not consume layout width: content is full-width by default; the sidebar reveals as a slide-over overlay and dismisses without re-cramping content.
- The 4 floored provider-settings views (and the 2 proportional ones if covered) are fully usable below 720px: the master-detail stacks vertically, both panes full-width, the page scrolls, Save always reachable; at ≥720px they keep the side-by-side layout.
- Every other mode reflows gracefully at Tablet width with no hidden primary control (L1).
- iPad multitasking (Split View / Slide Over / Stage Manager) reflows live across the 720 boundary with no crash (L2/L3).
- **Wide (≥1024px) is unchanged**, including the persisted sidebar-collapse preference; band thresholds and the `Tablet == Wide` content-routing mapping are untouched; no native/Rust-core change; no `platform/tablet/` layer built; the provider stack is CSS-only (no Rust change to the 4 views beyond adding marker classes).

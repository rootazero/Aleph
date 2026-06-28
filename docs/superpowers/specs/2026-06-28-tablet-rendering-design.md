# Tablet-Specific Rendering (FormFactor::Tablet) — Design Spec

**Date:** 2026-06-28
**Status:** Approved (ready for plan)
**Slice:** #2 of the three deferred iPad follow-ups (#1 multitasking ✅ merged, #2 Tablet-specific rendering, #3 touch ergonomics). Each is its own spec → plan → implementation cycle.
**Predecessor:** `2026-06-28-ipad-multitasking-design.md` (the #1 slice that enabled Split View / Stage Manager, producing the dynamic 640–1023px widths this slice now renders for).

---

## 1. Goal

Give the **640–1023px Tablet band** a usable layout. Today `FormFactor::Tablet` is dead code in every consumer — all 12 form-factor sites in `app.rs` branch only on `== FormFactor::Phone`, so Tablet renders the full Wide desktop layout, just cramped. This slice adapts the Wide views **in place** at the two points where Wide genuinely breaks at Tablet width; it does **not** build a parallel platform layer.

One sentence: *at Tablet width, turn the two layout-consuming secondary panels (the nav sidebar and the provider-settings detail pane) into slide-over overlays so primary content always gets full width.*

---

## 2. Locked Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| D1 | Architecture | **Targeted adaptation of the Wide views (定点适配 Wide)** | Reuse all Wide views; branch to Tablet behavior only where Wide breaks. The empty `platform/tablet/` placeholder is **not** built out — a parallel layer (like `platform/phone/`) would duplicate view logic and add a third layout to maintain (R3 / P6 / YAGNI). Rejected alternatives: a dedicated `platform/tablet/` layer; a "Wide content + Tablet shell chrome" middle path. |
| D2 | Sidebar at Tablet | **Overlay-on-reveal, default-collapsed** | At Tablet width the 256px `ModeSidebar` stops consuming layout width: default-collapsed (content full width), revealed on demand as a slide-over overlay via the **existing** fixed toggle + Esc. Reuses the existing `sidebar_collapsed` signal + `.sidebar-collapsed` CSS. At Wide it is unchanged (in-flow column, honors the persisted preference). |
| D3 | Provider master-detail at narrow width | **Detail drawer (slide-over) below ~720px effective width** | Where the side-by-side master-detail can't fit even with the sidebar hidden, the list takes full width and the detail slides in from the right with a back affordance — Save always reachable. At ≥720px the existing side-by-side is unchanged. Trigger is the **container's available width (~720px)**, NOT the whole Tablet band, so 760–1023px keeps the comfortable side-by-side. |

---

## 3. Architecture & Approach

**Unifying principle:** at Tablet width, secondary panels become **slide-over overlays** instead of layout-consuming columns, so the primary content pane always renders at full width. The two secondary panels that consume width today are the left `ModeSidebar` and the provider-settings detail column; both become overlays at narrow widths.

This stays inside R1/R2/R4/R6: all changes live in the WASM panel (`interfaces/webchat/`) and its CSS. No native shell change, no Rust core change, no new platform layer.

**Crucially, the 12 form-factor routing branches in `app.rs` stay Phone-vs-Wide.** Tablet renders the Wide content views; the Tablet *adaptation* is carried by (a) a form-factor-driven shell-level class that re-presents the sidebar as an overlay, and (b) a width-driven responsive switch inside the provider-settings views. We do **not** add new `FormFactor::Tablet` content-routing consumers in `app.rs`.

**Band thresholds are unchanged:** `< 640 → Phone`, `640–1023 → Tablet`, `≥ 1024 → Wide`, and `Tablet` continues to render the same content components as `Wide`. The `~720px` provider drawer trigger is a *local* responsive breakpoint inside the settings views, not a global band-threshold change.

---

## 4. Panel Changes

### 4.1 Sidebar overlay at Tablet (D2) — benefits every mode

The existing mechanism (read during brainstorming):
- `MemoryState.sidebar_collapsed: RwSignal<bool>` (persisted to localStorage) drives a shell-root `.sidebar-collapsed` class (`app.rs:200`).
- CSS `.aleph-shell.sidebar-collapsed .aleph-sidebar { transform: translateX(-100%); width: 0; overflow: hidden; }` (`tailwind.css:1877`) slides the sidebar fully off-screen, reclaiming all 256px.
- A fixed top-left `.aleph-sidebar-toggle` (`app.rs:225`) + Esc (`app.rs:156`) reveal it.

The Tablet adaptation:
1. **Default-collapsed at Tablet.** When the form factor is Tablet, the sidebar starts collapsed so content gets full width.
2. **Reveal presents as an overlay, not an in-flow push.** A new form-factor-driven shell class (e.g. `.aleph-shell.ff-tablet`) makes the revealed sidebar an absolutely-positioned overlay sliding over the content, instead of animating the 256px column back into flow and re-cramping content. (Even a push-on-reveal is acceptable as a fallback — it is a transient deliberate action — but overlay is the target.)
3. **Wide unchanged.** At ≥1024px the sidebar is the in-flow column it is today and honors the persisted `sidebar_collapsed` preference.

**Key technical question for the plan** (do NOT regress Wide's persisted-preference behavior): how to default-collapse at Tablet without an Effect that clobbers the localStorage-restored Wide preference on mount. Candidate approaches, plan picks one:
- (a) An Effect keyed on the form-factor `Memo` that force-collapses only on the *transition into* Tablet and restores the prior state on the *transition into* Wide (track previous band; do nothing while staying in-band so the manual toggle still works).
- (b) A CSS-only overlay: the `.ff-tablet` class makes the sidebar an overlay that is hidden by default regardless of the signal, with the toggle/Esc still flipping `sidebar_collapsed` to reveal it — leaving the Wide signal path untouched.

### 4.2 Provider-settings master-detail drawer (D3) — the one genuinely-broken view family

Confirmed by grep: **4 settings views carry a hard `min-w-[400px]` + `min-w-[320px]` = 720px floor**:
- `platform/wide/views/settings/generation_providers/mod.rs`
- `platform/wide/views/settings/embedding_providers/mod.rs`
- `platform/wide/views/settings/reranking_providers/mod.rs`
- `platform/wide/views/settings/acp_harnesses/mod.rs`

Two sibling master-detail views (`settings/providers/mod.rs`, `settings/search/mod.rs`) already use `min-w-0` (columns shrink freely, no overflow), but get cramped at the same narrow widths.

The Tablet adaptation: below **~720px** of available width for the master-detail container, switch from side-by-side to **list-full-width + detail-as-slide-over-overlay**:
- List renders full width.
- Selecting a row slides the detail in from the right (reusing the existing selected-item signal that already drives which detail shows).
- A back affordance deselects / dismisses the overlay, returning to the full-width list.
- Save (and all detail controls) are always on-screen.
- At ≥720px the existing side-by-side master-detail is unchanged.

**Scope of the fix — required vs. polish:**
- **Required** (functional breakage): the **4 floored views**. These overflow and must get the drawer (or, at minimum, lose the floor).
- **Polish** (consistency): the **2 proportional views** (`providers`, `search`). They don't overflow, but uniform drawer behavior is nicer. Include them **iff** a shared wrapper makes it nearly free; otherwise defer to #2-polish or a later slice.

**Implementation preference for the plan:** investigate extracting a **shared responsive master-detail wrapper** that the floored views (and ideally all six) consume, so the drawer behavior is defined once rather than copied four-to-six times. If the views are too bespoke to share cheaply, apply the drawer per-view to the 4 floored ones. The `~720px` trigger should prefer a **container query** (`@container`) on the settings content area over a JS width signal, so the switch is robust to the sidebar being revealed; a width signal from `FormFactorState` is the fallback if container queries don't fit the existing CSS pipeline.

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
- **Cosmetic polish** beyond the two functional fixes — typography scaling, spacing tokens, a bespoke Tablet aesthetic.
- **Band-threshold or `Tablet == Wide` content-routing change** in `app.rs` / `viewport.rs`.
- **Multi-window** (that was out of scope for #1 too).

---

## 6. Verification Gate (three tiers, cheapest first)

Panel reflow is pure web, so most verification needs no device.

### L1 — Browser (cheapest; no device)
Serve the built panel; resize across `320 / 680 / 760 / 900 / 1100 px`. For each width walk **every mode** (Chat / Memory / Agents / Teams / Dashboard / Settings / Extensions) and confirm reflow with no hidden primary control.

| Width | Band | Sidebar | Provider settings | Expected |
|-------|------|---------|-------------------|----------|
| 320px | Phone | n/a (PhoneSettings) | n/a (PhoneProviders) | phone drill-down |
| 680px | Tablet | collapsed overlay | **drawer** (<720) | list full-width; tap row → detail slides over; Save reachable |
| 760px | Tablet | collapsed overlay | **side-by-side** (≥720) | comfortable two-column; sidebar reveal slides over content |
| 900px | Tablet | collapsed overlay | side-by-side | same |
| 1100px | Wide | in-flow column (persisted pref) | side-by-side | regression — desktop unchanged |

**Binding checks:** (a) at 680px the provider-settings Save is reachable via the drawer; (b) revealing the sidebar at 760px overlays content rather than re-cramping it; (c) at 1100px desktop is byte-for-byte the prior behavior including the persisted collapse preference.

### L2 — iPad Simulator
Split View (1/2, 1/3), Slide Over, Stage Manager resize: as the divider drags across the 720 boundary, the provider settings flips between drawer and side-by-side live, the sidebar stays an overlay, nothing crashes. Use `iPad Pro 11-inch (M5), OS=26.5` (27.0 `simctl` hangs, per #1).

### L3 — Real device (TestFlight)
Folds into the owed iPad real-device QA. Same checks as L2 plus the feel of the drawer slide and sidebar overlay by touch. Reached via `just ios-testflight`.

### Unit
- `viewport.rs` band classification is already tested at 640/1024 — unchanged.
- If the provider drawer uses a JS width signal/classifier (fallback path), add a threshold test around ~720px. If it's a pure container-query CSS switch, the test is the L1 visual gate (no unit-testable logic), as in prior slices.

---

## 7. Open Questions (resolved during planning, not blocking)

1. **Sidebar default-collapse wiring (§4.1):** Effect-on-band-transition (a) vs CSS-only overlay (b). The plan picks the one that does not regress Wide's persisted preference; both are viable.
2. **Shared wrapper vs per-view, and the 2 proportional views (§4.2):** whether a single responsive master-detail wrapper can be cheaply extracted (then apply to all 6 for consistency) or the drawer is applied per-view to just the 4 floored ones. Resolved by reading the 4–6 view modules during planning.
3. **Drawer trigger mechanism (§4.2):** container query (`@container`, preferred) vs `FormFactorState` width signal (fallback). Resolved by checking whether the Tailwind/CSS pipeline supports container queries here.

None blocks writing the plan, which encodes the chosen branch with its verification.

---

## 8. Success Criteria

- At Tablet width (640–1023px), the nav sidebar does not consume layout width: content is full-width by default; the sidebar reveals as a slide-over overlay and dismisses without re-cramping content.
- The 4 floored provider-settings views (and the 2 proportional ones if covered) are fully usable below 720px: list full-width, detail slides over, Save always reachable; at ≥720px they keep the side-by-side layout.
- Every other mode reflows gracefully at Tablet width with no hidden primary control (L1).
- iPad multitasking (Split View / Slide Over / Stage Manager) reflows live across the 720 boundary with no crash (L2/L3).
- **Wide (≥1024px) is unchanged**, including the persisted sidebar-collapse preference; band thresholds and the `Tablet == Wide` content-routing mapping are untouched; no native/Rust change; no `platform/tablet/` layer built.

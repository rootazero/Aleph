# Touch Ergonomics — Hover-Reveal Foundation — Design Spec

**Date:** 2026-06-28
**Status:** Approved (ready for plan)
**Slice:** #3 of the three deferred iPad follow-ups (#1 multitasking ✅ merged, #2 Tablet-specific rendering — spec written, #3 touch ergonomics). Each is its own spec → plan → implementation cycle.
**Predecessor context:** #1 enabled iPad multitasking; #2 adapts the Tablet layout. This slice makes the panel's hover-only **action** affordances reachable by touch.

---

## 1. Goal

On coarse-pointer / touch devices (iPad, touchscreen), make **hover-only action affordances always-visible** so they can be reached by tap. Today the panel has **zero coarse-pointer adaptation** — no `@media (hover: none)` / `@media (pointer: coarse)` anywhere, no touch handling (only the WebGL canvas sets `touch-action`). Actions hidden behind `opacity-0 group-hover:opacity-100` (session delete/edit, message copy/delete, memory note actions, JSON copy, composer attachment actions) are **invisible and unreachable on touch** — the one genuine functional breakage.

This slice is **foundation + reveal only.** It does **not** resize tap targets or add gesture equivalents (deferred follow-ups, §5).

One sentence: *under `@media (hover: none)`, force the standard hover-reveal opacity utilities visible, then sweep the variant patterns so every action affordance shows on touch — and leave decorative hover flourishes alone.*

---

## 2. Locked Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| D1 | Scope depth | **Foundation + hover-reveal only** | Breakage-first, consistent with #1/#2 discipline. The only *functional* breakage on touch is the unreachable action. Tap-target sizing (friction, not breakage) and mouse-gesture equivalents are separable follow-ups. |
| D2 | Detection gate | **`@media (hover: none)`** (capability, not size) | Correct in iPad WKWebView (reports `hover: none`); a hybrid iPad + trackpad reports `hover: hover` and correctly keeps hover-gating (it has a precise pointer). Pure CSS — no JS, no `FormFactor` coupling, no false-positive on a narrow desktop window. Chosen over `(pointer: coarse)` (a stylus is coarse yet can hover) and over a `FormFactor`-based gate. |
| D3 | Implementation shape | **Global foundation rule + targeted variant sweep** | A global `@media (hover: none)` override of the standard `group-hover:opacity-100` / `hover:opacity-100` utilities covers the common case in one place with zero component churn; a targeted grep-driven pass catches the variants the global rule misses (named groups, partial opacity, hidden/invisible). Chosen over a pure per-component marker class (more churn) and over a pure global `!important` blanket (misses variants, over-reaches). |
| D4 | Decorative hover | **Not revealed** | `group-hover:scale-*`, `group-hover:text-*` color shifts, and similar flourishes lose **no functionality** on touch. Leaving them avoids always-on visual clutter. Only **action** affordances are revealed. |

---

## 3. Architecture & Approach

All changes live in the WASM panel's CSS (`interfaces/webchat/styles/tailwind.css`) plus a small targeted pass over the enumerated affordance components. No native change, no Rust core change, no JS/Leptos state, no `FormFactor` consumer. This honors R1/R2/R4/R6 and P6 (a capability media query is the minimal correct mechanism).

The reveal gate is `@media (hover: none)`. Within it:
- **Part 1 (global foundation):** force the standard Tailwind hover-reveal utilities visible. Tailwind compiles `group-hover:opacity-100` → `.group:hover .group-hover\:opacity-100 { opacity: 1 }`; on a no-hover pointer `.group:hover` never matches, so the affordance stays `opacity-0`. A rule under `@media (hover: none)` that sets the compiled utility classes to `opacity: 1` reveals every affordance using the standard pattern, in one place.
- **Part 2 (variant sweep):** the global rule cannot catch named-group variants (`group-hover/json:`), partial-opacity targets (`opacity-60`), or non-opacity reveals (`hidden group-hover:block`, `invisible group-hover:visible`). A grep-driven audit of the enumerated families fixes each so it reveals on touch — preferably by normalizing it onto the standard pattern (or a shared reveal class) so it is covered by Part 1, or by an explicit per-site `@media (hover: none)` rule where normalization isn't clean.

---

## 4. Panel Changes

### 4.1 Foundation rule (`styles/tailwind.css`)

Add a `@media (hover: none)` block that reveals the standard hover-opacity utilities. Indicative (exact selectors finalized in the plan against the generated CSS):

```css
@media (hover: none) {
  /* Reveal hover-gated ACTION affordances on touch — the `.group:hover`
     trigger never fires without a hovering pointer. */
  .group-hover\:opacity-100,
  .hover\:opacity-100 { opacity: 1 !important; }
}
```

Notes for the plan:
- Verify against the actual generated class names in `dist` (Tailwind escaping of `:`), and confirm these utilities are emitted.
- `focus-within:opacity-100` already reveals on keyboard focus (`messages.rs:661`) — the touch reveal is consistent with it.
- Scope the `!important` as narrowly as the cascade allows; it exists only to beat the inline `opacity-0` utility.

### 4.2 Variant sweep (enumerated affordance families)

Grep-driven audit (`hover:`, `group-hover`, `opacity-0`, `invisible`, `hidden group-hover`) across `src/`, fixing every **action** affordance the foundation rule misses. Known families from exploration:

| File:line | Affordance | Pattern / note |
|-----------|------------|----------------|
| `components/chat_sidebar.rs:1153` | session row delete/edit | `opacity-0 group-hover:opacity-100` — covered by foundation; verify |
| `components/chat_sidebar.rs:1421` | session row delete/edit (2nd) | same |
| `components/messages.rs:661` | message copy/delete | `opacity-0 group-hover:opacity-100 focus-within:opacity-100` — covered; verify |
| `.../memory/mod.rs:500` | memory note actions | `opacity-0 group-hover:opacity-100 hover:text-primary` — covered; verify |
| `.../json_viewer.rs:293` | JSON copy/expand | **variant**: `group-hover/json:opacity-60 hover:opacity-100` (named group + partial opacity) — needs explicit fix |
| `components/composer/mod.rs` | attachment actions | `hover:opacity-100` — covered; verify |

The plan resolves each: confirm coverage by the foundation rule, or normalize/patch the variant. Newly-discovered action affordances from the grep sweep are folded in; decorative-only matches (scale/color) are explicitly skipped (D4).

### 4.3 Explicitly unchanged

- Tap-target sizes (the ~24–28px icon buttons) — **not resized** this slice (D1).
- The 4 `on:mousedown` prevent-default handlers + drag interactions — **no touch equivalents** this slice (D1).
- Decorative hover flourishes (`group-hover:scale-*`, color-only `group-hover:text-*`) — **not revealed** (D4).
- All non-hover UI, all Rust/Leptos logic, all native code.

---

## 5. Out of Scope (deferred follow-ups)

- **44px tap-target sizing** under coarse pointer (#3-b) — the smallest icon buttons enlarged to ≥44px.
- **Mouse-gesture touch equivalents** (#3-c) — the 4 `on:mousedown` prevent-default sites + drag interactions.
- **Full 377-hover audit** — only **action** affordances using the reveal patterns are in scope; decorative hover is left as-is.
- **Tablet layout** — that is #2.

---

## 6. Verification Gate (three tiers, cheapest first)

The reveal is pure CSS, so emulation verifies most of it.

### L1 — Browser with `@media (hover: none)` emulation (cheapest)
In DevTools, emulate a no-hover / touch device (or force `@media (hover: none)`), then walk each affordance family (§4.2) and confirm the action is **visible without hovering**: chat-session delete/edit, message copy/delete, memory note actions, JSON copy, composer attachment actions. Also confirm in a **normal** (hover-capable) viewport that behavior is **unchanged** — affordances still appear on hover only, no always-on clutter, decorative flourishes intact.

### L2 — iPad Simulator
Open each affordance family and confirm the action is tappable without any hover gesture (it is visible from the start). Use `iPad Pro 11-inch (M5), OS=26.5` (27.0 `simctl` hangs, per #1).

### L3 — Real device (TestFlight)
Folds into the owed iPad real-device QA. Same checks as L2 by touch. Reached via `just ios-testflight`.

### Unit
None — CSS-only foundation with no JS/Leptos logic; the binding gate is the L1 visual check (as in the native-flag slices #1).

---

## 7. Open Questions (resolved during planning, not blocking)

1. **Exact generated selectors (§4.1):** confirm the Tailwind-escaped class names (`.group-hover\:opacity-100` etc.) actually emitted in the built CSS, and whether any are purged. Resolved by reading `dist`/the generated CSS during planning.
2. **Variant normalization vs explicit rule (§4.2):** for each variant (e.g. `json_viewer` named group + `opacity-60`), whether to normalize onto the standard pattern (covered by Part 1) or add an explicit `@media (hover: none)` rule. Resolved per-site while editing.
3. **Sweep completeness (§4.2):** the grep may surface additional action affordances beyond the enumerated families; each is triaged action-vs-decorative during the sweep.

None blocks writing the plan.

---

## 8. Success Criteria

- On `@media (hover: none)` (iPad WKWebView / touch emulation), every enumerated action affordance — chat-session delete/edit, message copy/delete, memory note actions, JSON copy, composer attachment actions — is **visible without hovering** and tappable.
- On a hover-capable pointer, behavior is **byte-for-byte unchanged**: affordances appear on hover only; no always-on clutter; decorative flourishes intact.
- Changes are confined to the panel CSS plus the targeted variant fixes; no tap-target resize, no gesture work, no native/Rust/JS-state change, no `FormFactor` coupling.
- iPad real-device QA confirms hidden actions are now reachable by touch (L2/L3).

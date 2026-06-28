# Touch Ergonomics — Hover-Reveal Foundation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On coarse-pointer / touch devices, make the panel's hover-only **action** affordances visible (tappable) by adding one `@media (hover: none)` CSS block that forces the standard hover-reveal opacity utilities visible. No Rust, no per-component edits, no native change.

**Architecture:** Panel CSS only (`interfaces/webchat/styles/tailwind.css`). Tailwind v4 emits `.group-hover\:opacity-100:is(:where(.group):hover *){opacity:1}` and `.hover\:opacity-100:hover{opacity:1}` — both gated on a hovering pointer, so on touch the element stays at its `opacity-0`. A single unlayered `@media (hover: none)` rule that targets the **bare** utility classes reveals every action affordance using the standard pattern. Investigation confirmed there are exactly 4 `group-hover:opacity-100` action affordances + 4 `hover:opacity-100` controls (1 hidden, 3 dimmed) and **zero** reveal variants the rule misses (no named-group opacity reveals, no partial-only opacity, no `invisible→visible` / `hidden→block` toggles) — so the spec's "variant sweep" is a confirmatory grep, not edits.

**Tech Stack:** Tailwind v4 + hand-written `styles/tailwind.css` (the hand-written rules are unlayered, so they beat the layered `.opacity-0` utility — verified on the #2 stack rule); `just wasm` build (regenerates `dist/tailwind.css`; no Rust recompile since no `.rs` changes).

## Global Constraints

Every task implicitly includes these (verbatim from `docs/superpowers/specs/2026-06-28-touch-ergonomics-design.md`):

- **D2 — detection gate is `@media (hover: none)`** (capability, not size). NOT `(pointer: coarse)`, NOT `FormFactor`. Correct in iPad WKWebView; a hybrid iPad+trackpad reports `hover: hover` and correctly keeps hover-gating.
- **D4 — decorative hover is NOT revealed.** Only the opacity-reveal **action** affordances. Do NOT touch `group-hover:scale-*` or color-only `group-hover:text-*` flourishes (the rule targets only `opacity-100`, so these are untouched by construction).
- **D1 — foundation + reveal only.** Do NOT resize tap targets (44px work is #3-b, deferred). Do NOT add touch-gesture equivalents (the 4 `on:mousedown` sites are #3-c, deferred).
- **Hover-capable pointers stay byte-for-byte unchanged** — the rule is inside `@media (hover: none)`, inert on desktop / hover-capable devices.
- **Panel CSS only** — no Rust/Leptos change, no native change, no `FormFactor` coupling.
- **NEVER commit secrets**; commit the regenerated `dist/tailwind.css` alongside the source. NEVER `git add` unrelated files (the repo has concurrent uncommitted work in `build.rs` / `justfile` / `src/harness/**` / `Info.plist` — never stage those). NEVER `git add -A`.
- Commit style `panel: <description>`, no `Co-Authored-By` trailer.

---

### Task 1: Add the `@media (hover: none)` hover-reveal foundation

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css` (append one `@media (hover: none)` block near the other hand-written media rules, e.g. just after the `@media (max-width: 720px)` block added by #2)
- Regenerated, commit: `interfaces/webchat/dist/tailwind.css`
- Do NOT touch: any `.rs` file, any `dist/aleph_panel*` (no Rust change → those won't change), anything outside `interfaces/webchat/`

**Interfaces:** none (single-task, CSS-only).

**Context for the implementer:** The reveal affordances that this rule fixes (verified by grep — do NOT edit these files, they are listed only so you understand what the rule covers):
- `src/platform/wide/views/memory/mod.rs:500` — memory note action, `opacity-0 group-hover:opacity-100`
- `src/platform/wide/views/chat/messages.rs:661` — message actions, `opacity-0 group-hover:opacity-100 focus-within:opacity-100`
- `src/components/chat_sidebar.rs:1153` and `:1421` — session row actions, `opacity-0 group-hover:opacity-100`
- `src/components/json_viewer.rs:293` — JSON copy, `opacity-0 group-hover/json:opacity-60 hover:opacity-100` (the `hover:opacity-100` is what the rule reveals)
- `src/platform/wide/views/chat/messages.rs:310`, `src/components/team_task_strip.rs:86`, `src/components/session_tabs.rs:90` — controls dimmed to `opacity-50/60` with `hover:opacity-100` (already visible on touch; the rule brightens them to full)

All are action controls — none decorative. The rule is blunt by design (targets the two utility classes), which is correct here because every occurrence is an action affordance.

- [ ] **Step 1: Add the `@media (hover: none)` block**

Append to `interfaces/webchat/styles/tailwind.css` (immediately after the `@media (max-width: 720px) { … }` provider-stack block from #2 is a good home; anywhere among the hand-written unlayered rules works):

```css
/* Touch / coarse-pointer: reveal hover-gated ACTION affordances. Tailwind
   emits `.group-hover\:opacity-100` and `.hover\:opacity-100` gated on a
   hovering pointer, so on touch the element stays at its `opacity-0`. Forcing
   the bare utility classes visible reveals every such affordance (session
   delete/edit, message copy/delete, memory note actions, JSON copy, …). The
   rule lives in `@media (hover: none)` so hover-capable pointers (desktop, an
   iPad with a trackpad) are unaffected; it targets only `opacity-100`, so
   decorative `group-hover:scale-*` / color flourishes are untouched. This
   block is unlayered, so it beats the layered `.opacity-0` utility. */
@media (hover: none) {
  .group-hover\:opacity-100,
  .hover\:opacity-100 { opacity: 1; }
}
```

- [ ] **Step 2: Build & verify the rule reaches dist and wins**

Run from repo root:
```bash
just wasm
```
Expected: succeeds (`dist/` regenerated). Since no `.rs` changed, the Rust compile is a no-op/up-to-date and only `dist/tailwind.css` changes.

Then verify the compiled rule is present:
```bash
grep -o '@media (hover:none){[^@]*opacity:1[^}]*}}' interfaces/webchat/dist/tailwind.css | head -1
# fallback if minified differently:
grep -c 'hover:none' interfaces/webchat/dist/tailwind.css        # expect >= 1
```
Expected: the `@media (hover:none)` block with `.group-hover\:opacity-100,.hover\:opacity-100{opacity:1}` (minified) appears in `dist/tailwind.css`.

- [ ] **Step 3: Confirm no uncovered reveal variants remain (the spec's "sweep", as verification)**

Run from `interfaces/webchat`:
```bash
cd /Volumes/TBU4/Workspace/Aleph/interfaces/webchat
grep -rn 'group-hover/[a-z]*:opacity-100' src/                 # named-group reveals — expect NONE
grep -rn 'group-hover:opacity-[4-9]0' src/ | grep -v 'hover:opacity-100'   # partial-only — expect NONE
grep -rn 'group-hover:visible\|group-hover:block\|group-hover:flex' src/   # visibility/display toggles — expect NONE
```
Expected: all three return nothing — confirming the single foundation rule covers every reveal affordance and no per-site variant edit is needed. If any returns a hit, STOP and report it (the spec's conditional per-site fix would then apply); given the pre-verification, none is expected.

- [ ] **Step 4: Commit (source + regenerated dist)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/styles/tailwind.css interfaces/webchat/dist/tailwind.css
git status --short          # verify ONLY those two paths are staged; never the concurrent build.rs/justfile/harness/Info.plist work
git commit -m "panel: reveal hover-gated action affordances on touch (@media hover:none)"
```
Expected: only those two paths staged and committed.

---

## Operator / QA Verification Gate (NOT subagent checkboxes)

Runtime verifications the user performs — the spec's §6 L1/L2/L3 gate.

**L1 — Browser with `@media (hover: none)` emulation (cheapest).** In DevTools, emulate a touch / no-hover device (or force `@media (hover: none)`), then confirm each action affordance is **visible without hovering**: chat-session delete/edit, message copy/delete, memory note actions, JSON copy, composer attachment actions. Then in a **normal hover-capable** viewport confirm behavior is **unchanged** — affordances still appear on hover only, no always-on clutter, decorative scale/color flourishes intact.

**L2 — iPad Simulator.** Open each affordance family; confirm the action is tappable without any hover gesture (visible from the start). Use `iPad Pro 11-inch (M5), OS=26.5` (27.0 `simctl` hangs).

**L3 — Real device (TestFlight).** Folds into the owed iPad real-device QA, via `just ios-testflight`.

**Conditional follow-up (only if a real gap surfaces):** if any affordance is still hidden on touch, it uses a reveal pattern the rule doesn't cover (a named group, a visibility/display toggle) — add a targeted `@media (hover: none)` rule for that specific class, bounded to that affordance. Pre-verification (Step 3) predicts none.

---

## Plan Self-Review

**1. Spec coverage:**
- §2 D1 (foundation + reveal only) → Task 1 (no tap-size, no gesture work).
- §2 D2 (`@media (hover: none)` gate) → Task 1 Step 1.
- §2 D3 (global foundation rule + variant sweep) → Step 1 (rule) + Step 3 (sweep = confirmatory grep; pre-verification shows zero variants, so the "targeted variant pass" is empty).
- §2 D4 (decorative not revealed) → the rule targets only `opacity-100`; Global Constraints.
- §4.1 (foundation rule, exact selectors) → Step 1 with the verified Tailwind-v4 emitted classes.
- §4.2 (variant sweep) → Step 3, resolved to zero uncovered variants.
- §6 (L1/L2/L3) → Operator gate.
- §8 success criteria → Task 1 + Operator gate.

No spec requirement is without a home.

**2. Placeholder scan:** No TBD/TODO. The exact CSS block is given; the grep checks have exact commands + expected output. Step 3's "expect NONE" is backed by the pre-implementation verification recorded in the plan's Architecture/Context.

**3. Type consistency:** The two selectors `.group-hover\:opacity-100` / `.hover\:opacity-100` match the Tailwind-v4 emitted class names confirmed in `dist/tailwind.css` (`.group-hover\:opacity-100:is(:where(.group):hover *)` and `.hover\:opacity-100:hover`). The `@media (hover: none)` query string is identical in the rule (Step 1), the dist verification (Step 2), and the Operator gate.

# iPad Multitasking (Split View / Stage Manager) — Design Spec

> **⚠️ Superseded 2026-08-24 — the committed `Info.plist` this document edits no longer exists.**
> `mobile/ios/AlephPaneliOS/Resources/Info.plist` is xcodegen *output* and is now
> gitignored beside the generated `.xcodeproj`; `project.yml`'s `info.properties`
> block is the only source, and there is nothing to restore before a commit. Every
> step below that stages that file, or that asks a regeneration to preserve its
> `${ALEPH_VERSION}` / `${ALEPH_BUILD}` placeholders, describes the world as it was
> — the current one is stated once, in `mobile/ios/README.md` and
> `mobile/ios/.gitignore`. Kept as the record of what was done: do not re-add the
> file by following it.

**Date:** 2026-06-28
**Status:** Approved (ready for plan)
**Slice:** #1 of the three deferred iPad follow-ups (#1 multitasking, #2 Tablet-specific rendering, #3 touch ergonomics). Each is its own spec → plan → implementation cycle.
**Predecessor:** `2026-06-28-ipad-shell-enablement-design.md` (the iPad shell slice that set `UIRequiresFullScreen: true`; this slice relaxes it).

---

## 1. Goal

Make the iOS Panel app a first-class **single-window multitasking participant** on iPad: it can share the screen with another app via Split View / Slide Over, and be freely resized / placed in Stage Manager. The deliverable is **native enablement + a verification gate that proves nothing breaks functionally at dynamic widths** — not a layout redesign.

One sentence: *remove the one flag that blocks multitasking, then prove the panel reflows correctly and no control becomes unreachable at the widths multitasking produces.*

---

## 2. Locked Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| D1 | Multitasking scope | **A. Participant / single-window** | Satisfies the Split View / Stage Manager core ask with a one-flag change and zero red-line risk. Multi-window (multiple Aleph scenes) is a heavier, separate feature that would duplicate connection/pairing state per scene — deferred (YAGNI). |
| D2 | #1 ↔ #2 boundary | **A. Enable + guarantee no breakage** | #1 owns: enable multitasking, verify dynamic-width reflow, fix *functional* breakage (a control pushed out of the viewport and unreachable). Cosmetic cramping of the `Tablet == Wide` layout is an **appearance** concern owned entirely by #2. The two specs do not overlap. |

---

## 3. Architecture & Approach

The chosen approach is **native enablement + verification gate**, deliberately *not* a responsive-layout rework:

- **Reflow plumbing already exists.** `interfaces/webchat/src/state/viewport.rs::FormFactorState` listens to `window.resize`; `app.rs` carries mirror shell-root resize listeners. When the Split View divider drags or a Stage Manager window resizes, `WKWebView` reflows and fires `resize` → the panel re-classifies its band live. Multitasking's dynamic widths are absorbed by existing machinery with no new code.
- **`FormFactor` bands are unchanged.** `< 640 → Phone` (drill-down), `640–1023 → Tablet` (**renders identically to Wide today**), `≥ 1024 → Wide`. This slice does **not** change the band thresholds or the `Tablet == Wide` mapping.
- **Net of the work is one native flag + a verification pass.** Panel code changes only if the verification gate (§6) finds a genuine functional breakage.

This honors R1/R2/R4/R6: the native layer remains a pure transport shell (one Info.plist key), all UI logic stays in the WASM panel, and no business logic enters the shell.

---

## 4. Native Changes (iOS shell)

Scope: `mobile/ios/`. No Swift source changes — single-window multitasking needs none.

1. **`mobile/ios/project.yml`** — delete the `UIRequiresFullScreen: true` line from the `AlephPaneliOS` target's `info.properties`. Removing it (with the iPad device family `1,2` already set by the predecessor slice) lets iPadOS offer Split View, Slide Over, and Stage Manager.
2. **Regenerate `mobile/ios/AlephPaneliOS/Resources/Info.plist` with a bare `xcodegen generate`** — i.e. **unset `ALEPH_VERSION` / `ALEPH_BUILD` / `PANEL_URL` first**, then `xcodegen generate`. This is the hard lesson from the predecessor slice's Critical incident: exporting the version vars before generating expands `${ALEPH_VERSION}` / `${ALEPH_BUILD}` into literals in the committed plist. The committed plist MUST keep the `${...}` placeholders. After regen, the plist must differ from `HEAD` **only** by the removed `UIRequiresFullScreen` key — verify with `git diff`.
3. **Orientations:** the iPad four-orientation set (`UISupportedInterfaceOrientations~ipad`) was declared by the predecessor slice — Split View's orientation prerequisite is already satisfied. **No orientation change.**
4. **No scene / multi-window code** (D1). `AlephPaneliOSApp.swift`'s single `WindowGroup` scene is unchanged. `UIApplicationSupportsMultipleScenes` is NOT added.
5. **No `PanelWebView` / `ContentView` change.** The full-screen `WKWebView`, `.ignoresSafeArea()`, and the `viewport-fit=cover` meta injection already adapt to any window size.

**Native delta = one line deleted + one bare regen.** The committed `project.yml` continues to carry no team/signing identity (signing stays on the xcodebuild CLI, per the distribution slice).

---

## 5. Panel Scope (no-breakage line)

Static audit of `interfaces/webchat/src/platform/wide` against the 640–1023 Tablet band:

- **Already safe (no change):** drawers/overlays carry `max-w-[Nvw]` viewport caps (`extensions/installed.rs` `w-[480px] max-w-[94vw]`, `memory/drawer.rs` `w-[380px] max-w-[90vw]`, `teams/components/create_form.rs` `w-[26rem] max-w-[92vw]`); kanban / plan DAG / tasks / usage tables are `overflow-x` scrollable **by design**; chat bubbles use relative `max-w-[80%]`.
- **One real functional-breakage candidate — provider settings master-detail.** `settings/generation_providers`, `settings/embedding_providers`, `settings/reranking_providers`, `settings/acp_harnesses` each render a two-column master-detail with left `min-w-[400px]` + right `min-w-[320px]` = **720px floor**. Combined with the always-rendered 256px `ModeSidebar`, these pages need ≈ **976px** to lay out without horizontal overflow.
  - This is a **pre-existing** condition: full-screen iPad **portrait** (mini 744 / Air 820 / Pro 11" 834) is already < 976, so these settings pages already overflow on a portrait iPad today — the predecessor slice's real-device QA, still owed, has not surfaced it. Multitasking raises the hit-rate of sub-976 widths.

**The no-breakage line (D2):** the verification gate (§6) determines, at representative widths, whether a primary control in the right column (e.g. the provider's Save action) is **pushed off-screen and unreachable** (even via horizontal scroll). If unreachable → **#1 applies a minimal fix** (allow the master-detail to stack vertically below a threshold, or drop the `min-w` floors so the columns shrink). If reachable-but-cramped → **leave it; #2 owns the proper Tablet layout.** No proactive layout work is done in #1 beyond an actually-unreachable control.

Expected outcome: **zero or one minimal panel change.** The slice is native-flag + verification first; any panel fix is contingent on a confirmed unreachable control.

---

## 6. Verification Gate (three tiers, cheapest first)

Panel reflow is pure web, so most verification needs no device.

### L1 — Browser (cheapest; no device/simulator)

Serve the built panel and resize the window across the bands. For each width, walk **every mode** (Chat / Memory / Agents / Teams / Dashboard / Settings / Extensions) and confirm the layout reflows and **no primary control is hidden by horizontal overflow**:

| Width | Band | Expected | Focus check |
|-------|------|----------|-------------|
| 320px | Phone | Slide Over / narrow 1-3 split → Phone drill-down | landing + drill-down reachable |
| 700px | Tablet (== Wide) | desktop split-pane, cramped but **functionally complete** | **provider settings: is Save reachable?** |
| 900px | Tablet (== Wide) | same, near the 976 floor | same |
| 1100px | Wide | normal desktop split-pane | regression |

L1 produces the binding verdict on the provider-settings candidate (§5) and resolves the open question on `ModeSidebar` auto-collapse (§8).

### L2 — iPad Simulator

Confirm native multitasking actually engages: Split View (1/2, 1/3), Slide Over, and Stage Manager resize. The app must participate in the split, **reflow live** while the divider drags, and not crash. Use an available iPad simulator (the predecessor slice pinned `iPad Pro 11-inch (M5), OS=26.5` because 27.0 `simctl` hangs).

### L3 — Real Device (TestFlight)

Folds into the **owed iPad real-device QA** from the predecessor slices: same checks as L2 plus the feel of dragging the divider by touch. Reached via the distribution slice's `just ios-testflight` flow (Operator gate; requires the paid-account setup already documented).

### Unit

`viewport.rs` already unit-tests band classification at the 640 / 1024 boundaries — no new test unless L1 forces a panel fix, in which case that fix carries its own test. The native flag has no unit-testable logic (Operator/QA gate, as in the predecessor slices).

---

## 7. Out of Scope

- **Multi-window / multiple scenes** — two Aleph windows side by side (D1).
- **Beautiful narrow-Tablet layout** — the entire 640–1023 rendering redesign is **#2 Tablet-specific rendering**.
- **Touch ergonomics** — hover-fallbacks, 44pt tap targets, touch gestures are **#3**.
- **Keyboard avoidance / safe-area work** beyond the existing `viewport-fit=cover` + CSS `env(safe-area-*)`.
- **Provider-settings cosmetic cramping** — only an *unreachable* control is in scope (§5); making it look good is #2.

---

## 8. Open Questions (resolved during verification, not blocking)

1. **Does `ModeSidebar` auto-collapse at Tablet widths, or only via the manual `aleph-sidebar-toggle`?** If it auto-collapses below some width, the 256px reclaims and the provider-settings floor drops from ≈976 to ≈720 — changing the breakage verdict. L1 determines this and the spec's §5 finding is updated to fact.
2. **Is the provider-settings Save control actually unreachable at 700–900px, or merely cramped?** L1 binding verdict decides whether #1 ships a minimal stacking fix or defers to #2.

Both are answered in the verification pass; neither blocks writing the plan, which encodes the conditional fix.

---

## 9. Success Criteria

- iPad app offers Split View, Slide Over, and Stage Manager (L2/L3 confirm).
- Dragging a Split View divider reflows the panel live with no crash (L2/L3).
- At every representative width (§6 L1), every mode reflows and no primary control is unreachable — with the provider-settings master-detail either confirmed reachable or minimally fixed.
- Committed `Info.plist` differs from `HEAD` only by the removed `UIRequiresFullScreen` key; `${ALEPH_VERSION}` / `${ALEPH_BUILD}` placeholders preserved.
- No Swift source change; no multi-scene API added; band thresholds and `Tablet == Wide` mapping unchanged.

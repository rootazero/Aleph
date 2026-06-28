# iPad Shell Enablement — Design Spec

> First iPad slice. Make the existing iOS Panel shell (`mobile/ios/`) run as a
> real iPad app, reusing the panel's existing desktop split-pane layout with
> **zero panel changes**. Sibling to `2026-06-28-ios-panel-pairing-screen-design.md`.

**Date:** 2026-06-28
**Status:** Approved (brainstorming) → ready for plan
**Scope owner:** `mobile/ios/` only (+ one shared SwiftUI restyle)

---

## 1. Goal

Ship the iOS Panel app as a native iPad application. On iPad it renders the
**existing desktop split-pane (Wide) layout** — no phone drill-down, no
tablet-specific UI. The only net-new visual surface is the native pairing
screen, restyled to match the desktop connect wizard.

One sentence: *enable the iPad device family on the existing iOS shell, verify
the webview reports an iPad-width viewport that triggers the Wide layout, and
unify the native pairing screen with the desktop dark connect card.*

---

## 2. Scope

### In scope
- `project.yml`: iPad device family, full-screen-only, iPad orientations.
- `PairingView.swift`: restyle to the desktop dark bordered card (applies to
  **both** iPhone and iPad — shared file, deliberate unification).
- New `Color(hex:)` utility + unit test (the one genuinely testable unit).
- Build + runtime QA on an iPad simulator.

### Out of scope (deferred, listed in §7)
- Any panel (`interfaces/webchat/`) change — confirmed unnecessary (§3).
- iPad-specific `FormFactor::Tablet` rendering (split ratios, tablet spacing).
- Multitasking (Split View / Stage Manager / Slide Over).
- Touch ergonomics of the Wide layout (hover-only affordances, tap targets).
- iPad distribution / App Store / TestFlight (same deferral as the iPhone slice).
- App icon changes (none needed — §4.2).

---

## 3. Architecture & the zero-panel-change finding

The iPad app is the same shape as the iPhone app: a native SwiftUI shell hosting
a `WKWebView` that loads the WASM panel from a paired server URL (R2/R4/R6 — the
native layer is transport bootstrap only; all app UI is the panel).

**Why the panel needs no change.** `interfaces/webchat/src/state/viewport.rs`
classifies the viewport into three bands:

- `Phone` (`< 640px`)
- `Tablet` (`640–1023px`)
- `Wide` (`≥ 1024px`)

Every consumer in `app.rs` branches **only** on `== FormFactor::Phone`
(verified: all `form_factor.get() == FormFactor::Phone` comparisons, no `Tablet`
arm). So `Tablet` and `Wide` render the desktop split-pane layout **identically
today**. `PanelWebView` injects `width=device-width` as the viewport meta, so a
full-screen iPad webview reports the real point width — portrait iPad ≈ 744–1024,
landscape ≈ 1080–1366 — which lands in the Tablet/Wide bands. **Result: iPad
shows the desktop split-pane with no panel code change.** This matches the global
navigation law: phones drill down, iPad gets the split-pane.

---

## 4. Component changes (all under `mobile/ios/`)

### 4.1 `project.yml` — device family, full-screen, orientations
- `settings.base.TARGETED_DEVICE_FAMILY`: `"1"` → `"1,2"` (iPhone + iPad).
- `targets.AlephPaneliOS.info.properties`:
  - Add `UIRequiresFullScreen: true` — opts out of Split View / Stage Manager /
    Slide Over (the chosen full-screen-only behavior).
  - Keep `UISupportedInterfaceOrientations: [UIInterfaceOrientationPortrait]`
    (iPhone stays portrait-only).
  - Add `UISupportedInterfaceOrientations~ipad:
    [Portrait, PortraitUpsideDown, LandscapeLeft, LandscapeRight]` — iPad gets
    all four orientations (idiom-suffixed key; iOS reads `~ipad` on iPad and the
    base key on iPhone).
- `NSAppTransportSecurity`, `UILaunchScreen` unchanged.

> Note: `UIRequiresFullScreen` is discouraged by Apple for *new multitasking*
> apps but is the clean, supported way to opt out of multitasking. Distribution
> is deferred, and full-screen-only is the explicit product decision, so it is
> appropriate here.

### 4.2 App icon — no change
`AppIcon.appiconset/Contents.json` uses the single-size `universal` /
`platform: ios` / `1024x1024` format. Xcode auto-derives every idiom, including
iPad, from the one asset. No edit required.

### 4.3 `PairingView.swift` — desktop dark bordered card (shared, both idioms)
Restyle the existing form (title / subtitle / host field / Connect button /
error) to match `desktop/shell/splash/connect.html`. **No popup, no scrim** — a
single centered, bordered card.

Layout (a `ZStack` over a full-screen background):
- Full-screen background `#0d0d10`, ignoring safe area.
- A `✦ Aleph` wordmark above the card.
- The card: `maxWidth 420`, `padding 28`, background `#17171c`, corner radius 14,
  `1px` border `#2a2a32`, soft shadow. Horizontally **and** vertically centered
  (so it sits centered on the big iPad canvas; on iPhone it also centers — fine).
- Identical in portrait and landscape (centered either way).

Palette (hardcoded dark — does **not** follow the system light/dark setting, so
the three entry points — desktop / iPhone / iPad — look identical):

| Element | Color |
|---|---|
| Screen background | `#0d0d10` |
| Card background | `#17171c` |
| Card / field border | `#2a2a32` |
| Field background | `#0d0d10` |
| Primary text (title) | `#e8e8ea` |
| Secondary text (subtitle) | `#9a9aa2` |
| Connect button | `#4f46e5` (text `#ffffff`) |
| Error text | `#ff6b6b` |
| `✦` wordmark glyph | `#4f46e5` (accent — see note) |

The host `TextField` moves off `.roundedBorder` to a custom dark style
(`#0d0d10` fill + `#2a2a32` border). The Connect button moves off
`.borderedProminent`'s tint to an explicit `#4f46e5` filled style. All existing
behavior is preserved: prefill `initialText`, disabled-while-empty / submitting,
`.onSubmit(connect)`, the optional red `message`.

> **Accent note (resolve at review):** the wireframe previewed the `✦` glyph in
> pink `#ff2d78`. The spec defaults it to the desktop accent `#4f46e5` so there
> is a **single** accent color (button + glyph), maximally unified with desktop.
> Flag during spec review if pink is preferred.

### 4.4 New `Color(hex:)` utility + unit test
No hex→Color helper exists today. Add a small, pure, testable helper:
- `Color.components(fromHex:) -> (r: Double, g: Double, b: Double, a: Double)?`
  — parses a 6- or 8-digit hex string (optionally `#`-prefixed), returns `nil`
  on malformed input. **This pure function is the unit-tested unit.**
- `Color(hex:)` wraps it, falling back to `.clear` on `nil` so a hex typo can
  never crash the view (P7 — no reachable trap; all hex inputs are our own
  compile-time literals, so `.clear` makes a mistake visible at QA instantly).

This is the only meaningful new logic in the slice.

### 4.5 `ContentView` / `PanelWebView` — no change
`ContentView` already switches `.pairing` → `PairingView`, `.connected(url)` →
`PanelWebView`, and `PanelWebView` already `.ignoresSafeArea()` (fills the
screen). On iPad, the connected webview fills the canvas and the panel self-
renders Wide. No edit; runtime QA confirms (§6).

### Files-touched summary
| File | Change |
|---|---|
| `mobile/ios/project.yml` | device family `1,2`, `UIRequiresFullScreen`, `~ipad` orientations |
| `mobile/ios/AlephPaneliOS/Views/PairingView.swift` | desktop dark bordered card + wordmark + centering |
| `mobile/ios/AlephPaneliOS/Views/Color+Hex.swift` (new) | `Color(hex:)` + pure components parser |
| `mobile/ios/AlephPaneliOSTests/ColorHexTests.swift` (new) | unit tests for the parser |
| App icon, `ContentView`, `PanelWebView`, panel | **no change** |

---

## 5. Data flow (unchanged)

`AppState.resolve()` (env → Keychain → reachability probe) → `.pairing` or
`.connected`. iPad reuses the iPhone slice's pairing/Keychain/probe machinery
verbatim. The only behavioral difference is which layout the panel paints, and
that is driven entirely by the webview's reported width (§3).

---

## 6. Verification

This is a **config + styling** slice. Beyond `Color(hex:)`, there is no new
unit-testable logic; the spec deliberately does **not** invent assertion-free
SwiftUI view tests. Three gates:

1. **Build gate.** `xcodebuild build` succeeds for an **iPad simulator**
   destination (e.g. a booted iPad Pro). Proves the iPad device family + Info.plist
   compile and link.
2. **Test gate.** The existing 23 unit tests stay green, plus the new
   `Color(hex:)` parser tests (valid 6/8-digit, `#`-prefix, malformed → `nil`).
3. **Runtime QA (the real gate)** on an iPad simulator:
   - Pairing card renders centered, dark, bordered; `✦ Aleph` above it.
   - Connect with a reachable address → navigates to the panel.
   - Connected panel shows the **desktop split-pane** (sidebar + content), not the
     phone drawer, in **both** portrait and landscape.
   - Rotate portrait ↔ landscape: layout reflows cleanly, stays split-pane.
   - No Split View handle / not resizable (full-screen-only confirmed).
   - Shake-to-reconfigure still returns to the pairing card.

> QA caveat carried from the iPhone slice: launching via Xcode ▶ Run injects
> `PANEL_URL`, which bypasses the pairing screen. To exercise the pairing card
> itself, launch an installed `.app` with no `PANEL_URL` set.

---

## 7. Known limitations & follow-ups (not in this slice)

- **Narrow-portrait density.** The desktop split-pane on the narrowest iPad
  portrait (iPad mini, 744pt: 256 sidebar + 488 content) may feel tight. Runtime
  QA judges whether a later threshold tweak is warranted. No change now.
- **Touch ergonomics.** The Wide layout was designed for mouse/trackpad; hover-
  only affordances and small tap targets on iPad are a known follow-up.
- **Multitasking.** Split View / Stage Manager deferred (full-screen-only now).
- **Tablet-specific rendering.** `FormFactor::Tablet` stays equal to `Wide`
  (YAGNI); a dedicated tablet treatment is a separate future spec.
- **Distribution.** App Store / TestFlight / IPA packaging deferred, as in the
  iPhone slice.

---

## 8. Redline & principle compliance

- **R2 (single UI source):** native layer is transport bootstrap (pairing) only;
  all app UI stays in the WASM panel. No business UI added to the shell.
- **R3 / P6 (minimalism / YAGNI):** reuse the existing Wide layout; no tablet-
  specific UI; no panel changes; one small testable helper.
- **R4 (I/O-only interface):** the shell does no persistence/business logic
  beyond connection config.
- **R6 (one core, many channels):** iPad is another channel onto the same core.
- **P7 (defensive design):** `Color(hex:)` cannot trap on malformed input.

---

## 9. Open question for spec review

- §4.3 accent: `✦` glyph in indigo `#4f46e5` (single accent, default) vs pink
  `#ff2d78` (as previewed). Default is indigo unless you say otherwise.

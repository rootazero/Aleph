# iPad Shell Enablement Implementation Plan

> **⚠️ Superseded 2026-08-24 — the committed `Info.plist` this document edits no longer exists.**
> `mobile/ios/AlephPaneliOS/Resources/Info.plist` is xcodegen *output* and is now
> gitignored beside the generated `.xcodeproj`; `project.yml`'s `info.properties`
> block is the only source, and there is nothing to restore before a commit. Every
> step below that stages that file, or that asks a regeneration to preserve its
> `${ALEPH_VERSION}` / `${ALEPH_BUILD}` placeholders, describes the world as it was
> — the current one is stated once, in `mobile/ios/README.md` and
> `mobile/ios/.gitignore`. Kept as the record of what was done: do not re-add the
> file by following it.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing iOS Panel shell (`mobile/ios/`) run as a native full-screen iPad app that renders the panel's existing desktop split-pane (Wide) layout, and unify the native pairing screen with the desktop dark connect card — with zero panel (`interfaces/webchat/`) changes.

**Architecture:** The iPad app is the same native-shell-over-WKWebView shape as the iPhone app. The panel already classifies an iPad-width viewport into its `Tablet`/`Wide` bands (both render the desktop split-pane), so no panel code changes. Work is three pieces: a small testable `Color(hex:)` helper, `project.yml` device-family/orientation/full-screen config, and a `PairingView` restyle shared by both idioms.

**Tech Stack:** Swift 5.9, SwiftUI, iOS 17.0, xcodegen, xcodebuild, Swift Testing.

## Global Constraints

- iOS 17.0 deployment target, Swift 5.9 — do not raise.
- **Zero changes to `interfaces/webchat/`** (the WASM panel) or any Rust code. Do NOT run cargo. The native layer is transport config only (R2/R4).
- Device family becomes `"1,2"` (iPhone + iPad). iPhone stays **portrait-only**; iPad gets **all four** orientations. `UIRequiresFullScreen: true` (no Split View / Stage Manager).
- Bundle ID `ai.aleph.panel` and the VERSION-wired version are unchanged — do not touch them.
- Pairing screen is a **centered, bordered dark card — no popup, no scrim**. Exact palette (hardcoded, does not follow system light/dark):
  - screen bg `0d0d10` · card bg `17171c` · border `2a2a32` · field bg `0d0d10` · title text `e8e8ea` · subtitle text `9a9aa2` · Connect button `4f46e5` (text white) · error text `ff6b6b` · `✦` glyph `4f46e5`.
- Tests use **Swift Testing** (`import Testing`, `@Suite`, `@Test`, `#expect`) — never XCTest.
- Commit messages: `ios: <description>`. No `Co-Authored-By` trailer (attribution disabled globally).
- **Authoritative signal is `xcodebuild`.** SourceKit / editor diagnostics produce false errors for iOS-only APIs and cross-file symbols — ignore them; only an `xcodebuild` failure is real.
- Regenerate the project without needing a running server/token: `export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')" && xcodegen generate` (bare). Do **not** run `./generate.sh` (it calls a debug `aleph-server` for a token).

---

### Task 1: `Color(hex:)` helper + unit tests

**Files:**
- Create: `mobile/ios/AlephPaneliOS/Views/Color+Hex.swift`
- Test: `mobile/ios/AlephPaneliOSTests/ColorHexTests.swift`

**Interfaces:**
- Produces:
  - `static func Color.rgba(fromHex: String) -> (red: Double, green: Double, blue: Double, alpha: Double)?` — pure parser, `nil` on malformed input.
  - `init Color(hex: String)` — falls back to `.clear` on malformed input.

- [ ] **Step 1: Write the failing tests**

Create `mobile/ios/AlephPaneliOSTests/ColorHexTests.swift`:

```swift
import Testing
import SwiftUI
@testable import AlephPaneliOS

@Suite struct ColorHexTests {
    @Test("parses 6-digit hex")
    func sixDigit() throws {
        let c = try #require(Color.rgba(fromHex: "0d0d10"))
        #expect(abs(c.red - 13.0 / 255) < 0.0001)
        #expect(abs(c.green - 13.0 / 255) < 0.0001)
        #expect(abs(c.blue - 16.0 / 255) < 0.0001)
        #expect(c.alpha == 1.0)
    }

    @Test("accepts a leading hash")
    func leadingHash() throws {
        let c = try #require(Color.rgba(fromHex: "#4f46e5"))
        #expect(abs(c.red - 79.0 / 255) < 0.0001)
        #expect(abs(c.green - 70.0 / 255) < 0.0001)
        #expect(abs(c.blue - 229.0 / 255) < 0.0001)
        #expect(c.alpha == 1.0)
    }

    @Test("parses 8-digit hex with alpha")
    func eightDigitAlpha() throws {
        let c = try #require(Color.rgba(fromHex: "ff000080"))
        #expect(abs(c.red - 1.0) < 0.0001)
        #expect(abs(c.green - 0.0) < 0.0001)
        #expect(abs(c.blue - 0.0) < 0.0001)
        #expect(abs(c.alpha - 128.0 / 255) < 0.0001)
    }

    @Test("malformed input returns nil")
    func malformed() {
        #expect(Color.rgba(fromHex: "xyz") == nil)        // non-hex
        #expect(Color.rgba(fromHex: "12345") == nil)      // wrong length
        #expect(Color.rgba(fromHex: "") == nil)           // empty
        #expect(Color.rgba(fromHex: "gggggg") == nil)     // non-hex digits, right length
    }
}
```

- [ ] **Step 2: Regenerate the project so the new test file is picked up, then run the tests to verify they FAIL**

```bash
cd mobile/ios
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
xcodegen generate
# pick any booted simulator (boot one if none: `xcrun simctl boot "iPhone 16 Pro"`)
xcrun simctl list devices booted
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' test 2>&1 | tail -25
```
Expected: FAIL — compile error "type 'Color' has no member 'rgba'" / "no exact matches for `Color(hex:)`".

> If `iPhone 16 Pro` is ambiguous or unavailable, substitute a booted UDID from `xcrun simctl list devices booted` as `name=<...>` or `id=<UDID>`.

- [ ] **Step 3: Write the helper**

Create `mobile/ios/AlephPaneliOS/Views/Color+Hex.swift`:

```swift
import SwiftUI

extension Color {
    /// Parse a 6- or 8-digit hex string (optional leading `#`, surrounding
    /// whitespace tolerated) into sRGB components in `0...1`. Returns `nil` on
    /// malformed input. Pure and unit-tested — the one bit of real logic in the
    /// iPad styling work.
    static func rgba(fromHex hex: String) -> (red: Double, green: Double, blue: Double, alpha: Double)? {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s = String(s.dropFirst()) }
        guard s.count == 6 || s.count == 8, let value = UInt64(s, radix: 16) else {
            return nil
        }
        if s.count == 8 {
            return (
                red: Double((value >> 24) & 0xff) / 255,
                green: Double((value >> 16) & 0xff) / 255,
                blue: Double((value >> 8) & 0xff) / 255,
                alpha: Double(value & 0xff) / 255
            )
        }
        return (
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255,
            alpha: 1.0
        )
    }

    /// SwiftUI `Color` from a hex string. Falls back to `.clear` on malformed
    /// input so a typo in one of our own compile-time literals is visible at QA
    /// instead of crashing the view (P7 — no reachable trap).
    init(hex: String) {
        guard let c = Color.rgba(fromHex: hex) else {
            self = .clear
            return
        }
        self = Color(.sRGB, red: c.red, green: c.green, blue: c.blue, opacity: c.alpha)
    }
}
```

- [ ] **Step 4: Regenerate (new source file) and run the tests to verify they PASS**

```bash
cd mobile/ios
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
xcodegen generate
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' test 2>&1 | tail -25
```
Expected: PASS — the 4 `ColorHexTests` plus the pre-existing 23 tests all succeed (`** TEST SUCCEEDED **`).

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Views/Color+Hex.swift mobile/ios/AlephPaneliOSTests/ColorHexTests.swift
git commit -m "ios: add Color(hex:) helper with unit tests"
```

---

### Task 2: iPad device family, full-screen, and orientations

**Files:**
- Modify: `mobile/ios/project.yml` (device family at line 16; Info.plist properties around lines 44–54)

**Interfaces:**
- Consumes: nothing.
- Produces: a project whose built app declares `UIDeviceFamily = [1, 2]`, `UIRequiresFullScreen = true`, iPhone portrait-only, and iPad all-orientation.

- [ ] **Step 1: Set the device family**

In `mobile/ios/project.yml`, change the `settings.base` device family:

```yaml
    TARGETED_DEVICE_FAMILY: "1,2"
```
(was `"1"`.)

- [ ] **Step 2: Add iPad orientations + full-screen to the Info.plist properties**

In `mobile/ios/project.yml`, the existing block is:

```yaml
        UISupportedInterfaceOrientations:
          - UIInterfaceOrientationPortrait
```

Replace it with (keep the iPhone portrait key, add the iPad-suffixed key and the full-screen flag):

```yaml
        UIRequiresFullScreen: true
        UISupportedInterfaceOrientations:
          - UIInterfaceOrientationPortrait
        UISupportedInterfaceOrientations~ipad:
          - UIInterfaceOrientationPortrait
          - UIInterfaceOrientationPortraitUpsideDown
          - UIInterfaceOrientationLandscapeLeft
          - UIInterfaceOrientationLandscapeRight
```

Leave the `NSAppTransportSecurity`, `UILaunchScreen`, `CFBundleDisplayName`, and version keys exactly as they are.

- [ ] **Step 3: Regenerate and verify the keys landed**

```bash
cd mobile/ios
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
xcodegen generate
# Device family is a build setting:
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS -showBuildSettings 2>/dev/null \
  | grep TARGETED_DEVICE_FAMILY
# Orientations + full-screen are written into the generated Info.plist:
grep -A6 "UIRequiresFullScreen\|UISupportedInterfaceOrientations~ipad" AlephPaneliOS/Resources/Info.plist
```
Expected: `TARGETED_DEVICE_FAMILY = 1,2`; the Info.plist shows `UIRequiresFullScreen` true and the four `~ipad` orientations.

- [ ] **Step 4: Build for an iPad simulator to prove the family compiles/links**

```bash
cd mobile/ios
xcrun simctl list devices available | grep -i ipad   # pick an available iPad
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPad Pro 11-inch (M4)' build 2>&1 | tail -20
```
Expected: `** BUILD SUCCEEDED **`.

> Substitute an available iPad name/UDID from the `simctl` list if `iPad Pro 11-inch (M4)` is not present. `build` does not require booting the simulator.

- [ ] **Step 5: Commit**

```bash
git add mobile/ios/project.yml mobile/ios/AlephPaneliOS/Resources/Info.plist
git commit -m "ios: enable iPad device family, full-screen, and iPad orientations"
```

> Note: `AlephPaneliOS/Resources/Info.plist` is regenerated by xcodegen from `project.yml`, so it is committed alongside the source-of-truth change.

---

### Task 3: Restyle `PairingView` as the desktop dark connect card

**Files:**
- Modify (full rewrite): `mobile/ios/AlephPaneliOS/Views/PairingView.swift`

**Interfaces:**
- Consumes: `Color(hex:)` from Task 1.
- Produces: the restyled pairing screen (no API change — `PairingView(initialText:message:)` is unchanged, so `ContentView` needs no edit).

- [ ] **Step 1: Rewrite `PairingView.swift`**

Replace the entire contents of `mobile/ios/AlephPaneliOS/Views/PairingView.swift` with:

```swift
import SwiftUI

/// Native first-run / reconfigure screen. Transport config ONLY (which server
/// to connect to) — all app UI lives in the WASM panel (R2/R4). Styled to match
/// the desktop lite shell's `connect.html` wizard: a centered, bordered dark
/// card (no popup, no scrim). Shared by iPhone and iPad so every entry point —
/// desktop / iPhone / iPad — looks identical. Colors are hardcoded dark and do
/// not follow the system light/dark setting.
struct PairingView: View {
    @EnvironmentObject private var appState: AppState

    let initialText: String
    let message: String?

    @State private var address: String
    @State private var submitting = false

    // Desktop connect.html palette.
    private let screenBg = Color(hex: "0d0d10")
    private let cardBg = Color(hex: "17171c")
    private let border = Color(hex: "2a2a32")
    private let titleText = Color(hex: "e8e8ea")
    private let subtitleText = Color(hex: "9a9aa2")
    private let accent = Color(hex: "4f46e5")
    private let errorColor = Color(hex: "ff6b6b")

    init(initialText: String, message: String?) {
        self.initialText = initialText
        self.message = message
        _address = State(initialValue: initialText)
    }

    private var submitDisabled: Bool {
        submitting || address.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        ZStack {
            screenBg.ignoresSafeArea()

            VStack(spacing: 18) {
                HStack(spacing: 6) {
                    Text("✦").foregroundStyle(accent)
                    Text("Aleph").foregroundStyle(titleText)
                }
                .font(.title3).bold()

                card
            }
            .frame(maxWidth: 420)
            .padding(24)
        }
        // Force the whole pairing screen dark so the system TextField's default
        // placeholder reads as light-gray on the dark field regardless of the
        // device's light/dark setting (keeps the three shells identical).
        .preferredColorScheme(.dark)
    }

    private var card: some View {
        VStack(spacing: 16) {
            Text("Connect to Aleph")
                .font(.title2).bold()
                .foregroundStyle(titleText)

            Text("Enter your Aleph server address — e.g. 192.168.1.5 or http://gw.example.com")
                .font(.footnote)
                .foregroundStyle(subtitleText)
                .multilineTextAlignment(.center)

            TextField(
                "",
                text: $address,
                prompt: Text("host, host:port, or http(s)://host")
            )
            .textFieldStyle(.plain)
            .foregroundStyle(titleText)
            .tint(accent)
            .keyboardType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled(true)
            .submitLabel(.go)
            .onSubmit(connect)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(screenBg)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(border, lineWidth: 1))

            Button(action: connect) {
                Text(submitting ? "Connecting…" : "Connect")
                    .font(.body).bold()
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 11)
                    .background(accent)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .disabled(submitDisabled)
            .opacity(submitDisabled ? 0.5 : 1)

            if let message {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(errorColor)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(28)
        .background(cardBg)
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(border, lineWidth: 1))
        .shadow(color: .black.opacity(0.5), radius: 20, x: 0, y: 10)
    }

    private func connect() {
        guard !submitting else { return }
        submitting = true
        Task {
            await appState.submit(address)
            submitting = false
        }
    }
}
```

- [ ] **Step 2: Build for iPhone and iPad, and run the test suite green**

```bash
cd mobile/ios
# No new files added, so no regenerate needed; build both idioms.
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' build 2>&1 | tail -15
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPad Pro 11-inch (M4)' build 2>&1 | tail -15
xcodebuild -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' test 2>&1 | tail -15
```
Expected: both `** BUILD SUCCEEDED **`; `** TEST SUCCEEDED **` with the 23 + 4 (`ColorHexTests`) tests passing. The `PairingView` change is styling only — no test asserts its appearance, so no test should change behavior.

> SourceKit may red-underline `foregroundStyle` on `Text` inside `prompt:` or the `Color` stored properties — ignore it. Only an `xcodebuild` failure counts.

- [ ] **Step 3: Commit**

```bash
git add mobile/ios/AlephPaneliOS/Views/PairingView.swift
git commit -m "ios: restyle pairing screen as desktop dark connect card (iPhone + iPad)"
```

---

## Runtime QA (owed to the user — not an automated step)

After all three tasks, the user verifies on an iPad simulator (per spec §6). This is the real acceptance gate; the build/test gates above only prove it compiles and the existing logic is intact.

- Pairing card renders centered, dark `#0d0d10` background, bordered `#17171c` card, `✦ Aleph` above it — no popup/scrim.
- Connect to a reachable address → navigates to the panel.
- Connected panel shows the **desktop split-pane** (sidebar + content), not the phone drawer, in **both** portrait and landscape.
- Rotate portrait ↔ landscape: reflows cleanly, stays split-pane.
- Not resizable / no Split View handle (full-screen-only confirmed).
- Shake-to-reconfigure returns to the pairing card.

> QA caveat (from the iPhone slice): Xcode ▶ Run injects `PANEL_URL`, which bypasses the pairing screen. To exercise the pairing card, launch an installed `.app` with **no** `PANEL_URL` set. See the iOS-panel test flow in long-term memory.

---

## Self-Review (filled in by the plan author)

- **Spec coverage:** §4.1 → Task 2; §4.2 (icon, no change) → no task, verified n/a; §4.3 → Task 3; §4.4 → Task 1; §4.5 (ContentView/PanelWebView no change) → no task, confirmed unchanged (PairingView API is stable); §6 verification → build/test steps + Runtime QA section. §3 panel-zero-change is honored by touching no panel files.
- **Type consistency:** `Color.rgba(fromHex:)` and `Color(hex:)` are defined in Task 1 and consumed identically in Task 3. Palette hex values match the Global Constraints and spec §4.3 table exactly.
- **No placeholders:** every code step is complete; commands have expected output.

# iPad Multitasking (Split View / Stage Manager) Implementation Plan

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

**Goal:** Make the iOS Panel a single-window iPad multitasking participant (Split View / Slide Over / Stage Manager) by removing the one Info.plist flag that blocks it, and prove via a structural gate that nothing else changed.

**Architecture:** Native-only enablement. The panel's `FormFactorState` already listens to `window.resize`, so `WKWebView` reflow under dynamic multitasking widths is absorbed by existing machinery — no panel code changes. The single deliverable is deleting `UIRequiresFullScreen: true` from `project.yml`, regenerating the committed Info.plist bare (placeholders preserved), and verifying the diff is exactly that one key. Runtime reflow verification is an Operator/QA gate, not a code task.

**Tech Stack:** XcodeGen (`project.yml` → `.xcodeproj` + Info.plist), xcodebuild (iOS Simulator), no Swift source change, no Rust/WASM change.

## Global Constraints

Every task implicitly includes these (verbatim from `docs/superpowers/specs/2026-06-28-ipad-multitasking-design.md`):

- **D1 — single-window only.** Do NOT add `UIApplicationSupportsMultipleScenes`. Do NOT add scene-lifecycle code. `AlephPaneliOSApp.swift`'s single `WindowGroup` stays unchanged.
- **D2 — no-breakage line.** This slice fixes a panel layout ONLY if a primary control is pushed off-screen and unreachable even via scroll. Cosmetic cramping of the `Tablet == Wide` layout is out of scope (owned by #2). Static analysis already concluded the one candidate (provider-settings master-detail) is reachable via the shell `<main>`'s horizontal scroll → no panel fix expected.
- **Committed `Info.plist` must keep the `${ALEPH_VERSION}` and `${ALEPH_BUILD}` literal placeholders.** Regenerate with env vars UNSET (`unset ALEPH_VERSION ALEPH_BUILD PANEL_URL` before `xcodegen generate`) — setting them expands the placeholders into literals (the predecessor slice's Critical incident).
- **`project.yml` carries no team / signing identity** (signing lives on the xcodebuild CLI in the distribution slice). Do not add `DEVELOPMENT_TEAM`.
- **No band-threshold or `Tablet == Wide` mapping change** in `interfaces/webchat/src/state/viewport.rs` or `app.rs`.
- **No Swift source change** — single-window multitasking needs none.
- **Never `git add` the `.xcodeproj`** (gitignored; its scheme embeds a `PANEL_URL` secret).
- Commit style `ios: <description>`, no `Co-Authored-By` trailer.

---

### Task 1: Enable iPad multitasking (remove `UIRequiresFullScreen`)

**Files:**
- Modify: `mobile/ios/project.yml` (delete the `UIRequiresFullScreen: true` line in the `AlephPaneliOS` target's `info.properties`)
- Modify (regenerated, committed): `mobile/ios/AlephPaneliOS/Resources/Info.plist` (loses the `UIRequiresFullScreen` key only)
- Do NOT touch: any `*.swift`, `interfaces/webchat/**`, `mobile/ios/README.md`

**Interfaces:**
- Consumes: nothing from other tasks (single-task plan).
- Produces: nothing other tasks rely on (single-task plan).

**Context for the implementer:** `project.yml` is the XcodeGen source; it declares `info.path: AlephPaneliOS/Resources/Info.plist` + `info.properties`, so `xcodegen generate` writes that committed Info.plist from the properties. The iPad device family (`TARGETED_DEVICE_FAMILY: "1,2"`) and the four iPad orientations were already set by the predecessor slice — removing `UIRequiresFullScreen` is the only remaining thing iPadOS needs to offer Split View / Slide Over / Stage Manager to a single-window app.

- [ ] **Step 1: Capture the pre-change Info.plist baseline**

This is the verification anchor — the committed plist must end up differing from `HEAD` by exactly the removed key.

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph/mobile/ios
git show HEAD:mobile/ios/AlephPaneliOS/Resources/Info.plist | grep -n 'UIRequiresFullScreen\|ALEPH_VERSION\|ALEPH_BUILD'
```
Expected output (the key is present at HEAD; both placeholders present):
```
20:		<string>${ALEPH_VERSION}</string>
22:		<string>${ALEPH_BUILD}</string>
35:	<key>UIRequiresFullScreen</key>
```

- [ ] **Step 2: Remove the flag from `project.yml`**

In `mobile/ios/project.yml`, the `AlephPaneliOS` target's `info.properties` currently contains:
```yaml
        UILaunchScreen:
          UIColorName: ""
        UIRequiresFullScreen: true
        UISupportedInterfaceOrientations:
          - UIInterfaceOrientationPortrait
```
Delete the single line `        UIRequiresFullScreen: true` so it reads:
```yaml
        UILaunchScreen:
          UIColorName: ""
        UISupportedInterfaceOrientations:
          - UIInterfaceOrientationPortrait
```
Leave every other line (device family, orientations, ATS, encryption flag, version placeholders) untouched.

- [ ] **Step 3: Regenerate the committed Info.plist BARE (placeholders preserved)**

Env vars MUST be unset so `${ALEPH_VERSION}`/`${ALEPH_BUILD}` stay literal in the committed plist.

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph/mobile/ios
unset ALEPH_VERSION ALEPH_BUILD PANEL_URL
xcodegen generate
```
Expected: `Loaded project ... Created project at .../AlephPaneliOS.xcodeproj` with no error. (This also regenerates the gitignored `.xcodeproj`; that is fine and must NOT be committed.)

- [ ] **Step 4: Verify the structural invariants (this task's "tests")**

Run each check; all must pass.

```bash
cd /Volumes/TBU4/Workspace/Aleph/mobile/ios
# (a) flag gone from BOTH source and generated plist
grep -c UIRequiresFullScreen project.yml                              # expect: 0
grep -c UIRequiresFullScreen AlephPaneliOS/Resources/Info.plist       # expect: 0
# (b) version placeholders preserved (NOT expanded to literals)
grep -c '${ALEPH_VERSION}' AlephPaneliOS/Resources/Info.plist         # expect: 1
grep -c '${ALEPH_BUILD}'  AlephPaneliOS/Resources/Info.plist          # expect: 1
# (c) the committed plist differs from HEAD by ONLY the removed key
git -C /Volumes/TBU4/Workspace/Aleph diff --no-color -- mobile/ios/AlephPaneliOS/Resources/Info.plist
```
Expected `git diff` (exactly two removed lines, nothing else — no added/changed lines):
```
-	<key>UIRequiresFullScreen</key>
-	<true/>
```
If the diff shows any OTHER change (reformatting, expanded `${...}` placeholders, reordered keys), STOP: the regen ran with env vars set or a stale toolchain — re-run Step 3 after confirming `unset`, and reconcile until the diff is exactly the two lines above.

- [ ] **Step 5: Confirm the regenerated project still builds for an iPad simulator**

A build-only sanity check that the regen produced a valid project (no run, no PANEL_URL needed).

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph/mobile/ios
xcodebuild build \
  -project AlephPaneliOS.xcodeproj \
  -scheme AlephPaneliOS \
  -destination 'generic/platform=iOS Simulator' \
  -quiet
```
Expected: ends with `** BUILD SUCCEEDED **`. (SourceKit/editor diagnostics are not authoritative here — only `xcodebuild` is, per the predecessor iPad slice.)

- [ ] **Step 6: Commit (project.yml + Info.plist ONLY — never the `.xcodeproj`)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add mobile/ios/project.yml mobile/ios/AlephPaneliOS/Resources/Info.plist
git status --short          # verify ONLY those two paths are staged; .xcodeproj must NOT appear
git commit -m "ios: enable iPad multitasking (remove UIRequiresFullScreen)"
```
Expected: commit succeeds; `git status --short` before commit shows exactly `M mobile/ios/project.yml` and `M mobile/ios/AlephPaneliOS/Resources/Info.plist` staged (the regenerated `AlephPaneliOS.xcodeproj/` stays unstaged and gitignored).

---

## Operator / QA Verification Gate (NOT subagent checkboxes)

These are runtime verifications the user performs; they are the spec's §6 L1/L2/L3 gate. They are listed here for completeness and to define the single conditional follow-up — they are **not** implementer steps (a subagent cannot eyeball a resized browser or drive Stage Manager).

**L1 — Browser (cheapest).** Serve the built panel; resize the window across `320 / 700 / 900 / 1100 px`. For each width walk every mode (Chat / Memory / Agents / Teams / Dashboard / Settings / Extensions) and confirm the layout reflows and no primary control is hidden. **Binding check:** open Settings → a provider page (Generation / Embedding / Reranking / ACP harnesses) at 700–900px and confirm the right-column **Save** control is reachable (the shell `<main>` should scroll horizontally). Static analysis predicts reachable-via-scroll; L1 confirms.

**L2 — iPad Simulator.** Split View (1/2, 1/3), Slide Over, Stage Manager resize → app participates, reflows live while dragging the divider, no crash. Use an available iPad simulator (predecessor pinned `iPad Pro 11-inch (M5), OS=26.5`; 27.0 `simctl` hangs).

**L3 — Real device (TestFlight).** Folds into the owed iPad real-device QA. Reached via the distribution slice's `just ios-testflight` Operator flow.

**Single conditional follow-up (only if L1/L2/L3 contradicts the static analysis):** if the provider-settings Save control is found genuinely **unreachable** (not merely cramped), file a minimal #1 fix — allow that master-detail to stack vertically below a width threshold, or drop the `min-w-[400px]`/`min-w-[320px]` floors — guarded by D2. Otherwise the cramping is cosmetic and owned by #2 Tablet-specific rendering; no further #1 work.

---

## Plan Self-Review

**1. Spec coverage:**
- §1 goal (single-window multitasking enablement) → Task 1.
- §2 D1 (single-window) / D2 (no-breakage line) → Global Constraints + Operator gate's conditional follow-up.
- §4 native changes (remove flag, bare regen, no orientation/scene/Swift change) → Task 1 Steps 2–4 + Global Constraints.
- §5 panel scope (provider-settings candidate, no expected change) → Operator gate binding check + conditional follow-up.
- §6 verification (L1/L2/L3) → Operator/QA Verification Gate.
- §7 out-of-scope (multi-window, #2 beauty, #3 touch, README) → Global Constraints + no README task.
- §8 open questions (`<main>` horizontal scroll, Save reachability) → resolved statically in the plan's analysis; L1 confirms.
- §9 success criteria (multitasking offered, live reflow, plist diff = one key, no Swift/scene/band change) → Task 1 Steps 4–6 + Global Constraints + Operator gate.

No spec requirement is without a home.

**2. Placeholder scan:** No TBD/TODO/"handle edge cases". Every step has the exact command and expected output. The conditional follow-up names the concrete files/values, not a vague "fix if needed".

**3. Type consistency:** No cross-task interfaces (single task). The two file paths, the two grep tokens (`${ALEPH_VERSION}`, `${ALEPH_BUILD}`), the removed key (`UIRequiresFullScreen`), and the commit message are used identically everywhere they appear.

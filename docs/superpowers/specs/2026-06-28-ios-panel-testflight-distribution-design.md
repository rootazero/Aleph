# iOS Panel TestFlight Distribution — Design Spec

> **⚠️ Superseded 2026-08-24 — the committed `Info.plist` this document edits no longer exists.**
> `mobile/ios/AlephPaneliOS/Resources/Info.plist` is xcodegen *output* and is now
> gitignored beside the generated `.xcodeproj`; `project.yml`'s `info.properties`
> block is the only source, and there is nothing to restore before a commit. Every
> step below that stages that file, or that asks a regeneration to preserve its
> `${ALEPH_VERSION}` / `${ALEPH_BUILD}` placeholders, describes the world as it was
> — the current one is stated once, in `mobile/ios/README.md` and
> `mobile/ios/.gitignore`. Kept as the record of what was done: do not re-add the
> file by following it.

> First iOS distribution slice. Give the existing iOS Panel app a repeatable way
> to reach real devices via **TestFlight internal testing**, driven by one local
> script. Pure shell/ops — **zero panel change, zero Rust, zero CI**. Unblocks the
> real-device QA that the iPad and iPhone shell slices are still owed.
> Sibling to `2026-06-28-ipad-shell-enablement-design.md`.

**Date:** 2026-06-28
**Status:** Approved (brainstorming) → ready for plan
**Scope owner:** `mobile/ios/` only (+ its README)

---

## 1. Goal

Ship the iOS Panel app (`ai.aleph.panel`, iPhone + iPad) to TestFlight so it
installs on real devices and auto-updates, without the 7-day free-account expiry
and without the manual Xcode-▶-Run-per-device dance.

One sentence: *add one local release script that archives, signs, and uploads a
distribution build to App Store Connect for TestFlight internal testing, plus the
one-time-setup runbook and the small `project.yml`/Info.plist additions it needs.*

This is the orthogonal, zero-redline-risk member of the four deferred iPad
follow-ups (the other three — multitasking, Tablet-specific rendering, touch
ergonomics — remain their own future specs). It is sequenced first because it
unblocks real-device QA of everything already shipped.

---

## 2. Decisions (locked in brainstorming)

| Axis | Decision | Why |
|---|---|---|
| Channel | **TestFlight** | Real-device QA + auto-update + a few testers; no full App Review for internal testers. Best fit for a self-hosted thin client. |
| Test scope | **Internal testing only** | Testers = up to 100 people added to App Store Connect with a role. **No Beta App Review, no demo-server-for-reviewer, no export-compliance review gauntlet.** Builds installable ~immediately after processing. |
| Automation | **Local script MVP** | One `just`/shell target on your Mac, App Store Connect API key. Unblocks QA fast; avoids the CI-signing rabbit hole. CI deferred. |
| Membership | **Paid Apple Developer Program ($99/yr)** | Hard prerequisite for any of the above. Documented in §5; assumed in hand. |

---

## 3. Architecture & shape

The slice is **pure shell/ops**. It changes nothing about how the app behaves
once connected: distribution builds ship the **native pairing screen** (no baked
`PANEL_URL`), so a tester pairs to their **own** reachable `aleph-server`. This
preserves the existing "no secrets in source" model (R2/R4/R6 untouched — the
native layer remains transport bootstrap only; all app UI stays in the WASM
panel).

It produces exactly two deliverables:
1. A new local release script (`mobile/ios/release-testflight.sh`, surfaced as a
   `just ios-testflight` target) + the small `project.yml`/Info.plist additions
   it depends on (§4).
2. A one-time-setup runbook (§5) for enrollment, the App Store Connect app
   record, and the API key.

No panel (`interfaces/webchat/`) change. No Rust. No GitHub Actions. No new unit
logic in the shell.

---

## 4. The release script + config changes (all under `mobile/ios/`)

### 4.1 `release-testflight.sh` flow

1. **Marketing version** = CalVer from the repo's single `VERSION` source →
   `export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"` (same line
   the existing `generate.sh` uses).
2. **Build number** = `export ALEPH_BUILD="$(( $(date +%s) / 60 ))"` — integer
   minutes since the Unix epoch (see §4.3).
3. **Bare regenerate** — `xcodegen generate` with **no `PANEL_URL` exported**, so
   the distribution build ships the pairing screen and the committed Info.plist
   keeps its `${…}` placeholders (the Task-2 bare-regen lesson from the iPad
   slice).
4. **Archive** — `xcodebuild archive` for a generic iOS device, Release config,
   with **signing passed on the command line** (§4.4) so the committed
   `project.yml` carries no team/signing identity:

   ```
   xcodebuild archive \
     -project AlephPaneliOS.xcodeproj -scheme AlephPaneliOS \
     -configuration Release -destination 'generic/platform=iOS' \
     -archivePath build/AlephPaneliOS.xcarchive \
     -allowProvisioningUpdates \
     -authenticationKeyPath "$ASC_KEY_PATH" \
     -authenticationKeyID "$ASC_KEY_ID" \
     -authenticationKeyIssuerID "$ASC_ISSUER_ID" \
     DEVELOPMENT_TEAM="$ALEPH_TEAM_ID" CODE_SIGN_STYLE=Automatic
   ```
5. **Export** — `xcodebuild -exportArchive` with a **script-generated**
   `ExportOptions.plist` (written to a gitignored temp path, teamID from
   `$ALEPH_TEAM_ID`, `method: app-store-connect`, `uploadSymbols: true`) →
   produces a signed `.ipa`.
6. **Upload** — `xcrun altool --upload-app -t ios -f <ipa>
   --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"`. Success → the build
   appears in App Store Connect → TestFlight. (The plan pins exact flags; if a
   future Xcode drops `altool`, the equivalent is an `ExportOptions.plist` with
   `destination: upload`. The plan picks one and states it.)

The script fails fast (`set -euo pipefail`) and prints which env vars are missing
if the credentials are not set, so a half-configured run never produces a
mystery archive.

### 4.2 `project.yml` / Info.plist additions

Only these two property changes (everything else in §`targets.AlephPaneliOS.info`
is unchanged):

- `CFBundleVersion: ${ALEPH_BUILD}` (was `${ALEPH_VERSION}`). `CFBundleShortVersionString`
  **stays** `${ALEPH_VERSION}`.
- Add `ITSAppUsesNonExemptEncryption: false` — the app uses only OS-standard
  HTTPS/TLS (exempt under the standard exemption), so this removes the per-upload
  export-compliance prompt in App Store Connect.

`generate.sh` (the dev/sim flow) must also export `ALEPH_BUILD` (any value, e.g.
the same minutes expression) so the bare placeholder still resolves when running
in the simulator — otherwise `${ALEPH_BUILD}` would leak literally into the
dev Info.plist. (The plan wires this one-line addition to `generate.sh`.)

### 4.3 Build-number strategy (the one tricky point)

TestFlight rejects re-upload of an already-seen `(CFBundleShortVersionString,
CFBundleVersion)` pair, and `CFBundleVersion` must be a dotted run of integers
within Apple's size limits. CalVer (`YY.M.D`) permits only **one** value per day,
but QA needs several uploads per day. So marketing version and build number are
**decoupled**:

- `CFBundleShortVersionString = ${ALEPH_VERSION}` — CalVer, human-facing, unchanged.
- `CFBundleVersion = ${ALEPH_BUILD}` = **integer minutes since the Unix epoch**
  (`$(( $(date +%s) / 60 ))`). Today ≈ 29.7 million — a single integer far under
  the uint32 ceiling (4.29e9), strictly monotonic at one-minute resolution, and
  **stateless** (nothing committed, no counter file).

> Limitation: two uploads within the same wall-clock minute would collide. For a
> manual local flow this never happens in practice; the script can sleep-and-retry
> or the operator re-runs a minute later. Documented, not engineered around.

**Alternative considered (open question, §9):** `git rev-list --count HEAD`
(commit count). Rejected as the default because re-archiving the *same* commit
during a QA loop would collide; the timestamp does not.

### 4.4 Signing model

**Xcode automatic signing**, driven by the **App Store Connect API key**, with
all signing inputs passed on the `xcodebuild` command line (`DEVELOPMENT_TEAM`,
`CODE_SIGN_STYLE=Automatic`, `-allowProvisioningUpdates`, the three
`-authenticationKey*` flags). Consequences:

- The committed `project.yml` keeps **no** `DEVELOPMENT_TEAM`, no provisioning
  profile, no signing certificate reference. The dev/sim ▶Run path is byte-for-byte
  unchanged.
- No `fastlane match` / shared cert repo (that is a team-scale concern; deferred).

### 4.5 Files-touched summary

| File | Change |
|---|---|
| `mobile/ios/release-testflight.sh` (new) | archive → export → upload to TestFlight |
| `mobile/ios/project.yml` | `CFBundleVersion: ${ALEPH_BUILD}`; add `ITSAppUsesNonExemptEncryption: false` |
| `mobile/ios/generate.sh` | also `export ALEPH_BUILD=…` so the sim flow resolves the placeholder |
| `mobile/ios/README.md` | distribution section: prerequisites, one-time setup, `release-testflight.sh` usage |
| `justfile` | `ios-testflight` target wrapping the script |
| `.gitignore` | ensure generated `ExportOptions.plist` temp path is ignored (if not already covered) |
| panel, Swift sources, `ContentView`, App icon | **no change** |

---

## 5. One-time setup runbook (manual, once)

Documented in the spec and `mobile/ios/README.md`; performed once by the operator,
not automated:

1. **Apple Developer Program enrollment** ($99/yr). Prerequisite for everything.
2. **App Store Connect app record** for bundle ID `ai.aleph.panel`: app name
   "Aleph Panel", an SKU, primary language. Created once in the ASC web UI.
3. **App Store Connect API key** (Users and Access → Integrations → Keys):
   download the `.p8` once; record **Key ID** and **Issuer ID**. Place the key at
   `~/.appstoreconnect/private_keys/AuthKey_<KEY_ID>.p8` (where `altool`/`xcodebuild`
   look it up) and/or point `$ASC_KEY_PATH` at it.
4. **Team ID** recorded for `$ALEPH_TEAM_ID`.
5. **Add internal testers** in App Store Connect (TestFlight tab) — anyone given
   an Account Holder / Admin / App Manager / Developer / Marketing role, up to 100.

Required environment (all local, never committed):
`ALEPH_TEAM_ID`, `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_PATH`.

---

## 6. Secrets & no-leak model

Extends the existing model (README "No secrets in source") verbatim:

| Artifact | Committed? | Why safe |
|---|---|---|
| `release-testflight.sh`, `project.yml` (`${ALEPH_BUILD}` placeholder), `generate.sh`, README, `justfile` | yes | placeholders / runtime resolution only — no IDs, no keys |
| `.p8` API key, Key ID, Issuer ID, `ALEPH_TEAM_ID` | **no** (env + `~/.appstoreconnect/`, gitignored) | personal credentials |
| generated `ExportOptions.plist` (teamID baked) | **no** (gitignored temp, script-generated each run) | personal ID |
| distribution build's connection target | **n/a** | ships the pairing screen — no `PANEL_URL` baked |

Nothing committable gains a personal identifier or key — the same discipline as
the existing gitignored `${PANEL_URL}` scheme injection.

---

## 7. Verification

This is a config + tooling slice; there is **no new unit-testable logic** in the
shell, so the spec does not invent assertion-free tests. Three gates:

1. **Archive/export gate.** `release-testflight.sh` runs clean on a configured
   Mac: bare regenerate → `xcodebuild archive` (Release, device) succeeds →
   `-exportArchive` produces a signed `.ipa`. After the run, `git status` shows
   the committed `project.yml`/Info.plist still hold their `${…}` placeholders
   (bare-regen lesson).
2. **Upload/processing gate.** `altool --upload-app` returns success; the build
   appears in App Store Connect → TestFlight and finishes processing **without an
   export-compliance prompt** (confirms `ITSAppUsesNonExemptEncryption: false`).
3. **Real-device gate (the real one).** Install via TestFlight on a real **iPhone**
   and a real **iPad** → native pairing card → connect to a reachable
   `aleph-server` → panel renders: phone drill-down on iPhone, **desktop
   split-pane on iPad** — closing the loop on the iPad slice's still-owed
   real-device QA.

> The existing 27 Swift unit tests stay green (no shell source behavior changes);
> running them is not a gate of this slice but should not regress.

---

## 8. Out of scope (deferred follow-ups)

- **External testers** + Beta App Review + a demo-server-for-reviewer in review
  notes + export-compliance review (the self-hosted-thin-client review problem).
- **CI automation** — GitHub Actions macOS runner, signing certs + API key as CI
  secrets, integration with the CalVer `just release` flow.
- **App Store public release** — full App Review, metadata, screenshots, privacy
  nutrition labels.
- **`fastlane match`** — shared/encrypted team certificates.
- The other three deferred iPad items: **#1 multitasking**, **#2 Tablet-specific
  rendering**, **#3 touch ergonomics** — each its own future spec.

---

## 9. Open questions for spec review

1. **Build number scheme (§4.3):** integer minutes since epoch (default) vs
   `git rev-list --count HEAD`. Default is timestamp-minutes (robust to QA
   re-archives of the same commit). Flag if you prefer commit-count.
2. **Upload mechanism (§4.1 step 6):** `xcrun altool --upload-app` (default,
   most documented, key-based) vs `xcodebuild -exportArchive` with
   `destination: upload`. The plan picks one and pins exact flags; state a
   preference here if you have one.
3. **`just ios-testflight` vs standalone `./release-testflight.sh`:** the spec
   ships both (target wraps script). Say if you want only the script.

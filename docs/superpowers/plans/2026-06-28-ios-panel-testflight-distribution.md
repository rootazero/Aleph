# iOS Panel TestFlight Distribution Implementation Plan

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

**Goal:** Add one local release script (+ small config and docs) that archives, signs, and uploads an iOS Panel distribution build to App Store Connect for TestFlight internal testing.

**Architecture:** Pure shell/ops under `mobile/ios/`. No panel, Rust, Swift, or CI change. A new `release-testflight.sh` resolves the CalVer marketing version and a monotonic build number, regenerates the Xcode project with no baked `PANEL_URL` (ships the pairing screen), archives with signing passed on the `xcodebuild` CLI (so the committed `project.yml` holds no team/signing identity), exports a signed IPA via a script-generated `ExportOptions.plist`, and uploads with `xcrun altool` using an App Store Connect API key.

**Tech Stack:** XcodeGen (`project.yml`), `xcodebuild archive`/`-exportArchive`, `xcrun altool`, App Store Connect API key (`.p8`), `just`, bash.

## Global Constraints

- **Scope = `mobile/ios/` only** (+ its `README.md` and the root `justfile`). Zero change to `interfaces/webchat/` (panel), Rust, Swift sources, `ContentView`, app icon, or CI workflows.
- **Bundle ID `ai.aleph.panel` unchanged.** The committed `project.yml` carries **no** `DEVELOPMENT_TEAM`, provisioning profile, or signing certificate — all signing inputs are passed on the `xcodebuild` command line.
- **Marketing version** = CalVer from the repo's single `VERSION` file, injected as `${ALEPH_VERSION}`. `CFBundleShortVersionString` **stays** `${ALEPH_VERSION}`.
- **Build number** = `${ALEPH_BUILD}` = `$(( $(date +%s) / 60 ))` (integer minutes since the Unix epoch — monotonic, stateless). `CFBundleVersion` **becomes** `${ALEPH_BUILD}`.
- **Add `ITSAppUsesNonExemptEncryption: false`** to the Info.plist properties (YAML boolean `false`, unquoted).
- **Distribution builds ship the pairing screen — NEVER bake `PANEL_URL`** into a distribution build. No server IP/token in any committed file.
- **No secrets committed:** the `.p8` key, Key ID, Issuer ID, and Team ID stay in environment variables + `~/.appstoreconnect/private_keys/`. The generated `ExportOptions.plist`, the `.xcarchive`, and the `.ipa` all live under `mobile/ios/build/` (already gitignored).
- **The committed `Info.plist` must be BARE-generated** (placeholders preserved), produced by `xcodegen generate` with `ALEPH_VERSION`/`ALEPH_BUILD`/`PANEL_URL` **unset**. (Lesson from the iPad slice: exporting those vars before `xcodegen generate` bakes literal values into the committed plist.)
- **Upload mechanism:** `xcrun altool --upload-app -t ios`. `ExportOptions.plist` uses `method: app-store-connect`, `destination: export`.
- **Test scope = internal testing only** — no Beta App Review, no demo server, no external testers (out of scope).
- **`xcodegen`/`xcodebuild` are authoritative.** Editor/SourceKit diagnostics are not relevant (no Swift is added).
- **Commit style:** `ios: <description>` / `docs: <description>`, English, **no `Co-Authored-By`** (attribution disabled globally).
- **No paid Apple credentials in the dev environment.** Steps that require a paid Apple Developer membership (real archive/export/upload, device install) are marked **Operator gate** and are NOT implementer checkbox steps — the implementer delivers a syntactically-valid, env-validated script and config; the human runs the credentialed gates later.

---

### Task 1: Decoupled build number + export-compliance config

Make `CFBundleVersion` a separate injected placeholder (`${ALEPH_BUILD}`) from the
marketing version, declare encryption exemption, and teach the dev/sim flow to
resolve the new placeholder — while keeping the committed Info.plist bare.

**Files:**
- Modify: `mobile/ios/project.yml` (the `targets.AlephPaneliOS.info.properties` block)
- Modify: `mobile/ios/generate.sh` (add the `ALEPH_BUILD` export)
- Regenerate (committed, bare): `mobile/ios/AlephPaneliOS/Resources/Info.plist`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the `${ALEPH_BUILD}` placeholder in `Info.plist:CFBundleVersion`, resolved from the `ALEPH_BUILD` environment variable at `xcodegen generate` time. Task 2's `release-testflight.sh` sets `ALEPH_BUILD` before generating.

- [ ] **Step 1: Observe current state (RED)**

Run:
```bash
cd mobile/ios
unset ALEPH_VERSION ALEPH_BUILD PANEL_URL
xcodegen generate >/dev/null
/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' AlephPaneliOS/Resources/Info.plist
/usr/libexec/PlistBuddy -c 'Print :ITSAppUsesNonExemptEncryption' AlephPaneliOS/Resources/Info.plist 2>&1 || true
```
Expected:
- `CFBundleVersion` prints `${ALEPH_VERSION}` (currently tied to the marketing version).
- The `ITSAppUsesNonExemptEncryption` print fails with `Does Not Exist`.

- [ ] **Step 2: Edit `project.yml` — split the build number + declare encryption exemption**

In `mobile/ios/project.yml`, inside `targets.AlephPaneliOS.info.properties`, change the `CFBundleVersion` line and add the encryption key. The block becomes:

```yaml
      properties:
        CFBundleDisplayName: Aleph Panel
        CFBundleShortVersionString: ${ALEPH_VERSION}
        CFBundleVersion: ${ALEPH_BUILD}
        # App uses only OS-standard HTTPS/TLS (exempt under the standard
        # encryption exemption). Declaring this removes the per-upload export-
        # compliance prompt in App Store Connect / TestFlight.
        ITSAppUsesNonExemptEncryption: false
        UILaunchScreen:
          UIColorName: ""
        UIRequiresFullScreen: true
```

Leave every other property (`UISupportedInterfaceOrientations*`, `NSAppTransportSecurity`, the ATS comment) exactly as-is.

- [ ] **Step 3: Edit `generate.sh` — export `ALEPH_BUILD` so the sim flow resolves the placeholder**

In `mobile/ios/generate.sh`, immediately after the existing `export ALEPH_VERSION=...` line, add:

```bash
# TestFlight requires a unique CFBundleVersion per upload, but CalVer is one/day.
# Decouple: marketing version = CalVer (ALEPH_VERSION); build number =
# integer minutes since the Unix epoch (monotonic, stateless). The dev/sim flow
# only needs *a* value so the ${ALEPH_BUILD} placeholder resolves on generate.
export ALEPH_BUILD="$(( $(date +%s) / 60 ))"
```

- [ ] **Step 4: Verify the placeholders are wired (GREEN, bare)**

Run:
```bash
cd mobile/ios
unset ALEPH_VERSION ALEPH_BUILD PANEL_URL
xcodegen generate >/dev/null
/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' AlephPaneliOS/Resources/Info.plist
/usr/libexec/PlistBuddy -c 'Print :ITSAppUsesNonExemptEncryption' AlephPaneliOS/Resources/Info.plist
```
Expected:
- `CFBundleVersion` prints `${ALEPH_BUILD}` (literal placeholder — bare regen preserves it).
- `ITSAppUsesNonExemptEncryption` prints `false`.

- [ ] **Step 5: Verify substitution resolves when the vars are set**

Run:
```bash
cd mobile/ios
export ALEPH_VERSION=26.6.28 ALEPH_BUILD="$(( $(date +%s) / 60 ))"
xcodegen generate >/dev/null
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' AlephPaneliOS/Resources/Info.plist
/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' AlephPaneliOS/Resources/Info.plist
```
Expected:
- `CFBundleShortVersionString` prints `26.6.28`.
- `CFBundleVersion` prints a ~8-digit integer (e.g. `29070123`) — the resolved build number.

- [ ] **Step 6: Restore the bare Info.plist and commit**

Run:
```bash
cd mobile/ios
unset ALEPH_VERSION ALEPH_BUILD PANEL_URL
xcodegen generate >/dev/null
/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' AlephPaneliOS/Resources/Info.plist   # must print ${ALEPH_BUILD}
```
Expected: `CFBundleVersion` prints `${ALEPH_BUILD}` (placeholder restored — safe to commit).

Then:
```bash
cd "$(git rev-parse --show-toplevel)"
git add mobile/ios/project.yml mobile/ios/generate.sh mobile/ios/AlephPaneliOS/Resources/Info.plist
git commit -m "ios: decouple CFBundleVersion (\${ALEPH_BUILD}) + declare encryption exemption"
```

---

### Task 2: `release-testflight.sh` archive → export → upload script

The single deliverable of the slice: a fail-fast local script that produces and
uploads a TestFlight build. Implementer-runnable verification covers syntax and
the missing-credential guard; the credentialed end-to-end run is an Operator gate.

**Files:**
- Create: `mobile/ios/release-testflight.sh` (executable)

**Interfaces:**
- Consumes: `Info.plist:CFBundleVersion = ${ALEPH_BUILD}` from Task 1; the repo `VERSION` file; the env vars `ALEPH_TEAM_ID`, `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_PATH`.
- Produces: a build uploaded to App Store Connect → TestFlight. Surfaced as a `just` target in Task 3 (`just ios-testflight` → `cd mobile/ios && ./release-testflight.sh`).

- [ ] **Step 1: Write the missing-credential guard test (RED)**

Run (the script does not exist yet):
```bash
cd mobile/ios
env -u ALEPH_TEAM_ID -u ASC_KEY_ID -u ASC_ISSUER_ID -u ASC_KEY_PATH bash release-testflight.sh; echo "exit=$?"
```
Expected: `bash: release-testflight.sh: No such file or directory`, `exit=127`.

- [ ] **Step 2: Create `mobile/ios/release-testflight.sh`**

Create the file with exactly this content:

```bash
#!/usr/bin/env bash
# Build + upload an iOS Panel distribution build to App Store Connect (TestFlight,
# internal testing). Pure ops: ships the native pairing screen (NO baked PANEL_URL),
# signs via the App Store Connect API key with all signing inputs on the xcodebuild
# CLI, so the committed project.yml holds no team/signing identity.
#
# Required environment (never committed — see mobile/ios/README.md):
#   ALEPH_TEAM_ID   Apple Developer Team ID (e.g. ABCDE12345)
#   ASC_KEY_ID      App Store Connect API Key ID
#   ASC_ISSUER_ID   App Store Connect API Issuer ID
#   ASC_KEY_PATH    path to AuthKey_<ASC_KEY_ID>.p8
#                   (also place it in ~/.appstoreconnect/private_keys/ for altool)
set -euo pipefail
cd "$(dirname "$0")"

# --- 1. Validate credentials up front (fail fast, before any build work) ------
missing=()
for v in ALEPH_TEAM_ID ASC_KEY_ID ASC_ISSUER_ID ASC_KEY_PATH; do
  if [ -z "${!v:-}" ]; then missing+=("$v"); fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "error: missing required env: ${missing[*]}" >&2
  echo "see mobile/ios/README.md → Distribution (TestFlight) for one-time setup." >&2
  exit 1
fi
if [ ! -f "$ASC_KEY_PATH" ]; then
  echo "error: ASC_KEY_PATH not found: $ASC_KEY_PATH" >&2
  exit 1
fi

# --- 2. Versions -------------------------------------------------------------
# Marketing version = CalVer from the repo's single VERSION source.
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
# Build number = integer minutes since the Unix epoch: monotonic, stateless, and
# well under the uint32 ceiling. TestFlight rejects a re-used (version, build) pair.
export ALEPH_BUILD="$(( $(date +%s) / 60 ))"

# --- 3. Regenerate the project for DISTRIBUTION ------------------------------
# Version + build are exported (baked into the archive); PANEL_URL is NOT, so the
# build ships the pairing screen and leaks no server IP/token.
unset PANEL_URL
xcodegen generate
echo "→ version ${ALEPH_VERSION} build ${ALEPH_BUILD}"

# --- 4. Archive (signing entirely on the CLI; project.yml stays identity-free) -
ARCHIVE="build/AlephPaneliOS.xcarchive"
rm -rf "$ARCHIVE"
xcodebuild archive \
  -project AlephPaneliOS.xcodeproj \
  -scheme AlephPaneliOS \
  -configuration Release \
  -destination 'generic/platform=iOS' \
  -archivePath "$ARCHIVE" \
  -allowProvisioningUpdates \
  -authenticationKeyPath "$ASC_KEY_PATH" \
  -authenticationKeyID "$ASC_KEY_ID" \
  -authenticationKeyIssuerID "$ASC_ISSUER_ID" \
  DEVELOPMENT_TEAM="$ALEPH_TEAM_ID" \
  CODE_SIGN_STYLE=Automatic

# --- 5. Export a signed IPA via a generated ExportOptions.plist ---------------
# Written under build/ (gitignored); teamID from env — no personal ID committed.
# method: app-store-connect is the Xcode 15+ value (older Xcode wants "app-store").
OPTS="build/ExportOptions.plist"
cat > "$OPTS" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key>
  <string>app-store-connect</string>
  <key>teamID</key>
  <string>${ALEPH_TEAM_ID}</string>
  <key>uploadSymbols</key>
  <true/>
  <key>destination</key>
  <string>export</string>
</dict>
</plist>
PLIST

EXPORT_DIR="build/export"
rm -rf "$EXPORT_DIR"
xcodebuild -exportArchive \
  -archivePath "$ARCHIVE" \
  -exportOptionsPlist "$OPTS" \
  -exportPath "$EXPORT_DIR" \
  -allowProvisioningUpdates \
  -authenticationKeyPath "$ASC_KEY_PATH" \
  -authenticationKeyID "$ASC_KEY_ID" \
  -authenticationKeyIssuerID "$ASC_ISSUER_ID"

IPA="$(ls "$EXPORT_DIR"/*.ipa | head -1)"
echo "→ exported ${IPA}"

# --- 6. Upload to App Store Connect (TestFlight) -----------------------------
# altool finds AuthKey_<ASC_KEY_ID>.p8 in ~/.appstoreconnect/private_keys/.
xcrun altool --upload-app -t ios -f "$IPA" \
  --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"

echo "✓ Uploaded. It will appear under App Store Connect → TestFlight after processing."
echo "  NOTE: this run resolved version/build into AlephPaneliOS/Resources/Info.plist."
echo "        Run ./generate.sh (or 'git checkout AlephPaneliOS/Resources/Info.plist')"
echo "        before committing, to restore the \${ALEPH_VERSION}/\${ALEPH_BUILD} placeholders."
```

Then make it executable:
```bash
chmod +x mobile/ios/release-testflight.sh
```

- [ ] **Step 3: Verify syntax (GREEN — syntax)**

Run:
```bash
cd mobile/ios
bash -n release-testflight.sh && echo "syntax OK"
command -v shellcheck >/dev/null && shellcheck release-testflight.sh && echo "shellcheck clean" || echo "(shellcheck not installed — bash -n is the gate)"
```
Expected: `syntax OK`. If `shellcheck` is present, `shellcheck clean` with no warnings.

- [ ] **Step 4: Verify the missing-credential guard (GREEN — guard)**

Run:
```bash
cd mobile/ios
env -u ALEPH_TEAM_ID -u ASC_KEY_ID -u ASC_ISSUER_ID -u ASC_KEY_PATH bash release-testflight.sh; echo "exit=$?"
```
Expected: prints `error: missing required env: ALEPH_TEAM_ID ASC_KEY_ID ASC_ISSUER_ID ASC_KEY_PATH` to stderr and `exit=1` — i.e. it fails BEFORE any `xcodegen`/`xcodebuild` runs.

- [ ] **Step 5: Verify the build-number expression is a monotonic integer**

Run:
```bash
a=$(( $(date +%s) / 60 )); sleep 1; b=$(( $(date +%s) / 60 ))
echo "$a $b"; [ "$b" -ge "$a" ] && [ "$a" -gt 0 ] && [ "$a" -lt 4294967296 ] && echo "monotonic integer OK"
```
Expected: two ~8-digit integers (b ≥ a), then `monotonic integer OK`.

- [ ] **Step 6: Commit**

```bash
cd "$(git rev-parse --show-toplevel)"
git add mobile/ios/release-testflight.sh
git commit -m "ios: add release-testflight.sh (archive → export → TestFlight upload)"
```

> **Operator gate (manual — run by the human; needs paid membership + the one-time setup in Task 3's README section). NOT an implementer step.**
> ```bash
> export ALEPH_TEAM_ID=ABCDE12345 ASC_KEY_ID=XXXXXXXXXX ASC_ISSUER_ID=aaaa-bbbb-cccc \
>        ASC_KEY_PATH=~/.appstoreconnect/private_keys/AuthKey_XXXXXXXXXX.p8
> cd mobile/ios && ./release-testflight.sh
> ```
> Expected: archive → export → `✓ Uploaded`; the build appears in App Store Connect → TestFlight and finishes processing **with no export-compliance prompt** (confirms `ITSAppUsesNonExemptEncryption`). Then install via TestFlight on a real iPhone and iPad → pairing card → connect to a reachable `aleph-server` → panel renders (phone drill-down on iPhone, desktop split-pane on iPad).

---

### Task 3: `just ios-testflight` target + README distribution runbook

Surface the script as a `just` target and document the one-time setup + usage so
the operator can run the credentialed gate without re-deriving any of it.

**Files:**
- Modify: `justfile` (add the `ios-testflight` recipe)
- Modify: `mobile/ios/README.md` (add a `## Distribution (TestFlight)` section)

**Interfaces:**
- Consumes: `mobile/ios/release-testflight.sh` and its four env vars from Task 2.
- Produces: nothing downstream (final task).

- [ ] **Step 1: Add the `just` target**

Append to the root `justfile`:

```makefile
# Build + upload an iOS Panel distribution build to TestFlight (internal testing).
# Requires a paid Apple Developer membership + the ASC env vars
# (ALEPH_TEAM_ID / ASC_KEY_ID / ASC_ISSUER_ID / ASC_KEY_PATH).
# See mobile/ios/README.md → Distribution (TestFlight).
ios-testflight:
    cd mobile/ios && ./release-testflight.sh
```

- [ ] **Step 2: Verify the target is registered**

Run:
```bash
just --list | grep ios-testflight
just --show ios-testflight
```
Expected: `--list` shows the `ios-testflight` recipe with its comment; `--show` prints the recipe body `cd mobile/ios && ./release-testflight.sh`.

- [ ] **Step 3: Add the README distribution section**

Append to `mobile/ios/README.md`:

```markdown
## Distribution (TestFlight)

The app ships to real devices via **TestFlight internal testing**, driven by one
local script (`release-testflight.sh`). The build carries no baked `PANEL_URL` —
testers pair to their own `aleph-server`, exactly like the sim/dev flow.

### Prerequisites (one-time)

1. **Apple Developer Program** membership ($99/yr).
2. **App Store Connect app record** for bundle ID `ai.aleph.panel` (app name
   "Aleph Panel", an SKU, a primary language) — created once in the App Store
   Connect web UI.
3. **App Store Connect API key** (Users and Access → Integrations → Keys):
   download the `.p8` once; note the **Key ID** and **Issuer ID**. Place the key
   at `~/.appstoreconnect/private_keys/AuthKey_<KEY_ID>.p8` (where `altool` and
   `xcodebuild` look it up).
4. **Team ID** — note it for `ALEPH_TEAM_ID`.
5. **Internal testers** — in App Store Connect → TestFlight, add up to 100 people
   (anyone with an Account Holder / Admin / App Manager / Developer / Marketing
   role). Internal testing needs no Beta App Review.

### Usage

```bash
export ALEPH_TEAM_ID=ABCDE12345          # your Apple Developer Team ID
export ASC_KEY_ID=XXXXXXXXXX             # App Store Connect API Key ID
export ASC_ISSUER_ID=aaaa-bbbb-cccc      # App Store Connect API Issuer ID
export ASC_KEY_PATH=~/.appstoreconnect/private_keys/AuthKey_XXXXXXXXXX.p8

just ios-testflight        # or: cd mobile/ios && ./release-testflight.sh
```

The script resolves the CalVer marketing version from `VERSION`, computes a
unique build number (integer minutes since the Unix epoch), archives a signed
Release build (signing passed on the `xcodebuild` CLI), exports a signed IPA,
and uploads it. After processing, the build appears under App Store Connect →
TestFlight for your internal testers.

> After a release run, the local `AlephPaneliOS/Resources/Info.plist` holds the
> resolved version/build. Run `./generate.sh` (or
> `git checkout AlephPaneliOS/Resources/Info.plist`) before committing to restore
> the `${ALEPH_VERSION}`/`${ALEPH_BUILD}` placeholders.

### No secrets in source

| Artifact | Committed? | Why safe |
|------|-----------|---------|
| `release-testflight.sh`, `project.yml`, `generate.sh`, README, `justfile` | yes | placeholders / runtime resolution only — no IDs, no keys |
| `.p8` API key, Key ID, Issuer ID, `ALEPH_TEAM_ID` | **no** (env + `~/.appstoreconnect/`) | personal credentials |
| `build/ExportOptions.plist` (teamID baked), `*.xcarchive`, `*.ipa` | **no** (`build/` is gitignored) | personal ID / build output |
| distribution build's connection target | **n/a** | ships the pairing screen — no `PANEL_URL` baked |
```

- [ ] **Step 4: Verify the README env vars match the script**

Run:
```bash
cd mobile/ios
diff <(grep -oE 'ALEPH_TEAM_ID|ASC_KEY_ID|ASC_ISSUER_ID|ASC_KEY_PATH' release-testflight.sh | sort -u) \
     <(grep -oE 'ALEPH_TEAM_ID|ASC_KEY_ID|ASC_ISSUER_ID|ASC_KEY_PATH' README.md | sort -u) \
  && echo "env vars match"
```
Expected: no diff output, then `env vars match`.

- [ ] **Step 5: Commit**

```bash
cd "$(git rev-parse --show-toplevel)"
git add justfile mobile/ios/README.md
git commit -m "docs: document iOS Panel TestFlight distribution + just target"
```

---

## Notes for the executor

- **No paid Apple credentials are assumed in the dev environment.** Every
  implementer checkbox above runs without an Apple account: `xcodegen generate`,
  `PlistBuddy`, `bash -n`, the missing-credential guard, `just --show`, and the
  README/script consistency check. The credentialed end-to-end run and the
  real-device install are the **Operator gate** in Task 2 — leave them for the
  human.
- **`xcodegen` mutates `AlephPaneliOS/Resources/Info.plist` and the gitignored
  `.xcodeproj`.** Only commit the Info.plist in its **bare** form (placeholders).
  Never `git add` the `.xcodeproj` (gitignored; would leak a scheme `PANEL_URL`).
- If `xcodegen` is not on `PATH`, install via `brew install xcodegen` (the iPad
  slice used it; it is expected to be present).
```

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
echo "  NOTE: this run resolved version/build into AlephPaneliOS/Resources/Info.plist,"
echo "        which is generated + gitignored — nothing to restore before committing."

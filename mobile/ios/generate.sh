#!/usr/bin/env bash
# Regenerate the Xcode project from project.yml with the local connection URL
# baked into the (gitignored) scheme, so Xcode's > Run connects automatically.
#
# This script holds NO literal token — it reads a fresh one at runtime, so the
# script itself is safe. The resolved PANEL_URL is written only into the
# generated .xcodeproj scheme, which .gitignore excludes. Nothing committable
# ever contains the IP/token.
#
# After running this, open AlephPaneliOS.xcodeproj and just press > Run.
set -euo pipefail
cd "$(dirname "$0")"
# Version strings come from the repo's single VERSION source (CalVer), mirrored
# into the generated Info.plist the same way PANEL_URL is injected into the scheme.
# That plist (AlephPaneliOS/Resources/Info.plist) is xcodegen OUTPUT and is
# gitignored alongside the .xcodeproj — this script rewrites it on every run, so
# there is nothing to restore afterwards. project.yml holds the properties.
export ALEPH_VERSION="$(cat ../../VERSION | tr -d '[:space:]')"
# TestFlight requires a unique CFBundleVersion per upload, but CalVer is one/day.
# Decouple: marketing version = CalVer (ALEPH_VERSION); build number =
# integer minutes since the Unix epoch (monotonic, stateless). The dev/sim flow
# only needs *a* value so the ${ALEPH_BUILD} placeholder resolves on generate.
export ALEPH_BUILD="$(( $(date +%s) / 60 ))"

ROUTE="${1:-/settings}"   # /settings | / | /memory ...

# Connection target baked into the (gitignored) scheme.
# Default = LOCAL full core on this Mac (token fetched live from the repo's debug
# binary, which embeds the current phone UI; override the binary with CORE_BIN).
# To target a REMOTE core instead, pre-set PANEL_URL (used verbatim), e.g. the
# deployed server:
#   PANEL_URL="http://172.245.43.211:18790/?token=$(ssh ColoCrossing '~/.local/bin/aleph-server bootstrap-token' | tail -1)" ./generate.sh
if [ -z "${PANEL_URL:-}" ]; then
  REPO_ROOT="$(cd ../.. && pwd)"
  CORE_BIN="${CORE_BIN:-$REPO_ROOT/target/debug/aleph-server}"
  TOKEN="$("$CORE_BIN" bootstrap-token | tail -1)"
  export PANEL_URL="http://127.0.0.1:18790${ROUTE}?token=${TOKEN}"
fi

xcodegen generate
echo "✓ Generated. Scheme env PANEL_URL → ${PANEL_URL%%\?*}?token=<token>"
echo "  Open AlephPaneliOS.xcodeproj and press > Run."

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

# Locate the repo's debug aleph-server (embeds the current phone UI). Default is
# relative to the repo root so this works from any clone; override with CORE_BIN.
REPO_ROOT="$(cd ../.. && pwd)"
CORE_BIN="${CORE_BIN:-$REPO_ROOT/target/debug/aleph-server}"
ROUTE="${1:-/settings}"   # /settings | / | /memory ...
TOKEN="$("$CORE_BIN" bootstrap-token | tail -1)"

export PANEL_URL="http://127.0.0.1:18790${ROUTE}?token=${TOKEN}"
xcodegen generate
echo "✓ Generated. Scheme env PANEL_URL → http://127.0.0.1:18790${ROUTE}?token=<token>"
echo "  Open AlephPaneliOS.xcodeproj and press > Run (make sure the local core on :18790 is up)."

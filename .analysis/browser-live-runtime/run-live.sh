#!/usr/bin/env bash
# Live interruption cases: real grant, real frame, then a real Gateway action.
set -u
cd "$(dirname "$0")/../.."
R=.analysis/browser-live-runtime
SIG="$PWD/$R/signals"
export ALEPH_BROWSER_LIVE_CHROME_PATH='C:/Program Files/Google/Chrome/Application/chrome.exe'
export ALEPH_BROWSER_LIVE_SIGNAL_DIR="$SIG"
for c in revoke-live navigate-fence; do
  rm -rf "$SIG"; mkdir -p "$SIG"
  (cd "$R" && node e2e-issue.js --out "url-$c.txt" 2>&1 | grep -E 'grant issued|fatal|failed' | cut -c1-60)
  export ALEPH_BROWSER_LIVE_VIEWER_URL="$(cat "$R/url-$c.txt")"
  node "$R/e2e-live-action.js" "$c" "$SIG" > "$R/driver-$c.log" 2>&1 &
  DRV=$!
  bash qa/browser_live/run.sh --case "$c" > "$R/qa-$c.log" 2>&1
  rc=$?
  wait $DRV; drc=$?
  echo "CASE $c exit=$rc driver=$drc $(head -c 200 "$R/driver-$c.log")"
  sed -E 's/#[A-Za-z0-9_.~%=&-]{20,}/#<REDACTED>/g' "$R/qa-$c.log" | grep -E '✓|✘|passed|failed|Error:|Received|Expected' | head -8
done

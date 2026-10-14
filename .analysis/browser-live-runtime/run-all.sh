#!/usr/bin/env bash
# Issue a fresh real grant per case and run the browser-live selectors.
set -u
cd "$(dirname "$0")/../.."
R=.analysis/browser-live-runtime
export ALEPH_BROWSER_LIVE_CHROME_PATH='C:/Program Files/Google/Chrome/Application/chrome.exe'
iss() { (cd "$R" && node e2e-issue.js --out "$1" ${2:-} 2>&1 | grep -E 'grant issued|revoke|fatal|failed|no ' | cut -c1-80); }
runcase() {
  bash qa/browser_live/run.sh --case "$1" > "$R/qa-$1.log" 2>&1
  local rc=$?
  echo "CASE $1 exit=$rc"
  sed -E 's/#[A-Za-z0-9_.~%=&-]{20,}/#<REDACTED>/g' "$R/qa-$1.log" | grep -E '✓|✘|passed|failed|Error:' | head -4
}
for c in stream control-all-entries standalone-same-target grant-boundary; do
  iss "url-$c.txt"; export ALEPH_BROWSER_LIVE_VIEWER_URL="$(cat "$R/url-$c.txt")"; runcase "$c"
done
iss url-bs1.txt; iss url-bs2.txt
export ALEPH_BROWSER_LIVE_VIEWER_URL="$(cat "$R/url-bs1.txt")" ALEPH_BROWSER_LIVE_SECOND_VIEWER_URL="$(cat "$R/url-bs2.txt")"
runcase binding-switch
iss url-reap.txt --revoke; export ALEPH_BROWSER_LIVE_REAP_VIEWER_URL="$(cat "$R/url-reap.txt")"; runcase reap
unset ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL
bash qa/browser_live/run.sh --case managed-companion > /dev/null 2> "$R/qa-mc.err"
echo "CASE managed-companion exit=$? $(tail -1 "$R/qa-mc.err" | sed 's/.*: //')"

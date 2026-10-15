#!/usr/bin/env bash
set -euo pipefail

# Real browser-live viewer runner. Every selected case requires a live launch
# URL supplied by the caller; there is no synthetic resource or fake grant.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CASE="${1:-}"
if [[ "${CASE}" == "--case" ]]; then
  CASE="${2:-}"
fi

case "${CASE}" in
  stream|control-all-entries|standalone-same-target|binding-switch|managed-companion|grant-boundary|reap|revoke-live|navigate-fence|target-destroy|ttl-expiry)
    ;;
  *)
    echo "usage: qa/browser_live/run.sh --case <stream|control-all-entries|standalone-same-target|binding-switch|managed-companion|grant-boundary|reap|revoke-live|navigate-fence|target-destroy|ttl-expiry>" >&2
    exit 2
    ;;
esac

case "${CASE}" in
  stream|control-all-entries|standalone-same-target|grant-boundary)
    : "${ALEPH_BROWSER_LIVE_VIEWER_URL:?ALEPH_BROWSER_LIVE_VIEWER_URL must be a real Gateway-issued viewer URL}"
    ;;
  binding-switch)
    : "${ALEPH_BROWSER_LIVE_VIEWER_URL:?ALEPH_BROWSER_LIVE_VIEWER_URL must be set}"
    : "${ALEPH_BROWSER_LIVE_SECOND_VIEWER_URL:?ALEPH_BROWSER_LIVE_SECOND_VIEWER_URL must be a second real Gateway-issued viewer URL}"
    ;;
  managed-companion)
    : "${ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL:?ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL must be a real managed-companion viewer URL}"
    ;;
  revoke-live|navigate-fence|target-destroy|ttl-expiry)
    : "${ALEPH_BROWSER_LIVE_VIEWER_URL:?ALEPH_BROWSER_LIVE_VIEWER_URL must be a real Gateway-issued viewer URL}"
    : "${ALEPH_BROWSER_LIVE_SIGNAL_DIR:?ALEPH_BROWSER_LIVE_SIGNAL_DIR must name a directory watched by the Gateway-side driver}"
    ;;
  reap)
    : "${ALEPH_BROWSER_LIVE_REAP_VIEWER_URL:?ALEPH_BROWSER_LIVE_REAP_VIEWER_URL must be a real expiring/revoked viewer URL}"
    ;;
esac

cd "${ROOT_DIR}"
exec npx playwright test \
  --config=qa/browser_live/playwright.config.ts \
  --project=chromium \
  --grep "(^| )${CASE}:" \
  "viewer.spec.ts"

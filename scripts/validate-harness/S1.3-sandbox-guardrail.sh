#!/usr/bin/env bash
# S1.3 — Sandbox guardrail: WorkspaceSandbox + denied_paths configured + root intact
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.3"
MODULE="M9"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

SBX=$(grep -cE "Sandbox: WorkspaceSandbox rooted at" "$LOG" 2>/dev/null || true); SBX=${SBX:-0}
[[ "$SBX" -ge 1 ]] && p=true || p=false
check "sandbox_initialized" "log_grep" ">= 1" "$SBX" "$p"

DEN=$(grep -oE "denied_paths_count=[0-9]+" "$LOG" 2>/dev/null | grep -oE "[0-9]+" | head -1 || true); DEN=${DEN:-0}
[[ "$DEN" -ge 1 ]] && p=true || p=false
check "denied_paths_configured" "log_grep" ">= 1" "$DEN" "$p"

if [[ -d / && -d /etc && -d /usr ]]; then
  ROOT="intact"; p=true
else
  ROOT="MISSING"; p=false
fi
check "root_intact" "fs_assertion" "/, /etc, /usr exist" "$ROOT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

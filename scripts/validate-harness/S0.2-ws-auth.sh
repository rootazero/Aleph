#!/usr/bin/env bash
# S0.2 — gateway WS authenticated event present (gateway critical)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.2"
MODULE="gateway"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

N=$(grep -c "gateway\.ws\.authenticated.*bedac\|ws.*authenticated.*bedac" "$LOG" 2>/dev/null || true)
N=${N:-0}
[[ "$N" -ge 1 ]] && p=true || p=false
check "ws_authenticated" "log_grep" ">= 1" "$N" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

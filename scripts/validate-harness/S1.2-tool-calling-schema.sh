#!/usr/bin/env bash
# S1.2 — Tool calling schema validation (M6, critical)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.2"
MODULE="M6"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

VAL=$(grep -cE "tool_call\.validated.*valid=true|tool_call\.validated.*\"valid\":true" "$LOG" 2>/dev/null || true); VAL=${VAL:-0}
[[ "$VAL" -ge 1 ]] && p=true || p=false
check "schemars_validation" "log_grep" ">= 1" "$VAL" "$p"

if [[ -f /tmp/aleph-test/fib.rs ]]; then
  FW="exists"; p=true
else
  FW="missing"; p=false
fi
check "file_written" "fs_assertion" "exists" "$FW" "$p"

ERR=$(grep -cE "tool_not_found|tool_call\.error.*tool_not_found" "$LOG" 2>/dev/null || true); ERR=${ERR:-0}
END=$(grep -c "loop\.end" "$LOG" 2>/dev/null || true); END=${END:-0}
if [[ "$ERR" -ge 1 ]] && [[ "$END" -ge 1 ]]; then p=true; else p=false; fi
check "tool_not_found_structured" "log_grep" ">= 1 err + loop.end seen" "$ERR err / $END loop_end" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

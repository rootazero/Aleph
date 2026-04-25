#!/usr/bin/env bash
# S0.3 — Think-Act loop trace events + session_events row present (M1 critical)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.3"
MODULE="M1"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

S=$(grep -c "loop\.start" "$LOG" 2>/dev/null || true); S=${S:-0}
[[ "$S" -ge 1 ]] && p=true || p=false
check "loop_start" "log_grep" ">= 1" "$S" "$p"

E=$(grep -c "loop\.end" "$LOG" 2>/dev/null || true); E=${E:-0}
[[ "$E" -ge 1 ]] && p=true || p=false
check "loop_end" "log_grep" ">= 1" "$E" "$p"

T=$(grep -cE '\bthink\b' "$LOG" 2>/dev/null || true); T=${T:-0}
[[ "$T" -ge 1 ]] && p=true || p=false
check "think_event" "log_grep" ">= 1" "$T" "$p"

A=$(grep -cE '\bact\b' "$LOG" 2>/dev/null || true); A=${A:-0}
[[ "$A" -ge 1 ]] && p=true || p=false
check "act_event" "log_grep" ">= 1" "$A" "$p"

ROWS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM session_events" 2>/dev/null || true); ROWS=${ROWS:-0}
[[ "$ROWS" -ge 1 ]] && p=true || p=false
check "session_events_growth" "sql_assertion" ">= 1" "$ROWS" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

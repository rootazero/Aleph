#!/usr/bin/env bash
# S3.1 — Subagent fork (M11, high)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.1"
MODULE="M11"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: spawn event in any agent-events-like table
SPAWN=0
for tbl in agent_events subagent_events events; do
  N=$(sqlite3 "$DB" "SELECT COUNT(*) FROM $tbl WHERE event_type LIKE '%spawn%'" 2>/dev/null || true)
  N=${N:-0}
  SPAWN=$(( SPAWN + N ))
done
[[ "$SPAWN" -ge 1 ]] && p=true || p=false
check "spawn_event" "sql_assertion" ">= 1" "$SPAWN" "$p"

# Check 2: child session row (parent_session_id IS NOT NULL)
NEW_SESS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM sessions WHERE parent_session_id IS NOT NULL" 2>/dev/null || true)
NEW_SESS=${NEW_SESS:-0}
[[ "$NEW_SESS" -ge 1 ]] && p=true || p=false
check "child_session_row" "sql_assertion" ">= 1" "$NEW_SESS" "$p"

# Check 3: handoff back to parent in log
HANDOFF=$(grep -cE "subagent\.handoff_back|handoff_back" "$LOG" 2>/dev/null || true)
HANDOFF=${HANDOFF:-0}
[[ "$HANDOFF" -ge 1 ]] && p=true || p=false
check "handoff_back" "log_grep" ">= 1" "$HANDOFF" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

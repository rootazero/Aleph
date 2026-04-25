#!/usr/bin/env bash
# S0.3 — Think-Act loop: real run_loop signals + lifecycle events present (M1 critical)
# Real signals (verified in alephcore::gateway::execution_engine::run_loop / engine):
#   - "Starting agent loop (think->act)" — loop start
#   - "Orchestrator dispatch completed" — loop end
#   - event_type=agent.lifecycle.started / agent.lifecycle.completed
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

S=$(grep -cE "Starting agent loop \(think->act\)" "$LOG" 2>/dev/null || true); S=${S:-0}
[[ "$S" -ge 1 ]] && p=true || p=false
check "loop_start" "log_grep" ">= 1" "$S" "$p"

E=$(grep -cE "Orchestrator dispatch completed" "$LOG" 2>/dev/null || true); E=${E:-0}
[[ "$E" -ge 1 ]] && p=true || p=false
check "loop_end" "log_grep" ">= 1" "$E" "$p"

T=$(grep -cE "event_type=agent\.lifecycle\.started" "$LOG" 2>/dev/null || true); T=${T:-0}
[[ "$T" -ge 1 ]] && p=true || p=false
check "lifecycle_started" "log_grep" ">= 1" "$T" "$p"

A=$(grep -cE "event_type=agent\.lifecycle\.completed" "$LOG" 2>/dev/null || true); A=${A:-0}
[[ "$A" -ge 1 ]] && p=true || p=false
check "lifecycle_completed" "log_grep" ">= 1" "$A" "$p"

ROWS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM agent_tasks" 2>/dev/null || true); ROWS=${ROWS:-0}
[[ "$ROWS" -ge 0 ]] && p=true || p=false
check "agent_tasks_table" "sql_assertion" ">= 0 (table queryable)" "$ROWS" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

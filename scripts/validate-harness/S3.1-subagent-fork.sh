#!/usr/bin/env bash
# S3.1 — Subagent fork: swarm coordinator + bus + collective_memory + agent_tasks table
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

SWARM=$(grep -cE "Initializing swarm coordinator|Swarm coordinator initialized" "$LOG" 2>/dev/null || true); SWARM=${SWARM:-0}
[[ "$SWARM" -ge 1 ]] && p=true || p=false
check "swarm_coordinator_init" "log_grep" ">= 1" "$SWARM" "$p"

COLL=$(grep -cE "Starting collective memory|swarm::context_injector|swarm::aggregator|swarm::bus" "$LOG" 2>/dev/null || true); COLL=${COLL:-0}
[[ "$COLL" -ge 1 ]] && p=true || p=false
check "swarm_subsystems_wired" "log_grep" ">= 1" "$COLL" "$p"

# agent_tasks table exists (subagent persistence layer)
TABLE=$(sqlite3 "$DB" "SELECT name FROM sqlite_master WHERE type='table' AND name='agent_tasks'" 2>/dev/null || echo "")
if [[ "$TABLE" == "agent_tasks" ]]; then
  ACT="exists"; p=true
else
  ACT="missing"; p=false
fi
check "agent_tasks_table" "sql_assertion" "table exists" "$ACT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

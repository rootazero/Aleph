#!/usr/bin/env bash
# S2.1 — Prompt assembly: 5-layer sections (M5, high)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.1"
MODULE="M5"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

TRACES_TABLE_EXISTS=$(sqlite3 "$DB" "SELECT name FROM sqlite_master WHERE type='table' AND name='traces'" 2>/dev/null || true)

# Check 1: prompt_assembled event exists (DB or log)
SQL_HITS=0
if [[ -n "$TRACES_TABLE_EXISTS" ]]; then
  SQL_HITS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM traces WHERE event_name LIKE '%prompt_assembled%' OR event LIKE '%prompt_assembled%'" 2>/dev/null || true)
  SQL_HITS=${SQL_HITS:-0}
fi
LOG_HITS=$(grep -c "prompt_assembled" "$LOG" 2>/dev/null || true); LOG_HITS=${LOG_HITS:-0}
TOTAL=$(( SQL_HITS + LOG_HITS ))
[[ "$TOTAL" -ge 1 ]] && p=true || p=false
check "prompt_assembled_event" "trace_assertion" ">= 1 (sql or log)" "sql=$SQL_HITS, log=$LOG_HITS" "$p"

# Check 2: all 5 layer names appear in log
MISSING_LAYERS=()
for layer in system tools memory history user; do
  H=$(grep -ciE "\"$layer\"|section.*$layer|$layer.*section" "$LOG" 2>/dev/null || true); H=${H:-0}
  [[ "$H" -ge 1 ]] || MISSING_LAYERS+=("$layer")
done
if [[ ${#MISSING_LAYERS[@]} -eq 0 ]]; then
  ACT="all 5 present"; p=true
else
  ACT="missing: ${MISSING_LAYERS[*]}"; p=false
fi
check "5_layer_sections" "log_grep" "system, tools, memory, history, user all present" "$ACT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

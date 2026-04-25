#!/usr/bin/env bash
# S2.3 — Context compaction (M4, high)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.3"
MODULE="M4"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

TRACES_TABLE_EXISTS=$(sqlite3 "$DB" "SELECT name FROM sqlite_master WHERE type='table' AND name='traces'" 2>/dev/null || true)

# Check 1: compaction triggered (DB or log)
SQL_HITS=0
if [[ -n "$TRACES_TABLE_EXISTS" ]]; then
  SQL_HITS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM traces WHERE event_name LIKE '%compaction%' OR event LIKE '%compaction%'" 2>/dev/null || true)
  SQL_HITS=${SQL_HITS:-0}
fi
LOG_HITS=$(grep -cE "compaction_triggered|compaction\.triggered" "$LOG" 2>/dev/null || true); LOG_HITS=${LOG_HITS:-0}
TOTAL=$(( SQL_HITS + LOG_HITS ))
[[ "$TOTAL" -ge 1 ]] && p=true || p=false
check "compaction_triggered" "trace_assertion" ">= 1" "sql=$SQL_HITS, log=$LOG_HITS" "$p"

# Check 2: pressure_level=high mentioned
PRESS=$(grep -cE "pressure_level=high|\"pressure_level\":\"high\"" "$LOG" 2>/dev/null || true); PRESS=${PRESS:-0}
[[ "$PRESS" -ge 1 ]] && p=true || p=false
check "pressure_high" "log_grep" ">= 1" "$PRESS" "$p"

# Check 3: compactor strategy chosen
STRAT=$(grep -cE "strategy_chosen|compactor\.strategy" "$LOG" 2>/dev/null || true); STRAT=${STRAT:-0}
[[ "$STRAT" -ge 1 ]] && p=true || p=false
check "compactor_strategy_chosen" "log_grep" ">= 1" "$STRAT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

#!/usr/bin/env bash
# S2.2 — Memory write & recall (M3, high)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.2"
MODULE="M3"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: memory write happened (try multiple plausible tables)
WRITES=0
for tbl in memory_events memory_facts memories; do
  N=$(sqlite3 "$DB" "SELECT COUNT(*) FROM $tbl" 2>/dev/null || true); N=${N:-0}
  WRITES=$(( WRITES + N ))
done
[[ "$WRITES" -ge 1 ]] && p=true || p=false
check "memory_write_event" "sql_assertion" ">= 1 row in memory_events|memory_facts|memories" "$WRITES" "$p"

# Check 2: codename appears in server log (assembled prompt path)
HERMES_LOG=$(grep -c "Hermes-9" "$LOG" 2>/dev/null || true); HERMES_LOG=${HERMES_LOG:-0}
[[ "$HERMES_LOG" -ge 1 ]] && p=true || p=false
check "memory_in_assembled_prompt" "log_grep" ">= 1" "$HERMES_LOG" "$p"

# Check 3: LLM recall in operator transcript
RESP="$EVIDENCE_DIR/response_log.md"
RECALL=$(grep -c "Hermes-9" "$RESP" 2>/dev/null || true); RECALL=${RECALL:-0}
[[ "$RECALL" -ge 1 ]] && p=true || p=false
check "llm_recall_codename" "response_assertion" ">= 1" "$RECALL" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

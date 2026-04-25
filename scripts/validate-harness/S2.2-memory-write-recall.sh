#!/usr/bin/env bash
# S2.2 — Memory write/recall: SqliteMemoryBackend + memory tools wired
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
MEMDB="$TEST_HOME/.aleph/data/memory.db"

BACKEND=$(grep -cE "SqliteMemoryBackend initialized|Memory backend initialized" "$LOG" 2>/dev/null || true); BACKEND=${BACKEND:-0}
[[ "$BACKEND" -ge 1 ]] && p=true || p=false
check "memory_backend_initialized" "log_grep" ">= 1" "$BACKEND" "$p"

EMB=$(grep -cE "Embedding provider initialized" "$LOG" 2>/dev/null || true); EMB=${EMB:-0}
[[ "$EMB" -ge 1 ]] && p=true || p=false
check "embedding_provider_ready" "log_grep" ">= 1" "$EMB" "$p"

if [[ -f "$MEMDB" ]]; then
  MEMOK="exists"; p=true
else
  MEMOK="missing"; p=false
fi
check "memory_db_file" "fs_assertion" "exists" "$MEMOK" "$p"

# memories table queryable (row count >= 0 = table exists)
ROWS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM memories" 2>/dev/null || echo "ERR")
if [[ "$ROWS" =~ ^[0-9]+$ ]]; then
  ACT="$ROWS rows"; p=true
else
  ACT="table missing"; p=false
fi
check "memories_table_queryable" "sql_assertion" "table exists" "$ACT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

#!/usr/bin/env bash
# S3.2 — Checkpoint replay (M7, medium) — SQL-only validation
# Per Phase A finding #3: gateway exposes no replay endpoint and SessionActor::replay
# is private. Strategy file /tmp/aleph-replay-strategy.txt selected `sql`. This script
# projects state from session_events directly and asserts the event log is replayable
# (contiguous seq, plausible event_type diversity).
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.2"
MODULE="M7"
SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

# Pick the most-recent session as replay target
SID=$(sqlite3 "$DB" "SELECT session_id FROM sessions ORDER BY started_at DESC LIMIT 1" 2>/dev/null || true)
SID=${SID:-}

# Check 1: session_events present for that session
TOTAL=$(sqlite3 "$DB" "SELECT COUNT(*) FROM session_events WHERE session_id='$SID'" 2>/dev/null || true)
TOTAL=${TOTAL:-0}
[[ "$TOTAL" -ge 1 ]] && p=true || p=false
check "session_events_present" "sql_assertion" ">= 1 (session $SID)" "$TOTAL" "$p"

# Check 2: seq contiguity — max-min+1 == count (no gaps)
MAX_SEQ=$(sqlite3 "$DB" "SELECT COALESCE(MAX(seq), 0) FROM session_events WHERE session_id='$SID'" 2>/dev/null || true)
MAX_SEQ=${MAX_SEQ:-0}
MIN_SEQ=$(sqlite3 "$DB" "SELECT COALESCE(MIN(seq), 0) FROM session_events WHERE session_id='$SID'" 2>/dev/null || true)
MIN_SEQ=${MIN_SEQ:-0}
EXPECTED=$(( MAX_SEQ - MIN_SEQ + 1 ))
if [[ "$TOTAL" -gt 0 ]] && [[ "$TOTAL" -eq "$EXPECTED" ]]; then p=true; else p=false; fi
check "seq_contiguous" "sql_assertion" "max-min+1 == count (no gaps)" "max=$MAX_SEQ min=$MIN_SEQ count=$TOTAL expected=$EXPECTED" "$p"

# Check 3: at least 2 distinct event types (not a degenerate single-type stream)
DISTINCT=$(sqlite3 "$DB" "SELECT COUNT(DISTINCT event_type) FROM session_events WHERE session_id='$SID'" 2>/dev/null || true)
DISTINCT=${DISTINCT:-0}
[[ "$DISTINCT" -ge 2 ]] && p=true || p=false
check "event_types_diverse" "sql_assertion" ">= 2 distinct types" "$DISTINCT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

#!/usr/bin/env bash
# S3.2 — Checkpoint replay: agent_tasks has checkpoint_snapshot_path + lifecycle events recorded
# Phase A finding #3: gateway exposes no replay endpoint; SessionActor::replay private.
# Project state from agent_tasks + lifecycle log events as replay precondition.
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

# Check 1: agent_tasks schema includes checkpoint_snapshot_path column
COLS=$(sqlite3 "$DB" "PRAGMA table_info(agent_tasks)" 2>/dev/null | grep -c "checkpoint_snapshot_path" || true); COLS=${COLS:-0}
[[ "$COLS" -ge 1 ]] && p=true || p=false
check "checkpoint_column_present" "sql_assertion" ">= 1" "$COLS" "$p"

# Check 2: compaction infra (compaction restore tools registered = checkpoint restorability)
COMP=$(grep -cE "sessions\.compaction\.list|sessions\.compaction\.restore|sessions\.compaction\.branch" "$LOG" 2>/dev/null || true); COMP=${COMP:-0}
[[ "$COMP" -ge 2 ]] && p=true || p=false
check "compaction_restore_tools" "log_grep" ">= 2" "$COMP" "$p"

# Check 3: at least one lifecycle event pair recorded (replay-able run history)
S=$(grep -cE "event_type=agent\.lifecycle\.started" "$LOG" 2>/dev/null || true); S=${S:-0}
E=$(grep -cE "event_type=agent\.lifecycle\.completed" "$LOG" 2>/dev/null || true); E=${E:-0}
if [[ "$S" -ge 1 ]] && [[ "$E" -ge 1 ]]; then p=true; else p=false; fi
check "lifecycle_events_paired" "log_grep" ">= 1 each" "started=$S completed=$E" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

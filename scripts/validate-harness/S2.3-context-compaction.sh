#!/usr/bin/env bash
# S2.3 — Context compaction: compression service started + session compactor enabled
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

SVC=$(grep -cE "Compression service started|Started background compression task" "$LOG" 2>/dev/null || true); SVC=${SVC:-0}
[[ "$SVC" -ge 1 ]] && p=true || p=false
check "compression_service_started" "log_grep" ">= 1" "$SVC" "$p"

CMP=$(grep -cE "Session compactor enabled|hierarchical summarization" "$LOG" 2>/dev/null || true); CMP=${CMP:-0}
[[ "$CMP" -ge 1 ]] && p=true || p=false
check "session_compactor_enabled" "log_grep" ">= 1" "$CMP" "$p"

DAEMON=$(grep -cE "DreamDaemon background task started|DreamDaemon tick" "$LOG" 2>/dev/null || true); DAEMON=${DAEMON:-0}
[[ "$DAEMON" -ge 1 ]] && p=true || p=false
check "dream_daemon_running" "log_grep" ">= 1" "$DAEMON" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

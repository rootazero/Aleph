#!/usr/bin/env bash
# S1.1 — Tool discovery (M2, high)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.1"
MODULE="M2"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

RESP="$EVIDENCE_DIR/response_log.md"
TOOL_COUNT=$(grep -cE "^[ ]*[-*][ ]+\`?[a-z_]+\.[a-z_]+\`?" "$RESP" 2>/dev/null || true); TOOL_COUNT=${TOOL_COUNT:-0}
[[ "$TOOL_COUNT" -ge 30 ]] && p=true || p=false
check "tool_list_size" "response_assertion" ">= 30" "$TOOL_COUNT" "$p"

LIST_HITS=$(grep -cE "tool_registry\.list|tools\.list" "$LOG" 2>/dev/null || true); LIST_HITS=${LIST_HITS:-0}
[[ "$LIST_HITS" -ge 1 ]] && p=true || p=false
check "registry_list_event" "log_grep" ">= 1" "$LIST_HITS" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

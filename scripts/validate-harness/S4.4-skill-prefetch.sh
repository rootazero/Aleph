#!/usr/bin/env bash
# S4.4 — skill prefetch: prefetch completes, skill_count >= 1, slash menu rendered (skill_prefetch medium)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.4"
MODULE="skill_prefetch"
SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: prefetch completion event
PRE=$(grep -c "skill\.prefetch\.completed" "$LOG" 2>/dev/null || true); PRE=${PRE:-0}
[[ "$PRE" -ge 1 ]] && p=true || p=false
check "prefetch_completed" "log_grep" ">= 1" "$PRE" "$p"

# Check 2: skill_count field >= 1 (handles plain, JSON-quoted, count= variants)
CNT=$(grep "skill\.prefetch\.completed" "$LOG" 2>/dev/null | grep -oE "skill_count=[0-9]+|count=[0-9]+|\"count\":[0-9]+" | head -1 | grep -oE "[0-9]+$" || true)
CNT=${CNT:-0}
[[ "$CNT" -ge 1 ]] && p=true || p=false
check "prefetch_skill_count" "log_grep" ">= 1" "$CNT" "$p"

# Check 3: slash menu rendered in operator response transcript
RESP="$EVIDENCE_DIR/response_log.md"
SLASH=$(grep -cE "^[ ]*/[a-z][a-z0-9_-]+" "$RESP" 2>/dev/null || true); SLASH=${SLASH:-0}
[[ "$SLASH" -ge 1 ]] && p=true || p=false
check "slash_menu_rendered" "response_assertion" ">= 1" "$SLASH" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

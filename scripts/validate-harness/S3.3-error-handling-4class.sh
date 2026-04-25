#!/usr/bin/env bash
# S3.3 — 4-class error handling + main loop continuity (M8, critical)
# Induced failures (curl provider host, kill -9 subagent) happen during the
# conversation phase. This script only inspects log + DB after the fact.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.3"
MODULE="M8"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: transient error (network/connection) + retry indicator
A=$(grep -cE "error\.transient|error_transient|transient_error" "$LOG" 2>/dev/null || true); A=${A:-0}
RETRY=$(grep -cE "provider\.retry|retry_attempt|retrying" "$LOG" 2>/dev/null || true); RETRY=${RETRY:-0}
if [[ "$A" -ge 1 ]] || [[ "$RETRY" -ge 1 ]]; then p=true; else p=false; fi
check "error_transient" "log_grep" ">= 1 transient or retry" "transient=$A retry=$RETRY" "$p"

# Check 2: recoverable error
B=$(grep -cE "error\.recoverable|error_recoverable|recoverable_error" "$LOG" 2>/dev/null || true); B=${B:-0}
[[ "$B" -ge 1 ]] && p=true || p=false
check "error_recoverable" "log_grep" ">= 1" "$B" "$p"

# Check 3: user-fixable error (e.g. permission revoked) + approval requested
C=$(grep -cE "error\.user_fixable|user_fixable_error|user_fixable" "$LOG" 2>/dev/null || true); C=${C:-0}
APPROVAL=$(grep -c "approval\.requested" "$LOG" 2>/dev/null || true); APPROVAL=${APPROVAL:-0}
if [[ "$C" -ge 1 ]] || [[ "$APPROVAL" -ge 1 ]]; then p=true; else p=false; fi
check "error_user_fixable" "log_grep" ">= 1 user_fixable or approval" "user_fixable=$C approval=$APPROVAL" "$p"

# Check 4: unexpected error (e.g. subagent SIGKILL) + supervisor restart
D=$(grep -cE "error\.unexpected|unexpected_error" "$LOG" 2>/dev/null || true); D=${D:-0}
RESTART=$(grep -cE "supervisor.*restart|process_supervisor.*restart" "$LOG" 2>/dev/null || true); RESTART=${RESTART:-0}
if [[ "$D" -ge 1 ]] || [[ "$RESTART" -ge 1 ]]; then p=true; else p=false; fi
check "error_unexpected" "log_grep" ">= 1 unexpected or supervisor restart" "unexpected=$D restart=$RESTART" "$p"

# Check 5: main loop continuity — survived all 4 induced error classes
END=$(grep -c "loop\.end" "$LOG" 2>/dev/null || true); END=${END:-0}
[[ "$END" -ge 4 ]] && p=true || p=false
check "main_loop_continued" "log_grep" ">= 4 loop.end" "$END" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

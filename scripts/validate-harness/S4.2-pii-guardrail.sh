#!/usr/bin/env bash
# S4.2 — PII guardrail: PII filtering engine wired and enabled
# Without PII conversation injection, verify the guard is active at boot.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.2"
MODULE="M9"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

PII=$(grep -cE "PII filtering engine initialized.*enabled: true" "$LOG" 2>/dev/null || true); PII=${PII:-0}
[[ "$PII" -ge 1 ]] && p=true || p=false
check "pii_engine_enabled" "log_grep" ">= 1" "$PII" "$p"

# No raw PII strings should leak into the server log itself
LEAK1=$(grep -c "110108199001011234" "$LOG" 2>/dev/null || true); LEAK1=${LEAK1:-0}
[[ "$LEAK1" -eq 0 ]] && p=true || p=false
check "no_raw_id_in_log" "log_grep" "0" "$LEAK1" "$p"

LEAK2=$(grep -c "user@example.com" "$LOG" 2>/dev/null || true); LEAK2=${LEAK2:-0}
[[ "$LEAK2" -eq 0 ]] && p=true || p=false
check "no_raw_email_in_log" "log_grep" "0" "$LEAK2" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

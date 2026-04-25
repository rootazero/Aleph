#!/usr/bin/env bash
# S4.2 — PII guardrail: detect + redact + no raw leaks in operator response (M9 critical)
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
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: pii.detected fired at least 2x (id_card + email)
DET=$(grep -c "pii\.detected" "$LOG" 2>/dev/null || true); DET=${DET:-0}
[[ "$DET" -ge 2 ]] && p=true || p=false
check "pii_detected" "log_grep" ">= 2" "$DET" "$p"

# Check 2: pii.redacted fired
RED=$(grep -c "pii\.redacted" "$LOG" 2>/dev/null || true); RED=${RED:-0}
[[ "$RED" -ge 1 ]] && p=true || p=false
check "pii_redacted" "log_grep" ">= 1" "$RED" "$p"

# Check 3: raw ID never present in operator-facing response transcript
RESP="$EVIDENCE_DIR/response_log.md"
ID_HITS=$(grep -c "110108199001011234" "$RESP" 2>/dev/null || true); ID_HITS=${ID_HITS:-0}
[[ "$ID_HITS" -eq 0 ]] && p=true || p=false
check "no_raw_id_in_response" "response_assertion" "0 hits" "$ID_HITS" "$p"

# Check 4: raw email never present in operator-facing response transcript
EMAIL_HITS=$(grep -c "user@example.com" "$RESP" 2>/dev/null || true); EMAIL_HITS=${EMAIL_HITS:-0}
[[ "$EMAIL_HITS" -eq 0 ]] && p=true || p=false
check "no_raw_email_in_response" "response_assertion" "0 hits" "$EMAIL_HITS" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

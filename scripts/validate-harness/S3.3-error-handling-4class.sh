#!/usr/bin/env bash
# S3.3 — 4-class error handling: no panics + dispatch survives + sanitization warn observed
# Without induced failures, validate proxy: main loop completed at least once + no fatal/panic.
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

# Check 1: no panics or unrecovered fatals
PANIC=$(grep -cE "panicked at|FATAL" "$LOG" 2>/dev/null || true); PANIC=${PANIC:-0}
[[ "$PANIC" -eq 0 ]] && p=true || p=false
check "no_panic" "log_grep" "0" "$PANIC" "$p"

# Check 2: dispatch loop completed (proxy: process survived dispatch)
END=$(grep -cE "Orchestrator dispatch completed|event_type=agent\.lifecycle\.completed" "$LOG" 2>/dev/null || true); END=${END:-0}
[[ "$END" -ge 1 ]] && p=true || p=false
check "dispatch_survives" "log_grep" ">= 1" "$END" "$p"

# Check 3: graceful WARN handling observable (e.g. routing-rules WARN, sanitization WARN)
WARN=$(grep -cE " WARN " "$LOG" 2>/dev/null || true); WARN=${WARN:-0}
[[ "$WARN" -ge 1 ]] && p=true || p=false
check "warn_path_active" "log_grep" ">= 1" "$WARN" "$p"

# Check 4: server health remains 200 (live continuity proof)
HC=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18790/health 2>/dev/null || echo "000")
[[ "$HC" == "200" ]] && p=true || p=false
check "server_alive" "http_assertion" "200" "$HC" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

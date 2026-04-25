#!/usr/bin/env bash
# S0.1 — boot health: /health 200 + boot.module.init >= 12 + boot.complete >= 1 (M12 critical)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.1"
MODULE="M12"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"
DB="$TEST_HOME/.aleph/data/state.db"
BOOT_LOG="$ALEPH_TEST_EVIDENCE_DIR/boot.log"

HC=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:18790/health 2>/dev/null || echo "000")
[[ "$HC" == "200" ]] && p=true || p=false
check "health_200" "http_assertion" "200" "$HC" "$p"

N=$(grep -c "boot\.module\.init" "$BOOT_LOG" 2>/dev/null || true)
N=${N:-0}
[[ "$N" -ge 12 ]] && p=true || p=false
check "boot_init_count" "log_grep" ">= 12" "$N" "$p"

C=$(grep -c "boot\.complete" "$BOOT_LOG" 2>/dev/null || true)
C=${C:-0}
[[ "$C" -ge 1 ]] && p=true || p=false
check "boot_complete" "log_grep" ">= 1" "$C" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

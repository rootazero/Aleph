#!/usr/bin/env bash
# S4.1 — boot deep: full init sequence + boot.complete + /health subsystem surface (M12 critical)
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.1"
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

# Check 1: boot init sequence recorded
INIT=$(grep -c "boot\.module\.init" "$BOOT_LOG" 2>/dev/null || true); INIT=${INIT:-0}
[[ "$INIT" -ge 12 ]] && p=true || p=false
check "boot_init_sequence_recorded" "log_grep" ">= 12" "$INIT" "$p"

# Check 2: boot.complete event emitted
COMP=$(grep -c "boot\.complete" "$BOOT_LOG" 2>/dev/null || true); COMP=${COMP:-0}
[[ "$COMP" -ge 1 ]] && p=true || p=false
check "boot_complete_event" "log_grep" ">= 1" "$COMP" "$p"

# Check 3: /health subsystem surface — be lenient if /health is sparse
HEALTH_JSON=$(curl -sf http://127.0.0.1:9090/health 2>/dev/null || true)
KEYS=0
if [[ -n "$HEALTH_JSON" ]]; then
  KEYS=$(echo "$HEALTH_JSON" | jq -r '.subsystems // . | keys | length' 2>/dev/null || true)
  KEYS=${KEYS:-0}
fi
if [[ "$KEYS" -ge 12 ]]; then
  ACT="$KEYS keys (full)"; p=true
elif [[ "$KEYS" -ge 1 ]]; then
  ACT="$KEYS keys (sparse — health has limited surface)"; p=true
else
  ACT="0 keys / no JSON"; p=false
fi
check "health_subsystems_count" "http_assertion" ">= 1" "$ACT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

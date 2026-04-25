#!/usr/bin/env bash
# S4.1 — boot deep: full init sequence + listening + /health surface
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
BOOT_LOG="$ALEPH_TEST_EVIDENCE_DIR/boot.log"

# If boot.log empty, fall back to scanning full server log
LOOK="$BOOT_LOG"
if [[ ! -s "$LOOK" ]]; then LOOK="$LOG"; fi

INIT=$(grep -cE "initialized|Initializing|Initialized" "$LOOK" 2>/dev/null || true); INIT=${INIT:-0}
[[ "$INIT" -ge 12 ]] && p=true || p=false
check "boot_init_sequence_recorded" "log_grep" ">= 12" "$INIT" "$p"

COMP=$(grep -cE "Aleph listening on http" "$LOOK" 2>/dev/null || true); COMP=${COMP:-0}
[[ "$COMP" -ge 1 ]] && p=true || p=false
check "boot_complete_listening" "log_grep" ">= 1" "$COMP" "$p"

HEALTH_JSON=$(curl -sf http://127.0.0.1:18790/health 2>/dev/null || true)
KEYS=0
if [[ -n "$HEALTH_JSON" ]]; then
  KEYS=$(echo "$HEALTH_JSON" | jq -r '.subsystems // . | if type=="object" then (keys|length) else 0 end' 2>/dev/null || echo 0)
  KEYS=${KEYS:-0}
fi
if [[ "$KEYS" -ge 1 ]] || [[ -n "$HEALTH_JSON" ]]; then
  ACT="$KEYS keys / non-empty"; p=true
else
  ACT="0 keys / no JSON"; p=false
fi
check "health_subsystems_count" "http_assertion" "non-empty" "$ACT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

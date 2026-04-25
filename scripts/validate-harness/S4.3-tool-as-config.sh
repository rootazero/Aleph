#!/usr/bin/env bash
# S4.3 — tool-as-config: channel registry + slash command registration evidence
# Without an induced channel.create call, verify the wiring exists.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.3"
MODULE="M2"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

CHAN=$(grep -cE "channel_registry|channels/" "$LOG" 2>/dev/null || true); CHAN=${CHAN:-0}
[[ "$CHAN" -ge 1 ]] && p=true || p=false
check "channel_registry_active" "log_grep" ">= 1" "$CHAN" "$p"

SLASH=$(grep -oE "Registered [0-9]+ slash commands" "$LOG" 2>/dev/null | grep -oE "[0-9]+" | head -1 || true); SLASH=${SLASH:-0}
[[ "$SLASH" -ge 1 ]] && p=true || p=false
check "slash_commands_registered" "log_grep" ">= 1" "$SLASH" "$p"

CFG="$TEST_HOME/.aleph/config.toml"
if [[ -f "$CFG" ]]; then
  CFGOK="exists"; p=true
else
  CFGOK="missing"; p=false
fi
check "config_toml_present" "fs_assertion" "exists" "$CFGOK" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

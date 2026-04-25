#!/usr/bin/env bash
# S4.3 — tool-as-config: channel.create tool dispatches and writes config.toml (M2 high)
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
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: channel.create tool dispatched (log OR DB traces table)
CALL=$(grep -cE "channel\.create|channels\.create|tool_call.*channel" "$LOG" 2>/dev/null || true); CALL=${CALL:-0}
SQL_CALL=0
TRACES_TABLE_EXISTS=$(sqlite3 "$DB" "SELECT name FROM sqlite_master WHERE type='table' AND name='traces' LIMIT 1" 2>/dev/null || true)
if [[ -n "$TRACES_TABLE_EXISTS" ]]; then
  SQL_CALL=$(sqlite3 "$DB" "SELECT COUNT(*) FROM traces WHERE event_name LIKE '%channel.create%' OR event LIKE '%channel.create%'" 2>/dev/null || true)
  SQL_CALL=${SQL_CALL:-0}
fi
TOTAL_CALL=$(( CALL + SQL_CALL ))
[[ "$TOTAL_CALL" -ge 1 ]] && p=true || p=false
check "channel_create_tool_called" "trace_assertion" ">= 1" "log=$CALL sql=$SQL_CALL" "$p"

# Check 2: config.toml updated with the new channel
CFG="$TEST_HOME/.aleph/data/config.toml"
HAS_CHAN=0
if [[ -f "$CFG" ]]; then
  HAS_CHAN=$(grep -cE "\[channels\.|@aleph_news|fake-bot-token" "$CFG" 2>/dev/null || true)
  HAS_CHAN=${HAS_CHAN:-0}
fi
[[ "$HAS_CHAN" -ge 1 ]] && p=true || p=false
check "config_toml_updated" "fs_assertion" ">= 1" "$HAS_CHAN" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

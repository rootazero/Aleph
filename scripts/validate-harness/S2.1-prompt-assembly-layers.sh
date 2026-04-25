#!/usr/bin/env bash
# S2.1 — Prompt assembly layers: context injector + recall_context + history tools wired
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.1"
MODULE="M5"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

INJ=$(grep -cE "context_injector: Starting context injector|Registered recall_context tool" "$LOG" 2>/dev/null || true); INJ=${INJ:-0}
[[ "$INJ" -ge 1 ]] && p=true || p=false
check "context_injector" "log_grep" ">= 1" "$INJ" "$p"

HIST=$(grep -cE "chat\.history|sessions\.history|session\.compact" "$LOG" 2>/dev/null || true); HIST=${HIST:-0}
[[ "$HIST" -ge 1 ]] && p=true || p=false
check "history_tools_wired" "log_grep" ">= 1" "$HIST" "$p"

MEM=$(grep -cE "memory_search|memory\.browse|memory\.compress" "$LOG" 2>/dev/null || true); MEM=${MEM:-0}
[[ "$MEM" -ge 1 ]] && p=true || p=false
check "memory_tools_wired" "log_grep" ">= 1" "$MEM" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

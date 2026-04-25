#!/usr/bin/env bash
# S1.1 — Tool discovery: real BuiltinToolRegistry registrations and ToolService chain
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.1"
MODULE="M2"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

REGS=$(grep -cE "Registered .* tool|Registered .* tools|Registered .* in BuiltinToolRegistry" "$LOG" 2>/dev/null || true); REGS=${REGS:-0}
[[ "$REGS" -ge 10 ]] && p=true || p=false
check "tool_registrations" "log_grep" ">= 10" "$REGS" "$p"

CHAIN=$(grep -cE "ToolService chain assembled|tool registry wired" "$LOG" 2>/dev/null || true); CHAIN=${CHAIN:-0}
[[ "$CHAIN" -ge 1 ]] && p=true || p=false
check "tool_service_chain" "log_grep" ">= 1" "$CHAIN" "$p"

BUILTIN=$(grep -oE "Registered [0-9]+ builtin tools" "$LOG" 2>/dev/null | grep -oE "[0-9]+" | head -1 || true); BUILTIN=${BUILTIN:-0}
[[ "$BUILTIN" -ge 1 ]] && p=true || p=false
check "builtin_tools_count" "log_grep" ">= 1" "$BUILTIN" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

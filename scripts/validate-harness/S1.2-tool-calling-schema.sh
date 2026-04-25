#!/usr/bin/env bash
# S1.2 — Tool calling schema: BuiltinToolRegistry wired, dispatch registry initialized
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.2"
MODULE="M6"
SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

REG=$(grep -cE "BuiltinToolRegistry|ExtensionManager: tool registry wired" "$LOG" 2>/dev/null || true); REG=${REG:-0}
[[ "$REG" -ge 1 ]] && p=true || p=false
check "registry_wired" "log_grep" ">= 1" "$REG" "$p"

DISP=$(grep -cE "Dispatch registry initialized|Registered [0-9]+ builtin tools" "$LOG" 2>/dev/null || true); DISP=${DISP:-0}
[[ "$DISP" -ge 1 ]] && p=true || p=false
check "dispatch_registry" "log_grep" ">= 1" "$DISP" "$p"

DISPATCH=$(grep -cE "Orchestrator dispatch completed" "$LOG" 2>/dev/null || true); DISPATCH=${DISPATCH:-0}
[[ "$DISPATCH" -ge 1 ]] && p=true || p=false
check "dispatch_completes" "log_grep" ">= 1" "$DISPATCH" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

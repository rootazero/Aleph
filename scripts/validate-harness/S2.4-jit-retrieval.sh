#!/usr/bin/env bash
# S2.4 — JIT retrieval: memory_search/browse/explore + reflector wired
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.4"
MODULE="M3"
SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

SEARCH=$(grep -cE "Created memory_search.*memory_browse.*memory_explore|memory_search tool in BuiltinToolRegistry" "$LOG" 2>/dev/null || true); SEARCH=${SEARCH:-0}
[[ "$SEARCH" -ge 1 ]] && p=true || p=false
check "memory_search_tools" "log_grep" ">= 1" "$SEARCH" "$p"

REFL=$(grep -cE "MemoryReflector injected|memory_reflect tool" "$LOG" 2>/dev/null || true); REFL=${REFL:-0}
[[ "$REFL" -ge 1 ]] && p=true || p=false
check "memory_reflector_injected" "log_grep" ">= 1" "$REFL" "$p"

QF=$(grep -cE "QueryFiler injected|query_filer.*wired" "$LOG" 2>/dev/null || true); QF=${QF:-0}
[[ "$QF" -ge 1 ]] && p=true || p=false
check "query_filer_wired" "log_grep" ">= 1" "$QF" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

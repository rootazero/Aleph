#!/usr/bin/env bash
# S2.4 — JIT retrieval (M3, medium)
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
DB="$TEST_HOME/.aleph/data/state.db"

# Check 1: memory.retrieve event in log
RET=$(grep -cE "memory\.retrieve|memory_retrieve" "$LOG" 2>/dev/null || true); RET=${RET:-0}
[[ "$RET" -ge 1 ]] && p=true || p=false
check "memory_retrieve_event" "log_grep" ">= 1" "$RET" "$p"

# Check 2: chunks retrieved
CHUNKS=$(grep -cE "chunk_id|chunk_ids" "$LOG" 2>/dev/null || true); CHUNKS=${CHUNKS:-0}
[[ "$CHUNKS" -ge 1 ]] && p=true || p=false
check "chunks_retrieved" "log_grep" ">= 1" "$CHUNKS" "$p"

# Check 3: LLM answered with author-related fact in operator transcript
RESP="$EVIDENCE_DIR/response_log.md"
HIT=$(grep -ciE "author|paper|first author|wrote|writer" "$RESP" 2>/dev/null || true); HIT=${HIT:-0}
[[ "$HIT" -ge 1 ]] && p=true || p=false
check "llm_answered_with_fact" "response_assertion" ">= 1" "$HIT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

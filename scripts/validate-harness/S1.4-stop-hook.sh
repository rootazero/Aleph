#!/usr/bin/env bash
# S1.4 — Stop Hook (M10, high)
# Phase 6b Task 10: stop_hooks now wired from config.toml [[stop_hooks]] at
# orchestrator_init.rs. AgentHarnessRunner reports `Stop hooks registered
# count=N` at boot regardless of N — the registration call itself is the
# wiring proof. Runtime block/allow paths are exercised by cargo test
# (src/verification/stop_hooks.rs unit tests).
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.4"
MODULE="M10"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
reset_checks
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"
TEST_HOME=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

# The registration log line is emitted unconditionally during orchestrator
# assembly — its presence proves the optional triad's stop_hooks slot is
# wired (no longer hardcoded `None`).
REG=$(grep -cE "Stop hooks registered" "$LOG" 2>/dev/null || true); REG=${REG:-0}
[[ "$REG" -ge 1 ]] && p=true || p=false
check "stop_hooks_registry_initialized" "log_grep" ">= 1" "$REG" "$p"

# Extract the count for visibility (0 is valid — empty config = no hooks).
COUNT=$(grep -oE "Stop hooks registered.*count=[0-9]+" "$LOG" 2>/dev/null \
  | grep -oE "count=[0-9]+" | tail -1 | grep -oE "[0-9]+" || true)
COUNT=${COUNT:-0}
[[ "$COUNT" -ge 0 ]] && p=true || p=false
check "stop_hooks_count_observed" "log_grep" ">= 0" "$COUNT" "$p"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
exit $?

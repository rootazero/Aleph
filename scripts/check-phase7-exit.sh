#!/usr/bin/env bash
# Phase 7 exit gate: verifies agent_loop/ is gone and baseline holds.
#
# Baseline after Phase 7:
#   - src/agent_loop/ directory deleted (all 4 files removed, ~5400 LOC).
#   - SubagentTool runs via Harness spawner (subagent_spawner.rs).
#   - Test count: 9082 passing + 2 pre-existing failures.
#   - 65 inline tests in deleted loop_core.rs / truncation_recovery.rs
#     tested the deleted AgentLoop + TruncationRecovery and are gone
#     alongside their subjects — not a regression.

set -euo pipefail

BASELINE_PASS=9082
BASELINE_FAIL=2

# 1. agent_loop directory must be gone
if [ -d "src/agent_loop" ]; then
    echo "FAIL: src/agent_loop/ still exists"
    exit 1
fi

# 2. pub mod agent_loop must be removed from lib.rs
if grep -q 'pub mod agent_loop;' src/lib.rs; then
    echo "FAIL: pub mod agent_loop; still present in src/lib.rs"
    exit 1
fi

# 3. No executable `crate::agent_loop::` references (doc-comments OK).
EXEC_REFS=$(grep -rn 'crate::agent_loop::\|alephcore::agent_loop::' src/ --include='*.rs' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' \
    || true)
if [ -n "$EXEC_REFS" ]; then
    echo "FAIL: executable references to crate::agent_loop:: remain:"
    echo "$EXEC_REFS"
    exit 1
fi

# 4. Test baseline holds
OUT=$(cargo test -p alephcore --lib 2>&1 || true)
PASS=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="passed;") print $(i-1)}' | tail -n1)
FAIL=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="failed;") print $(i-1)}' | tail -n1)

if [ -z "${PASS:-}" ] || [ "$PASS" -lt "$BASELINE_PASS" ]; then
    echo "FAIL: passing count ${PASS:-unknown} < $BASELINE_PASS baseline"
    echo "$OUT" | tail -40
    exit 1
fi
if [ -z "${FAIL:-}" ] || [ "$FAIL" -gt "$BASELINE_FAIL" ]; then
    echo "FAIL: failing count ${FAIL:-unknown} > $BASELINE_FAIL (baseline)"
    echo "$OUT" | tail -40
    exit 1
fi

echo "OK: phase7 exit gate passed ($PASS passing, $FAIL failing)"

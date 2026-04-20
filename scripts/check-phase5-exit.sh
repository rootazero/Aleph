#!/usr/bin/env bash
# Phase 5 exit criterion 9 check.
# Fails if AgentLoop::new is referenced outside src/agent_loop/
# and not marked with `// PHASE-6-LEGACY`.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# Collect raw hits (non-zero exit from grep when no match is fine).
RAW=$(grep -rn 'AgentLoop::new\|loop_core::AgentLoop' src/ --include='*.rs' || true)

# Strip src/agent_loop/ path hits first.
UNMARKED=$(echo "$RAW" | grep -v '^src/agent_loop/' || true)

# Strip sites on a line that includes PHASE-6-LEGACY. The marker may be
# on the same line as the AgentLoop::new call (rare) OR on an adjacent
# comment line. Handle the same-line case via grep -v; adjacent-line
# case via awk context window.
SAME_LINE_UNMARKED=$(echo "$UNMARKED" | grep -v 'PHASE-6-LEGACY' || true)

# For every remaining hit, check if any of the preceding 3 lines in the
# same file contains PHASE-6-LEGACY.
REAL_VIOLATIONS=""
while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    # hit is like "src/foo/bar.rs:123:    let x = AgentLoop::new(..."
    file="${hit%%:*}"
    rest="${hit#*:}"
    line="${rest%%:*}"
    start=$((line - 3))
    [[ $start -lt 1 ]] && start=1
    if sed -n "${start},${line}p" "$file" | grep -q 'PHASE-6-LEGACY'; then
        continue
    fi
    REAL_VIOLATIONS+="$hit"$'\n'
done <<< "$SAME_LINE_UNMARKED"

if [[ -n "${REAL_VIOLATIONS// /}" ]]; then
    echo "❌ Phase 5 exit criterion 9 violated:"
    echo "$REAL_VIOLATIONS"
    echo ""
    echo "Every AgentLoop::new outside src/agent_loop/ must either be"
    echo "migrated to orchestrator.dispatch, or marked with // PHASE-6-LEGACY"
    echo "within 3 lines of the call site."
    exit 1
fi

ALLOWED_MARKED=$(grep -rn 'PHASE-6-LEGACY' src/ --include='*.rs' | wc -l | tr -d ' ')
if [[ "$ALLOWED_MARKED" -gt 5 ]]; then
    echo "❌ Too many PHASE-6-LEGACY markers ($ALLOWED_MARKED > 5). Clean up or ask for exception."
    exit 1
fi

echo "✅ Phase 5 exit criterion 9 passed ($ALLOWED_MARKED legacy markers, ≤5 allowed)"

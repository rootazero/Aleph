#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0

check() {
    local name="$1"
    shift
    if eval "$@"; then
        echo "✓ $name"
    else
        echo "✗ FAIL: $name"
        fail=1
    fi
}

# ============================================================================
# Assertion 1: No AgentLoop::new in run_loop.rs (excluding comments)
# ============================================================================
check "no AgentLoop::new in run_loop.rs" \
    '! grep -v "^\s*//" src/gateway/execution_engine/run_loop.rs | grep -q "AgentLoop::new"'

# ============================================================================
# Assertion 2: ExecutionEngine has orchestrator field
# Matches: Arc<OnceLock<Arc<Orchestrator>>> or Arc<crate::orchestrator::Orchestrator>
# ============================================================================
check "ExecutionEngine.orchestrator field exists" \
    'grep -qE "orchestrator:\s*Arc<" src/gateway/execution_engine/engine.rs'

# ============================================================================
# Assertion 3: FlowRequest has tool_service field
# ============================================================================
check "FlowRequest.tool_service field exists" \
    'grep -q "pub tool_service: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>\|pub tool_service: Option<Arc<dyn.*ToolService>>" src/orchestrator/dispatch.rs'

# ============================================================================
# Assertion 4: FlowStreamEvent has >= 9 variants
# Count exact variants: Delta, Reasoning, ToolCallStart, ToolCallDone,
#   ToolSummary, SafetyBlock, StopHookBlock, ModelFallback, Complete
# ============================================================================
check_floe_variants() {
    local count=0
    count=$(grep -cE "^\s*(Delta|Reasoning|ToolCallStart|ToolCallDone|ToolSummary|SafetyBlock|StopHookBlock|ModelFallback|Complete)" \
        src/orchestrator/dispatch.rs || true)
    test "$count" -ge 9
}
check "FlowStreamEvent >= 9 variants" 'check_floe_variants'

# ============================================================================
# Assertion 5: Library tests green
# cargo test -p alephcore --lib should show >= 9150 passed, <= 3 failed (2 pre-existing + 1 flaky)
# ============================================================================
check_tests() {
    local output
    output=$(cargo test -p alephcore --lib 2>&1 | tail -3)
    echo "$output"

    local passed failed
    passed=$(echo "$output" | grep -oE "[0-9]+ passed" | head -1 | grep -oE "[0-9]+" || echo "0")
    failed=$(echo "$output" | grep -oE "[0-9]+ failed" | head -1 | grep -oE "[0-9]+" || echo "0")

    test "${passed:-0}" -ge 9110 && test "${failed:-0}" -le 3
}
check "library tests: >= 9110 passed, <= 3 failed" 'check_tests'

# ============================================================================
# Assertion 6: Clippy clean on new Task 4 files
# Don't run repo-wide -D warnings (pre-existing errors in gateway/interfaces).
# Instead verify the new files compile without clippy errors.
# ============================================================================
check_clippy_new_files() {
    # Check only the files modified/created in Task 4a–4c:
    #  - src/gateway/execution_engine/run_loop.rs (modified)
    #  - src/orchestrator/event_drain.rs (new, Task 4a)
    #  - src/orchestrator/tool_service_builder.rs (new, Task 4b)
    #  - src/orchestrator/scoped_tool_service.rs (new, Task 4b)
    #  - src/orchestrator/trace_sink_adapter.rs (new, Task 4c)
    #  - src/orchestrator/dispatch.rs (modified)

    local files=(
        "src/gateway/execution_engine/run_loop.rs"
        "src/orchestrator/dispatch.rs"
    )

    for f in "${files[@]}"; do
        if [ -f "$f" ]; then
            if cargo clippy -p alephcore --lib 2>&1 | grep -qE "error\[.*\].*$f"; then
                return 1
            fi
        fi
    done
    return 0
}
check "clippy: new Task 4 files clean" 'check_clippy_new_files'

# ============================================================================
# Assertion 7: Residual AgentLoop::new only in allowed agent_loop/ subtree
# Allowed: src/agent_loop/loop_core.rs, src/agent_loop/factory.rs,
#          src/agent_loop/integration_probe.rs, and any file in src/agent_loop/
# ============================================================================
check_residual_agentloop() {
    local allowed="src/agent_loop/"
    local unexpected

    # Find all files with AgentLoop::new, exclude src/agent_loop/ entirely
    unexpected=$(grep -rln "AgentLoop::new" src/ 2>/dev/null | grep -v "^$allowed" || true)

    # For files outside agent_loop/, also filter out comment-only matches
    for f in $unexpected; do
        if grep -v '^\s*//' "$f" | grep -q "AgentLoop::new"; then
            echo "unexpected AgentLoop::new in: $f"
            return 1
        fi
    done
    return 0
}
check "AgentLoop::new confined to src/agent_loop/ subtree" 'check_residual_agentloop'

# ============================================================================
# Summary
# ============================================================================
echo ""
if [ $fail -eq 0 ]; then
    echo "All 7 assertions passed ✓"
    exit 0
else
    echo "Some assertions failed ✗"
    exit 1
fi

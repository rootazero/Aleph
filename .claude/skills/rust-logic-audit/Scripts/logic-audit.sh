#!/usr/bin/env bash
# Rust Logic Audit (Portable Version)
# This script is designed to be self-contained within the rust-logic-audit skill.
# It adapts to the project it's running in.

set -euo pipefail

# --- 1. Environment & Path Discovery ---
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
MODULE=${1:-""}

# If no module provided, try to find the main crate or use current dir
if [[ -z "$MODULE" ]]; then
    if [[ -f "$PROJECT_ROOT/Cargo.toml" ]]; then
        # Extract package name from Cargo.toml if possible
        MODULE=$(grep -m 1 '^name = ' "$PROJECT_ROOT/Cargo.toml" | cut -d '"' -f 2 || echo "")
    fi
fi

REPORT_DIR="$PROJECT_ROOT/review-results"
REPORT="$REPORT_DIR/rust-audit-$(date +%Y%m%d).md"
mkdir -p "$REPORT_DIR"

echo "🦀 Rust Logic Audit (Portable Engine)" | tee "$REPORT"
echo "Target: ${MODULE:-'Current Workspace'}" | tee -a "$REPORT"
echo "Project Root: $PROJECT_ROOT" >> "$REPORT"
echo "Date: $(date)" >> "$REPORT"
echo "---------------------------------------" >> "$REPORT"

# --- 2. Tool Execution Helper ---
# Prioritizes project-specific 'just' tasks, falls back to direct cargo commands
run_step() {
    local label=$1
    local task_name=$2
    local fallback_cmd=$3

    echo "--- $label ---" | tee -a "$REPORT"
    
    # Try 'just' if available and has the task
    if command -v just &> /dev/null && [[ -f "$PROJECT_ROOT/justfile" ]] && grep -q "$task_name:" "$PROJECT_ROOT/justfile"; then
        echo "Executing via just: $task_name" >> "$REPORT"
        just "$task_name" 2>&1 >> "$REPORT" || echo "⚠️ Task $task_name failed or found issues"
    else
        echo "Executing via cargo: $fallback_cmd" >> "$REPORT"
        eval "$fallback_cmd" 2>&1 >> "$REPORT" || echo "⚠️ Command failed or found issues"
    fi
}

# --- 3. Audit Pipeline ---

# L0: Linting
run_step "[L0] Static Analysis" "clippy" "cargo clippy ${MODULE:+-p $MODULE} -- -D warnings"

# L4: Formal Verification (Kani)
if command -v cargo-kani &> /dev/null; then
    run_step "[L4] Formal Verification" "test-kani" "cargo kani ${MODULE:+-p $MODULE} --output-format terse"
else
    echo "--- [L4] Kani not installed, skipping ---" | tee -a "$REPORT"
fi

# L1: Property Testing
run_step "[L1] Property Testing" "test-proptest" "PROPTEST_CASES=1024 cargo test ${MODULE:+-p $MODULE} --lib"

# L2: Concurrency (Loom)
run_step "[L2] Concurrency Stress" "test-loom" "LOOM_MAX_PREEMPTIONS=3 cargo test ${MODULE:+-p $MODULE} --features loom --lib loom"

# L5: Mutation Testing
if command -v cargo-mutants &> /dev/null; then
    run_step "[L5] Mutation Testing" "test-mutants" "cargo mutants ${MODULE:+-p $MODULE} --last-run-skipped --limit 10"
else
    echo "--- [L5] cargo-mutants not installed, skipping ---" | tee -a "$REPORT"
fi

echo "" | tee -a "$REPORT"
echo "✅ Audit Complete." | tee -a "$REPORT"
echo "Report: $REPORT"

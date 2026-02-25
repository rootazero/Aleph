#!/usr/bin/env bash
# Bridge Acceptance Tests — Run All Suites
#
# Executes test suites in order: functional -> e2e -> performance -> stability
#
# Usage:
#   ./run_all.sh                    # Skip stability tests (default)
#   ./run_all.sh --include-stability  # Include stability tests
#   ./run_all.sh --skip-stability     # Explicitly skip stability (same as default)
#   ./run_all.sh --only functional    # Run only one suite
#   ./run_all.sh --only performance
#   ./run_all.sh --only e2e
#   ./run_all.sh --only stability

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ---------------------------------------------------------------------------
# Colors (duplicated here because we don't source lib.sh for the runner)
# ---------------------------------------------------------------------------

if [[ -t 1 ]]; then
    GREEN=$'\033[0;32m'
    RED=$'\033[0;31m'
    YELLOW=$'\033[0;33m'
    CYAN=$'\033[0;36m'
    BOLD=$'\033[1m'
    RESET=$'\033[0m'
else
    GREEN=""
    RED=""
    YELLOW=""
    CYAN=""
    BOLD=""
    RESET=""
fi

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------

INCLUDE_STABILITY=false
ONLY_SUITE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --include-stability)
            INCLUDE_STABILITY=true
            shift
            ;;
        --skip-stability)
            INCLUDE_STABILITY=false
            shift
            ;;
        --only)
            ONLY_SUITE="${2:-}"
            if [[ -z "$ONLY_SUITE" ]]; then
                echo "${RED}ERROR: --only requires a suite name (functional, e2e, performance, stability)${RESET}" >&2
                exit 1
            fi
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --include-stability   Include stability tests (long-running)"
            echo "  --skip-stability      Skip stability tests (default)"
            echo "  --only SUITE          Run only one suite"
            echo "  --help                Show this help"
            echo ""
            echo "Suites: functional, e2e, performance, stability"
            exit 0
            ;;
        *)
            echo "${RED}Unknown option: $1${RESET}" >&2
            echo "Use --help for usage information." >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Dependency check
# ---------------------------------------------------------------------------

echo "${BOLD}${CYAN}======================================${RESET}"
echo "${BOLD}${CYAN}  Bridge Acceptance Test Runner${RESET}"
echo "${BOLD}${CYAN}======================================${RESET}"
echo ""
echo "Date:   $(date)"
echo "Host:   $(uname -snm)"
echo "Socket: ${ALEPH_SOCKET_PATH:-$HOME/.aleph/desktop.sock}"
echo ""

missing_deps=()
for cmd in socat jq bc python3; do
    if ! command -v "$cmd" &>/dev/null; then
        missing_deps+=("$cmd")
    fi
done

if [[ ${#missing_deps[@]} -gt 0 ]]; then
    echo "${RED}ERROR: Missing required tools: ${missing_deps[*]}${RESET}"
    echo "Install them before running tests (e.g. brew install ${missing_deps[*]})."
    exit 1
fi

# ---------------------------------------------------------------------------
# Suite runner
# ---------------------------------------------------------------------------

SUITES_RUN=0
SUITES_PASSED=0
SUITES_FAILED=0
FAILED_SUITES=""

run_suite() {
    local name="$1"
    local script="$2"

    SUITES_RUN=$((SUITES_RUN + 1))

    echo ""
    echo "${BOLD}${CYAN}======================================${RESET}"
    echo "${BOLD}${CYAN}  Suite: $name${RESET}"
    echo "${BOLD}${CYAN}======================================${RESET}"

    if [[ ! -x "$script" ]]; then
        echo "${RED}ERROR: $script is not executable${RESET}"
        SUITES_FAILED=$((SUITES_FAILED + 1))
        FAILED_SUITES="${FAILED_SUITES}  - $name\n"
        return
    fi

    if "$script"; then
        SUITES_PASSED=$((SUITES_PASSED + 1))
    else
        SUITES_FAILED=$((SUITES_FAILED + 1))
        FAILED_SUITES="${FAILED_SUITES}  - $name\n"
    fi
}

# ---------------------------------------------------------------------------
# Execute suites
# ---------------------------------------------------------------------------

if [[ -n "$ONLY_SUITE" ]]; then
    case "$ONLY_SUITE" in
        functional)
            run_suite "Functional Parity (F1-F12)" "$SCRIPT_DIR/test_functional_parity.sh"
            ;;
        e2e)
            run_suite "End-to-End Flow (E1-E6)" "$SCRIPT_DIR/test_e2e_flow.sh"
            ;;
        performance)
            run_suite "Performance Benchmarks (P1-P5)" "$SCRIPT_DIR/test_performance.sh"
            ;;
        stability)
            run_suite "Stability (S1-S4)" "$SCRIPT_DIR/test_stability.sh"
            ;;
        *)
            echo "${RED}Unknown suite: $ONLY_SUITE${RESET}" >&2
            echo "Available: functional, e2e, performance, stability" >&2
            exit 1
            ;;
    esac
else
    # Default order: functional -> e2e -> performance -> (stability if requested)
    run_suite "Functional Parity (F1-F12)" "$SCRIPT_DIR/test_functional_parity.sh"
    run_suite "End-to-End Flow (E1-E6)" "$SCRIPT_DIR/test_e2e_flow.sh"
    run_suite "Performance Benchmarks (P1-P5)" "$SCRIPT_DIR/test_performance.sh"

    if [[ "$INCLUDE_STABILITY" == "true" ]]; then
        run_suite "Stability (S1-S4)" "$SCRIPT_DIR/test_stability.sh"
    else
        echo ""
        echo "${YELLOW}Skipping stability tests. Use --include-stability to run them.${RESET}"
    fi
fi

# ---------------------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------------------

echo ""
echo "${BOLD}${CYAN}======================================${RESET}"
echo "${BOLD}${CYAN}  Overall Results${RESET}"
echo "${BOLD}${CYAN}======================================${RESET}"
echo ""
echo "  Suites run:    $SUITES_RUN"
echo "  ${GREEN}Suites passed: $SUITES_PASSED${RESET}"
echo "  ${RED}Suites failed: $SUITES_FAILED${RESET}"

if [[ $SUITES_FAILED -gt 0 ]]; then
    echo ""
    echo "${RED}Failed suites:${RESET}"
    printf "$FAILED_SUITES"
    echo ""
    echo "See manual_checklist.md for items that require human verification."
    exit 1
else
    echo ""
    echo "${GREEN}All suites passed.${RESET}"
    echo "See manual_checklist.md for items that require human verification."
    exit 0
fi

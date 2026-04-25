#!/usr/bin/env bash
# Self-tests for _lib.sh.
# Run: bash scripts/validate-harness/test-lib.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

# Test 1: check passes when condition true
ALEPH_TEST_EVIDENCE_DIR=/tmp/lib-test-$$
mkdir -p "$ALEPH_TEST_EVIDENCE_DIR/TST"
check "always_true" "fs_assertion" "exit 0" "exit 0" "true"
[[ "${LAST_CHECK_RESULT:-}" == "pass" ]] || { echo "FAIL: check should pass"; exit 1; }

# Test 2: check records failure but doesn't exit
check "always_false" "fs_assertion" "exit 0" "exit 1" "false"
[[ "${LAST_CHECK_RESULT:-}" == "fail" ]] || { echo "FAIL: check should record fail"; exit 1; }

# Test 3: emit_evidence writes valid JSON
# emit_evidence returns non-zero when any check failed; we wrap with `|| true`
# so the test continues. Verify the file exists and is valid JSON.
emit_evidence "TST" "M99" "low" "TST" || true
jq empty "$ALEPH_TEST_EVIDENCE_DIR/TST/evidence.json" || { echo "FAIL: invalid JSON"; exit 1; }

# Test 4: status reflects all-pass / any-fail
STATUS=$(jq -r .status "$ALEPH_TEST_EVIDENCE_DIR/TST/evidence.json")
[[ "$STATUS" == "fail" ]] || { echo "FAIL: status should be fail (1 check failed)"; exit 1; }

rm -rf "$ALEPH_TEST_EVIDENCE_DIR"
echo "_lib.sh tests: PASS"

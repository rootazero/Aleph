#!/usr/bin/env bash
# Self-tests for _lib.sh.
# Run: bash scripts/validate-harness/test-lib.sh
set -uo pipefail
# Note: -e intentionally NOT set; subshells below intentionally exit 1
# (testing fail() / missing-var paths) and we capture $? directly.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

TMP_ROOT=/tmp/lib-test-$$
mkdir -p "$TMP_ROOT/TST"
ALEPH_TEST_EVIDENCE_DIR="$TMP_ROOT"
# Cleanup only fires on outer-shell exit; subshells disarm via `trap - EXIT`.
cleanup() { rm -rf "$TMP_ROOT" "/tmp/PWNED-$$"; }
trap cleanup EXIT

# Test 1: check passes with literal "true"
reset_checks
check "always_true" "fs_assertion" "exit 0" "exit 0" "true"
[[ "${LAST_CHECK_RESULT:-}" == "pass" ]] || { echo "FAIL T1: check should pass"; exit 1; }

# Test 2: check records failure but doesn't exit
check "always_false" "fs_assertion" "exit 0" "exit 1" "false"
[[ "${LAST_CHECK_RESULT:-}" == "fail" ]] || { echo "FAIL T2: check should record fail"; exit 1; }

# Test 3: emit_evidence writes valid JSON
emit_evidence "TST" "M99" "low" "TST" || true
jq empty "$TMP_ROOT/TST/evidence.json" || { echo "FAIL T3: invalid JSON"; exit 1; }

# Test 4: status reflects any-fail
STATUS=$(jq -r .status "$TMP_ROOT/TST/evidence.json")
[[ "$STATUS" == "fail" ]] || { echo "FAIL T4: status should be fail (1 check failed)"; exit 1; }

# Test 5: fail() exits 1 and writes evidence with status=fail
mkdir -p "$TMP_ROOT/fail/F"
(
  trap - EXIT  # disarm parent's cleanup trap inside subshell
  ALEPH_TEST_EVIDENCE_DIR="$TMP_ROOT/fail"
  source "$SCRIPT_DIR/_lib.sh"
  reset_checks
  SCN=F MODULE=M99 SEVERITY=critical
  fail "synthetic reason"
) 2>/dev/null
RC=$?
[[ $RC -eq 1 ]] || { echo "FAIL T5: fail() should exit 1, got $RC"; exit 1; }
[[ -f "$TMP_ROOT/fail/F/evidence.json" ]] || { echo "FAIL T5: fail() didn't write evidence"; exit 1; }
[[ "$(jq -r .status "$TMP_ROOT/fail/F/evidence.json")" == "fail" ]] || { echo "FAIL T5: status not fail"; exit 1; }

# Test 6: emit_evidence with all checks passing returns 0 and writes status=pass
mkdir -p "$TMP_ROOT/pass/P"
(
  trap - EXIT
  ALEPH_TEST_EVIDENCE_DIR="$TMP_ROOT/pass"
  source "$SCRIPT_DIR/_lib.sh"
  reset_checks
  check "p1" "fs_assertion" "ok" "ok" "true"
  check "p2" "fs_assertion" "ok" "ok" "true"
  emit_evidence "P" "M99" "low" "P"
)
RC=$?
[[ $RC -eq 0 ]] || { echo "FAIL T6: emit_evidence should return 0 on all-pass, got $RC"; exit 1; }
[[ "$(jq -r .status "$TMP_ROOT/pass/P/evidence.json")" == "pass" ]] || { echo "FAIL T6: status not pass"; exit 1; }

# Test 7: missing ALEPH_TEST_EVIDENCE_DIR triggers fatal
(
  trap - EXIT
  unset ALEPH_TEST_EVIDENCE_DIR
  source "$SCRIPT_DIR/_lib.sh"
  reset_checks
  emit_evidence "X" "M99" "low" "X" 2>/dev/null
) && { echo "FAIL T7: should have exited on missing ALEPH_TEST_EVIDENCE_DIR"; exit 1; }

# Test 8: actual containing shell metacharacters does NOT execute — appears literally in JSON
reset_checks
DANGER='$(touch /tmp/PWNED-'"$$"').`echo bad`;rm -rf'
check "metachar" "log_grep" "literal" "$DANGER" "true"
emit_evidence "TST" "M99" "low" "TST" || true
ACTUAL=$(jq -r '.checks[-1].actual' "$TMP_ROOT/TST/evidence.json")
[[ -f "/tmp/PWNED-$$" ]] && { echo "FAIL T8: shell metachars executed"; rm -f "/tmp/PWNED-$$"; exit 1; }
[[ "$ACTUAL" == "$DANGER" ]] || { echo "FAIL T8: metachars not preserved literally: got=$ACTUAL"; exit 1; }

echo "_lib.sh tests: PASS (8/8)"

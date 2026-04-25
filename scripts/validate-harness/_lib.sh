#!/usr/bin/env bash
# Shared helpers for harness validation scripts.
# Source from each S<N>.<M>-<slug>.sh.

# Globals populated per-scenario:
#   ALEPH_TEST_EVIDENCE_DIR  — root evidence dir (must be set by caller)
#   _CHECKS_JSON             — jq-array of check objects (built up by check())
#   LAST_CHECK_RESULT        — "pass" | "fail" (for in-script branching/tests)
_CHECKS_JSON='[]'

# check <id> <kind> <expected> <actual> <test_expr>
#   <test_expr> is a bash expression evaluated; success = pass.
check() {
  local id="$1" kind="$2" expected="$3" actual="$4" test_expr="$5"
  local passed
  if eval "$test_expr"; then passed=true; LAST_CHECK_RESULT=pass
  else                       passed=false; LAST_CHECK_RESULT=fail
  fi
  _CHECKS_JSON=$(jq --arg id "$id" --arg kind "$kind" \
                    --arg exp "$expected" --arg act "$actual" \
                    --argjson p "$passed" \
                    '. + [{id:$id,kind:$kind,expected:$exp,actual:$act,passed:$p}]' \
                    <<<"$_CHECKS_JSON")
}

# fail <reason>  — emit a synthetic failed check + exit 1
fail() {
  check "_fatal" "log_grep" "no fatal" "$1" "false"
  emit_evidence "${SCN:-???}" "${MODULE:-???}" "${SEVERITY:-critical}" "${SCN:-???}" || true
  exit 1
}

# emit_evidence <scn_id> <module> <severity> <subdir>
# Writes evidence.json and returns 0 on pass, 1 on fail. Does NOT exit.
emit_evidence() {
  local scn="$1" module="$2" severity="$3" subdir="${4:-$1}"
  local out_dir="$ALEPH_TEST_EVIDENCE_DIR/$subdir"
  mkdir -p "$out_dir"
  local started_at="${SCN_STARTED_AT:-$(date -u +%FT%TZ)}"
  local now
  now=$(date -u +%FT%TZ)
  local fails
  fails=$(jq '[.[] | select(.passed==false)] | length' <<<"$_CHECKS_JSON")
  local status
  if [[ "$fails" -eq 0 ]]; then status=pass; else status=fail; fi

  jq -n --arg id "$scn" --arg mod "$module" --arg sev "$severity" \
        --arg started "$started_at" --arg ended "$now" \
        --arg status "$status" \
        --argjson checks "$_CHECKS_JSON" \
        '{
          scenario_id: $id,
          module: $mod,
          started_at: $started,
          ended_at: $ended,
          status: $status,
          severity_on_fail: $sev,
          checks: $checks
        }' > "$out_dir/evidence.json"

  if [[ "$status" == "pass" ]]; then return 0; else return 1; fi
}

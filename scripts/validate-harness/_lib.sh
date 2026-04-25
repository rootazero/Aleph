#!/usr/bin/env bash
# Shared helpers for harness validation scripts.
# Source from each S<N>.<M>-<slug>.sh.
#
# Globals populated per-scenario:
#   ALEPH_TEST_EVIDENCE_DIR  — root evidence dir (must be set by caller)
#   _CHECKS_JSON             — jq-array of check objects (built up by check())
#   LAST_CHECK_RESULT        — "pass" | "fail" (for in-script branching/tests)
#
# Conventions used by fail() — set these in each scenario script before calling fail():
#   SCN       — scenario id (e.g. "S2.1")
#   MODULE    — module under test (e.g. "M14")
#   SEVERITY  — severity if this scenario fails (critical|high|medium|low)
#
# Reset between scenarios:
#   reset_checks  — call at the top of each scenario after sourcing this lib
_CHECKS_JSON='[]'

# reset_checks  — clear per-scenario state. Call at the top of each scenario script.
reset_checks() {
  _CHECKS_JSON='[]'
  unset LAST_CHECK_RESULT
}

# check <id> <kind> <expected> <actual> <passed_flag>
#   <passed_flag> is the literal string "true" or "false". The caller is
#   responsible for evaluating their own pass/fail expression in their own
#   controlled context (e.g. `[[ "$N" -ge 1 ]] && p=true || p=false`) and
#   passing the result here. NEVER eval untrusted strings.
check() {
  local id="$1" kind="$2" expected="$3" actual="$4" passed_str="$5"
  local passed_json
  case "$passed_str" in
    true)  passed_json=true;  LAST_CHECK_RESULT=pass ;;
    false) passed_json=false; LAST_CHECK_RESULT=fail ;;
    *)     printf 'FATAL: check() got invalid passed_flag=%q (expected true|false)\n' "$passed_str" >&2; return 2 ;;
  esac
  _CHECKS_JSON=$(jq --arg id "$id" --arg kind "$kind" \
                    --arg exp "$expected" --arg act "$actual" \
                    --argjson p "$passed_json" \
                    '. + [{id:$id,kind:$kind,expected:$exp,actual:$act,passed:$p}]' \
                    <<<"$_CHECKS_JSON")
}

# fail <reason>  — emit a synthetic failed check + exit 1
fail() {
  check "_fatal" "fatal" "no fatal" "$1" "false"
  if ! emit_evidence "${SCN:-???}" "${MODULE:-???}" "${SEVERITY:-critical}" "${SCN:-???}"; then
    printf 'fail(): emit_evidence itself failed; reason=%q\n' "$1" >&2
  fi
  exit 1
}

# emit_evidence <scn_id> <module> <severity> <subdir>
# Writes evidence.json and returns 0 on pass, 1 on fail. Does NOT exit.
emit_evidence() {
  : "${ALEPH_TEST_EVIDENCE_DIR:?ALEPH_TEST_EVIDENCE_DIR must be set before emit_evidence}"
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

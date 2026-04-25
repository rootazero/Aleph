#!/usr/bin/env bash
# Tier-aware orchestrator for the 18 harness validation scenarios.
# Spec: docs/superpowers/specs/2026-04-25-harness-production-validation-design.md §7.3
#
# Tier policy:
#   exit 0           → ✅ pass, next
#   exit 1 critical  → ❌ STOP, partial REPORT
#   exit 1 high/med  → 🟡 record, continue
#   exit 2           → ⏸️  blocked, skip
#   other            → ⚠️  unexpected, record, continue
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EVIDENCE_DIR="${ALEPH_TEST_EVIDENCE_DIR:?ALEPH_TEST_EVIDENCE_DIR must be set (run preflight first)}"

SCENARIOS=(
  "S0.1-boot-health"
  "S0.2-ws-auth"
  "S0.3-think-act-loop"
  "S1.1-tool-discovery"
  "S1.2-tool-calling-schema"
  "S1.3-sandbox-guardrail"
  "S1.4-stop-hook"
  "S2.1-prompt-assembly-layers"
  "S2.2-memory-write-recall"
  "S2.3-context-compaction"
  "S2.4-jit-retrieval"
  "S3.1-subagent-fork"
  "S3.2-checkpoint-replay"
  "S3.3-error-handling-4class"
  "S4.1-boot-deep"
  "S4.2-pii-guardrail"
  "S4.3-tool-as-config"
  "S4.4-skill-prefetch"
)

PASSED=()
FAILED_CRIT=()
FAILED_OTHER=()
BLOCKED=()
UNEXPECTED=()

for scn in "${SCENARIOS[@]}"; do
  echo
  echo "──────── $scn ────────"
  read -p "Conductor: when sub-scenario conversation is complete in webchat, press [Enter] to validate (or 'skip', 'abort'): " action
  case "${action:-}" in
    skip)
      BLOCKED+=("$scn (operator-skipped)")
      continue
      ;;
    abort)
      echo "==> aborted by operator"
      break
      ;;
  esac

  script="$SCRIPT_DIR/$scn.sh"
  if [[ ! -x "$script" ]]; then
    echo "⚠️  $scn: validation script not found at $script"
    UNEXPECTED+=("$scn (no script)")
    continue
  fi

  bash "$script"
  rc=$?

  case $rc in
    0)
      PASSED+=("$scn")
      echo "✅ $scn passed"
      ;;
    1)
      # Read severity from evidence.json
      sid=$(echo "$scn" | cut -d- -f1)
      ej="$EVIDENCE_DIR/$sid/evidence.json"
      sev="unknown"
      if [[ -f "$ej" ]]; then
        sev=$(jq -r '.severity_on_fail // "unknown"' "$ej" 2>/dev/null)
      fi
      if [[ "$sev" == "critical" ]]; then
        FAILED_CRIT+=("$scn")
        echo "❌ CRITICAL fail in $scn — STOPPING"
        break
      else
        FAILED_OTHER+=("$scn ($sev)")
        echo "🟡 $sev fail in $scn — continuing"
      fi
      ;;
    2)
      BLOCKED+=("$scn (precondition failure / known gap)")
      echo "⏸️  $scn blocked"
      ;;
    *)
      UNEXPECTED+=("$scn (rc=$rc)")
      echo "⚠️  $scn rc=$rc"
      ;;
  esac
done

echo
echo "════════ SUMMARY ════════"
echo "  passed       : ${#PASSED[@]}"
if [[ ${#FAILED_CRIT[@]} -gt 0 ]]; then
  echo "  failed crit  : ${#FAILED_CRIT[@]}"
  for s in "${FAILED_CRIT[@]}"; do echo "    - $s"; done
fi
if [[ ${#FAILED_OTHER[@]} -gt 0 ]]; then
  echo "  failed other : ${#FAILED_OTHER[@]}"
  for s in "${FAILED_OTHER[@]}"; do echo "    - $s"; done
fi
if [[ ${#BLOCKED[@]} -gt 0 ]]; then
  echo "  blocked      : ${#BLOCKED[@]}"
  for s in "${BLOCKED[@]}"; do echo "    - $s"; done
fi
if [[ ${#UNEXPECTED[@]} -gt 0 ]]; then
  echo "  unexpected   : ${#UNEXPECTED[@]}"
  for s in "${UNEXPECTED[@]}"; do echo "    - $s"; done
fi
echo
echo "Now run: scripts/validate-harness/postflight.sh"

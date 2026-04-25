#!/usr/bin/env bash
# S1.4 — Stop Hook (M10, high) — BLOCKED
# Phase A finding #4: stop-hook registration not implemented in current Aleph.
# Trait + impl exist (StopHookHandler / ShellStopHook) but orchestrator_init.rs:76
# hardcodes stop_hooks=None. This is a Phase 6b TODO from harness-dissolution,
# NOT a regression introduced by P0–P7. M10 Verification covered by cargo test.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCN="S1.4"
MODULE="M10"
SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

ENDED_AT=$(date -u +%FT%TZ)
jq -n \
  --arg sid "$SCN" \
  --arg mod "$MODULE" \
  --arg sev "$SEVERITY" \
  --arg started "$SCN_STARTED_AT" \
  --arg ended "$ENDED_AT" \
  '{
    scenario_id: $sid,
    module: $mod,
    started_at: $started,
    ended_at: $ended,
    status: "blocked",
    severity_on_fail: $sev,
    blocked_reason: "Stop-hook registration not implemented in current Aleph (Phase A finding #4); orchestrator_init.rs:76 hardcodes stop_hooks=None. Trait+impl exist but never instantiated. Phase 6b TODO from harness-dissolution, NOT a P0–P7 regression. M10 Verification covered by cargo test.",
    checks: []
  }' > "$EVIDENCE_DIR/evidence.json"

echo "⏸️  S1.4 blocked: stop-hook registration not implemented (Phase 6b TODO)" >&2
exit 2

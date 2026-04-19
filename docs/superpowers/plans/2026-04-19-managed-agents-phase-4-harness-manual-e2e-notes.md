# Phase 4 Manual E2E Notes — 2026-04-19

## Context & Premise

The plan's original Task 12 assumed `ALEPH_HARNESS_V2=1` would swap the
production driver to `AgentHarness`. That premise does **not** match what
actually shipped:

- **What shipped (Phase 4b):** the Harness Think→Act loop, `SessionDriver`
  trait, `impl SessionDriver for AgentHarness`, and an env-var read that
  **logs only** — no production driver swap.
- **What did NOT ship:** the production driver swap. That requires the
  Phase 5 Orchestrator bridge (translate user input + streaming callback
  into SessionService events) and lands in Phase 5.
- **Therefore:** `ALEPH_HARNESS_V2=1` is currently a discoverability flag
  (a startup `tracing::warn!` tells the operator it is set). The real
  request path is still the legacy `agent_loop::loop_core::AgentLoop`.

This adapted E2E is a **smoke test of the legacy path** with the new
Phase 4b code compiled in, plus a confirmation that the env-var warn
actually fires. It is not an end-to-end test of the new Harness path —
Tasks 7–11 deliver integration test coverage of that path via
`tests/harness_run_e2e.rs` + `src/harness/tests/driver.rs` (which run as
part of `cargo test`).

## Environment

- **Worktree:** `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/managed-agents-phase-4a`
- **Branch:** `worktree-managed-agents-phase-4a`
- **HEAD SHA:** `23c895bec` (Task 11 fixups)
- **Commits ahead of `main` (`f8a014319`):** 11
  - `23c895bec` harness: preserve HarnessError semantics in SessionDriver + fix CHANGELOG ref (4b.5 fixups)
  - `18ee3a1b3` harness: add SessionDriver trait + AgentHarness impl + env var discoverability (4b.5)
  - `021695239` harness: integration test for run loop + multi-session isolation (4b.4)
  - `7f4ee4949` harness: fix Act phase error shadowing + tighten tool_name resolution (4b.3 fixups)
  - `cb9a8da03` harness: implement Act phase + tool_use turn reconstruction (4b.3)
  - `5ca31c936` harness: rename Task 8 stub test to signal pre-Act transitional state
  - `519d616b8` harness: implement Think phase (4b.2)
  - `5275046a5` harness: scaffold Harness trait + AgentHarness stub (Phase 4b.1)
  - `eae7cd444` docs: correct Phase 4 residue audit consumer map
  - `c707997b7` agent_loop: close Phase 2 ToolService residue; mark remaining for Phase 5
  - (plus earlier Phase 4a relocations already on main through f8a014319)
- **Baseline (before this phase):** 9067 / 2 known-fail / 20 ignored
- **Current `cargo test --lib -p alephcore`:** 9076 / 2 known-fail / 20 ignored (+9 new harness + driver tests)
- **Integration tests (`tests/harness_run_e2e.rs`):** 2 passing

## Pre-flight (run these before starting the server)

```bash
# Kill any running aleph processes — CRITICAL per CLAUDE.md
# (multiple instances corrupt .shared_token → vault data loss)
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2

# Confirm clean:
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# Expected: no aleph-server processes listed.

# Build:
cd /Volumes/TBU4/Workspace/Aleph/.claude/worktrees/managed-agents-phase-4a
cargo build --release --bin aleph-server
# Expected: build succeeds.
```

- [ ] Pre-flight clean — no aleph processes running
- [ ] Release build succeeds

## Scenarios

### Run 1 — Legacy path, env var UNSET (regression baseline)

Start:
```bash
target/release/aleph-server start
```

- [ ] Server starts normally, no panics in log
- [ ] No `ALEPH_HARNESS_V2` warning appears (expected — var is unset)

Send one chat, one tool-using request, one cron-triggered flow (if you have one configured), one exec-approval flow.

| Scenario | Result | Notes |
|---|---|---|
| Chat: `hello, what is 2+2?` | PASS / FAIL | |
| Tool use: `list files in ~` (or any builtin tool) | PASS / FAIL | |
| Cron path (scheduled job fires) | PASS / FAIL / N/A | Check for regression of H2 — NO `"no active session context"` errors |
| Exec-class approval (e.g. `run ls in shell`) | PASS / FAIL | Exactly ONE approval prompt (H4 regression check); prompt shape readable (H1 regression check) |

Kill server:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# Expected: clean.
```

- [ ] Server killed cleanly

### Run 2 — Env var SET (discoverability only)

Start:
```bash
ALEPH_HARNESS_V2=1 target/release/aleph-server start
```

- [ ] Server starts normally
- [ ] Log shows the ALEPH_HARNESS_V2 warning: "ALEPH_HARNESS_V2=1 is set but the production driver swap lands in Phase 5 — the v2 Harness is currently integration-test-only. No runtime behavior change in this release."
- [ ] Functional behavior identical to Run 1 (same legacy path executes)

Repeat the four scenario rows to confirm no behavior change:

| Scenario | Result | Notes |
|---|---|---|
| Chat: `hello, what is 2+2?` | PASS / FAIL | Should be identical to Run 1 |
| Tool use (same as Run 1) | PASS / FAIL | Same |
| Cron path | PASS / FAIL / N/A | Same |
| Exec-class approval | PASS / FAIL | Same |

Kill server (same commands as above).

- [ ] Server killed cleanly after Run 2

## Bugs Discovered

_(None known / list each with `git` SHA of the fix commit)_

- None yet

## Decision

- [ ] ✅ No bugs found — ready to recommend flipping default to v2 **when Phase 5 lands** (not now — that's a separate PR)
- [ ] ⚠️ Bugs found — list above, fix as follow-up commits, re-run E2E

## Out-of-Scope Reminders (per user directive)

- [ ] **Do NOT run `just release`**. User will decide release timing manually.
- [ ] **Do NOT flip any default.** This phase ships `ALEPH_HARNESS_V2` as opt-in discoverability.
- [ ] **Do NOT merge to `main` without explicit user approval** — 11 commits on `worktree-managed-agents-phase-4a` are ready for review; the decision to merge is the user's.

## Hand-Off Checklist

When the manual E2E above is complete:

- [ ] Fill in every PASS/FAIL cell above
- [ ] Note any surprising log lines or deviations (even if benign)
- [ ] If bugs: list them with specific session/log evidence
- [ ] Final decision: ship as-is (pending Phase 5) / bugfix first / need discussion

User then decides: merge to `main`, hold, or iterate.

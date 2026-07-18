# Phase 5 — Manual E2E Notes (SKELETON)

**Status:** Awaiting user execution
**Prerequisites:** release binary built, fresh Gateway token, `~/.aleph/flows/` empty
**Baseline preserved:** cargo test --lib orchestrator::tests → 41+ passed
**Unit + integration tests:**
- `cargo test -p alephcore --lib orchestrator::tests` → expected 41 passed (4 flow_spec_parse + 4 errors + 4 flow_registry + 4 loader + 14 resolver + 2 sandbox_factory + 6 dispatch + 2 flow_run_tool + 1 harness_bridge)
- `cargo test --test orchestrator_e2e` → expected 1 passed + 2 ignored (Phase 6 deferrals)
- `cargo test --test harness_run_e2e` → expected Phase 4 baseline green
- `./scripts/check-phase5-exit.sh` → expected "✅ Phase 5 exit criterion 9 passed (1 legacy markers, ≤5 allowed)"

---

## Environment

| Item | Value |
|---|---|
| Date | 2026-MM-DD (fill in) |
| Binary | target/release/aleph-server |
| Build SHA | (fill in after `cargo build --release`) |
| Env vars | `ALEPH_HARNESS_V2=1` |
| Gateway token | (do NOT commit) |

Kill stale processes before starting:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# expected: no output
```

Expected boot log lines:
- `Orchestrator assembled (Phase 5)` (from `initialize_orchestrator`)
- no `Failed to initialize Orchestrator` warning

---

## Scenarios

### Scenario 1 — Default chat via `main` agent

**Goal:** validate Gateway → (legacy AgentLoop path, the orchestrator is provisioned but Phase 5 deferred the run_loop swap). Should produce a normal completion.

**Steps:**
1. Curl a simple prompt to the OpenAI-compatible completion endpoint.
2. Verify response text is coherent.
3. Verify no regressions from Phase 4 baseline (same behavior).

**Result:** ⬜ pending — (fill in)

### Scenario 2 — flow_run composition

**Goal:** validate opt-in `flow_run` tool.

**Note:** Task 12 deferred ToolService registration to Phase 6. The tool exists as code (`FlowRunTool` type) but is NOT wired into ToolService yet. This scenario is expected to **not invoke flow_run at runtime** until Phase 6. Skip this scenario in Phase 5 manual E2E OR verify it's correctly unavailable.

**Result:** ⬜ deferred to Phase 6 (Task 12 adapter wiring)

### Scenario 3 — `gateway.flow.reload` RPC

**Goal:** validate the reload handler.

**Note:** Task 14 deferred the RPC router registration to Phase 6. The handler exists (`handle_flow_reload`) but is not yet registered as a JSON-RPC method. Skip this scenario in Phase 5 manual E2E.

**Result:** ⬜ deferred to Phase 6 (Task 14 invocation wiring)

### Scenario 4 — Recursion guard

**Goal:** assert depth 4 cap.

**Note:** Requires working `flow_run` from Scenario 2. Defer.

**Result:** ⬜ deferred to Phase 6

---

## Bugs Discovered

(None yet — fill in during live testing.)

---

## Decision

⬜ Pending user decision after scenarios run:

- Ship current Phase 5 landing (orchestrator exists + is provisioned + has unit + e2e test coverage; Gateway still uses legacy AgentLoop path; flow_run + flow.reload are Phase 6 wiring follow-ups)?
- Or iterate on Phase 6 wiring before release?

**Baseline preserved:** all Phase 4 tests green; no regressions from Phase 5 additions.

---

## Phase 6 Follow-ups (tracked)

Every `PHASE-6:` / `PHASE-6-LEGACY` / "deferred to Phase 6" item across the Phase 5 commits. Grep:
```bash
grep -rn "PHASE-6" src/ --include='*.rs' | wc -l
```
Expected: ~10-15 markers (sandbox per-session, routing overrides, named providers, user flows dir, AgentRegistry unification, RPC registration, ToolService adapter for flow_run, etc.)

See individual commits for context: commits `9da6efe19` through `bae462859` all carry Phase-5 deferral notes.

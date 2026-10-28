# Terminal Build E2E Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a clean Panel candidate from the actual worktree HEAD and prove that the final staged HTML loads it through the real production host and versioned Terminal module, while preserving independent component and live-Gateway verdicts.

**Architecture:** Reuse the existing isolated-output build script and loopback Gateway fixture. Parameterize the existing component/host tests with one explicit staged root, add a small artifact validator for build provenance, and extend the existing host fixture only for effect-level browser evidence.

**Tech Stack:** Bun build script, Node `node:test`, Playwright/Chromium, Rust/Leptos WASM, wasm-bindgen/wasm-opt, loopback HTTP/WebSocket fixture.

**Spec:** `docs/superpowers/specs/2026-10-28-terminal-build-e2e-design.md`

## Global Constraints

- Work only in `.worktrees/terminal-build-e2e` on `terminal-build-e2e`; actual baseline HEAD is `3ac50c1be`, not the supplied `f48991a5b`.
- Never copy the shared checkout's historical `.panel-tabs` into the worktree; component and host tests must receive the newly built output root explicitly.
- Run `python qa/browser_live/check_memory.py` from the worktree root before every Cargo subprocess and accept only exit 0; run Cargo serially from its owning package directory.
- Do not treat hook-spawned Cargo as compliant verification. Do not merge, push, flip releases, or touch out-of-scope production subsystems.
- Do not record credentials, complete authorization URLs, session/PTY IDs, or secret-bearing traces.
- `live_gateway` remains `BLOCKED` without an explicitly authorized disposable target and cleanup scope.
- Production code changes require a repeatable defect, minimal RED, local fix, clean rebuild, and affected regressions.

## Review Focus

- A syntactically valid manifest could reference a different build ID or missing file; Task 1 pins exact revision/entry/asset agreement and negative cases.
- A public-base alias could serve HTML while versioned asset URLs resolve incorrectly; Task 2 pins real HTTP requests for `/`, `/panel/`, `/terminal`, and `/panel/terminal` according to the production router.
- An old socket result could overwrite a replacement epoch or remount disposed DOM; Task 2 pins late-result fencing and cleanup.
- A test could report success despite page errors or failed requests; Task 2 makes both explicit success assertions and includes diagnostics in results.
- A passing loopback run could be misreported as live parity; Task 3 keeps three independent verdicts and the literal live BLOCKED reason.

---

### Task 1: Bind clean-build provenance to both fixtures

**Files:**
- Create: `interfaces/webchat/tests/panel-build-artifacts.test.mjs`
- Modify: `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`
- Modify: `interfaces/webchat/tests/panel-terminal-host.test.mjs`
- Modify: `interfaces/webchat/package.json`

**Interfaces:**
- Consumes: existing `bun scripts/build-panel-tabs.mjs --output <dir>` and `createPanelGatewayFixture(staged)`.
- Produces: `PANEL_OUTPUT_ROOT=<absolute-or-package-relative-dir>` accepted by all three Node fixtures; artifact assertions consume `build.json`, `manifest.json`, `build-inputs.json`, final HTML, and the same staged files served by the host fixture.

- [ ] **Step 1: Write failing artifact tests** for actual HEAD equality, manifest/build ID agreement, exact versioned Terminal entry, descriptor `aleph/terminal` identity, host/Terminal JS/WASM/loader/CSS presence, no unprocessed Trunk assets, no staged `target`, and no Terminal module reference in initial host HTML. Add isolated copied-output negative cases for stale build ID, missing asset, wrong descriptor, and wrong public base.
- [ ] **Step 2: Run artifact tests against an empty isolated directory** and confirm a provenance/input failure rather than consumption of `.panel-tabs`.
- [ ] **Step 3: Implement the smallest artifact validation helpers inside the new test file** and replace fixed `.panel-tabs` resolution in protocol/host fixtures with `PANEL_OUTPUT_ROOT`; fail fast when it is absent or invalid.
- [ ] **Step 4: Run from the worktree root** `python qa/browser_live/check_memory.py`; only on exit 0 run from `interfaces/webchat` `bun run panel:tabs:build -- --output <new-output-root>` and record the gate and build result.
- [ ] **Step 5: Run** `PANEL_OUTPUT_ROOT=<new-output-root> node --test tests/panel-build-artifacts.test.mjs` from `interfaces/webchat`; expected PASS for the current output and explicit expected failures for corrupted copies.

### Task 2: Prove the real production-host loopback path

**Files:**
- Modify: `interfaces/webchat/tests/panel-terminal-host.test.mjs`
- Modify: `interfaces/webchat/tests/fixtures/panel-gateway.mjs` only if current event/socket controls cannot express the required cases
- Modify: `interfaces/webchat/tests/panel-terminal-protocol.test.mjs` only for demonstrated component effect gaps
- Modify: a local production routing/owner/loader/SDK/Terminal file only if a repeatable RED proves a product defect

**Interfaces:**
- Consumes: Task 1's `PANEL_OUTPUT_ROOT`, validated `build.json`/`manifest.json`, and unchanged production host/module APIs.
- Produces: non-secret result JSON containing route, actual asset paths, host/module initialization, RPC/event observations, page errors, failed requests, and cleanup state.

- [ ] **Step 1: Add failing host assertions** for exact current-build asset requests, no Terminal prefetch on Chat/Settings, production navigation and Shell/SDK/Loader mount, zero page errors/failed requests, and positive `/`, `/panel/`, `/terminal`, `/panel/terminal` route outcomes.
- [ ] **Step 2: Run the host fixture against Task 1's output** and classify failures as fixture gaps or repeatable product defects; do not patch production behavior speculatively.
- [ ] **Step 3: Implement minimal fixture controls/assertions** for connect/subscription/list/attach and spawn when needed, visible native Canvas output, UTF-8 input, post-baseline resize geometry, selected PTY exit/fallback, reconnect using a new socket, stale list/attach/subscribe fencing, missing/incompatible module isolation, and route teardown without closing non-test-owned PTYs.
- [ ] **Step 4: If a product defect is proven, write its minimal RED first**, then make only the owning local fix; rerun the memory gate and clean build, then rerun all affected fixtures.
- [ ] **Step 5: Run independently** `PANEL_OUTPUT_ROOT=<new-output-root> node --test tests/panel-terminal-protocol.test.mjs` and `PANEL_OUTPUT_ROOT=<new-output-root> node --test tests/panel-terminal-host.test.mjs` from `interfaces/webchat`; record separate component and production-host verdicts and exact failures, if any.

### Task 3: Verify, report, review, and commit

**Files:**
- Modify: `docs/reports/panel-terminal-tab-validation.md`
- Modify: `docs/reports/panel-terminal-live-parity-closure.md` only if needed to preserve prior facts and point to the new evidence
- Modify: `docs/reference/TERMINAL_RUNTIME.md` only with verified build/E2E facts
- Modify: `docs/reference/FEATURE_LOCATOR.md` only with verified build/E2E facts
- Create: a non-secret run record under `docs/superpowers/runs/` if needed to hold the full ledger without bloating the validation report

**Interfaces:**
- Consumes: Task 1 build ledger and Task 2 independent fixture results.
- Produces: committed bilingual evidence with `component`, `production_host_loopback`, and `live_gateway` verdicts; no live claim without authorized target and proven cleanup.

- [ ] **Step 1: Record exact commands/cwds and results**, including every memory gate, actual HEAD/build ID, source input hashes/tools, asset inventory and requests, initialization/RPC/events, cleanup, unexecuted checks, and remaining scope.
- [ ] **Step 2: Run applicable package checks serially** with a fresh root memory gate before each Cargo invocation; run `git diff --check`. Record unavailable checks rather than claiming them.
- [ ] **Step 3: Perform read-only live-target preflight only**; with no newly authorized disposable target write `live_gateway: BLOCKED` and `reason: no authorized disposable Gateway/project/session/credential/cleanup scope`.
- [ ] **Step 4: Request independent read-only review** focused on provenance, production-host authenticity, verdict boundaries, secret hygiene, and cleanup; resolve evidenced findings.
- [ ] **Step 5: Inspect the final diff and worktree status**, stage only task files, run the root memory gate before any commit that may invoke Cargo hooks, and commit with a conventional message such as `test(panel): add terminal build e2e`; do not merge or push.

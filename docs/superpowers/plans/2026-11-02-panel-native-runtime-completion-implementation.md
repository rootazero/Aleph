# Panel Native Runtime Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the Panel native QA evidence runner and record the source-backed pairing-surface boundary without launching WebView2 before a new explicit budget.

**Architecture:** Keep the existing helper protocol, six-step exact cleanup, Tauri credential bridge, and Panel product routes. Add an independent QA provenance preparation/validation boundary, make lifecycle/deadline state explicit, and keep ordinary fresh Panel pairing diagnosis in tests and documentation rather than adding a product UI path.

**Tech Stack:** Node.js ESM, `node:test`, existing QA native IPC protocol, Rust/Leptos source inspection, Markdown ledgers.

**Spec:** `docs/superpowers/specs/2026-11-02-panel-native-runtime-completion-design.md`

## Global Constraints

- Do not launch WebView2 or write OS credentials without explicit authorization. The user later authorized fresh artifacts and three bounded launches, each using the runner's 120-second business deadline; all three slots are now consumed. No fourth launch is permitted by this plan.
- Keep the product path unchanged: `projects.rs` → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD`.
- Do not add hidden pairing UI, direct invoke, browser-storage credentials, synthetic DOM, fake actor, `actor=None`, or a test endpoint.
- Native evidence remains layered: Rust trace, Gateway receipt, DOM monitor, artifact/process provenance, and exact cleanup cannot substitute for one another.
- Before every Cargo command, run `python qa/browser_live/check_memory.py` from the repository root; use `CARGO_BUILD_JOBS=1`; do not use `bash -lc` to alter the environment.
- Missing, malformed, or mismatched provenance returns structured `NOT_RUN`; incomplete cleanup is `AMBIGUOUS`; neither can be upgraded to `PASS`.
- Code comments are English; documentation is bilingual.

## Review Focus

- **Stale receipt at or below the cursor:** it must not satisfy a predicate or enter the receipt ledger twice. Test in Task 3.
- **Asynchronous helper shutdown:** stopped facts must be read after the `stopped` message arrives, and final child exit must remain distinct from listener/fixture shutdown. Test in Task 3.
- **Prelaunch manifest mutation:** changing shell/helper/reaper/WASM/config after preparation must fail before owned root creation. Test in Task 2.
- **Deadline during cleanup:** business actions stop at the deadline, but cleanup continues to a safe boundary and records budget exhaustion; an unproven root is never removed. Test in Task 4.
- **Ordinary fresh Panel without a browser resource:** no `PairingNotice` is expected, while TokenWall remains governed by the real auth verdict. Pin the bounded DOM classification and source-backed conclusion in Task 1.

---

### Task 1: Pin the pairing-surface diagnosis

**Files:**
- Modify: `qa/native_ipc/native_project_driver_test.mjs`
- Modify: `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md` (pairing-surface section)
- Modify: `docs/reference/FEATURE_LOCATOR.md` §6.11.2

**Interfaces:**
- Consumes: the existing monitor selectors `tokenWall`, `tokenInput`, `pairDesktop`, and the current `inspect`/`ask_pair` bounded actions.
- Produces: a test-backed statement that an authenticated monitor page with neither supported surface yields `pairing_surface_unavailable`; no new product selector or route.

- [x] **Step 1: Add the failing bounded monitor test**

  Extend `qa/native_ipc/native_project_driver_test.mjs` with a mock page that opens without `[data-testid=token-wall]`, `[data-testid=token-wall-input]`, or `[data-testid=pairing-notice-pair-desktop]`. Send the real monitor `inspect` command and assert the returned DOM facts are all false. Send `ask_pair` and assert the monitor refuses the absent control without invoking Tauri IPC.

- [x] **Step 2: Run the focused test and verify the current monitor behavior**

  Run:

  ```text
  node --test qa/native_ipc/native_project_driver_test.mjs
  ```

  Expected: the existing monitor behavior is visible in the test result; if the new assertion exposes a mismatch, preserve the real selector semantics and make the smallest monitor-test correction needed.

- [x] **Step 3: Implement only the diagnostic assertion**

  Keep the monitor as a bounded observer/controller of existing DOM controls. Do not add a fallback pairing selector or a direct IPC action. The native runner may continue to map the all-false initial inspection to `pairing_surface_unavailable`.

- [x] **Step 4: Run the focused test again**

  Expected: PASS, with no calls in `invokeLog` and no invented pairing surface.

- [x] **Step 5: Update the bilingual ledger and locator entry**

  Record that `PairingNotice` belongs to the workspace Browser viewer `LaunchSection`, ordinary fresh Panel has no browser slot/resource, and TokenWall depends on the real Gateway `needs_token`/native welcome conditions. Preserve the prior native result as `BLOCKED/pairing_surface_unavailable`; do not call this source/test proof a WebView2 proof.

---

### Task 2: Add independent QA provenance preparation and validation

**Files:**
- Create: `qa/native_ipc/native_project_provenance.mjs`
- Create: `qa/native_ipc/native_project_provenance_test.mjs`
- Modify: `qa/native_ipc/native_project_native_io.mjs:paths/assertArtifacts/runNativeProjectNative`
- Modify: `qa/native_ipc/native_project_run.mjs:preflightNativeProjectRun`
- Modify: `qa/native_ipc/native_project_runner.mjs:runNativeProject`

**Interfaces:**
- Produces `prepareNativeProvenance({ repoRoot, outputPath }) -> Promise<NativeProvenance>` as a separate QA preparation command.
- Produces `readNativeProvenance({ repoRoot, path }) -> NativeProvenance` and `validateNativeProvenance({ manifest, artifactPaths }) -> { ok: true } | { ok: false, reason: string }`.
- `NativeProvenance` contains the contract version, repository revision, named `artifact_sha256` entries for `shell`, `helper`, and `reaper`, `wasm_sha256`, the Panel build/source manifest digest, and the Tauri QA config digest. It contains no credentials and no arbitrary path text.
- Native execution consumes the already prepared manifest and compares current bytes against it; it never creates expected hashes during `runNativeProjectNative`.

- [x] **Step 1: Write provenance preparation/validation tests**

  In `native_project_provenance_test.mjs`, create temporary fixture files and test:

  - preparation records all required named digests and revision;
  - missing or malformed manifest returns a structured `NOT_RUN` preflight result;
  - mutating each shell/helper/reaper/WASM/config fixture after preparation returns `artifact_hash_mismatch` or the corresponding fixed reason;
  - a manifest with token-like arbitrary fields is rejected;
  - validation does not rewrite the manifest or pin itself from the mutated current bytes.

- [x] **Step 2: Run the new focused test and verify it fails for the missing module/behavior**

  Run:

  ```text
  node --test qa/native_ipc/native_project_provenance_test.mjs
  ```

  Expected: FAIL until the preparation and validation interfaces exist.

- [x] **Step 3: Implement the standalone QA provenance module**

  Use `node:crypto` and `node:fs` with the existing artifact path conventions. The CLI form must accept `--repo-root` and `--output`, write the manifest with exclusive creation semantics, and refuse the daily origin or missing artifact inputs. The runner-side validator reads the existing manifest and compares every required digest and revision/config field without changing it.

- [x] **Step 4: Wire structured `NOT_RUN` into preflight and native entry**

  Move artifact/provenance validation before owned-root/helper creation, but after initializing a run state and evidence ledger. On missing, malformed, stale, or mismatched provenance, return a `NOT_RUN` result with a single preflight phase and the fixed reason; cleanup remains empty because no owned resources were created. Keep the default runner read-only and do not let an exception-only path decide the public outcome.

- [x] **Step 5: Run the provenance and runner tests**

  Run:

  ```text
  node --test qa/native_ipc/native_project_provenance_test.mjs qa/native_ipc/native_project_run_test.mjs qa/native_ipc/native_project_runner_test.mjs
  ```

  Expected: PASS; the default runner does not launch, and the native path refuses an invalid manifest before creating an owned root.

---

### Task 3: Repair helper lifecycle, receipt cursor, and business deadline

**Files:**
- Modify: `qa/native_ipc/native_project_native_io.mjs:createHelper`
- Modify: `qa/native_ipc/native_project_native_io.mjs:waitReceipt`
- Create or modify: `qa/native_ipc/native_project_native_io_test.mjs`

**Interfaces:**
- `createHelper(binary, root, nonce, panelRoot, options?)` returns the existing `{ child, messages, closed, waitFor, send, waitAck, revokeExact, stop }` surface.
- `waitReceipt(helper, cursor, predicate, ledger, deadline, options?)` returns the first matching receipt with `seq > cursor` and records each accepted receipt exactly once.
- A single `deadline` remains the wall-clock cutoff for native business actions; every monitor/trace/receipt wait uses remaining time rather than resetting a full timeout.

- [x] **Step 1: Write deterministic lifecycle and cursor tests**

  Add tests that:

  - deliver `ack(shutdown)` followed later by `stopped(listenerClosed:true, fixtureClosed:true)` and assert `stop()` returns both facts after the wait;
  - verify `revokeExact()` observes the ack, closes stdin, and requires the child close/exit result;
  - return a receipt with `seq === cursor`, then a matching receipt with `seq === cursor + 1`, and assert only the latter satisfies the predicate and only one ledger receipt is added;
  - hit a deadline while waiting and assert the fixed `receipt_timeout`/`window_budget_exceeded` result is returned without extending the deadline.

- [x] **Step 2: Run the focused tests and verify the old snapshot/cursor behavior fails**

  Run:

  ```text
  node --test qa/native_ipc/native_project_native_io_test.mjs
  ```

  Expected: the stopped-facts test fails against the current pre-wait snapshot implementation.

- [x] **Step 3: Fix `createHelper.stop()`**

  Read the `stopped` message returned by the awaited `waitFor` call, or re-read `messages` only after the wait. Return listener/fixture facts independently from final child exit. Do not close the helper before the exact revoke step needs it.

- [x] **Step 4: Fix `waitReceipt()` and deadline accounting**

  Keep the predicate `(candidate.type === "receipt" && candidate.seq > after)` as a hard protocol boundary. Advance `after` only after recording an accepted receipt. Compute every timeout from the injected absolute deadline; do not use a fresh full timeout for each request. Keep the helper cursor separate from the evidence ledger sequence check.

- [x] **Step 5: Run the focused tests again**

  Expected: PASS for lifecycle, stale cursor, timeout, and redaction/order assertions.

---

### Task 4: Make cleanup phase and deadline exhaustion fail closed

**Files:**
- Modify: `qa/native_ipc/native_project_protocol.mjs:overallOutcome`
- Modify: `qa/native_ipc/native_project_run_core.mjs:createRunState/finishRun`
- Modify: `qa/native_ipc/native_project_evidence.mjs:performExactCleanup`
- Modify: `qa/native_ipc/native_project_cleanup.mjs:createNativeCleanup`
- Modify: `qa/native_ipc/native_project_native_io.mjs:finally cleanup path`
- Modify: `qa/native_ipc/native_project_cleanup_test.mjs`
- Modify: `qa/native_ipc/native_project_evidence_test.mjs`

**Interfaces:**
- Run state gains a boolean `budgetExceeded` set by the native driver when the business deadline is crossed.
- `performExactCleanup(state, operations, { budgetExceeded = false } = {})` continues the declared six-step order, records the cleanup phase from actual completed steps, and preserves `AMBIGUOUS` when the safety proof is incomplete or the budget was exceeded.
- The native driver tracks whether cleanup has started; its post-cleanup fallback must not call `helper.stop()` a second time after the helper cleanup operation has already run.

- [x] **Step 1: Add failing cleanup tests**

  Extend existing cleanup/evidence tests to assert:

  - a budget-exceeded run can continue all six cleanup operations and records a cleanup reason indicating budget exhaustion;
  - a failed helper stop prevents revoke, reaper, and root removal;
  - cleanup failure leaves outcome `AMBIGUOUS` even if earlier business phases are `BLOCKED` or `PASS`;
  - the native `finally` path does not invoke helper stop twice after cleanup has started.

- [x] **Step 2: Run the focused cleanup tests**

  Run:

  ```text
  node --test qa/native_ipc/native_project_cleanup_test.mjs qa/native_ipc/native_project_evidence_test.mjs qa/native_ipc/native_project_protocol_test.mjs
  ```

  Expected: the new deadline/double-stop assertions fail before implementation.

- [x] **Step 3: Implement explicit budget state and outcome derivation**

  Add `budgetExceeded` to run state and make `overallOutcome` return `AMBIGUOUS` when the declared business window was exceeded, even if the six cleanup steps later succeed. Keep the existing phase completeness and cleanup order checks.

- [x] **Step 4: Implement safe-boundary cleanup**

  Keep cleanup operations in the existing order: shell stop, helper stop, exact device revoke, exact credential delete, exact absence check, owned-root removal. Run cleanup after business deadline expiry with bounded operation waits. Allow root removal only after all preceding proofs succeed. Mark cleanup phase `PASS` only when all six facts are true and the business budget was not exceeded; otherwise preserve the specific blocked/ambiguous result.

- [x] **Step 5: Remove the duplicate helper stop**

  In the native driver, set a local cleanup-started flag before invoking `performExactCleanup`. Only use a fallback helper stop if cleanup construction fails before the cleanup state machine starts; never call it after the helper cleanup operation has been attempted.

- [x] **Step 6: Run cleanup tests again**

  Expected: PASS, including root-last and no-second-stop behavior.

---

### Task 5: Produce the reviewable preflight report without launching WebView2

**Files:**
- Modify: `qa/native_ipc/native_project_runner.mjs` CLI report
- Modify: `qa/native_ipc/native_project_run.mjs` preflight report
- Create: `qa/native_ipc/native_project_preflight_test.mjs` if existing tests cannot cover the report shape
- Modify: `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`
- Modify: `docs/reference/FEATURE_LOCATOR.md` §6.11.2

**Interfaces:**
- Default `node qa/native_ipc/native_project_runner.mjs --repo-root .` remains `NOT_RUN/preflight_only` and performs no mutation.
- Invalid provenance is reported as structured `NOT_RUN` with the phase/reason and empty cleanup/evidence arrays.
- A reviewable report includes the prepared manifest digest set, source-backed pairing result, launch gate state, exact cleanup sequence, and the fact that no WebView2 launch has occurred in this iteration.

- [x] **Step 1: Add report-shape assertions**

  Assert that default preflight, invalid manifest, and source-only pairing diagnosis each use the fixed outcome vocabulary and do not report native trace, credential, or WebView2 PASS.

- [x] **Step 2: Run the report tests before documentation changes**

  Run:

  ```text
  node --test qa/native_ipc/native_project_run_test.mjs qa/native_ipc/native_project_runner_test.mjs qa/native_ipc/native_project_preflight_test.mjs
  ```

- [x] **Step 3: Implement the report plumbing**

  Preserve the existing public result shape. Add only fixed redacted reasons and manifest summaries needed for review. Do not print tickets, tokens, lease IDs, old connection-file content, or arbitrary local paths.

- [x] **Step 4: Update the bilingual ledger and locator**

  Record the RED/GREEN pairing diagnosis, the corrected lifecycle/provenance behavior, the current `NOT_RUN` state before any new budget, and the exact remaining native blockers. Keep the previous `pairing_surface_unavailable` outcome unchanged historically.

- [x] **Step 5: Run all Node and docs checks**

  Run:

  ```text
  node --test qa/native_ipc/*_test.mjs
  python qa/browser_live/check_docs.py --self-test
  python qa/browser_live/check_docs.py
  git diff --check
  git status --short
  ```

  Expected: all applicable checks pass, the tracked worktree contains only the intended code/design/ledger changes, and no WebView2 process or credential operation has been started.

---

### Task 6: Build validation and handoff gate

**Files:**
- Modify only files required by Tasks 1–5.
- Create: reviewable preflight artifact under the existing ignored QA evidence location, if the repository workflow requires it; do not add credentials or token-bearing files.

- [x] **Step 1: Run focused Rust formatting only if Rust files changed**

  Run `rustfmt --edition 2021 --check` on changed Rust files. If no Rust files changed, record this check as not applicable.

- [x] **Step 2: Run Cargo checks with the memory gate**

  Before each command run `python qa/browser_live/check_memory.py` from `D:/Workspace/Aleph`, then use `CARGO_BUILD_JOBS=1` for the default and `native-qa` checks required by the plan. Do not use `bash -lc`.

- [x] **Step 3: Generate and inspect the independent provenance manifest**

  Run the separate QA preparation command against the already built artifacts. Inspect only redacted manifest fields and verify that the runner consumes it rather than creating a self-pin.

- [x] **Step 4: Produce the preflight report and execute the separately authorized bounded launches**

  The preflight report was produced before launch. The user then explicitly authorized fresh artifacts and three bounded native launches. All three used the existing product path and the 120-second business deadline. The first ended `AMBIGUOUS` after `pairing_surface_unavailable` and incomplete exact revoke proof. The second ended `BLOCKED/native_run_failed` with all six cleanup facts true after the localized polling/helper repair. The third, rebuilt against current HEAD with a fresh manifest, also ended `BLOCKED/native_run_failed` with all six cleanup facts true. No native credential or protected persistence proof was obtained, and no fourth launch was performed.

- [x] **Step 5: Commit the reviewable implementation**

  Use a conventional commit message such as `fix(qa): harden native project evidence runner`. Do not merge or push.

- [x] **Step 6: Request fresh whole-branch review**

  Dispatch a read-only reviewer after the commit. Include any timeout or SIGTERM explicitly; do not call the review clean unless it completes cleanly.

## Self-review

- Spec coverage: pairing diagnosis is Task 1; independent provenance and structured `NOT_RUN` are Task 2; helper/cursor/deadline are Task 3; safe cleanup is Task 4; ledger/preflight is Task 5; validation and launch gate are Task 6.
- Interface consistency: provenance preparation is consumed by `validateNativeProvenance`; run state owns `budgetExceeded`; cleanup receives that state and remains ordered by `CLEANUP_STEPS`; public runner shape remains unchanged.
- Review focus coverage: all five failure classes in the header map to explicit tests in Tasks 1–4.
- Proportion: tasks name files, interfaces, tests, and verification commands without prescribing implementation bodies beyond the safety-critical ordering and outcome rules.

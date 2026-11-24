# Panel Native Evidence Ledger Implementation Plan
# Panel Native Evidence Ledger 证据台账实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the native runner's under-specified `native_run_failed` result with a bounded, typed, redacted evidence ledger that identifies the last reached boundary, fixed failure reason, operation-scoped missing layers, and independent cleanup outcome while preserving the existing authenticated Panel path.

**Architecture:** Keep Rust `qa_trace` as the only credential-effect evidence and add a closed JS/Node lifecycle ledger around helper commands, monitor commands, trace waits, receipt waits, and cleanup. `native_project_evidence.mjs` owns event validation and failure-summary derivation; `native_project_native_io.mjs` owns I/O observation; `native_project_run_core.mjs` preserves phase and cleanup outcome semantics; `native_project_run.mjs` and the runner expose the bounded report. Product authentication, Tauri IPC, project-selection UI, and credential semantics remain unchanged.

**Tech Stack:** Node.js ESM test files using `node:test` and `node:assert/strict`; existing `.mjs` QA modules; Rust QA trace under the existing `qa-owned-root` feature only if the audit proves the current trace contract insufficient; existing Panel, lite-shell, helper, reaper, provenance, and preflight commands.

**Spec:** `docs/superpowers/specs/2026-11-02-panel-native-evidence-ledger-design.md`

## Global Constraints

- Keep the product path `Projects/Global → DashboardState::select_panel_project → PANEL_PROJECT_SELECT_METHOD`.
- Do not add pairing UI, hidden selectors, direct Tauri invoke, browser-storage credentials, fake actors, `actor=None`, or a test endpoint.
- Keep the six exact cleanup steps in order, with `owned_root_removed` last; cleanup failure or incomplete proof remains `AMBIGUOUS`.
- Keep provenance independent and exclusive-write; the runner consumes a prepared manifest and never self-pins current bytes.
- Reports and tracked ledgers must contain no raw token, ticket, credential, lease ID, device ID, arbitrary path, request ID, selector, URL, or raw OS/error text.
- Helper and monitor events contain only fixed operation classes, bounded statuses/reasons, and cursor values where the contract permits them; helper request IDs and payloads never enter evidence.
- Native credential evidence remains Rust `qa_trace` emitted after the real operation and post-operation lease check; load/save/delete, `ok`, fixed `errorCode`, and load `credentialPresent` remain authoritative.
- Preserve receipt filtering `seq > cursor` and same-cursor retry for uncorrelated `not_found`.
- Preserve source/VM/Node/helper/native evidence boundaries; DOM, Node WS/HTTP, helper receipts, and cleanup cannot upgrade native acceptance.
- Before every Cargo command, run `python qa/browser_live/check_memory.py` from the repository root, require the gate to pass, and use `CARGO_BUILD_JOBS=1` with serial execution.
- Do not connect to, stop, replace, or mutate the daily daemon or port `18790`; do not read old token-bearing connection files.
- Do not launch WebView2 during unit implementation. A later launch is a separate gate and is limited to one explicitly authorized 120-second business window; a second launch requires a new decision after reviewing the first report.
- Keep documentation bilingual where the touched document is bilingual and use conventional commit messages.

## Review Focus

1. **All-empty evidence after monitor progress:** an empty `helperMessages`/`receipts`/`nativeTrace` shape must identify the last observed boundary and missing operation layers; pin with `reports all-empty evidence as pairing-surface or monitor boundary rather than native_run_failed` in `qa/native_ipc/native_project_evidence_test.mjs`.
2. **Command intent versus effect:** a sent helper/monitor command must remain `sent` until a bounded response is observed; pin with `records helper send/ack/timeout/close separately` and `records monitor command result separately` in `qa/native_ipc/native_project_native_io_test.mjs`.
3. **Credential effect evidence:** a page or monitor acknowledgement must not satisfy native credential evidence; pin with `requires native trace for load/save/delete and preserves credentialPresent` in `qa/native_ipc/native_project_evidence_test.mjs` and existing Rust `qa_trace` tests.
4. **Cursor and stream failure behavior:** stale cursors, `not_found`, delayed records, poisoned streams, and closed pipes must fail closed without inventing receipts or traces; pin with `preserves seq > cursor and classifies trace stream failures` in `qa/native_ipc/native_project_native_io_test.mjs`.
5. **Business failure plus cleanup failure:** the first business failure must remain `primaryFailure` while cleanup has independent outcome/reason and can make the overall result `AMBIGUOUS`; pin with `preserves primary failure when cleanup is incomplete` in the evidence and run-core tests.

---

## File structure and responsibilities

The implementation stays within the existing QA modules; no new runtime dependency or product module is introduced.

- **Modify `qa/native_ipc/native_project_evidence.mjs`:** define the closed evidence vocabulary, validate and append lifecycle events, derive the operation-scoped failure summary, retain the existing public evidence arrays, and expose independent cleanup fields.
- **Modify `qa/native_ipc/native_project_run_core.mjs`:** retain existing phase/cleanup ordering and add the minimal primary-failure/cleanup-summary state needed for deterministic overall reporting.
- **Modify `qa/native_ipc/native_project_native_io.mjs`:** instrument helper send/ack/error/close, monitor connection/commands, native trace waits, receipt waits, and cleanup boundaries without recording payloads.
- **Modify `qa/native_ipc/native_project_run.mjs` and `qa/native_ipc/native_project_runner.mjs`:** expose the new bounded summary in `publicReport`, use explicit current phase/operation context for catch classification, and preserve source-only preflight behavior.
- **Modify `qa/native_ipc/native_project_protocol.mjs` only if needed:** place shared fixed vocabularies there when they are consumed by both protocol validation and evidence validation; do not widen helper or receipt wire contracts.
- **Test `qa/native_ipc/native_project_evidence_test.mjs`:** closed event schema, redaction, failure derivation, all-empty evidence, primary/cleanup separation, and public compatibility.
- **Test `qa/native_ipc/native_project_native_io_test.mjs`:** helper/monitor/trace/receipt lifecycle and boundary injection.
- **Test `qa/native_ipc/native_project_run_test.mjs`, `native_project_preflight_test.mjs`, and `native_project_runner_test.mjs`:** report and launch-gate compatibility.
- **Inspect `desktop/shell/src/qa_trace.rs`, `desktop/shell/src/main.rs`, `desktop/shell/src/keyring_bridge.rs`, `interfaces/webchat/src/api/native_credentials.rs`, and `src/bin/aleph-panel-native-qa-gateway/main.rs`:** verify the current native trace and command boundaries. Modify Rust only if the existing QA trace cannot expose a required fixed state; otherwise record the audit result without changing production code.
- **Modify `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md` and `docs/reference/FEATURE_LOCATOR.md` §6.11.2:** document the fixed failure vocabulary, evidence-layer limits, fresh verification, and any final native result.

## Interfaces fixed by this plan

The following names are implementation contracts for the later tasks:

```js
// native_project_evidence.mjs
export const EVIDENCE_LAYERS
export const EVIDENCE_STATUSES
export const EVIDENCE_OPERATIONS
export const FAILURE_REASONS
export function recordEvidenceEvent(ledger, event)
export function deriveFailureSummary({ ledger, phase, operation, thrownReason })
export function recordPrimaryFailure(ledger, failure)
export function recordCleanupOutcome(ledger, outcome)
```

`EvidenceEvent` has exactly these fields: `seq`, `layer`, `phase`, `operation`, `status`, `cursorBefore`, `cursorAfter`, and `reason`. `layer`, `phase`, `operation`, `status`, and `reason` are closed fixed values; cursor fields are non-negative integers or `null`; unknown fields are rejected. `FailureSummary` contains `primaryFailure`, `lastBoundary`, `failureReason`, `missingLayers`, `nativeTraceStatus`, `receiptStatus`, `helperStatus`, `monitorStatus`, `operation`, and independent `cleanupOutcome`/`cleanupReason`. `missingLayers` is calculated for the current failed operation, not globally for the entire run.

```js
// native_project_native_io.mjs
export function createHelper(binary, root, nonce, panelRoot, { binaryArgs = [], observe = () => {} } = {})
export async function waitReceipt(helper, cursor, predicate, ledger, deadline, context = {})
export async function waitNativeTrace(trace, command, { afterSeq = 0, timeoutMs = 0, observe = () => {} } = {})
```

`observe` receives only already-validated fixed event values. It never receives helper payloads, request IDs, tickets, credentials, URLs, or raw errors. `waitNativeTrace` preserves the existing `trace.wait` contract and returns the existing redacted trace record or `null`; it adds only lifecycle observation.

---

### Task 1: Add the closed evidence vocabulary and ledger validators

**Files:**
- Modify: `qa/native_ipc/native_project_evidence.mjs`
- Modify: `qa/native_ipc/native_project_protocol.mjs` only if shared constants are needed
- Test: `qa/native_ipc/native_project_evidence_test.mjs`

**Interfaces:**
- Produces `recordEvidenceEvent(ledger, event)`, `recordPrimaryFailure(ledger, failure)`, `recordCleanupOutcome(ledger, outcome)`, and `deriveFailureSummary({ ledger, phase, operation, thrownReason })`.
- Keeps `createEvidenceLedger`, `recordHelperMessage`, `recordNativeTrace`, `recordProvenance`, `performExactCleanup`, and `finishEvidence` source-compatible for existing callers.

- [ ] **Step 1: Write RED tests for event shape and closed values.** Add tests named `rejects unknown event fields and vocabularies`, `records monotonic value-free events`, and `retains existing public evidence arrays`. Assert exact event keys, increasing `seq`, null-or-integer cursor validation, rejection of payload/request-id/path/token fields, and unchanged existing array keys.
- [ ] **Step 2: Run the focused tests and verify RED.**

Run from `qa/native_ipc`:

```text
node --test native_project_evidence_test.mjs
```

Expected: the new tests fail because the event API and fields do not exist; all pre-existing tests remain the baseline reference.
- [ ] **Step 3: Implement the closed constants and validators.** Extend `createEvidenceLedger` with bounded `events` and a null failure summary. Implement `recordEvidenceEvent` with exact-key validation, monotonic sequence allocation, fixed vocabulary checks, and a bounded event count. Keep event values redacted before appending.
- [ ] **Step 4: Implement primary/cleanup storage without changing outcome semantics.** `recordPrimaryFailure` stores only the first fixed business failure; later business errors do not replace it. `recordCleanupOutcome` stores cleanup outcome/reason separately. `finishEvidence` adds the bounded event and summary fields while retaining `helperMessages`, `receipts`, `nativeTrace`, and `provenance`.
- [ ] **Step 5: Run the focused tests and verify GREEN.**

Run:

```text
node --test native_project_evidence_test.mjs
```

Expected: all selected tests pass and JSON serialization contains no secret sentinel.
- [ ] **Step 6: Commit the closed ledger contract.**

```text
git add qa/native_ipc/native_project_evidence.mjs qa/native_ipc/native_project_protocol.mjs qa/native_ipc/native_project_evidence_test.mjs
git commit -m "feat(qa): add typed native evidence ledger"
```

### Task 2: Add deterministic failure-summary derivation

**Files:**
- Modify: `qa/native_ipc/native_project_evidence.mjs`
- Modify: `qa/native_ipc/native_project_run_core.mjs`
- Test: `qa/native_ipc/native_project_evidence_test.mjs`
- Test: `qa/native_ipc/native_project_native_io_test.mjs`

**Interfaces:**
- `deriveFailureSummary({ ledger, phase, operation, thrownReason })` returns a closed `FailureSummary` and never copies `Error.message`.
- `recordPrimaryFailure(ledger, failure)` is first-failure-wins.
- `finishRun(state)` continues to return `AMBIGUOUS` for incomplete cleanup or budget exhaustion, even when a primary business failure exists.

- [ ] **Step 1: Write RED tests for the boundary map.** Add tests named `maps monitor surface absence to a precise reason`, `maps trace timeout and closed stream separately`, `maps receipt timeout without inventing a receipt`, `uses internal_unclassified only for unknown fixed branches`, and `preserves primary failure when cleanup is incomplete`. Assert fixed `lastBoundary`, `failureReason`, operation-scoped `missingLayers`, and independent cleanup fields.
- [ ] **Step 2: Run the focused tests and verify RED.**

Run:

```text
node --test native_project_evidence_test.mjs native_project_native_io_test.mjs
```

Expected: new classification assertions fail while current cleanup tests continue to establish the existing semantics.
- [ ] **Step 3: Implement the fixed failure map.** Derive the most specific boundary from validated events and fixed thrown reasons: artifact/provenance, helper ready/send/ack/close, monitor connect/inspect/command/close, `pairing_surface_unavailable`, native trace absent/timeout/closed/poisoned, receipt timeout/cursor failure, and `internal_unclassified`. Preserve `window_budget_exceeded` as the fixed budget reason.
- [ ] **Step 4: Implement operation-scoped missing layers.** Determine missing layers from the current failed phase and operation. A prior save trace cannot satisfy a later reload load operation. Keep the aggregate evidence arrays unchanged for compatibility.
- [ ] **Step 5: Run the focused tests and verify GREEN.**

Run:

```text
node --test native_project_evidence_test.mjs native_project_native_io_test.mjs
```

Expected: all boundary and cleanup assertions pass; unknown errors produce only `internal_unclassified` and no raw text.
- [ ] **Step 6: Commit the deterministic failure summary.**

```text
git add qa/native_ipc/native_project_evidence.mjs qa/native_ipc/native_project_run_core.mjs qa/native_ipc/native_project_evidence_test.mjs qa/native_ipc/native_project_native_io_test.mjs
git commit -m "fix(qa): classify native evidence boundaries"
```

### Task 3: Instrument helper, monitor, trace, and receipt lifecycle

**Files:**
- Modify: `qa/native_ipc/native_project_native_io.mjs`
- Modify: `qa/native_ipc/real_run_io.mjs` only if monitor connection diagnostics cannot be observed at the runner boundary
- Test: `qa/native_ipc/native_project_native_io_test.mjs`
- Test: `qa/native_ipc/native_project_evidence_test.mjs`

**Interfaces:**
- `createHelper(..., { observe })` records fixed helper operation classes before send and after ACK, fixed error, timeout, or close. It continues returning `{ id, cursor }` internally, but IDs never enter the ledger.
- `waitReceipt(..., context)` records `receipt_since` send/response lifecycle and preserves `seq > cursor` plus same-cursor retry.
- `waitNativeTrace(trace, command, options)` records attempt/observed/absent/closed/invalid status and uses `trace.diag()` without storing arbitrary diagnostics.

- [ ] **Step 1: Write RED tests for helper lifecycle.** Add fixture tests named `records helper command sent before writing`, `records helper ack separately from send`, `records helper timeout and close without request ids`, and `records stop and exact revoke lifecycle`. Assert fixed command class, cursor before/after, and no serialized ID or payload.
- [ ] **Step 2: Write RED tests for monitor and trace lifecycle.** Add tests named `records monitor connection and command result`, `records pairing surface polling attempts without DOM text`, `records native trace timeout`, `records poisoned native trace stream`, and `records native trace effect after arrival`. Assert trace records retain `panel_credential_load/save/delete`, `ok`, fixed `errorCode`, and `credentialPresent` for load.
- [ ] **Step 3: Run the focused tests and verify RED.**

Run:

```text
node --test native_project_native_io_test.mjs native_project_evidence_test.mjs
```

Expected: new lifecycle assertions fail because current send/wait functions emit no events.
- [ ] **Step 4: Implement helper and monitor observers.** Add an observer callback carrying only validated fixed fields. Record helper send before `stdin.write`; record ACK/error/timeout/close after the bounded wait; record monitor connection and each command/result around the existing `businessWait` boundary. Keep inspect DOM diagnostics limited to existing fixed booleans.
- [ ] **Step 5: Implement trace and receipt observers.** Wrap the existing `trace.wait` and `waitReceipt` calls. Emit `native_trace` events for waiting, observed, absent, closed, and poisoned outcomes; emit receipt events for send, `not_found` retry, observed sequence, timeout, and invalid response. Never change the receipt cursor predicate or the Rust trace schema.
- [ ] **Step 6: Run the focused tests and verify GREEN.**

Run:

```text
node --test native_project_native_io_test.mjs native_project_evidence_test.mjs
```

Expected: all lifecycle tests pass, including delayed/closed/poisoned streams and same-cursor receipt retry.
- [ ] **Step 7: Commit lifecycle instrumentation.**

```text
git add qa/native_ipc/native_project_native_io.mjs qa/native_ipc/real_run_io.mjs qa/native_ipc/native_project_native_io_test.mjs qa/native_ipc/native_project_evidence_test.mjs
git commit -m "feat(qa): record native command lifecycles"
```

### Task 4: Integrate boundary-aware classification into the native runner

**Files:**
- Modify: `qa/native_ipc/native_project_native_io.mjs`
- Modify: `qa/native_ipc/native_project_run.mjs`
- Modify: `qa/native_ipc/native_project_runner.mjs`
- Test: `qa/native_ipc/native_project_preflight_test.mjs`
- Test: `qa/native_ipc/native_project_run_test.mjs`
- Test: `qa/native_ipc/native_project_runner_test.mjs`

**Interfaces:**
- The runner tracks explicit `currentPhase` and `currentOperation` before each boundary instead of deriving the catch phase solely from `PHASES[state.phases.length]`.
- `publicReport(result)` exposes the existing outcome/phases/cleanup/evidence fields plus the bounded failure summary; it never exposes nonce, ticket, request ID, raw errors, or arbitrary DOM data.

- [ ] **Step 1: Write RED tests for the observed all-empty run.** Add an injected pre-pairing failure test named `reports last monitor boundary when helper receipts and native trace are empty`. Assert `helperMessages`, `receipts`, and `nativeTrace` are empty, but `lastBoundary`, fixed `failureReason`, `missingLayers`, helper/monitor/native/receipt statuses, and `primaryFailure` are populated.
- [ ] **Step 2: Write RED tests for preflight/report compatibility.** Extend existing preflight tests to assert source-only runs remain `NOT_RUN`, execute-without-approved-driver remains `BLOCKED`, `cleanupSequence` remains exact, and the new fields are closed and redacted.
- [ ] **Step 3: Run the focused tests and verify RED.**

Run:

```text
node --test native_project_preflight_test.mjs native_project_run_test.mjs native_project_runner_test.mjs
```

Expected: new summary assertions fail; existing launch-gate assertions must remain green.
- [ ] **Step 4: Integrate observers and explicit context.** Pass the ledger observer into helper creation, wrap monitor commands and connections, wrap all credential trace waits and receipt waits, and set the current operation before each call. In the catch block, record the first fixed business failure before cleanup and derive the summary from the events already observed.
- [ ] **Step 5: Integrate independent cleanup reporting.** Record cleanup lifecycle events and call `recordCleanupOutcome` after `performExactCleanup`. Preserve the six-step state machine and ensure cleanup errors/budget cannot replace `primaryFailure`; `finishRun` still determines `AMBIGUOUS` as before.
- [ ] **Step 6: Run the focused tests and verify GREEN.**

Run:

```text
node --test native_project_preflight_test.mjs native_project_run_test.mjs native_project_runner_test.mjs native_project_evidence_test.mjs native_project_native_io_test.mjs
```

Expected: all selected tests pass, including the all-empty evidence case and existing no-mutation preflight cases.
- [ ] **Step 7: Commit runner integration.**

```text
git add qa/native_ipc/native_project_native_io.mjs qa/native_ipc/native_project_run.mjs qa/native_ipc/native_project_runner.mjs qa/native_ipc/native_project_preflight_test.mjs qa/native_ipc/native_project_run_test.mjs qa/native_ipc/native_project_runner_test.mjs
git commit -m "fix(qa): expose native failure boundaries"
```

### Task 5: Audit the native trace and enforce redaction at the shell boundary

**Files:**
- Inspect: `desktop/shell/src/qa_trace.rs`
- Inspect: `desktop/shell/src/main.rs`
- Inspect: `desktop/shell/src/keyring_bridge.rs`
- Inspect: `interfaces/webchat/src/api/native_credentials.rs`
- Inspect: `src/bin/aleph-panel-native-qa-gateway/main.rs`
- Modify only if required: the inspected Rust QA files and their existing tests
- Test: existing Rust `qa_trace` tests plus `qa/native_ipc/monitor_contract.mjs` parser tests

**Interfaces:**
- Preserve the existing Rust trace record schema: `panel_credential_load/save/delete`, `ok`, fixed `resultShape`, fixed `errorCode`, and load-only `credentialPresent`, authenticated by the owned-root nonce and child PID.
- No production credential behavior or error surface changes are permitted.

- [ ] **Step 1: Write/extend the parser RED tests only if a required state is untestable.** Cover delayed stdout, interleaved non-trace lines, stream close, malformed trace, sequence gap, and exact load/save/delete shapes. Do not add a native launch.
- [ ] **Step 2: Run the existing parser/Rust-focused tests before any Rust change.**

Run from `qa/native_ipc`:

```text
node --test native_project_native_io_test.mjs native_project_evidence_test.mjs
```

If Rust source changes are necessary, first from the repository root run:

```text
python qa/browser_live/check_memory.py
```

Then run the narrow shell test command with `CARGO_BUILD_JOBS=1`; record the exact command and selected result.
- [ ] **Step 3: Make the smallest QA-only correction if the audit finds a missing trace state.** Keep `qa-owned-root` gating, fixed enums, post-operation emission, and no raw OS text. If the existing contract already satisfies the required distinctions, make no Rust edit and record that conclusion in the run ledger.
- [ ] **Step 4: Verify no raw values cross the trace boundary.** Assert serialized trace and final report do not contain credential values, tickets, lease IDs, device IDs, paths, URLs, or raw errors.
- [ ] **Step 5: Commit only if source changed.**

```text
git add desktop/shell/src/qa_trace.rs desktop/shell/src/main.rs desktop/shell/src/keyring_bridge.rs interfaces/webchat/src/api/native_credentials.rs src/bin/aleph-panel-native-qa-gateway/main.rs qa/native_ipc
 git commit -m "fix(qa): harden native trace diagnostics"
```

If no Rust source changed, do not create an empty commit; include the audit result in the documentation task.

### Task 6: Run complete non-native validation and update evidence documentation

**Files:**
- Modify: `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`
- Modify: `docs/reference/FEATURE_LOCATOR.md` §6.11.2
- Modify: `docs/superpowers/specs/2026-11-02-panel-native-evidence-ledger-design.md` status from approved draft to approved/implemented only after code is actually complete
- Test/verify: all `qa/native_ipc` tests and docs gates

- [ ] **Step 1: Run the full Node native IPC test set.** From `qa/native_ipc` run:

```text
node --test *_test.mjs
```

Expected: all tests pass; record the exact pass/fail count and any pre-existing warnings.
- [ ] **Step 2: Run documentation and diff gates.** From the repository root run:

```text
python qa/browser_live/check_docs.py
 git diff --check
```

Expected: docs gate exits 0 and diff check is empty.
- [ ] **Step 3: Rebuild current artifacts only after source validation.** For each Cargo invocation, run the memory gate first and use `CARGO_BUILD_JOBS=1`:

```text
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo build --no-default-features --features qa-owned-root -p aleph-desktop-shell

python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo build --features native-qa --bin aleph-panel-native-qa-gateway

python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo build -p aleph-client --example qa_credential_reaper --features qa-service

python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo check -p aleph-panel --lib --no-default-features --features panel-modular
```

Expected: current-HEAD shell/helper/gateway/reaper/Panel checks pass; report exact warnings and distinguish a native-target check from a fresh WASM artifact build.
- [ ] **Step 4: Generate and independently validate fresh provenance.** Use the existing provenance preparation entry point, exclusive file creation, and current revision. Verify only fixed digest/revision fields; do not print paths or secrets into the report. Preserve the independent manifest and do not let the runner generate it.
- [ ] **Step 5: Produce reviewable preflight without launching.** Run the source-only runner and capture `NOT_RUN/preflight_only`, `launchGate: new_explicit_budget_required`, zero WebView2 launches, exact cleanup sequence, artifact/provenance status, and the new fixed evidence fields.
- [ ] **Step 6: Update the bilingual run ledger and FEATURE_LOCATOR.** Document the final fixed boundary vocabulary, the all-empty evidence diagnosis, the shell trace audit, actual Node/docs/build results, and the fact that no native acceptance is claimed before a separately authorized launch. Do not rewrite historical launch facts.
- [ ] **Step 7: Commit validation documentation.**

```text
git add docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md docs/reference/FEATURE_LOCATOR.md docs/superpowers/specs/2026-11-02-panel-native-evidence-ledger-design.md
 git commit -m "docs(qa): record native evidence boundaries"
```

### Task 7: Separate authorized launch and evidence-based evaluation

**Files:**
- Modify only fresh ignored evidence artifacts under the authorized SDD evidence directory
- Read: `qa/native_ipc/native_project_runner.mjs`, current preflight, provenance manifest, and run ledger
- Test/verify: one explicitly authorized WebView2 business window only

- [ ] **Step 1: Review the fresh preflight and provenance before launch.** Confirm artifacts are current HEAD, manifest is independently validated, the new report is reviewable, cleanup is exact/root-last, and no daily daemon or `18790` action is planned.
- [ ] **Step 2: Obtain the separate launch authorization.** This plan does not treat design approval or implementation completion as WebView2 authorization. The allowed launch is one new 120-second business window; a second launch requires a new decision based on the first report.
- [ ] **Step 3: Run the one bounded launch through the existing product path.** Collect evidence in order: helper/shell ownership, monitor connection and surface, pairing, native save, same-device reload/native load with `credentialPresent:true`, Global/project A/project B, revocation boundary, and six-step exact cleanup. Do not substitute helper controls or DOM observations for native proof.
- [ ] **Step 4: Evaluate the report by evidence layer.** If blocked, report the exact reproducible last boundary, fixed reason, operation-scoped missing layers, primary business failure, independent cleanup facts, and the next architecture decision. If all required evidence passes, report native acceptance with each layer separately.
- [ ] **Step 5: Update the run ledger with actual launch facts.** Never claim a launch, receipt, trace, or cleanup proof that did not occur; preserve raw sensitive artifacts only in the existing ignored, access-controlled evidence location.

---

## Plan self-review

- **Spec coverage:** typed closed ledger, helper/monitor/trace/receipt/cleanup lifecycle, fixed failure mapping, operation-scoped missing layers, primary/cleanup separation, redaction, Rust trace audit, RED/GREEN tests, non-native validation, provenance/preflight, documentation, and separate launch gate are covered by Tasks 1–7.
- **Step scan:** every task has a failing-test or audit step, a bounded implementation step with exact files/interfaces, a focused verification command, and a conventional commit boundary. No task authorizes a launch before Task 7.
- **Type consistency:** the observer callback, `recordEvidenceEvent`, `deriveFailureSummary`, `recordPrimaryFailure`, `recordCleanupOutcome`, `waitNativeTrace`, and `waitReceipt` signatures are used consistently across the task interfaces. Existing public arrays and helper return `{ id, cursor }` remain compatible.
- **Review focus coverage:** all-empty evidence is pinned in Tasks 1/4; intent/effect distinction in Task 3; credential effect evidence in Tasks 2/5; cursor/stream failures in Tasks 2/3; and primary/cleanup separation in Tasks 2/4.
- **Proportion:** the plan fixes contracts and tests without prescribing implementation bodies. Existing modules are reused; no speculative Rust heartbeat, product route, new dependency, or broad refactor is included.

## Handoff

Plan complete and saved to `docs/superpowers/plans/2026-11-02-panel-native-evidence-ledger-implementation.md`. Please review the plan. Which execution approach would you prefer?

- **Subagent-driven** — a fresh worker implements each task and a fresh reviewer checks it before the next one starts, followed by a whole-branch review. Most thorough; costs more context and coordination.
- **Native** — I implement every task in this session and run one independent final reviewer. Faster and cheaper; the plan has enough fixed interfaces to keep the work coherent.

I recommend **native** because the tasks are tightly coupled through one JS evidence contract, while the user’s acceptance rules require a single owner for the evidence semantics and launch gate. Does the plan capture what you want, and which execution approach should we use?

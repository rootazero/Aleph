# Memory source-owned validation closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to execute this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close the remaining evidence gaps from the 2026-11-03 Memory source-owned boundary slice, and determine which of its 51 full-suite failures also occur on the pre-slice baseline without fixing unrelated failures.

**Architecture:** Keep this as a validation-only follow-up. Use real Admin API routers and existing MemoryRuntime state transitions to pin observable status and generation behavior where deterministic fixtures already allow it; do not add a public/test-only runtime control or a new lifecycle authority merely to force HTTP 204. Compare the serial full `alephcore --lib` suite at the completed local `main` against the exact pre-slice commit in an isolated detached worktree, and report the failure-set delta without changing unrelated modules.

**Tech Stack:** Rust, Tokio, Axum, Cargo/libtest, Git worktree, Python documentation checks.

**Prior evidence:** `docs/superpowers/runs/2026-11-03-memory-source-owned-boundary-run.md`
**Prior design:** `docs/superpowers/specs/2026-11-03-memory-source-owned-boundary-design.md`
**Prior implementation plan:** `docs/superpowers/plans/2026-11-03-memory-source-owned-boundary-plan.md`

**Status / 状态:** Handoff only; not implemented / 仅交接，尚未实施。

## Global Constraints

- Start by verifying the checkout is the intended local `main`, that the previous task commit `a4fa8088f4dcd6f939909069da8e08266983accb` is an ancestor, and that the worktree is clean; the previous task is already committed on `main`, so do not create a synthetic merge.
- Do not fetch, push, merge, rebase, reset, or switch the primary checkout. Do not overwrite user changes.
- The known pre-slice baseline is `063345f2f30bf999b28ee5d84aac860be8432fa7`. Use that exact local object for comparison; do not fetch to update it. If it is unavailable, record the blocker rather than substitute an unreviewed baseline.
- Before every Cargo command run `python qa/browser_live/check_memory.py`; proceed only when it prints `ALLOW` and at least `4294967296` bytes are available. Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` and `--test-threads=1` for suite comparisons.
- Every focused Cargo test must select at least one test. Record zero-selection and compile failures as non-evidence.
- Test actual runtime/router behavior. No sleeps, global mocks, public test-only API, or new lifecycle authority. Do not add production seams solely to make tests deterministic.
- Only make a production change if a real behavioral RED demonstrates a defect in this slice's endpoint contract. Do not repair unrelated full-suite failures.
- Keep raw/session archive ownership separate. Make no universal all-writer stop, native unload, RSS, process-isolation, or exactly-once claim.
- Linux/Windows native execution and `bun run check` remain environment/repository limitations unless the next session has those capabilities; do not fabricate substitutes.

## Review Focus

- The `204 No Content` branch must not be reported as effect-tested unless a deterministic real-runtime setup observes it before the owned reconciler's first scan.
- HTTP `503` should be tested against real unavailable runtime states where the existing fixtures can establish them; accessor-only evidence must remain labeled as such.
- Restart evidence must distinguish a current-generation report from an old-generation report by response content, not only `Arc` identity or a successful status.
- Baseline comparison must use identical command, platform, serialism, and test binary options; compare named failure sets, not only aggregate counts.
- Temporary baseline worktree/target output must be isolated and only the worktree created by this plan may be removed.

---

### Task 1: Close feasible Admin API effect evidence

**Files:**
- Modify tests only: `src/gateway/admin_api/reconciler.rs`.
- Modify `src/memory/runtime/tests.rs` only if a fixture assertion is materially incomplete and can be improved without exposing a new test control.
- Create: `docs/superpowers/runs/2026-11-03-memory-source-owned-validation-closure-run.md` (record the initial evidence and this task's outcomes before any source edit).

**Interfaces:**
- Consume the existing `/reconciler/latest` router and `AdminApiState.memory_runtime: Option<Arc<MemoryRuntime>>` behavior.
- Consume existing real runtime lifecycle fixtures and `MemoryRuntime::current_reconciler()`; do not change their production contract.
- Preserve endpoint responses: no current reconciler -> `503`; enabled current reconciler without a report -> `204`; current report -> `200`.

- [ ] **Step 1: Record starting evidence and inspect existing test controls**

  Read the prior run, `src/gateway/admin_api/reconciler.rs`, `src/memory/runtime/tests.rs`, and the owned daemon's first-tick behavior in `src/memory/events/handler.rs`. Record current tests versus residuals. Verify whether an existing private test fixture can hold the first real scan without adding a public or production lifecycle seam.

- [ ] **Step 2: Add deterministic tests only for behaviors that can be controlled with existing fixtures**

  Add or strengthen real-router tests to cover:

  - `204` before first scan only if an existing deterministic barrier can hold the real owned daemon before it publishes its first report.
  - `503` after stop and, where existing runtime fixtures permit, while Starting/Draining/Failed; retain accessor-level evidence separately when HTTP-state construction is not available without a new seam.
  - A restart where the first and second generation produce observably different report payloads; assert the HTTP response contains only the second generation's report.

  For generation differentiation, prefer changing real fixture state between stop and restart (for example, a real reconcile divergence whose report changes after repair). Do not add a generation ID to the public wire shape. If a required effect cannot be deterministically observed with existing interfaces, record the exact constraint and leave it as an explicit gap instead of inventing a test API.

- [ ] **Step 3: Run the affected tests under the memory gate**

  Before each Cargo command, run the memory gate. Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore --lib <exact-filter> -- --test-threads=1` from the repository root for the Admin API tests. Require a positive selected count and record the exact test names/results.

- [ ] **Step 4: Review whether any production change is justified**

  A passing current implementation requires no production edit. If an HTTP effect test demonstrates a real endpoint contract failure, make only the minimal fix in the endpoint/current runtime boundary and add a failing-before/passing-after record. Do not add a public or test-only production API merely to observe an internal state.

### Task 2: Attribute the 51 full-suite failures to the baseline

**Files:**
- Read-only baseline checkout at temporary path, detached at `063345f2f30bf999b28ee5d84aac860be8432fa7`.
- Test outputs under a task-owned temporary directory; do not commit logs or target artifacts.
- Update: `docs/superpowers/runs/2026-11-03-memory-source-owned-validation-closure-run.md`.

**Interfaces:**
- Current comparison revision: verify and record the task's current `main` HEAD at execution time; do not assume it remains `a4fa8088f` after this handoff commit.
- Baseline revision: exactly `063345f2f30bf999b28ee5d84aac860be8432fa7`, the `origin/main` tip recorded before the five local commits.
- Comparison command on both revisions: `cargo test -p alephcore --lib -- --test-threads=1`, with the memory gate and `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` before each invocation.

- [ ] **Step 1: Reproduce and capture the current full-suite failure names**

  Run the full serial command on current `main` once. Capture the complete output to a task-owned temporary log; derive the exact passed/failed/ignored counts and fully qualified failing test names. Do not classify based only on the prior aggregate `21834 passed; 51 failed; 19 ignored`.

- [ ] **Step 2: Create an isolated baseline worktree without network operations**

  Confirm the exact baseline object exists locally. Create one detached temporary worktree at that SHA and use an isolated `CARGO_TARGET_DIR` so builds and generated files cannot contaminate the main checkout. Do not run the current and baseline suites concurrently.

- [ ] **Step 3: Run the same full serial suite at baseline**

  Run the exact command from the Interfaces block with a fresh memory-gate check. Capture all test names and counts. If build or environment failure prevents comparable execution, record the exact error and stop attribution; do not infer baseline status.

- [ ] **Step 4: Compare named outcomes and clean only owned temporary state**

  Classify each current failure as also failing at baseline, failing only on current `main`, or not reproducible. Report the comparison boundary: this attributes to the aggregate five-commit local delta, not necessarily to the Memory commits alone. Report pass/fail count deltas and unstable tests explicitly. Remove only the temporary worktree and files created by this task after recording results; never use destructive cleanup against unrelated paths.

### Task 3: Final evidence, independent review, and local delivery

**Files:**
- Update: `docs/superpowers/runs/2026-11-03-memory-source-owned-validation-closure-run.md`.
- Modify only if needed for accurate pointers: `docs/reference/MEMORY_SYSTEM.md` and `docs/reference/FEATURE_LOCATOR.md`.

- [ ] **Step 1: Write the bilingual run record**

  Include the Admin HTTP effect tests and exact limitations, current-vs-baseline named failure comparison, exact commands and positive counts, available-memory gate outcomes, warnings, unavailable platform/script checks, and explicit non-claims. Do not label baseline failures pre-existing unless the same named test fails at the pinned baseline.

- [ ] **Step 2: Run documentation, formatting, and diff checks**

  Run:

  ```bash
  python qa/docs/check_markdown_links.py --self-test
  python qa/docs/check_markdown_links.py
  python qa/browser_live/check_docs.py --self-test
  rustfmt --edition 2021 --check --config skip_children=true src/gateway/admin_api/reconciler.rs src/memory/runtime/tests.rs
  git diff --check
  git diff --cached --check
  ```

  Record exact output. Do not run `bun run check` again unless a `check` script has appeared in the checkout.

- [ ] **Step 3: Obtain one fresh supported read-only whole-diff review**

  Review against the task-start `main` parent. Focus on race-free HTTP effects, accuracy of generation claims, valid baseline attribution, temporary-worktree isolation, and absence of unrelated fixes. Fix only concrete findings, then rerun affected checks.

- [ ] **Step 4: Commit and verify locally**

  Create a conventional local commit such as `test(memory): close reconciler evidence gaps`. Verify clean status, commit log, and that the original `4b6da265a` ancestor remains. Do not fetch, push, merge, reset, or switch the primary checkout.

## Completion Criteria

- Every newly claimed HTTP behavior has a deterministic positive-count real-effect test, or is explicitly retained as an unclosed limitation with the reason a safe deterministic test was not possible.
- The 51 previous full-suite failures are compared by fully qualified test name at current `main` and the exact pinned baseline, or baseline attribution is explicitly blocked by a captured environment/build error.
- No unrelated failures were repaired, no new lifecycle authority/test-only public API was added, and raw archival remains independent.
- The run record and review describe exact evidence and remaining platform limitations; local commit is present and the worktree is clean.

## Deferred / Deferred

This task does not attempt Linux/Windows native execution on an unavailable host, does not invent a `bun run check` script, does not fix unrelated baseline failures, and does not expand into all-writer Memory lifecycle work. It does not claim universal Memory stop, native unload, RSS reduction, process isolation, or external exactly-once.

# Memory source-owned boundary closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make derived Memory lifecycle work use one runtime-owned production authority while preserving independent raw/session archival, with deterministic evidence for reconciler authority, compression admission, callback rejection, and flush boundaries.

**Architecture:** Keep `MemoryRuntime` and its existing `MemoryBundle`/`MemoryTasks` as the sole owner for derived backend work and retained daemons. Make the Admin API resolve the current runtime generation instead of retaining a boot-time handler; remove the server-owned production reconciler start. Mark runtime-created `CompressionService` instances as runtime-bound so missing admission fails closed, while standalone/test constructors retain their explicit compatibility contract. Keep raw stores and `FlushRegistry` independently owned, but ensure all derived work reached through them uses the runtime-bound compression path.

**Tech Stack:** Rust, Tokio, `Arc`/`Weak`, SQLite through `SqliteMemoryBackend`, Axum Admin API, existing `MemoryRuntime` lifecycle tests, real `Notify`/channel handshakes, Cargo/Bun repository checks.

**Spec:** `docs/superpowers/specs/2026-11-03-memory-source-owned-boundary-design.md`

**Status / 状态:** Complete; Tasks 1–5 and final independent review delivered in local commit `a4fa8088f`; local `main` is clean and ahead of `origin/main` by five commits. / 已完成；Tasks 1–5 与最终独立审阅已由本地提交 `a4fa8088f` 交付；本地 `main` 工作区干净，领先 `origin/main` 五个提交。

## Global Constraints

- Preserve one concrete `MemoryRuntime` owner, one canonical backend admission path, and one OwnershipTree generation source.
- Do not add a registry, Capability kind, provider proxy, or universal supervisor.
- Raw Markdown/session/tool archival remains independently owned and continues while derived Memory is Off.
- Do not claim universal all-writer stop, native unload, RSS reclamation, process isolation, or external exactly-once.
- Every Cargo command must first run `python qa/browser_live/check_memory.py`; continue only on `ALLOW` with at least `4294967296` available bytes.
- Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; every focused test command must select a positive number of tests.
- Use real runtime/store/host objects and deterministic `Notify`/channel barriers; do not use sleep races, global mocks, or public test-only APIs.
- No Feishu, Channel, Terminal, MCP lazy, EffectClaim producer, Safe Replay, or raw archival ownership migration.
- Keep public unowned daemon APIs only for explicit standalone/test compatibility; prove they have no production caller where the runtime-owned replacement is required.
- Work locally on the authorized checkout; do not fetch, push, merge, force-reset, or switch to an unrelated branch.

## Review Focus

- **Reconciler generation:** Admin API must never return a report from an old runtime bundle after restart. Test: `admin_reconciler_resolves_current_runtime_generation_and_rejects_stale_report` in `src/gateway/admin_api/reconciler.rs`.
- **Runtime state gate:** `current_reconciler()` returns none in `Starting`, `Draining`, `Failed`, `Off`, and disabled states; only Running + enabled returns a handler. Accessor test covers Off/Starting/Draining/Running-disabled, while the separate `stop_error_is_sticky_but_cleanup_retry_reaches_off` test proves Failed. HTTP evidence covers 503 without handler/when disabled/after stop, 200 after explicit scan, and 200 after restart plus explicit scan. There is no deterministic effect test for 204 before the first scan.
- **Unowned derived fallback:** runtime-bound compression rejects missing operation before per-agent raw consumption in `compress_to_notes()`; the outer public `compress()` bookkeeping is outside this guard. Test: `runtime_bound_compression_rejects_unadmitted_fallback` in `src/memory/compression/service_lifecycle_tests.rs`.
- **Callback after stop:** a captured `MemoryReflector` callback must fail closed after runtime stop and must not append a recall-signal row. Test: `memory_reflector_callback_fails_closed_after_runtime_stop` in `src/memory/reflector/fs_reflector.rs` private tests.
- **Raw/derived separation:** session-end raw archival remains available while derived flush cannot acquire Memory admission, and an admitted flush drains through the existing operation boundary. Tests: `session_end_archives_raw_when_derived_memory_is_off` in `src/memory/runtime/service_lifecycle_tests.rs`; `flush_does_not_spawn_derived_work_after_runtime_stop` in `src/memory/runtime/service_lifecycle_tests.rs`; `admitted_flush_drains_before_runtime_stop_completes` in `src/memory/compression/service_lifecycle_tests.rs`.

---

### Task 1: Make the production reconciler runtime-owned

**Files:**
- Modify: `src/memory/runtime/mod.rs` — add the narrow current-generation accessor and status/config gate.
- Modify: `src/memory/runtime/bundle.rs` — expose the current reconciler handler to the runtime accessor without constructing another handler.
- Modify: `src/gateway/admin_api/mod.rs` — replace the boot-time `memory_handler` field with an optional `Arc<MemoryRuntime>` for dynamic resolution.
- Modify: `src/gateway/admin_api/reconciler.rs` — resolve the current handler per request and implement the agreed status matrix.
- Modify: `src/gateway/admin_api/secrets.rs` and `src/gateway/admin_api/mod.rs` tests — update default `AdminApiState` construction after the field change.
- Modify: `src/gateway/server/mod.rs` — remove the obsolete server-owned reconciler JoinHandle field, setter, and shutdown abort path.
- Modify: `src/bin/aleph-server/commands/start/mod.rs:2003-2043` — remove direct `MemoryCommandHandler::new`, public daemon spawn, server JoinHandle registration, and pass the runtime into Admin API state.
- Test: `src/gateway/admin_api/reconciler.rs` and the existing boot-order tests in `src/bin/aleph-server/commands/start/mod.rs`.

**Interfaces:**
- Consumes: `MemoryRuntime::status()`, current bundle/service state, `MemoryState`, and the existing `MemoryCommandHandler::last_reconcile_report()`.
- Produces: `pub(crate) fn current_reconciler(&self) -> Option<Arc<MemoryCommandHandler>>` with these exact semantics: return `Some` only for `Running` plus `memory.reconciler.enabled`; return `None` for `Off`, `Starting`, `Draining`, `Failed`, missing services, or disabled reconciler.
- Produces: `AdminApiState` stores `memory_runtime: Option<Arc<MemoryRuntime>>`; `/reconciler/latest` returns `503` for no current handler, `204` for enabled Running with no report, and `200` for a current report.

- [ ] **Step 1: Write the failing Admin API state-matrix tests**

  Add tests that exercise the real state-facing accessor contract. Keep the existing handler-only fixtures for endpoint serialization where useful, but add runtime-backed coverage for:

  - Running + enabled + no report → `204 No Content`.
  - Running + enabled + report → `200 OK` with the report.
  - Off, Starting, Draining, Failed → `503 Service Unavailable`.
  - Running + disabled → `503 Service Unavailable`.
  - Restarted runtime → old report is not returned before the new generation scans.

- [ ] **Step 2: Run the focused tests and verify they fail for the current wiring**

  Run from the repository root:

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore admin_reconciler_state_matrix_is_fail_closed
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore admin_reconciler_resolves_current_runtime_generation_and_rejects_stale_report
  ```

  Expected: FAIL because `AdminApiState` currently stores a boot-time handler and startup separately starts the public unowned daemon.

- [ ] **Step 3: Add the runtime-owned accessor**

  Implement `MemoryRuntime::current_reconciler(&self) -> Option<Arc<MemoryCommandHandler>>` in `src/memory/runtime/mod.rs`. It must first inspect the current lifecycle state, then read the current bundle and its services; it must not create a handler, clone a backend into a new handler, or expose an old bundle after rotation. Keep the accessor read-only and source-owned.

- [ ] **Step 4: Resolve the current handler from Admin API per request**

  Change `AdminApiState` and `latest()` so the request resolves `current_reconciler()` each time. Preserve the existing wire representation and `200`/`204` payload behavior. Return `503` with the existing service-unavailable shape when no current handler is available.

- [ ] **Step 5: Remove the server-owned production daemon**

  Delete the direct startup construction and `spawn_reconciler_daemon` call at `src/bin/aleph-server/commands/start/mod.rs:2003-2034`. Keep the runtime bundle’s `spawn_owned_reconciler_daemon` at `src/memory/runtime/bundle.rs:153-159`. Keep the public method only if standalone/test callers require it, and add or update a source guard proving production startup no longer calls it.

- [ ] **Step 6: Run the focused tests and verify they pass**

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore admin_reconciler_state_matrix_is_fail_closed
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore admin_reconciler_resolves_current_runtime_generation_and_rejects_stale_report
  ```

  Expected: all selected tests pass with a positive count; Admin API observes only the current runtime generation.

- [ ] **Step 7: Commit the self-contained reconciler change**

  ```bash
  git add src/memory/runtime/mod.rs src/memory/runtime/bundle.rs src/gateway/admin_api/mod.rs src/gateway/admin_api/reconciler.rs src/gateway/admin_api/secrets.rs src/gateway/server/mod.rs src/bin/aleph-server/commands/start/mod.rs
  git commit -m "fix(memory): make reconciler runtime owned"
  ```

### Task 2: Close runtime-bound compression fallback

**Files:**
- Modify: `src/memory/compression/service.rs:68-76,405-413,709-748` — add an explicit runtime-bound service mode and reject detached derived tasks in that mode.
- Modify: `src/memory/runtime/services.rs:100-160` — construct CompressionService in runtime-bound mode.
- Test: `src/memory/compression/service_lifecycle_tests.rs` — add the fallback rejection test using a real runtime-bound backend/service.

**Interfaces:**
- Consumes: `SqliteMemoryBackend::operation() -> Result<Option<MemoryOperation>, AlephError>` and the existing standalone `CompressionService::new(...)` constructor.
- Produces: `pub(crate) fn require_runtime(self) -> Self` used only by `MemoryServices::initialize`; runtime-bound `Ok(None)` branches return without `tokio::spawn`.
- Preserves: standalone/test construction semantics only where the caller explicitly did not request runtime-bound behavior.

- [ ] **Step 1: Write the failing fallback test**

  Add `runtime_bound_compression_rejects_unadmitted_fallback` using a real `MemoryRuntime`/`SqliteMemoryBackend` pair. Stop the runtime before invoking the runtime-bound compression path, then use a `Notify`/channel-backed task counter or real row assertion to prove no detached derived task is created. The test must also prove raw storage remains available where the path is raw-owned.

- [ ] **Step 2: Run the test and verify it fails**

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore runtime_bound_compression_rejects_unadmitted_fallback
  ```

  Expected: FAIL because the current `Ok(None)` branches call `tokio::spawn(task)`.

- [ ] **Step 3: Implement explicit runtime-bound mode**

  Add the smallest internal marker or constructor method that distinguishes runtime-created services from standalone services. In both `compress_to_notes` and threshold/profile scheduling branches, return/log the unavailable admission in runtime-bound mode; do not retain or detach a JoinHandle. Use `MemoryServices::initialize` to opt in. Do not infer production status from a weak pointer alone.

- [ ] **Step 4: Run focused compression and existing lifecycle tests**

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore runtime_bound_compression_rejects_unadmitted_fallback
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore compression
  ```

  Expected: the new test and all selected existing compression tests pass.

- [ ] **Step 5: Commit the self-contained compression change**

  ```bash
  git add src/memory/compression/service.rs src/memory/runtime/services.rs src/memory/compression/service_lifecycle_tests.rs
  git commit -m "fix(memory): close derived compression fallback"
  ```

### Task 3: Prove callback and FlushRegistry admission boundaries

**Files:**
- Modify: `src/memory/runtime/service_lifecycle_tests.rs` — add real callback and flush boundary tests.
- Modify only if required by a RED test: `src/memory/runtime/session_end.rs`, `src/memory/flush/mod.rs`, `src/memory/reflector/fs_reflector.rs`, or `src/memory/runtime/services.rs`.
- Test: existing runtime/session-end/flush test modules; do not create a second global registry or test-only public API.

**Interfaces:**
- Consumes: runtime-owned `MemoryReflector` callback from `src/memory/runtime/services.rs:195-212`; `MemoryOperation::spawn_session_end`; `FlushRegistry::begin`, `await_ready`, and `flush_agent_memory`.
- Produces: deterministic effect evidence that raw archive remains independent while derived writes require current runtime admission.

- [ ] **Step 1: Write failing callback test**

  Add `memory_reflector_callback_fails_closed_after_runtime_stop`. Invoke the real reflector callback while Running, coordinate completion with a channel, stop the runtime, invoke the same callback path again, and assert the second write fails or is rejected and no new recall-signal row appears.

- [ ] **Step 2: Write failing raw/derived flush tests**

  Add `session_end_archives_raw_when_derived_memory_is_off` and `flush_does_not_spawn_derived_work_after_runtime_stop`. Use actual raw and derived stores, acquire the `FlushGuard` synchronously, and use `Notify`/channels to observe completion. Assert raw archival can complete while derived compression refuses new admission; do not assert that an already admitted operation is cancelled.

- [ ] **Step 3: Run the tests and verify the REDs**

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore memory_reflector_callback_fails_closed_after_runtime_stop
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore session_end_archives_raw_when_derived_memory_is_off
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore flush_does_not_spawn_derived_work_after_runtime_stop
  ```

  Expected: at least the post-stop derived assertions fail or expose the current unowned fallback before implementation.

- [ ] **Step 4: Make only the minimal source-owned fix required by the REDs**

  Preserve the existing `MemoryOperation` admission and `FlushRegistry` readiness semantics. If a RED shows that a path bypasses the backend operation, route that specific derived write through the existing runtime-bound service; do not move raw writes into the runtime or add a new owner.

- [ ] **Step 5: Run focused callback/flush tests and existing lifecycle tests**

  ```bash
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore memory_reflector_callback_fails_closed_after_runtime_stop
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore session_end_archives_raw_when_derived_memory_is_off
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore flush_does_not_spawn_derived_work_after_runtime_stop
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore runtime::service_lifecycle_tests
  python qa/browser_live/check_memory.py
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p alephcore runtime::watcher_lifecycle_tests
  ```

  Expected: all selected tests pass with positive counts and deterministic barriers.

- [ ] **Step 6: Commit the self-contained boundary evidence**

  ```bash
  git add src/memory/runtime/service_lifecycle_tests.rs src/memory/runtime/session_end.rs src/memory/flush/mod.rs src/memory/reflector/fs_reflector.rs src/memory/runtime/services.rs
  git commit -m "test(memory): pin derived callback boundaries"
  ```

### Task 4: Record the census and current documentation

**Files:**
- Create: `docs/superpowers/runs/2026-11-03-memory-source-owned-boundary-run.md` — bilingual source census, implementation evidence, and deferred boundaries.
- Modify: `docs/reference/MEMORY_SYSTEM.md` — current reconciler authority, runtime-bound compression, callback admission, and raw/derived boundary.
- Modify: `docs/reference/FEATURE_LOCATOR.md` — current locator entry and exact source/test evidence.
- Modify: `docs/superpowers/plans/2026-11-01-memory-runtime-lifecycle-plan.md` only to correct stale status/history pointers; do not mark Tasks 6–10 complete.

**Interfaces:**
- Consumes: source/test results from Tasks 1–3 and the approved design.
- Produces: a six-column census/evidence table: claim, source, owner, admission, retention/stop, test/evidence.

- [ ] **Step 1: Write the run record from actual results**

  Separate production source wiring, static source guards, real effects, historical results, and unavailable checks. Include the reconciler duplicate resolution, the remaining standalone public APIs, pure CPU/test exclusions, raw ownership boundary, and the exact Admin API state matrix.

- [ ] **Step 2: Correct stale lifecycle-plan status without broadening claims**

  Change only language that says the runtime is entirely unimplemented. Preserve explicit pending status for raw/RPC split, remaining long-lived captures, complete startup/live E2E, and any Task 6–10 work not covered by this plan.

- [ ] **Step 3: Run documentation checks**

  ```bash
  python qa/docs/check_markdown_links.py --self-test
  python qa/docs/check_markdown_links.py
  python qa/browser_live/check_docs.py --self-test
  git diff --check
  git diff --cached --check
  ```

  Expected: all self-tests pass, `missing_repository_targets=0`, and both diff checks are clean.

- [ ] **Step 4: Commit documentation and evidence**

  ```bash
  git add docs/superpowers/runs/2026-11-03-memory-source-owned-boundary-run.md docs/reference/MEMORY_SYSTEM.md docs/reference/FEATURE_LOCATOR.md docs/superpowers/plans/2026-11-01-memory-runtime-lifecycle-plan.md
  git commit -m "docs(memory): record source-owned lifecycle boundaries"
  ```

### Task 5: Broad verification and independent review

**Files:**
- Review all changes from Tasks 1–4; modify only files with concrete findings.

- [ ] **Step 1: Run the focused final test set**

  Before every Cargo command, run the memory gate. Then run the positive-count reconciler, compression, callback, flush, runtime lifecycle, and Admin API tests serially with `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`.

- [ ] **Step 2: Run the repository canonical check if the changed Rust surface requires it**

  Run `bun run check` from the repository root after Rust implementation changes. If it is blocked by a pre-existing unrelated failure, record the exact failure and prove the affected focused tests pass; do not call the check passed.

- [ ] **Step 3: Run a fresh supported read-only whole-diff review**

  Ask a fresh supported Pi reviewer to inspect the complete diff, with special attention to stale Admin API generations, accidental production use of public unowned APIs, raw/derived boundary regressions, and claims exceeding the evidence. Do not claim approval if the reviewer is unavailable.

- [ ] **Step 4: Fix concrete findings and repeat affected checks**

  Re-run the memory gate and every focused test affected by a finding. Update the run record with the actual review verdict and any unavailable check.

- [ ] **Step 5: Verify the final local delivery**

  ```bash
  git status --short --branch
  git diff --check
  git diff --cached --check
  git log -1 --format='%H%n%s'
  git merge-base --is-ancestor 4b6da265a HEAD
  ```

  Expected: clean worktree, baseline ancestor preserved, conventional commits present, no push performed. Linux/Windows native execution remains explicitly disclosed if not available.

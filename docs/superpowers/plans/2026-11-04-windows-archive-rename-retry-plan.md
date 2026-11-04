# Windows FileSessionStore archive rename retry plan / Windows FileSessionStore 归档 rename 重试计划

**Execution status / 执行状态:** Implemented; scoped Windows acceptance and named baseline comparison complete, full-lib suite still non-green / 已实施，Windows 有界验收与逐名基线对照完成，全量库测试仍未绿。See [run record / 运行记录](../runs/2026-11-04-windows-archive-rename-retry-run.md) for executed outcomes and limitations; the original checklist below is retained as planning history / 已执行结果与限制以运行记录为准，下方保留原规划清单。

> **For agentic workers:** execute this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> **For agentic workers:** 按任务逐条执行本计划，步骤使用复选框语法。

**Goal:** Close the Windows-only production gap where `FileSessionStore::delete_session` can fail with `Archive session failed: 拒绝访问 (os error 5)` while archiving a session, so that the "a reused session key never overwrites an old archive" contract holds on Windows natively, with no harness-side masking.

**Architecture:** The archive move stays owned by the file backend. Inside `src/gateway/session_store/file_backend/`, the archive rename distinguishes a *transient Windows sharing violation* from a *name collision* and retries the former with a bounded, backoff-ed attempt budget while keeping the latter's `<key>-1`, `<key>-2` … contract (cap 1024) untouched. Explicit transcript-handle closure after append is conditional on Task 1's measured attribution establishing that the store's own handle causes the denial; otherwise append/flush/close behaviour stays unchanged. The harness-side retry that currently hides the defect is removed once the production side is proven. / 归档移动仍由 file backend 负责：有界重试 Windows 共享拒绝，保留 `<key>-N` 与 1024 碰撞上限；只有 Task 1 实测归因证实拒绝来自服务自身 transcript 句柄时，才增加显式关闭，否则保持 append/flush/close 行为不变。生产修复验证后移除 harness 重试。

**Tech Stack:** Rust, Tokio, Cargo/libtest, Windows-native MSVC toolchain, Python JSON-RPC QA harness.

**Prior evidence (authority):** the Windows-native run this plan starts from — providerless probe 102/102, configured-provider probe 110/110 (3 of 4 runs), one configured run failing at `ctrlc-stopped: session-tail capture requested` with `Archive session failed: 拒绝访问 (os error 5)`; live archive reuse observed as `agent_main_main_s1`, `-1` … `-19`; unit regression `gateway::session_store::file_backend::archive_tests::delete_session_reuses_a_key_without_colliding_with_its_archive` green.
**Harness merged into local `main` (authority for the QA tooling):** `f40fbf061 test(qa): add Windows console control harness for memory lifecycle probe` — `qa/memory_lifecycle/windows_console.py`, `qa/memory_lifecycle/archive_check.py`, `qa/memory_lifecycle/probe.py` (Windows console control event delivery, UTF-8 text handling, per-attempt delete recording).

**Status / 状态:** Implemented; Windows-native acceptance passed; pinned full-suite comparison and local delivery in progress / 已实施，Windows 原生验收通过，固定基线全量对照与本地交付进行中。Evidence / 证据：[run record / 运行记录](../runs/2026-11-04-windows-archive-rename-retry-run.md).

---

## Global Constraints

- Discover the active checkout from Git (`git rev-parse --show-toplevel`, `git branch --show-current`, `git status --short`); do not assume an absolute path. Work on the local branch the user has checked out; do not switch the primary checkout.
- **Task-start commit is `f40fbf061`.** Record it in the run record — it is the attribution baseline for any new failure.
- Work locally only. Unless the user asks in that session: no fetch, push, merge, rebase, reset, or history rewrite. Do not overwrite user changes; if the tree carries unrelated edits, stop and report.
- Before every Cargo command run `python qa/browser_live/check_memory.py`; proceed only when it prints `ALLOW` with at least `4294967296` available bytes. Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`, and `--test-threads=1` for focused and full runs.
- Every focused Cargo test must select at least one test. Zero-selection, compile errors and source scans are not behaviour evidence.
- Production changes are allowed **only** where a real behavioural RED demonstrates the defect in this slice's contract (Task 1 produces that RED). Do not repair unrelated failures; do not refactor modules outside the slice.
- Measure the real behaviour, on the real platform. Windows-native results only — do not generalize to Linux/macOS, and do not record a platform as verified when it was not run.
- `bun run check` does not exist in this checkout; do not run it or claim a substitute.

## Scope red lines

- The change stays inside `src/gateway/session_store/file_backend/` (plus the QA harness in `qa/memory_lifecycle/` and factual doc calibration). No new public API, no public test-only API, no second archive authority, no change to the archive naming contract.
- The Memory lifecycle contract is **not** weakened: raw chat / Markdown archive must stay usable while Memory is Off, derived RPC and tools must stay refused while Off, and start/stop/restart semantics must stay exactly as they are. A retry must not turn a refused write into a tolerated one, and must not swallow the OS error on a genuinely impossible move.
- A retry may never hide a real permission problem: the terminal error must still carry the underlying OS error, and the attempt budget must be finite.
- Do **not** touch the Memory runtime architecture, the Capability Core, the SessionStore trait shape, or the SQLite backend's behaviour.
- Explicitly **out of scope for this slice** (record as deferred with the reason, do not implement):
  - 真机 UI / desktop QA — needs a real desktop shell + browser session; unrelated to the archive move.
  - RSS  reclamation verification — process-level memory accounting is a different slice.
  - external-effect exactly-once — still a deferred Capability Core producer in this repository.
  - clustered / multi-process placement — local session drains are process-local by design; clustering is not implemented.
- Do not change which session-store backend a config selects as a side effect of this slice (see Task 1 Step 5: the default-backend observation is recorded, not fixed).

## Review focus

- The retry must be provably bounded and must not spin: attempt budget, backoff, and the terminal error must all be visible in the code and in the tests.
- The classifier must not confuse the two Windows meanings of `ERROR_ACCESS_DENIED`: rename-onto-an-existing-directory (a name collision, already handled by the `<key>-N` arm) versus a sharing violation on a destination that does not exist (the new retryable case). A retry that reuses the collision arm would create `<key>-1` archives for a plain denial.
- The `<key>-1` / `<key>-2` collision contract and its 1024 cap must be unchanged, and the existing archive regression test must stay green and selected.
- Closing the store's own transcript handle must not lose or reorder transcript bytes, and must not change the flush-before-read contract documented in `FileSessionStore::append_transcript`.
- The harness must stop masking the defect: after the production fix, the QA retry loop must be gone, so a regression fails the probe loudly.
- Every claim in the run record must match what the commands actually printed — no "retries until success", no "flaky but harmless" without a named failure.

---

### Task 1: RED evidence and attribution of the Windows sharing violation

**Files:**
- Create: `docs/superpowers/runs/2026-11-04-windows-archive-rename-retry-run.md` (starting state, RED evidence, attribution).
- Modify (tests only, no production behaviour change yet): `src/gateway/session_store/file_backend/mod.rs` (add `#[cfg(windows)]` reproduction test(s)).

**Interfaces:**
- Consume (read-only): `src/gateway/session_store/file_backend/mod.rs:779-830` (`delete_session` rename loop), `:29-43` (`sanitize_key_for_dir`), `:443-473` (`append_transcript`), `:2570-2596` (`archive_tests::delete_session_reuses_a_key_without_colliding_with_its_archive`), `src/gateway/session_store/error.rs`.
- Produce: a named, reproducible RED that proves the current production code fails to archive a session when the directory still holds a child handle; an attribution of the handle holder; a measured live failure rate.

- [ ] **Step 1: Record starting state**

  Record git root, branch, HEAD (`f40fbf061` expected), `git status --short`, the memory-gate reading, and the fact that the harness merged in `f40fbf061` is the tooling baseline. State that this is the attribution baseline for new failures.

- [ ] **Step 2: Prove the RED at the store level (Windows)**

  Add a `#[cfg(windows)]` test in the file backend's test module that (a) creates a session, (b) opens a child file inside the session directory **without** `FILE_SHARE_DELETE` (so a directory rename over it is denied), (c) calls `delete_session`, and (d) asserts the current behaviour: the archive move fails and the error text is the terminal `Archive session failed: …` form, with the OS error still present. The test must fail (or the assertion must fail) against the current implementation in the way the production failure does — that is the RED the production fix is allowed to answer. Record the exact test name and the exact error string.

- [ ] **Step 3: Attribute the handle holder**

  Determine whether the live failure is caused by (i) the store's own transcript handle still being open when `delete_session` runs (the store appends the transcript during the turn, then the delete renames the directory), (ii) an external scanner (Windows Defender real-time protection is enabled on this host), or (iii) both. Evidence to collect:
  - Whether the failure rate differs between a delete that immediately follows a write into the same session directory and a delete of a session with no preceding write.
  - Whether the failure disappears when the same delete is retried after a delay, and how long the delay must be.
  - Whether an explicitly closed transcript handle removes the failure (this is the decisive check for hypothesis (i); it is also the production fix shape).
  State the attribution as a fact with the measurement that supports it, and list what remains unexplained.

- [ ] **Step 4: Measure the live failure rate on the current build**

  Build the server with `cargo build -p alephcore --bin aleph-server --features alephcore/test-helpers`, refresh the SHA-256 and `build-record.json`, and run `python qa/memory_lifecycle/archive_check.py --binary <exe> --artifacts <dir> --cycles 20` at least three times against the **file** backend (the harness config already sets `[general] session_store_backend = "file"`). Record per-run: how many deletes failed on attempt 0, which ones, and the final archive directory list. This is the baseline failure rate the fix must drive to zero.

- [ ] **Step 5: Record the default-backend observation (do not fix it here)**

  While reproducing, record the fact that the configured-provider probe selects the **file** backend ("Session store initialized (file backend)") while the providerless probe selects the **SQLite** backend, because `Config.general` falls back to `GeneralConfig`'s derived `Default` (an empty `session_store_backend`) when no `[general]` table is present, whereas `default_session_store_backend()` returns `"file"` only for a present table that lacks the key. Consequence: the providerless probe never exercises the file backend at all, so the archive contract is only observable on the configured-provider path. Record this as a **finding for a separate slice** (the effective default differs from the documented default) and do not change it here — changing the effective backend for every deployment is a decision of its own.

### Task 2: Production fix inside the file backend

**Files:**
- Modify: `src/gateway/session_store/file_backend/mod.rs` — `delete_session` (rename retry), plus private helpers and tests; modify `append_transcript` only if Task 1 attributes the denial to the store's own handle / 仅在 Task 1 归因证实时修改 transcript 关闭。

**Interfaces:**
- Produce: `delete_session` archives a session even when the session directory transiently holds an open child handle, within a finite attempt budget; the `<key>-N` collision contract, the 1024 collision cap, and the terminal error's OS-error content are unchanged.

- [ ] **Step 1: Write the failing test first for the retry policy**

  A focused test that pins the intended contract: given a destination that does not exist and a rename denied by a Windows sharing violation, the store retries the **same** destination (it must not create a `<key>-1` archive for a plain denial) and eventually succeeds once the handle is released; and given a genuinely impossible move, the store still fails with the OS error attached. Keep the collision test (`delete_session_reuses_a_key_without_colliding_with_its_archive`) untouched and green — it is the regression net for the naming contract.

- [ ] **Step 2: Implement the retry classifier**

  Add a private helper in the file backend that classifies a rename failure into *name collision* (destination exists), *retryable sharing violation* (Windows `ERROR_ACCESS_DENIED` 5 / `ERROR_SHARING_VIOLATION` 32 / `ERROR_LOCK_VIOLATION` 33 with a destination that does not exist), and *terminal* (everything else, including POSIX errors). Keep the platform scope explicit and narrow: the retry must not turn a POSIX `EACCES` into a retry loop. Unit-test the classifier with fabricated `io::Error` values so the decision table is verified deterministically, on any platform, without touching the filesystem.

- [ ] **Step 3: Implement the bounded retry in the rename loop**

  In `delete_session`, keep the existing collision arm first (destination exists → next `<key>-N`, cap 1024). Then, for a retryable sharing violation, wait a bounded backoff and retry the **same** destination. When the budget is exhausted, return the terminal error with the underlying OS error still attached, and log the attempt count. Every parameter (attempt budget, backoff shape) must be a named constant in the file with a comment saying why it is bounded.

- [ ] **Step 4: Close the store's own transcript handle**

  If Task 1 Step 3 attributes the failure to the store's own handle, make `append_transcript` close the file explicitly after the flush instead of relying on drop semantics, preserving the documented flush-before-read behaviour. Prove it with the Task 1 Step 2 RED turning green, and prove that no transcript row is lost or reordered (read the transcript back and compare the row sequence).

- [ ] **Step 5: Run the focused suites under the memory gate**

  Run the session-store filter and the other five lifecycle filters from the previous slice as regression, with positive selection, and record exact names and counts.

### Task 3: Remove the harness-side masking and re-run the Windows live QA

**Files:**
- Modify: `qa/memory_lifecycle/probe.py` (remove the `sessions.delete` retry loop that currently retries "Archive session failed"), `qa/memory_lifecycle/archive_check.py` (keep per-attempt recording; it must now succeed on attempt 0).

**Interfaces:**
- Consume: `qa/memory_lifecycle/windows_console.py` (already merged) for graceful Windows shutdown via a real `CTRL_C_EVENT`.
- Produce: a probe that fails loudly on the defect it used to hide.

- [ ] **Step 1: Remove the harness retry and keep the recording**

  Delete the retry loop in `probe.py` and keep the attempt count in the check detail, so the harness reports "failed on attempt 1" instead of healing it. Record the diff meaning in the run record: the harness used to hide a production defect and now exposes it.

- [ ] **Step 2: Rebuild the artifact and refresh the build record**

  `cargo build -p alephcore --bin aleph-server --features alephcore/test-helpers`, copy the binary, recompute the SHA-256, and rewrite `build-record.json` (`source_revision`, `artifact`, `sha256`, `platform: "windows-native"`, `features: ["alephcore/test-helpers"]`). The probe verifies the digest of the input binary, the private copy and the build record — all three must agree.

- [ ] **Step 3: Re-run both probe modes**

  Run the providerless probe and the configured-provider probe with the current `probe.py`. Requirements: both exit 0; the configured probe contains **no** `Archive session failed` occurrence; the graceful-shutdown evidence is still `[SHUTDOWN] signal=ctrl_c` with exit code 0 (Windows has no SIGTERM — state this plainly, and treat forced kill as failure cleanup only, never as success). Record the exact `Checks: X/Y` for each mode.

- [ ] **Step 4: Re-run the archive reuse QA**

  Run `archive_check.py --cycles 20` at least three times. Requirements: zero deletes failing on attempt 0; the archive tree grows monotonically as `agent_main_main_s1`, `-1`, `-2`, … (no overwrite of an earlier archive); the reused key still increments.

- [ ] **Step 5: Confirm the surrounding contract did not move**

  From the same probe runs, confirm the still-required behaviours: raw chat and Markdown archive remain usable while Memory is Off; derived `memory.stats` and derived tools stay refused while Off; start/stop/restart still behaves as before; SQLite `PRAGMA integrity_check` is `ok` for every database; the endpoint file is removed and the listener is closed after shutdown.

### Task 4: Evidence, documentation, review, delivery

**Files:**
- Update: `docs/superpowers/runs/2026-11-04-windows-archive-rename-retry-run.md`.
- Modify only for factual calibration: `docs/reference/MEMORY_SYSTEM.md`, and `docs/reference/FEATURE_LOCATOR.md` if it makes claims about the archive move.

- [ ] **Step 1: Attribute any new failure**

  Run the serial suite `cargo test -p alephcore --lib -- --test-threads=1` at the slice's HEAD. If a failure is not already known to fail at `f40fbf061`, create one detached temporary worktree at `f40fbf061` with an isolated `CARGO_TARGET_DIR`, run the identical command there, and compare **named** failure sets (same / only-current / not-reproduced). Do not repair unrelated failures, and never claim a failure is pre-existing without the same named test failing at the pinned baseline.

- [ ] **Step 2: Write the bilingual run record**

  Include: starting state, the RED evidence with exact test names, the attribution measurement, the live failure rate before and after the fix, the exact commands with their memory-gate readings, the probe `Checks: X/Y` for both modes, the archive directory listings, the SQLite integrity and cleanup results, the console-signal method, and an explicit non-claims list (including the four deferred items from the scope red lines).

- [ ] **Step 3: Run documentation, formatting and diff checks**

  ```bash
  python qa/docs/check_markdown_links.py --self-test
  python qa/docs/check_markdown_links.py
  python qa/browser_live/check_docs.py --self-test
  rustfmt --edition 2021 --check --config skip_children=true src/gateway/session_store/file_backend/mod.rs
  git diff --check
  git diff --cached --check
  ```

  Record exact output. `cargo fmt -p <crate>` formats the whole crate — use the single-file `rustfmt --config skip_children=true` form, or compare `git diff --name-only` before and after and restore anything you did not intend to touch.

- [ ] **Step 4: Obtain one fresh read-only whole-diff review**

  Review against `f40fbf061` with this plan's Review focus verbatim. Fix concrete findings, then rerun the affected checks.

- [ ] **Step 5: Commit locally and verify**

  Conventional local commit (e.g. `fix(session-store): retry windows archive rename on sharing violations`), verify a clean tree, confirm `f40fbf061` is still an ancestor, and remove only the temporary worktree and files this plan created.

- [ ] **Step 6: Calibrate the reference docs factually**

  State what now holds on Windows (a transient sharing violation no longer aborts the archive move; the reused-key contract holds natively) and what remains open (the default-backend observation from Task 1 Step 5; the four deferred items; Linux/macOS unverified).

## Completion criteria

- A named, deterministic Windows RED exists for the defect, and it is green after the fix; the RED's exact name and error text are recorded.
- The handle-holder attribution is stated as a measured fact, with the measurement that supports it, and unexplained residue is named as residue.
- The archive move survives a transient Windows sharing violation within a finite, visible attempt budget; the terminal error still carries the OS error; a genuinely impossible move still fails.
- The reused-key contract is unchanged: `delete_session_reuses_a_key_without_colliding_with_its_archive` stays green and selected; `<key>-1` / `<key>-2` still appear and never overwrite; the 1024 collision cap is unchanged.
- The harness no longer retries the defect: the QA retry loop is gone and the probe fails loudly if the defect returns.
- Live Windows QA: providerless and configured probes both exit 0 with no `Archive session failed`; `archive_check.py --cycles 20` × 3 with zero attempt-0 failures and monotonic archive growth; graceful shutdown still proven by `[SHUTDOWN] signal=ctrl_c` and exit code 0; SQLite integrity `ok`; endpoint removed; listener closed.
- The Memory lifecycle contract is unchanged: Off-time raw chat/Markdown archive still works, derived RPC/tools still refused, start/stop/restart unchanged.
- No unrelated failure repaired; no new public API; no platform-generalization claims.
- A local conventional commit exists, the tree is clean, and the run record distinguishes evidence from non-claims.

## Deferred / Deferred

This slice does not run 真机 UI / desktop QA, does not verify RSS reclamation, does not implement or claim external-effect exactly-once, and does not implement clustered / multi-process placement — each is a separate slice with its own evidence requirements. It also does not change which session-store backend a configuration selects, does not alter the SQLite backend, and does not establish Linux or macOS behaviour: all results in this slice are **Windows-native only**.

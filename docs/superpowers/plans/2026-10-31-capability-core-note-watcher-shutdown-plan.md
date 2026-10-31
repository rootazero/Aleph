# Note-vault watcher owned shutdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing note-vault watcher retain, stop and await its real work, and use that boundary in production teardown. / 让既有 watcher 保留真实任务、可等待收尾，并接入生产 teardown。

**Architecture:** `NoteVaultWatcher` remains the sole owner. It stores its consumer handle and, when shutdown starts, the one blocking debouncer-stop helper handle; cancellation drops only the caller's wait. No new Memory provider, switch or capability registry is added. / source 原位保留 handle，无第二份 owner。

**Tech Stack:** Existing Rust/Tokio, `notify-debouncer-full` 0.7.0, existing SQLite NoteStore/tempfile; no dependencies added.

**Spec:** [Companion design](../specs/2026-10-31-capability-core-note-watcher-shutdown-design.md), read completely before tests/production edits.

**Approval / 授权:** This is an unimplemented proposal. Writing this plan is not production approval. Record later explicit execution authorization before edits.

## Global Constraints

- Locate git root dynamically. Base execution on local `main` containing Terminal commits `88fe4fc4c`, `cf52bb05a`, `b378b87bd` and this package; do not reset to an older remote.
- One writer per checkout, short branch such as `note-watcher-stop`; no automatic push or remote merge. Read AGENTS, REDLINES, DEVELOPMENT memory rules, CODE_ORGANIZATION frozen-file rules and memory routing first.
- Keep `spawn_note_vault_watcher<S>(indexer: Arc<NoteIndexer<S>>) -> Result<NoteVaultWatcher, AlephError>` unchanged; its existing `S: NoteStore + Send + Sync + 'static` bound stays.
- Add exactly `pub async fn shutdown(&mut self) -> Result<(), AlephError>` on the existing owner. Retain both real consumer and blocking stop-helper handles across cancellation; no take-then-await on a pending handle.
- `Debouncer::stop` waits internally but hides its native join error. Claim only returned stop helper + observed consumer completion, not native-thread no-panic proof.
- Drain batches already emitted; do not cancel a batch in progress or promise indexing of unsettled native debounce events. Existing per-file reconciliation errors retain their logging semantics.
- No internal timeout/hard shutdown latency promise. Callers can cancel and retry the same owner. Repeated success is Ok without work; observed JoinError remains Err on repeat.
- No config/generation/registry/global-slot/backend/descriptor/lease/Zahir/protocol/harness/Sandbox changes, no lockfile updates, no all-Memory coldness or native/RSS promises.
- Run `python qa/browser_live/check_memory.py` before **each** Cargo command; proceed only on `ALLOW` and at least 4 GiB. Set `CARGO_BUILD_JOBS=1`; never run Cargo concurrently in writer/reviewer contexts.
- Read TDD/current Rust guidance before test edits. Capture real RED/GREEN per behavior; a missing API compiler error proves only missing API, not cancellation/failure correctness.

## Review Focus

- Cancellation during blocking producer stop: exact stored helper remains owned and retry observes the same completion (Task 1).
- Consumer panic while producer stop is pending or vice versa: the independent drain still gets attempted; failure cannot become a later Ok (Task 1).
- Pending native debounce vs emitted queue: only emitted batches are promised drained; no claim of globally current index (Task 1).
- Drop/early boot error: fallback never reports explicit shutdown success and launches no new cleanup worker (Task 1); full early-boot asynchronous cleanup remains unproven.
- Normal versus returned server-run error: one common production call precedes memory locals leaving scope, without altering the original run result (Task 2).

---

### Task 1: Source-owned watcher shutdown and retained evidence

**Files:**

- Modify: `src/memory/notes/watcher.rs` (`NoteVaultWatcher`, spawn return and consumer launch).
- Create: `src/memory/notes/watcher/lifecycle_tests.rs` (test-only private module).
- Test: existing `memory::notes::watcher::tests` real filesystem case and new `memory::notes::watcher::lifecycle_tests`.

**Interfaces:**

- Consumes: existing `NoteIndexer<S>`, real debouncer and consumer.
- Produces: unchanged spawn API; `NoteVaultWatcher::shutdown(&mut self) -> impl Future<Output = Result<(), AlephError>>` (written as async fn).
- Internal choices: use optional retained handles and source-local sticky error evidence. No general lifecycle state machine. Await pending handles by mutable reference; after observing terminal completion record it before dropping the completed handle.

- [ ] **Step 1: Record authorization and baseline.** Create `docs/superpowers/runs/2026-10-31-note-watcher-shutdown-run.md` with actual checkout/branch/commits, selected execution method, fixed exclusions and current warnings. Do not mark planned checks passed. Commit authorization before production edits.
- [ ] **Step 2: Write source behavior tests.** Use real temp roots/SQLite and the production watcher. Pin these named behaviors with the following observations:

| Test | Required observations |
| --- | --- |
| `shutdown_waits_for_an_emitted_batch_and_releases_consumer_indexer` | Emit a real edit, establish that its batch entered the real consumer, hold reconciliation/completion with an owning-module barrier; shutdown stays Pending, releasing it lets the note reach SQLite; after Ok the stored consumer is gone and its indexer Arc is released. No manual cleanup join. |
| `cancelled_shutdown_retains_stop_and_consumer_handles` | Block retained stop helper, poll shutdown once, drop caller future; exact helper/consumer IDs unchanged. Release and retry; same tasks complete, no second stop worker. Repeat separately with consumer pending after producer completion. |
| `consumer_join_failure_stays_failed_after_producer_stop` | Panic the real retained consumer through a private seam; explicit shutdown reports Err, producer stop still attempted/observed, repeat remains Err. |
| `stop_helper_join_failure_still_observes_consumer` | A private injected panic around the stop helper produces real Tokio JoinError; its producer is dropped/stopped and consumer terminal result is observed, repeat remains Err. This does not simulate native Debouncer thread error exposure. |
| `successful_shutdown_is_idempotent_and_replacement_receives_edits` | Two shutdown calls do not spawn another task; spawn a new watcher afterwards and prove a new edit reaches the real store; the old consumer already completed. |
| `missing_root_has_no_resources_or_directory_side_effect` | Spawn fails on absent root, leaves it absent, and never publishes a watcher/consumer. |
| `drop_is_nonblocking_fallback_not_joined_shutdown` | Dropping an unclosed watcher while an emitted batch is held does not wait or claim success; after releasing the real batch, channel closure eventually ends the existing consumer. Record that explicit guarantees require retaining the owner. |

Use private fixture hooks near the owner to observe admission, wrap **real** handles, or block one real batch. Do not install global mocks or copy indexing logic. An explicit test sender may bypass nondeterministic OS delivery for cancellation/order tests; keep at least the existing real-watch effect test plus a new real-watch stop/replacement test. Capture terminal IDs only in tests. Native debounce pending events need a documented boundary, not a fake assertion of preservation.

- [ ] **Step 3: Run RED.** New-API tests may initially fail to compile because shutdown does not exist; record that only as missing API. Add the smallest compile-enabling owner/signature skeleton, then run the real batch/handle tests against an unimplemented immediate-Ok body to obtain a runtime RED (it must report Ready where a held real batch requires Pending). Add producer-stop and failure bookkeeping incrementally, capturing the cancellation/failure/idempotence omissions with their own runtime failures before fixing them. Keep the Drop test as a fallback behavior test; do not describe its intentionally weaker semantics as a regression that explicit shutdown will magically fix.

Run each selected RED filter using the memory gate then `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib memory::notes::watcher`; keep bounded logs of expected failures. Do not call a disconnected fake closure “production RED”.

- [ ] **Step 4: Implement only the owner boundary.** Retain the consumer handle before spawn returns. On first explicit shutdown move the debouncer into one stored `spawn_blocking` stop task. Await it, then channel-draining consumer, recording observed joins/errors independently. Continue independent cleanup after an error, preserve sticky failure and repeated-success idempotence. Add nonblocking Drop fallback with truthful warning. No internal timeout, task abort, detached cleanup launch or new enabled state.
- [ ] **Step 5: Run GREEN and regressions.** Run `memory::notes::watcher`, `memory::notes::indexer`, `memory::notes::graph` and `gateway::handlers::graph` separately and once together under the Cargo rules. Existing real-create/delete, bulk and corpus semantics stay intact. Report counts actually selected; do not copy prior Terminal counts.
- [ ] **Step 6: Commit.** `git add src/memory/notes/watcher.rs src/memory/notes/watcher/lifecycle_tests.rs` and `git commit -m "fix(memory): retain note watcher shutdown work"`. Update the run record with exact RED/GREEN and fixture limitations.

### Task 2: Production drain wiring, regression evidence and references

**Files:**

- Modify: `src/bin/aleph-server/commands/start/mod.rs` (watcher binding near current 2679; common run-result funnel near current 4100 only).
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs` (small named drain boundary + tests).
- Modify: `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/memory/NOTES.md` relevant current watcher paragraphs.
- Modify: new run record.

**Interfaces:**

- Consumes: Task 1's unchanged spawn API and awaited `shutdown`.
- Produces: `pub(in crate::commands::start) async fn drain_note_vault_watcher(watcher: &mut Option<NoteVaultWatcher>) -> Result<(), AlephError>` in `builder/subsystems.rs`. `None` is satisfied without construction; `Some` borrows and awaits the same owner in place. It does not take the option before awaiting or log-and-convert an error to Ok.

- [ ] **Step 1: Write and run RED for the production boundary helper.** In its binary test module, `note_watcher_drain_none_stays_cold` requires None→Ok without filesystem effects; `note_watcher_drain_completes_real_owner_and_is_idempotent` spawns a real watcher through public APIs, observes a real edit in SQLite, drains Some, then repeats without constructing a replacement. Initially an unwired helper body fails the completion assertion only if an observable real worker ownership assertion is available through public behavior; otherwise record missing-API/compiler RED honestly and rely on Task 1's runtime RED for the lifecycle guarantee. Do not expose source-private handle IDs/barriers to another crate just for this wrapper. Precise pending/cancel/JoinError coverage belongs to Task 1's owning-module tests.
- [ ] **Step 2: Implement helper and common call.** Rename `_note_vault_watcher` to a mutable binding; add the single awaited helper call immediately after `run_until_shutdown` returns, before subsequent resource disposal. Warn on error and continue sibling teardown; preserve `run_result` and its eventual return unchanged. No `.take()` across this wait, Config lock, detached cleanup or startup restructure. Keep the separate startup full-rebuild task untouched and disclosed.
- [ ] **Step 3: Verify integration.** Run the helper's binary tests with `cargo test -p alephcore --bin aleph-server note_watcher_drain`. Audit the real production call occurs on the shared normal/returned-error funnel and precedes scope drop; do not label this static wiring audit an OS-signal E2E test. Verify the helper is a direct in-place borrow of the source shutdown result, so Task 1's cancellation/error semantics are preserved without a `.take()` or error-to-Ok conversion. Existing siblings and original server error must remain unchanged in the diff.
- [ ] **Step 4: Run final bounded checks.** For each Cargo below run the memory gate first and use jobs 1. Stop on real failure and fix/re-review; do not replace a failed group with a narrower green claim.

```text
cargo test -p alephcore --lib memory::notes::watcher
cargo test -p alephcore --lib memory::notes::indexer
cargo test -p alephcore --lib memory::notes::graph
cargo test -p alephcore --lib gateway::handlers::graph
cargo test -p alephcore --lib -- memory::notes::watcher memory::notes::indexer memory::notes::graph gateway::handlers::graph
cargo test -p alephcore --bin aleph-server note_watcher_drain
cargo check -p alephcore --tests
cargo check -p alephcore --bin aleph-server
```

The combined command puts every filter after `--`, so libtest receives the four OR filters; Cargo itself accepts only one positional TESTNAME. Compare the combined `--list` set with the union of the four focused `--list` sets, then record actual selected/pass/fail counts. These inspections use the same per-Cargo memory gate. Check filter spelling against `--list` if selection is zero; zero selected is not passing coverage. Run affected-file `rustfmt --check --edition 2021 --config skip_children=true` and `git diff --check`. Cross-platform/native-stop semantics not run must be recorded as unproven. Do not broaden to full workspace/clippy/live Panel without a new reason.

- [ ] **Step 5: Independent review.** Fresh read-only native Pi reviewer reads design, final diff including new tests, exact evidence and dependency stop/Drop semantics. No concurrent Cargo, no second writer. Fix concrete findings and re-review. Distinguish native stop-call return, consumer completion, semantic index errors, cancellation ownership and remaining startup full-rebuild lifetime.
- [ ] **Step 6: Update current references and run record.** Document explicit shutdown precisely, its production funnel, cancellation retry and limits. Retain historical phase records. Never claim Memory cold boot/graph disable/Capability integration or full thread/RSS reclamation. Include actual local commits, test counts, warnings, failed/unrun checks, reviewer verdict and final branch/worktree status. Check introduced relative links and source names.
- [ ] **Step 7: Commit implementation and docs.** Use `fix(memory): await note watcher during shutdown` for integration, then `docs(memory): record watcher shutdown evidence`. Commit only intended files. Keep local branch/worktree for review, no automatic push or remote merge.

## Builder discretion and stop rules / 实施判断与停止条件

Internal handle/error field names, fixtures and ordinary error wording are local choices. Fixed lifecycle ordering, cancellation ownership, accepted-queue semantics, scope and success evidence are not. If a supported target cannot move the existing debouncer safely to the retained blocking task, if an owner must disappear across cancellation, or if broad boot/error-funnel restructuring is required, stop for a source-backed architecture report rather than widen implementation. No new permission is needed for routine reversible fixes within this approved slice; only a real architecture change needs a new decision.

English mirror: the plan yields one independently testable owner boundary and its production integration. It deliberately does not implement the later whole-Memory/provider roadmap.

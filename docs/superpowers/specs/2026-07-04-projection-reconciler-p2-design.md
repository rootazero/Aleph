# P2 ProjectionReconciler — Design Spec

> **Status:** Draft (awaiting user review → writing-plans)
> **Date:** 2026-07-04
> **Depends on:** P1 Session SSOT foundation (`2026-07-04-session-ssot-foundation-p1-design.md`, DONE, local `e7047c1ec..6a467e469`)
> **Scope lock:** Interrupted-run-scoped detection · File backend only (both confirmed by user)

## 1. Problem

After P1, `session_events` is the single source of truth (SSOT); the `messages`
projection (for the file backend: `transcript.jsonl`) is materialised
asynchronously by `MessageProjector` (an observer on `SessionService` appends,
draining a single ordered mpsc queue).

The projection is **eventually consistent**. On a hard crash (死机 / power
loss) *during* a run, an event can be durably appended to `session_events` yet
never reach `transcript.jsonl` — the async drain did not flush before the
process died. Agent recovery is unaffected (the harness replays the full event
log). But the **Panel display** loses those un-projected rows permanently.

`ResumeCoordinator` (`src/gateway/resume_coordinator.rs`, from the 2026-05-21
mid-run-resume work) already detects interrupted runs (trailing `RunStarted`
with no `RunFinished`), repairs dangling tool calls, and **re-triggers** the
run. Re-trigger restores agent continuation and produces a *fresh* assistant
reply (projected normally). But re-trigger uses `FlowInput::Resume`, which
**skips re-seeding** — it does not re-emit or re-project the crashed attempt's
already-logged rows (typically the user message that triggered the run). Those
rows stay missing from the transcript.

**User-visible symptom:** after a mid-run crash + restart, the Panel shows a
reply with no visible prompt (the question is gone from the chat), even though
the SSOT and agent context are intact.

## 2. Goal

A boot-time reconciler that fills the missing `transcript.jsonl` rows for the
crashed run from the event log, so the Panel display is complete after a
mid-run crash. Complementary to `ResumeCoordinator`: it handles display
back-fill; ResumeCoordinator handles agent re-execution.

Non-goals (explicit YAGNI boundary — see §9): full-session sweep, SQLite
backend, back-pressure-drop recovery, persistent watermark field, changes to
backfill or to ResumeCoordinator's re-trigger logic.

## 3. Key facts the design rests on

- **File backend persists the row id.** `FileSessionStore::append_transcript`
  serialises the whole `MessageRecord` (including `id`) to JSONL;
  `read_transcript` deserialises it back. `MessageProjector` writes
  `id = format!("{key}:{seq}")` (`session_projector.rs`), so the **source event
  seq is embedded in every projector-written row id** and is recoverable on
  read — no schema change needed. (Contrast: the SQLite `messages` table
  discards `id` via `AUTOINCREMENT`, which is why SQLite is out of scope.)
- **Run markers bracket every run.** `runner_impl.rs` emits `RunStarted` before
  `harness.run` and `RunFinished` after (both success and error paths), both via
  `session_service.emit_event` → into `session_events`. A crash between them
  leaves a trailing `RunStarted` with no `RunFinished`.
- **Detection infra already exists.** `load_run_markers()` returns, per session,
  its run markers in seq order (served by the `(session_id, event_type)`
  index). `classify_markers()` returns `Clean` / `Interrupted { trailing_starts }`.
- **The crashed run's events are in the log.** Message/tool events are emitted
  synchronously into `session_events` during the run; only the *projection*
  lagged. So the un-projected rows' source events sit in the tail of the log.
- **The user message precedes `RunStarted`.** `runner_impl.rs:151` calls
  `seed_session` (which emits `TurnStarted` + `UserMessage`) BEFORE emitting
  `RunStarted` at line 467 (comment at :550: "all seeded history/user events
  precede it"). So the crashed turn's `UserMessage` has a seq *below* the run's
  `RunStarted`. **Consequence:** the slice cannot start at `RunStarted` (it
  would miss the very prompt we need to fill). The slice boundary is instead the
  transcript's own materialisation watermark — see §6.
- **Default backend is `file`.** `general.rs::default_session_store_backend()`
  returns `"file"` (the inline comment "sqlite (default)" is stale/wrong). The
  file backend is the default and the E2E environment.

## 4. Architecture

New module `src/gateway/projection_reconciler.rs`, sibling to
`session_projector.rs` (high cohesion — both are events↔messages projection).

Runs at boot **before** `ResumeCoordinator::resume_interrupted_runs`, in the
**same detached task**, so back-filled old rows are appended before re-trigger
appends new rows (the file backend's `get_history` returns append order, not
timestamp order — ordering must be enforced by write order). The reconcile pass
runs **regardless of `[resume] enabled`** (display back-fill is independent of
re-execution); only the subsequent re-trigger is gated by that flag.

**R10:** zero `src/harness/` change. All logic lives in `src/gateway/` + boot
wiring.

## 5. Components & interfaces

```rust
/// One reconcile pass summary — for the boot log line and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub scanned: usize,        // sessions with run markers inspected
    pub reconciled: usize,     // interrupted sessions that had ≥1 row filled
    pub rows_filled: usize,    // total transcript rows appended
    pub skipped_clean: usize,  // newest marker is RunFinished — nothing to do
    pub skipped_legacy: usize, // non-empty transcript with no parseable seq id
}

pub struct ProjectionReconciler {
    event_store: Arc<dyn SessionEventStore>,
    session_store: Arc<dyn SessionStore>,
}

impl ProjectionReconciler {
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self;

    /// Scan run markers; for each interrupted session, fill missing transcript
    /// rows from its crashed-run event slice. Best-effort: per-session failure
    /// is logged and skipped; never panics, never blocks boot.
    pub async fn reconcile_interrupted(&self) -> ReconcileReport;
}
```

**Reused, not rebuilt:** `SessionEventStore::load_run_markers` /
`load_events_range`; `resume_coordinator::{classify_markers, ScanVerdict}`
(already `pub(crate)`). Run markers are used **only for detection** (which
sessions are interrupted); the slice boundary comes from the transcript
watermark (§6), not from any marker seq.

**Shared projection logic (DRY refactor):** extract the per-event projection
body from `MessageProjector::project_one` into a reusable `pub(crate)` unit in
`session_projector.rs` so the live drain and the reconciler share identical
projection semantics. Sketch:

```rust
pub(crate) type TurnAccums = HashMap<(String, TurnId), TurnAccum>;

/// Project one event into `store`. When `already` is `Some`, a row-producing
/// event whose seq is in the set updates the accumulator but its WRITE is
/// suppressed (idempotent reconcile). The live drain passes `None`.
pub(crate) async fn project_event(
    store: &Arc<dyn SessionStore>,
    accums: &mut TurnAccums,
    id: &SessionId,
    rec: &SessionEventRecord,
    already: Option<&HashSet<u64>>,
);
```

`MessageProjector::project_one` becomes a thin wrapper: `project_event(store,
accums, id, rec, None)`.

## 6. Data flow (per interrupted session)

1. `load_run_markers()` → groups. For each `(session_id, markers)`:
   `scanned++`; `classify_markers`; `Clean` → `skipped_clean++`, continue.
   (Detection only — clean sessions are never sliced.)
2. `session_store.get_history(session_id, None)` → transcript. Build
   `S: HashSet<u64>` = each row's `parse_source_seq(&row.id, &session_key)`:
   `id.rsplit_once(':')` where the prefix equals the session's key string, else
   ignore. This is the set of already-materialised source seqs.
3. **Legacy guard:** transcript non-empty && `S` empty → `skipped_legacy++`,
   continue (foreign/pre-P1 rows carry no parseable seq; never touched).
4. **Watermark:** `w = S.iter().max().copied().unwrap_or(0)` — the highest
   already-projected seq. Everything at or below `w` is materialised (as a
   seq-id row, or as a legacy row below the projection range); everything above
   is candidate un-projected work.
5. `load_all_events(session_id)` → the full event log (seq ASC). If every event
   has `seq <= w` → nothing to fill, continue (idempotent no-op).
6. Fresh `TurnAccums`; for each `rec` in the full log (seq order):
   `project_event(store, accums, id, rec, Some(w))`. Events with `seq <= w`
   have their write suppressed (already materialised — this covers both
   projector-keyed rows and mixed/legacy rows below the watermark); `LlmCall*`
   events never write rows so replaying them below `w` only feeds the
   accumulator (ensuring complete token aggregation even for turns that straddle
   the watermark). Rows appended → `rows_filled += n`; if `n>0`, `reconciled++`.
7. Emit one `tracing::info!` with the `ReconcileReport`.

## 7. Correctness arguments

- **Watermark captures the prompt.** The crashed turn's `UserMessage` is seeded
  *before* `RunStarted` (§3), so slicing from a marker seq would miss it.
  Slicing from `max(S)+1` does not: if the prompt row flushed, its seq is in `S`
  and it is already displayed; if it did not flush, `max(S)` sits at a prior
  turn's row and the prompt (above `w`) is in the tail and gets filled.
- **Idempotency / no duplicates:** a materialised row has its seq in `S`;
  `project_event(.., Some(&S))` suppresses its write, and the tail starts at
  `w+1` anyway. Re-running reconcile fills nothing new.
- **Token aggregation is complete (no undercount):** the reconciler replays the
  whole event log so the turn accumulator sees every `LlmCallEnded` of the
  straddling turn, even when tool rows between two LLM calls pushed `w` past the
  first `LlmCallEnded`. Row writes are suppressed for `seq <= w`, so
  already-materialised rows (including mixed/legacy rows) are never duplicated.
  The regression test `straddling_turn_aggregates_tokens_across_watermark` locks
  this guarantee: it verifies `input_tokens=15, output_tokens=27` on the
  back-filled assistant row when `w=7` sits between two `LlmCallEnded` events at
  seq 5 and seq 9.
- **Legacy/backfill dup avoided by construction:** reconcile only projects
  events with `seq > w`. Backfilled or pre-P1 legacy rows sit at or below `w`
  (they are the *earlier* materialised content) and are never re-projected. The
  narrow residue — a pre-P1 session whose first post-deploy run crashes before
  *any* new row flushes (`S` empty) — is caught by the legacy guard (§6.3):
  skipped, so no dup (the prompt stays in the SSOT for agent recovery; only its
  Panel row is deferred, an acceptable one-time edge that vanishes once the
  session has any finished post-P1 turn).
- **Crash-loops handled:** multiple trailing `RunStarted` (ResumeCoordinator
  retried and re-crashed) monotonically append events; all un-projected rows are
  above `w`, so one `[w+1..head]` pass fills them regardless of attempt count.
- **Display ordering:** reconcile appends the missing old rows before
  `ResumeCoordinator` re-triggers (same ordered task), so the transcript reads
  `[prompt (filled), fresh reply (re-run)]` (the file backend's `get_history`
  returns append order).
- **Interaction with re-trigger:** re-trigger (`FlowInput::Resume`) does not
  re-emit the logged user message; reconcile supplies its row; re-run continues
  after it. No double-projection.

## 8. Error handling & observability

- Best-effort throughout: `load_run_markers` failure → warn, empty report.
  Per-session read/append failure → warn, skip that session, continue.
- Never panics, never blocks boot (detached task, mirrors ResumeCoordinator).
- One `tracing::info!` summary line at end (mirrors `ResumeReport` logging).

## 9. Boot wiring

In `start/mod.rs`, restructure the existing resume spawn block (~2178-2211) so
one detached task runs both scans in order:

```text
if let Some(event_store) = session_event_store_for_resume.clone() {
    let reconciler = ProjectionReconciler::new(event_store.clone(), session_store_for_reconcile.clone());
    let resume_cfg = ...; let (exec_adapter, registry) = ...;   // Option
    tokio::spawn(async move {
        let rr = reconciler.reconcile_interrupted().await;      // ALWAYS
        tracing::info!(?rr, "ProjectionReconciler boot scan finished");
        if resume_cfg.enabled {
            if let (Some(exec), Some(reg)) = (exec_adapter, registry) {
                let coord = ResumeCoordinator::new(event_store, resume_cfg, exec, reg);
                let report = coord.resume_interrupted_runs().await;   // AFTER reconcile
                tracing::info!(..report.., "ResumeCoordinator boot scan finished");
            }
        }
    });
}
```

Requires a `session_store` clone reachable at the wiring site
(`session_store_for_reconcile`), captured earlier where `session_store` is
built (~365-374).

## 10. Testing

`#[cfg(test)]` in `projection_reconciler.rs`, using in-memory `SqliteEventStore`
+ a temp `FileSessionStore`:

- **Fills a missing prompt:** interrupted markers, event log has a
  `UserMessage`+`AssistantMessage` but transcript missing the user row → after
  reconcile the user row is present exactly once, in order.
- **Clean session skipped:** newest marker `RunFinished` → no writes,
  `skipped_clean == 1`.
- **Idempotent:** run reconcile twice → second pass fills 0 rows.
- **Legacy guard:** transcript with non-seq ids + interrupted markers →
  `skipped_legacy == 1`, no writes.
- **Token aggregation:** crashed slice with two `LlmCallEnded` before the
  missing `AssistantMessage` → filled assistant row's `input/output_tokens` sum
  both calls.
- **Ordering:** reconcile appends the missing user row; a subsequent appended
  assistant row lands after it → `get_history` order `[user, assistant]`.

## 11. YAGNI boundary (explicitly NOT in P2)

- No full-session sweep (only interrupted runs).
- No SQLite backend reconciler (would need a `source_seq` column + migration;
  documented follow-up; SQLite retains P1 eventual-consistency, is not the
  default, not in E2E).
- No back-pressure-drop recovery outside a run boundary.
- No persistent watermark field.
- No change to backfill or to ResumeCoordinator's re-trigger.

## 12. Redline check

- **R10** (thin harness): zero `src/harness/` change. ✓
- **R4** (I/O-only interfaces): reconciler is infra, not an Interface-layer
  handler; no business logic in gateway handlers. ✓
- **P2/P6** (cohesion / simplicity): one focused module, reuses existing
  detection + projection, no new persisted state. ✓
- **A3/A4** (reconstructible state / lifecycle): strengthens reconstructibility
  of the read projection from the SSOT at boot. ✓

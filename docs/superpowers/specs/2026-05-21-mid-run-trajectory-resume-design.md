# Mid-Run Trajectory Resume — Design

**Date:** 2026-05-21
**Cycle:** 6 of the long-task hardening directive (final item)
**Status:** Spec ready for review

---

## 1. Goal & Anti-Goals

### Goal

When a long agent run is interrupted by a process crash, `SIGKILL`, or a
deliberate server restart, the resident `aleph-server` detects the interrupted
run on its next boot and automatically re-triggers it. The harness replays the
durable session event log and continues from where it left off, so the task is
not silently lost.

### Anti-Goals

1. **No new persistence layer.** The session event log already durably records
   the full trajectory. Resume rides that log — no `agent_runs` table, no
   `src/session/checkpoint/` module. (The 2026-04-25 P6 design doc explicitly
   retracted a parallel checkpoint store; this spec honours that.)
2. **No changes to the `src/harness/` loop.** The harness already replays the
   event log on every `run()` entry. Resume orchestration lives entirely in the
   gateway + orchestrator layers — `src/harness/` stays within its R10 budget.
3. **No explicit/manual resume command.** Boot-scan auto-resume only. A
   user-facing `resume` tool is out of scope for this cycle.
4. **No user notification on abandonment.** When a run is given up on, it is
   logged and marked terminal; proactively messaging the user is a separate
   feature (YAGNI here).
5. **No mid-LLM-call special handling.** A crash during the LLM call leaves a
   `TurnStarted` with no `AssistantMessage`; the harness replays and simply
   issues the next LLM call. This works for free — no code required.

---

## 2. Background — What Already Exists

Aleph is event-sourced. Every `SessionEvent` is synchronously written to the
`session_events` SQLite table as it is emitted. The session actor replays from
SQLite on `wake()`; the harness `run()` recomputes its loop counters
(`iterations`, `tool_calls_made`, …) from the event log on every entry.

**Therefore the trajectory is already durable.** What is *not* durable is the
**run**: `SessionScheduler` tracks the active run in an in-memory `HashMap`
(`active_run_id`), and runs are `tokio::spawn`ed — tied to process lifetime.
When the process dies mid-run, the event log survives but the knowledge that a
run *should continue* is lost.

Resume therefore needs four things, none of which exist today:

1. A durable marker that a run started and (separately) finished.
2. A boot-time scan that finds runs that started but never finished.
3. Crash-boundary repair — the event log may end with a dangling tool call.
4. A re-trigger path that runs the harness without re-seeding input.

---

## 3. Architecture

Event-log run markers + a boot-scan `ResumeCoordinator`.

```
 normal run:   RunStarted ──▶ [turns…] ──▶ RunFinished{Completed|Cancelled|Errored}
 crash:        RunStarted ──▶ [turns…] ──▶ ✗ (process dies; no RunFinished)
 boot resume:  scan ▶ repair boundary ▶ re-trigger ▶ RunStarted ──▶ [turns…] ──▶ RunFinished
```

### 3.1 New `SessionEvent` variants

In `src/session/events.rs`:

```rust
/// A harness run began on this session.
RunStarted { run_id: String, at: Timestamp },

/// A harness run reached a terminal state on this session.
RunFinished { run_id: String, outcome: RunOutcome, at: Timestamp },
```

```rust
/// Terminal disposition of a harness run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    /// Run reached its natural end (model stop / final reply).
    Completed,
    /// Run was deliberately cancelled (user `/stop`). NOT resumed.
    Cancelled,
    /// Run ended with an error. NOT resumed (the error is in the log;
    /// re-running would likely hit the same error).
    Errored,
    /// Resume gave up on this run — cap reached or too old. Terminal.
    Abandoned,
}
```

A run is **interrupted** iff the session's event log ends with one or more
`RunStarted` events and no `RunFinished` after the last one.

`Timestamp` is the existing alias in `src/session/events.rs` (`type Timestamp =
i64`, unix ms); `now_ms()` produces it. `RunOutcome` is `snake_case`-renamed
to match the file's `#[serde(rename_all = "snake_case")]` enums. Note
`SessionEvent` deliberately does **not** derive `PartialEq` — variant tests
compare on the serialized JSON form.

Both variants also need: a `state.rs` projection arm (no state mutation — they
are pure markers, like `SessionWoken`), and a `store.rs` `event_type` string
(`"run_started"` / `"run_finished"`) plus inclusion in the marker-event match.

### 3.2 Orchestrator emits the markers

`AgentHarnessRunner::run()` in `src/orchestrator/harness_bridge.rs` already
holds the session service handle (it seeds the session). It emits:

- `RunStarted { run_id, at }` — immediately before calling `harness.run()`.
- `RunFinished { run_id, outcome, at }` — immediately after `harness.run()`
  returns, in *all* paths (success and error).

`outcome` mapping:

| `harness.run()` result | `terminate_reason()` | `RunOutcome` |
|------------------------|----------------------|--------------|
| `Ok(())`               | (any)                | `Completed`  |
| `Err(Cancelled)`       | `Cancelled`          | `Cancelled`  |
| `Err(_)` other         | (any)                | `Errored`    |

The `run_id` for the marker pair is generated locally in
`AgentHarnessRunner::run` (a fresh `uuid::Uuid`). The orchestrator does not
receive the gateway scheduler's `run_id`; the marker pair only needs to
correlate within one session log, so a locally-minted UUID suffices.

Because the orchestrator *always* emits `RunStarted` (including on each resume
attempt), repeated crashes leave **N consecutive `RunStarted` events** — this
is exactly the resume-attempt counter (see 3.4).

### 3.3 `ResumeCoordinator`

New module `src/gateway/resume_coordinator.rs`. One public entry point:

```rust
pub struct ResumeCoordinator { /* event store, execution adapter,
                                  agent registry, channel registry, config */ }

impl ResumeCoordinator {
    /// Scan for interrupted runs and re-trigger each. Best-effort:
    /// any failure is logged and skipped; never panics, never blocks boot.
    pub async fn resume_interrupted_runs(&self) -> ResumeReport;
}
```

`resume_interrupted_runs` performs:

1. **Scan.** The `SessionEventStore` trait is per-session only, so a new
   cross-session method is added: `load_run_markers()` returns, per session,
   that session's `RunStarted`/`RunFinished` events in seq order. Its SQL —
   `SELECT session_id, seq, payload_json, created_at FROM session_events WHERE
   event_type IN ('run_started','run_finished') ORDER BY session_id, seq` — is
   served by the existing `(session_id, event_type)` index. The coordinator
   groups by session; a session whose newest marker is `RunStarted` is an
   interrupted-run candidate.

2. **Recency filter.** A candidate whose dangling `RunStarted.at` is older than
   `resume.max_age_secs` is *not* resumed — emit `RunFinished{Abandoned}` for it
   (so it is not re-scanned on the next boot) and skip.

3. **Cap check.** Count the trailing run of consecutive `RunStarted` events
   (events after the last `RunFinished`, or from the start if none). If that
   count `>= resume.max_attempts`, the run has crash-looped — emit
   `RunFinished{Abandoned}` and skip. (Default `max_attempts = 3`.)

4. **Crash-boundary repair.** Walk the tail of the event log. For every
   `ToolCallRequested { call_id, .. }` that has no matching `ToolResult` or
   `ToolError` with the same `call_id`, emit a synthetic
   `ToolError { call_id, error: "interrupted by server restart", .. }`. This
   makes the log valid for the provider API (every `tool_use` gets a
   `tool_result`) and lets the LLM decide whether to retry. Repair is a pure
   event-log append — the harness is never touched.

5. **Re-trigger.** Mirror the existing system-initiated-run precedent —
   `src/tasks/cron/executor.rs` and `src/tasks/heartbeat/executor.rs` both
   start agent runs with no inbound chat message. Resolve the agent from
   `AgentRegistry`, build a `RunRequest` with `metadata["resume"] = "true"`,
   build an `EventEmitter` (a collecting emitter is sufficient — a resumed run's
   terminal reply is delivered through the channel registry, reconstructed from
   the `SessionKey`), and call `ExecutionAdapter::execute` directly. Cron and
   heartbeat already bypass `SessionScheduler`, so a system-initiated run racing
   an inbound message on the same session is a *pre-existing, accepted*
   condition — resume needs no bespoke scheduler integration. A semaphore of
   `resume.max_concurrent` permits in the `ResumeCoordinator` bounds the boot
   burst.

`ResumeReport` is a small struct (`scanned`, `resumed`, `abandoned`, `skipped`)
for the boot log line and for tests.

### 3.4 Resume mode — skip the seed

The resume signal rides `RunRequest.metadata["resume"] = "true"` — a new map
entry, **not** a new struct field, so the ~18 existing `RunRequest`
construction sites are untouched. At the execution-engine → orchestrator
boundary (where a `RunRequest` becomes a `FlowInput`), when `metadata["resume"]`
is set the engine produces a new `FlowInput::Resume` variant instead of
`FlowInput::Prompt`.

`AgentHarnessRunner::run` then receives `FlowInput::Resume`:

- `session_seed::seed_session` treats `Resume` as a **no-op** — the input is
  already a `UserMessage` event in the log being replayed; re-seeding would
  duplicate it.
- `last_user_query` returns `""` for `Resume` (no retrieval query).
- Every other exhaustive `match` on `FlowInput` (`flow_run_tool.rs`, the
  `orchestrator/tests/dispatch.rs` fixtures) gets a `Resume` arm.
- Everything else is identical — emit `RunStarted`, call `harness.run()` (which
  replays the repaired log and continues), emit `RunFinished`.

### 3.5 Boot wiring + config

In `src/bin/aleph-server/commands/start/`, after the gateway and execution
subsystems are constructed, spawn `ResumeCoordinator::resume_interrupted_runs`
as a detached background task — boot is **not** blocked on it.

New config section (all fields optional, with defaults):

```toml
[resume]
enabled = true          # master switch
max_age_secs = 86400    # don't resume runs interrupted > 24h ago
max_attempts = 3        # abandon after this many crash-loops
max_concurrent = 4      # cap simultaneous resumes at boot
```

When `enabled = false`, the coordinator is not spawned at all.

---

## 4. Data Flow

**Normal run.** Message → `SessionScheduler` → `ExecutionAdapter::execute` →
`AgentHarnessRunner::run()` seeds the user message, emits `RunStarted`, runs the
harness, emits `RunFinished{Completed|Cancelled|Errored}`.

**Crash mid-run.** The above proceeds through `RunStarted` and some turns, then
the process dies. The event log ends with `RunStarted` + partial turn events,
possibly a dangling `ToolCallRequested`.

**Boot resume.**
1. `ResumeCoordinator` scans → finds the session with a trailing `RunStarted`.
2. Recency + cap checks pass.
3. Crash boundary repaired — synthetic `ToolError` appended for each dangling
   tool call.
4. `RunRequest` built with `metadata["resume"]="true"`; `ExecutionAdapter::execute`
   invoked (cron-executor precedent).
5. At the engine→orchestrator boundary `metadata["resume"]` becomes
   `FlowInput::Resume`. `AgentHarnessRunner::run()` skips seeding, emits a fresh
   `RunStarted`, calls `harness.run()` — which replays the repaired log and
   continues the task.
6. Run ends → `RunFinished`.

---

## 5. Error Handling

- **Scan / DB failure:** logged; `resume_interrupted_runs` returns an empty
  report. Boot is unaffected.
- **Per-run failure** (agent missing, adapter error): isolated in a per-run
  `try`/log; other runs still resume.
- **Boot storm:** `resume.max_concurrent` semaphore bounds simultaneous
  resumes, protecting the freshly-booted process and provider rate limits.
- **Crash-loop:** the `max_attempts` cap converts a poisoned run into
  `RunFinished{Abandoned}` — terminal, never re-scanned.
- **Stale candidates:** runs older than `max_age_secs` are marked `Abandoned`
  so the scan cost stays bounded over time.

---

## 6. Redline Compliance

- **R3 (Core Minimalism):** no parallel persistence — two event variants on the
  existing log, one new gateway module. No new table; one query method added to
  the existing `SessionEventStore` trait (no new trait).
- **R10 (Thin Harness):** `src/harness/` is not modified. The harness already
  replays; resume detection, boundary repair, and re-trigger all live in
  gateway/orchestrator code.
- **R7 (LLM Sovereignty):** a dangling tool call becomes a `ToolError`; the LLM
  decides whether to retry — deterministic code does not re-plan.
- **R4 (I/O-Only Interfaces):** `ResumeCoordinator` is gateway *control-plane*
  (run orchestration), not a Channel/Bot/CLI/Panel — it does not process user
  I/O. Allowed.
- **P4 (Dependency Inversion):** `ResumeCoordinator` depends on the existing
  `SessionEventStore` / `ExecutionAdapter` traits, not concrete types.

---

## 7. Testing

**Unit:**
- `RunStarted` / `RunFinished` / `RunOutcome` serde round-trip.
- `state.rs` projection: the two variants apply as no-op markers.
- `ResumeCoordinator` scan classification, against a synthetic in-memory event
  store, for each session-tail shape: clean (`RunFinished` present), dangling
  `RunStarted`, dangling tool call, cap-exceeded (N×`RunStarted`), too-old.
- Crash-boundary repair: a dangling `ToolCallRequested` yields exactly one
  synthetic `ToolError` with the matching `call_id`; an already-answered call
  yields none.
- Cap: `max_attempts` consecutive `RunStarted` → `RunFinished{Abandoned}`, no
  re-trigger.

**Integration:**
- Seed an event store with a complete interrupted run (user message, a turn, a
  dangling tool call, trailing `RunStarted`). Run `resume_interrupted_runs`
  against a mock `ExecutionAdapter`. Assert: the synthetic `ToolError` was
  appended, and `execute` was called once with `metadata["resume"] == "true"`.
- `resume.enabled = false` → coordinator never calls `execute`.

---

## 8. Scope

Approximately 7 implementation tasks:

1. `RunStarted` / `RunFinished` / `RunOutcome` event variants + `state.rs`
   projection + `store.rs` `event_type` strings.
2. `FlowInput::Resume` variant + `seed_session` / `last_user_query` / other
   exhaustive-match arms + engine→orchestrator `metadata["resume"]` conversion.
3. Orchestrator emits `RunStarted` / `RunFinished` around `harness.run()`.
4. `SessionEventStore::load_run_markers()` cross-session query.
5. `ResumeCoordinator` — scan + recency + cap + crash-boundary repair.
6. `ResumeCoordinator` — re-trigger (cron-executor precedent) + boot wiring +
   `[resume]` config section.
7. Integration tests + audit.

---

## 9. Deferred / Cross-Cycle Notes

- **User notification on abandonment** — proactively telling the user "your task
  could not be resumed" (R5) is a separate feature. Deferred.
- **Explicit manual resume** — a user-invokable resume tool/RPC. Deferred (the
  cycle scope is boot-scan only).
- **Session-split (Cycle 5) interaction.** Cycle 6 branches off `main`, which
  does not yet contain Cycle 5's session-split. When *both* land on `main`, the
  split path (`perform_session_split`) must emit balanced run markers — a
  `RunFinished{Completed}` on the parent session and a `RunStarted` on the child
  — so the parent is not later mis-detected as an interrupted run. This is a
  small follow-up integration commit, to be done when the second of the two
  cycles merges. Documented here; not implemented in Cycle 6.

---

## 10. References

- `docs/superpowers/specs/2026-04-25-p6-checkpoint-boot-design.md` — the P6
  retraction of a parallel checkpoint store; this spec rides the event log
  instead, per that precedent.
- `docs/reference/SESSION_SERVICE.md` — event-sourced session model.
- `CLAUDE.md` R3, R4, R7, R10, P4.

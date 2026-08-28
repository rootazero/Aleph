# Logic Review Report
**Module**: `src/session/`
**Scope**: src/session/{actor.rs, epoch_registrar.rs, events.rs, in_process.rs, mod.rs, observer.rs, projection.rs, service.rs, steer_signal.rs, store.rs, tool_trace.rs} — 4372 LOC
**Date**: 2026-08-29
**Branch**: `audit/2026-08-29-session`
**Mode**: normal (security-critical — the SSOT log is the canonical source of truth for prompt replay)

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 2 |
| Suggested Test | 1 |

## Findings

### [Warning] `wake_locks` per-key mutex map grows unboundedly across distinct sessions
- **Location**: `src/session/in_process.rs:37, 48, 219–227, 286–315`
- **Trigger condition**: `wake()` is called once on session `S`, then `detach()` runs. The per-key `Arc<Mutex<()>>` allocated for `S` is never removed from `InProcessActorSessionService::wake_locks: tokio::sync::Mutex<HashMap<SessionId, Arc<Mutex<()>>>>`. The companion maps (`senders`, `broadcasters`) ARE removed by `detach()`, so this is a real asymmetry.
- **Expected behavior**: `detach()` releases every per-session resource; future `wake()` for the same session simply re-creates the entry.
- **Actual behavior**: A long-running daemon that wakes and detaches many distinct session ids (subagent ephemeral sessions, compaction children, room peers) accumulates one `Arc<Mutex<()>>` per unique id, forever. At ~hundreds of bytes per entry, a subagent-heavy workload at scale silently grows the map without bound.
- **Suggested fix**: One-line addition to the end of `detach()`, mirroring `senders.remove(id)` / `broadcasters.remove(id)`:

  ```rust
  self.wake_locks.lock().await.remove(id);
  ```

  Applied in this PR. A regression test (`detach_releases_the_wake_lock_entry`) was added in `src/session/in_process.rs` to lock in the behavior.

### [Warning] Dead "lagging subscriber" warn branch in `SessionActor::run`
- **Location**: `src/session/actor.rs:151–166` (original; replaced in this PR)
- **Trigger condition**: any append through the actor's main `EmitEvent` arm.
- **Expected behavior**: the doc and the `tracing::warn!` claim that the second clause (`&& receiver_count > 0`) distinguishes a "buffer full, slow subscriber" case from "no receivers".
- **Actual behavior**: tokio's `broadcast::Sender::send` only returns `Err` when `rx_cnt == 0` (verified at `tokio-1.52.3/src/sync/broadcast.rs:631`). A full `BROADCAST_BUFFER` (=256) does NOT error and instead lets the lagging receiver observe `RecvError::Lagged` on its next `recv()`. Therefore `send.is_err() && receiver_count > 0` is structurally false — the warn branch was dead code, and the doc on it was factually wrong about tokio's API. Operators had **no** log signal when a live subscriber was lagging.
- **Suggested fix**: replace with an honest, single-condition `if let Err(_record) = self.broadcaster.send(record)` branch that emits a `tracing::debug!` ("no receivers; event is durable in the SSOT log only"). The lagging-receiver detection is now correctly attributed to the *subscriber* side (`RecvError::Lagged`). Applied in this PR; the new comment also tells future readers *why* there is no producer-side lagger warn.

### [Note, not raised as a Warning] Self-heal retry asymmetry between main path and idle-drain path
- **Location**: `src/session/actor.rs:91–120` (main path) vs `src/session/actor.rs:209–232` (idle-drain path)
- **Observation**: the main path retries once on `(session_id, seq)` UNIQUE collision (after re-reading `head_seq` from the store); the idle-drain path does not.
- **Why this is NOT a bug**: the actor is about to terminate after the drain (the drain loop is the very last step before "return" inside the idle-timer arm). A retry is wasted work — the next `attach()` will spawn a fresh actor that replays from SQLite and starts from the correct `head_seq`. The caller receives `Err(SessionError::Storage(...))` and on retry goes through `attach()` → fresh actor → correct seq.
- **Could be worth a one-liner comment** on the drain's `EmitEvent` arm to explain why a retry isn't worth it. Not done here to keep the PR focused on the two real warnings above.

### [Note, not raised as a Warning] `note_steer_has_exactly_one_production_call_site` only inspects 4 files
- **Location**: `src/session/steer_signal.rs:498–547`
- **Observation**: the source-level census hardcodes a 4-file list (`gateway/execution_engine/{steering, gate, execute}.rs`, `gateway/session_projector.rs`). A new producer added to a different file (e.g. `builtin_tools/`) would silently slip past the assertion.
- **Why this is NOT raised as a Warning**: (1) the doc above the test explicitly says "Only files that could plausibly hold a producer", which is a deliberate maintenance choice; (2) the test is the only behavior-level guarantee we have that the wake edge has *exactly one* producer, and tightening it (e.g. globbing the whole crate) would break on incidental mentions. A future change that adds a second producer needs to (a) update the file list and (b) understand why this is structural (per the module doc on `SteerWatch`). Calling that out in the report, not in the file.

## Findings (cross-cutting inside the module)

- The `record_row` projection (already documents every internal-marker branch's intent) and the `event_type_tag` mirror in `store.rs` are independent enumerations that must stay in lockstep. A new `SessionEvent` variant MUST add an arm in both. Same for `extract_turn_id`. Existing pattern: `#[non_exhaustive] enum SessionEvent` plus tests that exercise each discriminant through serde. No bug found; mentioned for the future auditor.
- `SessionEventRecord` deliberately does not implement `PartialEq` (variants with non-comparable payloads); tests use `serde_json::to_string` round-trips. Consistent. No bug found.

## Wiring Audit Summary
| Component | Verification | Notes |
|---|---|---|
| `SessionService` trait + impl | `attach`, `emit_event`, `get_events`, `subscribe`, `wake`, `detach` all used by `harness`, `agents`, `gateway`, `builtin_tools` (graph: 127 cross-module call sites) | OK |
| `SessionEventStore` trait + `SqliteEventStore` | 15+ test/integration call sites | OK |
| `SessionEventObserver` (`MessageProjector`) | registered in `bin/aleph-server/commands/start/mod.rs:436`, used at `gateway/session_projector.rs:333` | OK |
| `SessionEpochRegistrar` | registered in `gateway/session_store/sqlite_backend/mod.rs:639`, used by `context::compact::{directive, session_split}` and `harness::deps` | OK |
| `note_steer` / `SteerWatch` | sole production producer confirmed by `note_steer_has_exactly_one_production_call_site`; consumers in `builtin_tools/{bash_exec,desktop/{wait_visual,verify_state},browser_tools/wait_for}` and `agents/subagent_tool::loop_tool` | OK (with the note above on the test's hardcoded file list) |
| `with_session_scope` | re-exported at `session::mod`; tests cover the round-trip | OK |
| `retire_live_events` | called from `gateway/handlers/chat.rs` (×2), `gateway/handlers/session/db_handlers/modify.rs` (×2), `gateway/agent_instance.rs`, `gateway/continuation_lifecycle.rs` | OK |
| `is_event_retired` | called from `gateway/session_projector.rs:141` (the `event_retired` guard) | OK |
| `set_global_session_service` / `set_global_session_event_store` | installed in `bin/aleph-server/commands/start/helpers.rs`; `decline_*` paths also wired (`bin/aleph-server/commands/start/mod.rs:457, 470`) | OK |
| `migrate_add_session_events` / `migrate_add_session_events_fts` | all `tests/`, `src/gateway/`, `bin/aleph-server/` SQLite bootstrap paths use them | OK |

### Total pub fns in module: 38
- **Verified callers**: 38 (every public function has an observable caller or is installed via the global handles; all four global-handle accessors are exercised through their install/decline pairs and through `install_test_event_store`).
- **Orphaned pub fns**: 0

### Sync primitives check (Aleph invariant R6)
- `src/session/{actor, in_process, store, events, mod, observer, projection, service}.rs`: use `tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot}` directly. These are async primitives and do not interact with the loom-instrumented sync layer. Acceptable per the SKILL's documented exceptions ("types that interface with external crate APIs"; tokio's channels are exactly that). No `std::sync::Mutex`/`RwLock` use anywhere in the module.
- `src/session/steer_signal.rs`: uses `crate::sync_primitives::Mutex` (per the inventory of inline tests + the registry being a globally-shared sync structure). Correct.
- `src/session/tool_trace.rs::tests`: uses `crate::sync_primitives::Arc` + `crate::sync_primitives::Mutex` for the test spy. Correct.

No R6 violations.

### Lock-hierarchy check (Aleph invariant R7)
- The only sync_primitives lock acquired in the module is `steer_signal::stations()`, which guards a per-session registry map. No actor holds a sync_primitives lock while making a call that traverses other modules' locks. No cross-level ordering risk observed.

### TOCTOU check (Aleph invariant R8)
- `actor.rs::run`: the `(session_id, seq)` UNIQUE collision path uses one atomic mutex (the store's connection) for retry — same critical-section resync logic as `gateway/execution_engine`'s reference fix. Good.
- `in_process.rs::spawn_actor`: double-checks `sender.is_closed()` inside its own write-lock; the window is bounded by the write-lock scope. Good.
- `in_process.rs::subscribe`: fast path holds `senders.read()`/`broadcasters.read()` for the whole snapshot → no TOCTOU within the read-lock scope. Good.
- `in_process.rs::wake`: per-key mutex serialises the shutdown-old/spawn-new/emit-marker sequence; the lock is held across all four steps. Good.

## Suggested Tests (one applied)

### [Suggested Test] `detach_releases_the_wake_lock_entry` (applied)
- **Location**: `src/session/in_process.rs::tests`
- **What it covers**: the regression for the unbounded-leak warning above. Two asserts (entry present after `wake()`, absent after `detach()`).
- **Why it matters**: without a sentinel test, a future refactor that drops the `wake_locks.remove(id)` line would silently regress, because the only externally-observable consequence is a slow `Arc<Mutex<()>>` per distinct session id over hours/days of uptime — easy to miss in unit runs.

## Files Modified by this Audit

- `src/session/in_process.rs` — `detach()` now removes the `wake_locks` entry; added `detach_releases_the_wake_lock_entry` test.
- `src/session/actor.rs` — dead "lagger warn" branch replaced with a simple "no receivers" debug + corrected comment about tokio's broadcast::Sender semantics.

## What was NOT reviewed
- `src/bin/aleph-server/commands/start/{helpers, orchestrator_init, mod}.rs` global-handle installation paths (orchestrator_wiring scope; this audit scoped to `src/session/`).
- `src/gateway/session_projector.rs` consumer of `is_event_retired` / `SessionEventObserver` (consuming-module scope).
- `src/gateway/btw/*`, `src/agents/subagent_tool/loop_tool.rs`, `src/builtin_tools/{bash_exec, desktop/*, browser_tools/*}.rs` consumers of `SteerWatch` (consuming-module scope).
- `tests/session_*` integration tests (test-machinery scope; integration of fixes covered by the unit tests in this PR).
- L1/L2 (`just test-proptest` / `just test-loom`) verification per the orchestrator's plan (this subagent ran no cargo commands per the brief).
- `src/session/tool_trace.rs` production path (the test there is incidental; the real `with_session_scope` is trivial `.scope(...).await` over a tokio task-local and was reviewed by reading).

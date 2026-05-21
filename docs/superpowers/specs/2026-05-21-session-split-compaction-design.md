# Session-Split Compaction — Design Spec

**Cycle**: Cycle 5 — Long-task hardening follow-up
**Date**: 2026-05-21
**Scope**: Item 1 of 4 deferred from [Cycle 2](./2026-05-20-long-task-hardening-design.md) — the last of the four.
**Net LOC estimate**: +450 / −30

## Problem Statement

In-place compaction (`ContextCompactor::compact(&mut messages, …)`) is recomputed
**every turn** from the full session event log. As a long autonomous task runs
for hundreds of turns, the `session_events` log grows unboundedly:
`get_events()` returns an ever-larger log, `prompt_builder.assemble()` does more
work, and `compact()` has more history to summarize — per-turn cost grows
linearly with task length even though the LLM only ever sees a bounded window.
Repeated in-place summarization also degrades quality: a summary-of-a-summary
loses fidelity each round.

**Session-split compaction** fixes both: when in-place compaction can no longer
keep pressure down, the harness ends the current session and continues in a
fresh one (`epoch + 1`), seeded with a single clean summary plus the verbatim
fresh tail. The parent session's full log is frozen — never re-read by the
loop — so per-turn cost resets to bounded. Cross-session recall
(`session_search`, already shipped) reaches back into the parent for detail.

## Scope

### In Scope (Cycle 5)

- **Tiered trigger** — in-place compaction stays for the `warning` tier; session-split
  fires when the `CompactionCircuitBreaker` trips (in-place is not keeping up).
- **`LoopDirective::SplitSession`** — new directive variant emitted by `ContextBudget`.
- **`perform_session_split`** — a new `src/context/compact/session_split.rs` module
  that creates the child session (`epoch + 1`), seeds it (summary + fresh tail),
  and registers the new epoch.
- **Harness loop integration** — `run()` holds a mutable current-session; `think.rs`
  handles `SplitSession`; the harness reports the final session id back.
- **Lineage** — recorded via the child's first event; `epoch` encodes the generation.

### Out of Scope

- Changing in-place compaction's `warning`-tier behavior — untouched.
- Cross-session recall mechanics — `session_search` / `session_search_summary`
  already exist and are not modified.
- A user-facing "view session lineage" tool — the data is recorded; surfacing it
  is a separate cycle.
- Splitting non-epoch session kinds (`Group`, `Task`, `Subagent`, `Ephemeral`) —
  `with_next_epoch()` returns those unchanged; for them, split degrades to the
  existing `FinalReply` fallback.

## Background — what already exists

- **`SessionKey::epoch`** — every `Main` / `DirectMessage` variant carries
  `epoch: u32`. `with_next_epoch()` returns the same key with `epoch + 1`.
  `epoch()` reads it. This is live infrastructure: the user-facing `/new` tool
  (`src/builtin_tools/sessions/new_tool.rs`) and the gateway already bump epoch
  to start fresh session generations.
- **`SessionStore::get_or_create(&key)`** (`src/gateway/session_store/`) — persists
  a session at a given key/epoch. `get_current_epoch(base_pattern)` derives the
  max epoch for a base key. The `/new` flow calls `get_or_create` on the
  next-epoch key; the gateway router then resolves the current epoch from it.
- **`ContextCompactor`** — already a harness dep (`deps.context_compactor`), holds
  the LLM summarizer + `SessionSummarySource` (zero-cost summary reuse).
- **`CompactionCircuitBreaker`** (`src/context/budget/mod.rs`) — counts consecutive
  compactions; `before_turn` escalates to `FinalReply` when it trips.
- **Cross-session recall** — `session_search` reads across session generations.

Session-split is therefore *automatic epoch bump + seeding*, reusing proven
mechanisms rather than inventing a lineage model.

## Design

### §1 — Tiered trigger

`ContextBudget` (`src/context/budget/mod.rs`) gains a per-run split counter:

```rust
pub struct ContextBudget {
    // … existing fields …
    split_count: usize,
    max_splits: usize,   // from config, default 3
}
```

`before_turn` directive resolution changes **only** at the circuit-breaker-trip
branch:

- Below `warning` → `Continue` (unchanged)
- `warning`..`critical` → `CompactAndContinue` (unchanged — in-place)
- Circuit breaker trips (in-place not keeping up):
  - if `split_count < max_splits` → **`SplitSession`** (new)
  - else → `FinalReply` (existing final fallback)
- `critical` threshold with no breaker history → `FinalReply` (unchanged)

`split_count` increments when the harness reports a completed split (see §4).
`max_splits` caps runaway splitting — after 3 splits in one run, fall back to
terminating with `FinalReply`.

### §2 — `LoopDirective::SplitSession`

```rust
pub enum LoopDirective {
    Continue,
    CompactAndContinue,
    FinalReply,
    StopDiminishing,
    SplitSession,   // NEW — Cycle 5
}
```

`LoopDirective` lives in `src/context/budget/` — not `src/harness/` — so adding a
variant does not touch the harness's R10 LOC budget. The harness's handling of
the variant is a single mechanical `match` arm (see §4).

### §3 — `perform_session_split` (`src/context/compact/session_split.rs`, new)

A free async function — lives in the Context module (one of the 12 harness
modules, deliberately outside `src/harness/`):

```rust
pub struct SplitOutcome {
    pub child_session_id: SessionId,
}

pub async fn perform_session_split(
    session: &dyn SessionService,
    epoch_registrar: &dyn SessionEpochRegistrar,
    compactor: &ContextCompactor,
    parent_session_id: &SessionId,
    events: &[SessionEventRecord],
    tail_start: usize,
) -> anyhow::Result<SplitOutcome>;
```

Steps:

1. **Fresh-tail boundary** — `tail_start` is passed in by the caller. `think.rs`
   already computes it via the harness-private `tail_start_index(events)` at the
   top of every turn (`super::tail_start_index`), so the split module receives
   the index rather than recomputing it — this avoids exposing a harness-private
   helper across the module boundary. `events[..tail_start]` is summarized;
   `events[tail_start..]` is the fresh tail copied verbatim.
2. **Summary** — feed the events *before* the tail to `ContextCompactor` to
   produce the `[Context Summary]` text. Reuse `SessionSummarySource` for the
   zero-API-cost path when summaries already exist. If summarization fails,
   return `Err` — the caller falls back to `FinalReply` (fail-soft, §4).
3. **Mint child** — `child = parent_session_id.with_next_epoch()`. If the key
   kind has no epoch (`with_next_epoch()` returns it unchanged → `child ==
   parent`), return `Err(NotSplittable)` so the caller falls back to `FinalReply`.
4. **Register epoch** — `epoch_registrar.register_epoch(&child).await?` so the
   gateway's `get_current_epoch` resolves to the new generation (see §5/R1).
5. **Seed the child** — emit, in order, to the child session via `SessionService`:
   - `SessionEvent::SessionForked { parent_session_id: parent.to_key_string() }`
     — a NEW lightweight variant recording explicit lineage.
   - `SessionEvent::SystemMessage` carrying the `[Context Summary]` text.
   - every fresh-tail event from `events[tail..]`, copied verbatim (new `seq`
     numbers under the child session id).
6. Return `SplitOutcome { child_session_id: child }`.

### §4 — Harness loop integration

**`SessionEpochRegistrar` trait** — a new narrow trait in the core/session layer
(`src/session/`), so the harness depends on an abstraction, not the gateway
(P4 dependency inversion; avoids a Core→Interface layer inversion):

```rust
#[async_trait]
pub trait SessionEpochRegistrar: Send + Sync {
    /// Persist `key` as a live session generation so epoch resolution
    /// (`get_current_epoch`) sees it.
    async fn register_epoch(&self, key: &SessionId) -> anyhow::Result<()>;
}
```

The gateway `SessionStore` implements it (`register_epoch` delegates to its
existing `get_or_create`). `HarnessDeps` gains
`session_epoch_registrar: Option<Arc<dyn SessionEpochRegistrar>>` — `None`
disables session-split (the loop falls back to `FinalReply`), so the feature is
opt-in by wiring, consistent with `context_compactor`/`context_budget`.

**`run()`** (`src/harness/agent.rs`) — replace the immutable `session_id:
&SessionId` threading with a mutable owned binding:

```rust
let mut current_session: SessionId = session_id.clone();
// … each turn passes &current_session …
// on a split signal from the turn, rebind:
//   current_session = child_id;
```

**`think.rs`** — handle the `SplitSession` directive alongside the existing
`CompactAndContinue` / `FinalReply` arms. When `before_turn` returns
`SplitSession` and both `context_compactor` and `session_epoch_registrar` are
wired:
- call `perform_session_split(...)`;
- on success: signal the new session id back to `run()` (extend the turn return
  with `Option<SessionId>`, or a `TurnState::SplitTo(SessionId)` — implementer's
  choice, pinned in the plan); `run()` rebinds `current_session`; the loop
  continues normally in the child;
- on error (summarization failed / not splittable): fall through to the existing
  `FinalReply` path — fail-soft, the run still terminates cleanly.

**Harness return** — `run()` already returns a result; extend the harness so the
orchestrator can read the *final* session id (the latest epoch). `harness_bridge`
(`src/orchestrator/`) reads it and updates its view (see §5/R2).

R10 check: the harness's added code is — one `match` arm dispatching to
`perform_session_split` (which lives outside `src/harness/`), and `run()`
rebinding a `SessionId`. No intent classification, no completion judgment, no
new heuristics. Mechanical scaffolding only. The split *decision* is made by
`ContextBudget` (a non-harness module). Compliant.

### §5 — Cross-layer coordination (the two real risks)

**R1 — epoch registration.** The gateway resolves the "current" epoch via
`SessionStore::get_current_epoch`. After a split, that must return the new
epoch, or inbound routing lands on the stale generation. Resolution: the split
calls `SessionEpochRegistrar::register_epoch(&child)` (§4), whose gateway
implementation calls the same `SessionStore::get_or_create` the `/new` tool
already uses. The harness never imports a gateway type — it sees only the narrow
core trait. When the registrar is not wired (`None`), session-split is disabled
and the loop uses `FinalReply`.

**R2 — orchestrator view.** `harness_bridge` resolves `session_id` from a key
string and runs the harness against it. After an internal split, that id is one
epoch stale. Resolution: the harness exposes the final session id; `harness_bridge`
updates its local `session_id` to the final epoch before persisting run results
/ trace. Any other consumer re-resolves through `get_current_epoch` (already the
gateway's normal path). The parent session's events remain intact and addressable.

## R-rule Compliance

| Rule | Check |
|------|-------|
| R1 (Brain-Limb) | Core/harness depends on the narrow `SessionEpochRegistrar` trait, not the gateway `SessionStore` concrete type — no layer inversion. |
| R3 (Core Minimalism) | Reuses `epoch` / `with_next_epoch` / `get_or_create` / `ContextCompactor` / `CompactionCircuitBreaker`. New code: one module + one trait + one directive variant + one event variant. No new dependency. |
| R4 (I/O-Only Interfaces) | Split logic is in the Context module, not an Interface. |
| R10 (Thin Harness) | The split *decision* is in `ContextBudget`; the split *logic* is in `src/context/compact/`. The harness gains only a `match` arm + a `SessionId` rebind — scaffolding, not cognition. No new harness intelligence. |

## Testing

| Layer | Coverage |
|-------|----------|
| Unit — `session_split.rs` | fresh-tail boundary respected; child key is `parent.with_next_epoch()`; child seeded with `SessionForked` + summary `SystemMessage` + verbatim fresh-tail events in order; non-epoch key kind → `Err(NotSplittable)`; summarizer failure → `Err`. |
| Unit — `ContextBudget` | circuit-breaker trip with `split_count < max_splits` → `SplitSession`; with `split_count == max_splits` → `FinalReply`; `split_count` increments on reported split. Existing breaker→`FinalReply` tests updated to breaker→`SplitSession`. |
| Unit — `SessionEpochRegistrar` | gateway `SessionStore` impl: `register_epoch` makes a subsequent `get_current_epoch` return the new epoch. |
| Integration — harness | `SplitSession` directive → `run()` rebinds `current_session` to the child → the loop's next turn fetches events from the child (short log, contains summary + fresh tail) → run continues and completes; the harness reports the child id. |
| Integration — fail-soft | summarizer error on `SplitSession` → loop falls back to `FinalReply`, terminates cleanly, no panic. |

## Risks

| ID | Risk | Mitigation |
|----|------|------------|
| R3 | `trace_sink` / `stall_tracker` may assume a fixed session id for a run. | Audit both during implementation. Trace events carry their own ids; the stall tracker is time-based, not session-keyed — expected safe, but verify with a test. |
| R5 | A split mid-run while a concurrent inbound message routes to the base key could race on epoch. | `register_epoch` is the same path `/new` uses; `get_current_epoch` takes the max epoch — last-writer-wins is acceptable, matching existing `/new` semantics. |
| R6 | Fresh-tail copy duplicates events under the child id — storage cost. | Fresh tail is bounded (`fresh_tail`, ~a few turns). The parent's old events are frozen, not copied. Net storage is bounded. |
| R7 | `max_splits` cap reached on a genuinely huge task → `FinalReply` terminates it. | Acceptable — 3 splits already extend a task ~3× the single-window budget; beyond that, terminating with a summary is the correct safety stop. Cap is config-tunable. |

## Implementation Order

1. `SessionEvent::SessionForked` variant + serialization round-trip test.
2. `SessionEpochRegistrar` trait (`src/session/`) + gateway `SessionStore` impl + test.
3. `LoopDirective::SplitSession` variant + `ContextBudget` split-counter + tiered
   `before_turn` resolution + unit tests (breaker→Split, cap→FinalReply).
4. `src/context/compact/session_split.rs` — `perform_session_split` + unit tests.
5. `HarnessDeps.session_epoch_registrar` field; `run()` mutable `current_session`;
   `think.rs` `SplitSession` arm; harness reports final session id.
6. `harness_bridge` reads the final session id (R2).
7. Integration tests — happy-path split + continue; fail-soft fallback.

Each step its own commit. Worktree branch: `worktree-feat-session-split`.

**Merge policy:** per the user's 2026-05-21 instruction, do NOT merge this branch
into `main` after implementation — stop at "ready" and wait for explicit
instruction.

## Reference

- `SessionKey` / `epoch` / `with_next_epoch`: `src/routing/session_key.rs`
- `SessionStore::get_or_create` / `get_current_epoch`: `src/gateway/session_store/mod.rs`
- `/new` epoch-bump precedent: `src/builtin_tools/sessions/new_tool.rs`
- `ContextCompactor` / `compact`: `src/context/compact/compactor.rs`
- `CompactionCircuitBreaker` / `LoopDirective` / `before_turn`: `src/context/budget/mod.rs`
- Harness loop: `src/harness/agent.rs` (`run`), `src/harness/agent/think.rs`
- Orchestrator bridge: `src/orchestrator/harness_bridge.rs`
- Cross-session recall (unchanged, consumes the lineage): `src/memory/session_search_summary/`

# Resumable Run Streaming (seq-cursor catch-up)

**Date**: 2026-06-09
**Scope**: Aleph gateway — Core protocol layer (Panel UI adoption deferred)
**Goal driver**: gateway gap-analysis vs openclaw / hermes-agent / Pi

## Problem (gap analysis, source-verified)

`run_event_bus::RunEvent` stamps a monotonic `seq: u64` on **every** variant
(TokenDelta / ReasoningDelta / ToolStart / ToolEnd / …) via
`ActiveRunHandle::next_seq()`. The seq is serialized onto the wire on every
token delta — yet across the entire core it has **zero readers** (`grep .seq`
hits only unrelated `group_chat` / `memory` subsystems). It is an
"advertised-but-unwired" load-bearing-looking field.

Consequence: `ActiveRunHandle::subscribe()` (the only non-test caller, in
`handlers/runs.rs`) returns a **fresh broadcast receiver = events from now
forward only**, with no replay. When a client is dropped (slow-consumer 1008,
or a network blip) mid-run and reconnects, it:

1. gets a `HelloSnapshot` that recovers **state domains only** (presence /
   health / config) and says **nothing about in-flight runs**;
2. re-subscribes and receives only **future** events;
3. **permanently loses the run-stream events emitted during the gap.**

`wait_for_run_end` even surfaces `WaitError::Lagged(n)` as an error
("missed N events") — a known failure mode that was never recovered.

### Reference patterns

| Project | Resume mechanism |
|---|---|
| hermes-agent | `_replay_session_history()` — **full** inline replay on load/resume, **blocks** the load response, unbounded |
| openclaw | per-client `seq` + `messageSeq` **cursor** replay; close-on-slow-consumer |
| Pi | session file rebuild on re-spawn |
| **Aleph today** | `seq` emitted, **no consumer**; close-on-slow-consumer → hello resync (**state only**, no run stream) |

## Design — bounded incremental catch-up

Wire the existing `seq` into a **bounded, seq-indexed replay ring** so a
reconnecting client replays only the gap (`seq > since_seq`), non-blocking,
type-safe cursor. Surpasses hermes (bounded incremental vs unbounded full
replay) and openclaw (run-stream replay vs none).

### ① Replay ring on `ActiveRunHandle` (`run_event_bus.rs`)

- New field `replay: Arc<Mutex<ReplayBuffer>>` where `ReplayBuffer` holds a
  `VecDeque<RunEvent>` (cap `RUN_REPLAY_CAP = 512`) + `oldest_retained_seq`.
- The emit path becomes: **push to ring under lock, then broadcast** — the ring
  is always the source of truth for `seq <= current_seq`. Over cap → `pop_front`
  and advance `oldest_retained_seq`.
- `emit()` keeps its signature/return; the ring push is internal.

### ② `subscribe_from(since_seq: Option<u64>)`

```
pub fn subscribe_from(&self, since_seq: Option<u64>)
    -> (ReplayOutcome, broadcast::Receiver<RunEvent>)
```

- **Under the ring lock**, atomically: (a) collect events with
  `seq > since_seq` (or all retained if `None`), (b) `event_tx.subscribe()`.
  One lock → catch-up boundary and live receiver start are coherent: **no gap,
  no duplicate** across the seam (the classic race; must be a single critical
  section).
- `ReplayOutcome`:
  - `Replaying { events: Vec<RunEvent>, current_seq }` — normal.
  - `Truncated { oldest_seq, current_seq }` — `since_seq < oldest_retained_seq`;
    client must fall back to full history refetch (graceful degradation reusing
    the existing chat-history path). No events returned (incomplete).
- `subscribe()` is kept as `subscribe_from(None)`-with-no-catch-up for existing
  callers (`run.wait`) — **behavior byte-identical** for them.

### ③ `run.subscribe` RPC (`handlers/runs.rs`)

New handler, registered alongside `run.wait` / `run.queue_message`. Does **not**
overload `run.wait` semantics.

- Params: `{ run_id: String, since_seq: Option<u64> }`
- Response (`RunSubscribeResponse`, tagged):
  - `Replaying { events: Vec<RunEvent>, current_seq: u64 }`
  - `Truncated { oldest_seq: u64, current_seq: u64 }`
  - `NotFound` — run absent from the registry (already completed + evicted);
    client falls back to chat history (terminal state is already persisted).
- Live stream continues via the existing event_bus topic path; the client
  dedups by `seq` (drops live events with `seq <= max replayed seq`).

### ④ `HelloSnapshot.active_runs` (`hello_snapshot.rs`)

- New field `active_runs: Vec<ActiveRunSummary>` with
  `ActiveRunSummary { run_id, session_key, status, current_seq }`.
- Lets a reconnecting client (incl. a *different* device) discover in-flight
  runs and the seq to resume from, without polling.
- Source: the run registry, injected into `build_hello_snapshot` via a
  **OnceLock provider** (mirrors existing `origin_fanout::set_channel_registry`
  / `set_node_registry`) — does **not** thread a new dep through `AuthContext`'s
  constructor. When no provider is set (tests / probes), the field is `[]`.

## Bounded memory & lifecycle

- Ring cap 512 events per run; terminal status evicts the handle from the
  registry (existing behavior) → ring freed.
- `RUN_REPLAY_CAP` is a module const; no config knob (YAGNI).

## Entropy reduction

`seq` graduates from zero-consumer dead weight to a load-bearing cursor — the
"wire it or delete it" decision resolved toward wiring (per user). No code is
removed; the dead field becomes live.

## Non-goals (deferred)

- Panel (Leptos WASM) `last_seq` tracking + reconnect `run.subscribe` call —
  needs `-p aleph-panel` + rust_embed rebuild chain.
- Per-frame seq on the **global** event bus (marginal; close+resync covers it).
- Config knob for ring capacity.

## Testing (core unit tests, written with the implementation)

1. ring eviction advances `oldest_retained_seq`; cap respected.
2. `subscribe_from` atomicity — emit before subscribe, subscribe mid-stream,
   emit after; assert catch-up + live form a **contiguous gap-free,
   duplicate-free** seq sequence.
3. `since_seq < oldest` → `Truncated`.
4. `since_seq == current_seq` → empty catch-up, live only.
5. `run.subscribe` handler: Replaying / Truncated / NotFound.
6. `HelloSnapshot` serializes `active_runs` (with and without provider).

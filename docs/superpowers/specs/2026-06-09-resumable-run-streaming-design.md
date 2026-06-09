# Gateway gap analysis → dead `run_event_bus` discovery → scoped 熵减

**Date**: 2026-06-09
**Scope**: Aleph gateway — gap analysis vs openclaw / hermes-agent / Pi
**Outcome**: delete the unregistered `run.*` RPC handler module (entropy
reduction). The originally-scoped "resumable run streaming" feature was
**dropped after source-verified premise falsification** (see below).

## How this started

Comparing Aleph's gateway against openclaw (WS+JSON-RPC gateway),
hermes-agent (ACP), and Pi (stdio), the candidate gap was *resumable run
streaming*: `run_event_bus::RunEvent` stamps a monotonic `seq: u64` on every
variant, but `grep .seq` found **zero readers**. The hypothesis: wire the seq
into a replay ring so a reconnecting client replays only the gap
(`seq > since_seq`) — surpassing hermes' unbounded full-history replay.

## Premise falsification (deeper read)

Tracing the actual run-streaming path overturned the hypothesis:

1. `RunEvent::` is **never constructed in the execution engine** (zero
   non-test hits). `ActiveRunHandle::new` appears only in tests.
2. `wait_for_run_end` is called only by `handlers/runs.rs`, which is itself
   **never registered** in any dispatch table.
3. The whole `run_event_bus` module is referenced in production only by
   `gateway/mod.rs` (a re-export). Its only real consumers are integration
   tests (`tests/world/subagent_ctx.rs`, `tests/steps/subagent_steps.rs`),
   which use it as an event-transport fixture.
4. The **live** run-streaming path is entirely different:
   `GatewayEventEmitter` → `GatewayEventFrame::ResponseChunk { run_id, seq, … }`
   → the global `event_bus` (topic `agent.run.*`) → per-client buffer → WS.
   Note the live frame carries its **own** `seq` (a separate counter on
   `GatewayEventEmitter`).

Conclusion: `run_event_bus` (`ActiveRunHandle` / `RunEvent` / `RunStatus` /
`wait_for_run_end`) plus the `run.wait` / `run.queue_message` handlers form a
**dead parallel run bus**, superseded by the `event_emitter` + global
`event_bus` path. Wiring "seq resume" into it would have been building
resume-on-resume against a bus that is itself unwired — redundant with the live
path and a R3/R10 violation.

## Action taken this round (scoped, safe)

Delete the **unregistered** `run.*` RPC handler module — the clearest,
self-contained, zero-external-consumer dead code:

- **Removed** `src/gateway/handlers/runs.rs` (445 LOC): `run.wait` /
  `run.queue_message` handlers + their request/response types. Never registered
  in any dispatch table; a leaf module (nothing imports from it); only its own
  `#[cfg(test)]` tests reference it.
- **Removed** `pub mod runs;` from `src/gateway/handlers/mod.rs`.

Verified by whole-repo grep: no consumer of `handlers::runs`,
`handle_run_wait`, `handle_run_queue_message`, `RunWaitRequest/Response`, or
`RunQueueMessageRequest/Response` outside the deleted file. Safe to remove
without `cargo check` (resource-governance constraint forbids it this round).

## Deliberately NOT touched (deferred, with reasons)

- **`run_event_bus.rs` core (904 LOC)** — also dead in production, but consumed
  by ~1188 LOC of BDD test scaffolding (`subagent_ctx.rs` + `subagent_steps.rs`)
  that would need untangling. A blind (no-cargo-check) deletion of that blast
  radius is unsafe; it needs a verified pass.
- **`loom_concurrency.rs`** — self-contained loom *models*; references
  `run_event_bus` only in doc comments. Not a consumer. Left intact.
- **Live-path resume** (`GatewayEventFrame.seq` + `event_bus`) — the genuine
  version of the resume feature, but additive and overlapping the existing
  close-on-slow-consumer + hello-resync recovery; marginal benefit. Deferred.

## Reference comparison (retained — the analysis remains valid)

| Project | Resume mechanism |
|---|---|
| hermes-agent | full inline history replay on load/resume (blocking, unbounded) |
| openclaw | per-client `seq` + `messageSeq` cursor replay; close-on-slow-consumer |
| Pi | session file rebuild on re-spawn |
| Aleph (live path) | close-on-slow-consumer → hello resync (state only); final transcript persisted to chat history |

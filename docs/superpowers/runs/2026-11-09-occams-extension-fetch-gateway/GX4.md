# Brief — GX4: src/gateway — pty + event_emitter + events (process IO, event fanout)

**Module area**: `gateway` · **Files**: 31 · **LOC**: 13248

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/pty/screen/perform/tests.rs` | 2379 |
| `src/gateway/pty/manager.rs` | 1852 |
| `src/gateway/events/frame.rs` | 1194 |
| `src/gateway/pty/session.rs` | 1023 |
| `src/gateway/event_emitter/redacting.rs` | 663 |
| `src/gateway/event_emitter/instant_buffer.rs` | 557 |
| `src/gateway/event_emitter/team_fanout.rs` | 533 |
| `src/gateway/event_emitter/origin_fanout.rs` | 513 |
| `src/gateway/pty/screen/perform/mod.rs` | 510 |
| `src/gateway/pty/screen/grid/mod.rs` | 416 |
| `src/gateway/event_emitter/types.rs` | 404 |
| `src/gateway/events/frame_census.rs` | 356 |
| `src/gateway/pty/screen/perform/csi.rs` | 303 |
| `src/gateway/pty/screen/grid/tests.rs` | 261 |
| `src/gateway/pty/mod.rs` | 235 |
| `src/gateway/event_emitter/tests.rs` | 230 |
| `src/gateway/pty/screen/diff.rs` | 216 |
| `src/gateway/pty/screen/grid/edit.rs` | 211 |
| `src/gateway/event_emitter/impls.rs` | 205 |
| `src/gateway/event_emitter/mod.rs` | 189 |
| `src/gateway/pty/jail.rs` | 184 |
| `src/gateway/pty/screen/perform/osc.rs` | 173 |
| `src/gateway/pty/screen/grid/write.rs` | 114 |
| `src/gateway/pty/screen/grid/scroll.rs` | 106 |
| `src/gateway/event_emitter/artifact_ping.rs` | 85 |
| `src/gateway/pty/screen/mod.rs` | 85 |
| `src/gateway/pty/screen/perform/esc.rs` | 76 |
| `src/gateway/pty/screen/grid/resize.rs` | 71 |
| `src/gateway/pty/screen/convert.rs` | 60 |
| `src/gateway/pty/screen/text.rs` | 36 |
| `src/gateway/events/mod.rs` | 8 |

## Area contracts (invariants that must survive any fix)

### Gateway area contracts (`src/gateway`)

- **R4 — Interface layer is pure I/O**: `handlers/`, `interfaces/`, `server/`, `control_plane/`, `admin_api/` are Bridge/Interface layers. Business logic leaking into them is a finding only when it *duplicates* a Core-side decision or bypasses a Core-side contract (shape #1: two statements of the same fact). "This handler is too long" is NOT a finding.
- **R1/R6 — Core vs. Bridge**: Rust Core is the only brain; multi-end surfaces do I/O. Gateway code must not re-implement Core behavior (e.g. re-deriving tool results, re-parsing policy that Core already resolved).
- **R5 — AI proactively reaches the user**: delivery must go through the user's existing work channel (the channel registry + `delivery_queue` + `reply_emitter`); a delivery path that silently drops, duplicates or reorders messages is a finding.
- **Single owner for state**: session state, visibility rules, channel bindings and queued deliveries each need exactly one owner. Two maps/registries holding the same fact → **P1** duplicate-source finding.
- **Concurrency**: `tokio` runtime — no blocking I/O (std fs/net/sleep) inside `async fn` on a runtime thread; no lock held across `.await`; no lock-order inversion between the two lock families; no unbounded channel/semaphore; bounded queues must have backpressure or a documented drop policy.
- **Ordering & retry**: message ordering, retry/backoff, idempotency and cancellation semantics are observable behavior — preserve them.
- **Secret hygiene**: tokens, API keys and webhook signing secrets must not appear in logs, error strings, or `Debug` impls.
- **Fail-closed**: security checks (webhook signature verification, auth, path/permission checks) must deny on error, never default-allow.
- **Channel adapters** (`interfaces/`): per-platform adapters are near-duplicates by nature. Report *specific divergences* in behavior (different retry, different signature check, different escaping) — not "these adapters duplicate each other".



## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/GX4.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/GX4.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

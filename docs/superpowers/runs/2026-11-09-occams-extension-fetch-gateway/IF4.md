# Brief — IF4: src/gateway/interfaces — imessage + matrix + line + xmpp adapters

**Module area**: `gateway` · **Files**: 40 · **LOC**: 10751

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/line/message_ops.rs` | 804 |
| `src/gateway/interfaces/imessage/db.rs` | 716 |
| `src/gateway/interfaces/imessage/bluebubbles/api.rs` | 618 |
| `src/gateway/interfaces/xmpp/message_ops/tests.rs` | 562 |
| `src/gateway/interfaces/matrix/events.rs` | 511 |
| `src/gateway/interfaces/xmpp/mod.rs` | 476 |
| `src/gateway/interfaces/line/mod.rs` | 472 |
| `src/gateway/interfaces/matrix/config.rs` | 472 |
| `src/gateway/interfaces/imessage/mod.rs` | 459 |
| `src/gateway/interfaces/line/webhook.rs` | 421 |
| `src/gateway/interfaces/xmpp/message_ops/ops.rs` | 388 |
| `src/gateway/interfaces/matrix/mod.rs` | 369 |
| `src/gateway/interfaces/line/types.rs` | 341 |
| `src/gateway/interfaces/imessage/bluebubbles/mod.rs` | 340 |
| `src/gateway/interfaces/matrix/outbound.rs` | 338 |
| `src/gateway/interfaces/xmpp/config.rs` | 327 |
| `src/gateway/interfaces/imessage/bluebubbles/inbound/mapper.rs` | 310 |
| `src/gateway/interfaces/imessage/config.rs` | 287 |
| `src/gateway/interfaces/xmpp/message_ops/stanza.rs` | 285 |
| `src/gateway/interfaces/imessage/target.rs` | 276 |
| `src/gateway/interfaces/imessage/sender.rs` | 257 |
| `src/gateway/interfaces/line/config.rs` | 208 |
| `src/gateway/interfaces/imessage/bluebubbles/inbound/webhook_server.rs` | 206 |
| `src/gateway/interfaces/matrix/media.rs` | 190 |
| `src/gateway/interfaces/matrix/dedupe.rs` | 147 |
| `src/gateway/interfaces/matrix/sync.rs` | 136 |
| `src/gateway/interfaces/imessage/reaction.rs` | 124 |
| `src/gateway/interfaces/imessage/bluebubbles/staging.rs` | 104 |
| `src/gateway/interfaces/matrix/client.rs` | 95 |
| `src/gateway/interfaces/imessage/bluebubbles/inbound/poll.rs` | 82 |
| `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs` | 76 |
| `src/gateway/interfaces/xmpp/message_ops/types.rs` | 72 |
| `src/gateway/interfaces/imessage/bluebubbles/inbound/dedup.rs` | 70 |
| `src/gateway/interfaces/imessage/bluebubbles/outbound/text.rs` | 57 |
| `src/gateway/interfaces/imessage/bluebubbles/config.rs` | 53 |
| `src/gateway/interfaces/xmpp/message_ops/mod.rs` | 34 |
| `src/gateway/interfaces/imessage/bluebubbles/inbound/mod.rs` | 33 |
| `src/gateway/interfaces/imessage/bluebubbles/outbound/attachment.rs` | 25 |
| `src/gateway/interfaces/imessage/bluebubbles/outbound/reaction.rs` | 7 |
| `src/gateway/interfaces/imessage/bluebubbles/outbound/mod.rs` | 3 |

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
   - cheap tier → `candidates/IF4.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF4.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

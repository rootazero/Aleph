# Brief — IF5: src/gateway/interfaces — slack + qq + wechat + nostr adapters

**Module area**: `gateway` · **Files**: 42 · **LOC**: 9991

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/slack/message_ops/api.rs` | 719 |
| `src/gateway/interfaces/slack/directory.rs` | 661 |
| `src/gateway/interfaces/nostr/message_ops/tests.rs` | 610 |
| `src/gateway/interfaces/slack/mod.rs` | 600 |
| `src/gateway/interfaces/slack/message_ops/tests.rs` | 540 |
| `src/gateway/interfaces/nostr/mod.rs` | 457 |
| `src/gateway/interfaces/wechat/types.rs` | 365 |
| `src/gateway/interfaces/nostr/message_ops/protocol.rs` | 341 |
| `src/gateway/interfaces/wechat/api.rs` | 338 |
| `src/gateway/interfaces/wechat/inbound/mapper.rs` | 332 |
| `src/gateway/interfaces/slack/config.rs` | 316 |
| `src/gateway/interfaces/slack/message_ops/files.rs` | 308 |
| `src/gateway/interfaces/nostr/config.rs` | 304 |
| `src/gateway/interfaces/wechat/mod.rs` | 280 |
| `src/gateway/interfaces/qq/mod.rs` | 274 |
| `src/gateway/interfaces/qq/delivery.rs` | 256 |
| `src/gateway/interfaces/nostr/message_ops/ops.rs` | 234 |
| `src/gateway/interfaces/wechat/config.rs` | 232 |
| `src/gateway/interfaces/slack/message_ops/socket.rs` | 230 |
| `src/gateway/interfaces/wechat/media.rs` | 226 |
| `src/gateway/interfaces/qq/gateway.rs` | 224 |
| `src/gateway/interfaces/wechat/runtime.rs` | 204 |
| `src/gateway/interfaces/qq/api.rs` | 198 |
| `src/gateway/interfaces/slack/message_ops/converter.rs` | 172 |
| `src/gateway/interfaces/slack/errors.rs` | 164 |
| `src/gateway/interfaces/wechat/outbound/markdown.rs` | 163 |
| `src/gateway/interfaces/qq/config.rs` | 143 |
| `src/gateway/interfaces/wechat/auth.rs` | 139 |
| `src/gateway/interfaces/slack/message_ops/debouncer.rs` | 131 |
| `src/gateway/interfaces/qq/handlers.rs` | 115 |
| `src/gateway/interfaces/wechat/inbound/dedup.rs` | 112 |
| `src/gateway/interfaces/wechat/inbound/policy.rs` | 104 |
| `src/gateway/interfaces/qq/access.rs` | 100 |
| `src/gateway/interfaces/qq/types.rs` | 86 |
| `src/gateway/interfaces/wechat/outbound/mapper.rs` | 74 |
| `src/gateway/interfaces/wechat/sync_buf.rs` | 70 |
| `src/gateway/interfaces/nostr/message_ops/types.rs` | 59 |
| `src/gateway/interfaces/nostr/message_ops/mod.rs` | 37 |
| `src/gateway/interfaces/slack/message_ops/mod.rs` | 35 |
| `src/gateway/interfaces/qq/error.rs` | 33 |
| `src/gateway/interfaces/wechat/inbound/mod.rs` | 3 |
| `src/gateway/interfaces/wechat/outbound/mod.rs` | 2 |

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
   - cheap tier → `candidates/IF5.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF5.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

# Brief — IF6: src/gateway/interfaces — sms, irc, signal, email, mattermost, webhook + root

**Module area**: `gateway` · **Files**: 23 · **LOC**: 12225

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/sms/transport.rs` | 2593 |
| `src/gateway/interfaces/irc/message_ops.rs` | 955 |
| `src/gateway/interfaces/mattermost/message_ops.rs` | 842 |
| `src/gateway/interfaces/email/message_ops.rs` | 816 |
| `src/gateway/interfaces/signal/message_ops.rs` | 746 |
| `src/gateway/interfaces/webhook/mod.rs` | 744 |
| `src/gateway/interfaces/webhook/message_ops.rs` | 533 |
| `src/gateway/interfaces/irc/mod.rs` | 452 |
| `src/gateway/interfaces/mattermost/mod.rs` | 448 |
| `src/gateway/interfaces/cli.rs` | 442 |
| `src/gateway/interfaces/sms/mod.rs` | 432 |
| `src/gateway/interfaces/signal/mod.rs` | 391 |
| `src/gateway/interfaces/sms/message_ops.rs` | 369 |
| `src/gateway/interfaces/email/mod.rs` | 361 |
| `src/gateway/interfaces/email/config.rs` | 348 |
| `src/gateway/interfaces/irc/config.rs` | 313 |
| `src/gateway/interfaces/signal/config.rs` | 277 |
| `src/gateway/interfaces/webhook/config.rs` | 261 |
| `src/gateway/interfaces/mod.rs` | 249 |
| `src/gateway/interfaces/mattermost/config.rs` | 246 |
| `src/gateway/interfaces/signal/error.rs` | 236 |
| `src/gateway/interfaces/sms/config.rs` | 100 |
| `src/gateway/interfaces/plugin.rs` | 71 |

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


### Batch-specific focus — `IF6` (sms, irc, signal, email, mattermost, webhook + interfaces root)

These adapters have the widest protocol surface (SMS/email are outbound-heavy; webhook is inbound-heavy). Prioritize: inbound verification/signature checks, outbound message assembly and escaping, and per-platform differences that silently change semantics (e.g. one platform's reply truncated, one platform's dedup missing).

## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/IF6.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF6.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

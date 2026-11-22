# Brief — IF1: src/gateway/interfaces — telegram (+streaming) channel adapter

**Module area**: `gateway` · **Files**: 25 · **LOC**: 8232

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/telegram/delivery.rs` | 1284 |
| `src/gateway/interfaces/telegram/mod.rs` | 1163 |
| `src/gateway/interfaces/telegram/error_cooldown.rs` | 631 |
| `src/gateway/interfaces/telegram/streaming/orchestrator.rs` | 460 |
| `src/gateway/interfaces/telegram/access.rs` | 434 |
| `src/gateway/interfaces/telegram/config.rs` | 422 |
| `src/gateway/interfaces/telegram/config_v2.rs` | 328 |
| `src/gateway/interfaces/telegram/handlers.rs` | 290 |
| `src/gateway/interfaces/telegram/polling.rs` | 284 |
| `src/gateway/interfaces/telegram/streaming/reasoning_extractor.rs` | 279 |
| `src/gateway/interfaces/telegram/chunking.rs` | 266 |
| `src/gateway/interfaces/telegram/approval.rs` | 263 |
| `src/gateway/interfaces/telegram/audit.rs` | 262 |
| `src/gateway/interfaces/telegram/config_resolver.rs` | 259 |
| `src/gateway/interfaces/telegram/streaming/lane_handle.rs` | 231 |
| `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs` | 215 |
| `src/gateway/interfaces/telegram/mention.rs` | 200 |
| `src/gateway/interfaces/telegram/streaming/status_controller.rs` | 199 |
| `src/gateway/interfaces/telegram/token_fingerprint.rs` | 198 |
| `src/gateway/interfaces/telegram/offset.rs` | 154 |
| `src/gateway/interfaces/telegram/streaming/lane_tracker.rs` | 148 |
| `src/gateway/interfaces/telegram/reaction_handler.rs` | 118 |
| `src/gateway/interfaces/telegram/bot_instance.rs` | 77 |
| `src/gateway/interfaces/telegram/sticker.rs` | 54 |
| `src/gateway/interfaces/telegram/streaming/mod.rs` | 13 |

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
   - cheap tier → `candidates/IF1.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF1.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

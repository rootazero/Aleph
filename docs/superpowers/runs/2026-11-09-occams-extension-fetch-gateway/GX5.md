# Brief — GX5: src/gateway — security + middleware + surface + link + message_assembly + pipeline

**Module area**: `gateway` · **Files**: 39 · **LOC**: 9593

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/security/store/mod.rs` | 744 |
| `src/gateway/security/store/bootstrap_tickets.rs` | 702 |
| `src/gateway/security/shared_token.rs` | 670 |
| `src/gateway/security/device_token_manager.rs` | 666 |
| `src/gateway/middleware/request_state.rs` | 592 |
| `src/gateway/link/manager.rs` | 485 |
| `src/gateway/security/store/tests.rs` | 399 |
| `src/gateway/security/store/tokens.rs` | 396 |
| `src/gateway/security/store/users.rs` | 396 |
| `src/gateway/middleware/rate_limit.rs` | 394 |
| `src/gateway/surface/r5_router.rs` | 379 |
| `src/gateway/security/store/slot.rs` | 341 |
| `src/gateway/security/store/identity.rs` | 333 |
| `src/gateway/middleware/metrics.rs` | 305 |
| `src/gateway/link/types.rs` | 301 |
| `src/gateway/security/artifact_caps.rs` | 249 |
| `src/gateway/security/canvas_caps.rs` | 237 |
| `src/gateway/security/crypto.rs` | 178 |
| `src/gateway/middleware/trace.rs` | 157 |
| `src/gateway/security/store/devices.rs` | 153 |
| `src/gateway/middleware/latency.rs` | 152 |
| `src/gateway/security/token_readonly.rs` | 142 |
| `src/gateway/middleware/redact_query.rs` | 133 |
| `src/gateway/surface/delivery.rs` | 132 |
| `src/gateway/middleware/chain.rs` | 130 |
| `src/gateway/surface/desktop.rs` | 128 |
| `src/gateway/security/store/types.rs` | 99 |
| `src/gateway/middleware/handler_service.rs` | 97 |
| `src/gateway/surface/mod.rs` | 89 |
| `src/gateway/message_assembly/assembler.rs` | 87 |
| `src/gateway/middleware/auth.rs` | 69 |
| `src/gateway/middleware/validate.rs` | 69 |
| `src/gateway/message_assembly/tests.rs` | 61 |
| `src/gateway/middleware/mod.rs` | 48 |
| `src/gateway/security/mod.rs` | 46 |
| `src/gateway/message_assembly/mod.rs` | 12 |
| `src/gateway/pipeline/mod.rs` | 9 |
| `src/gateway/middleware/traits.rs` | 8 |
| `src/gateway/link/mod.rs` | 5 |

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
   - cheap tier → `candidates/GX5.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/GX5.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

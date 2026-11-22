# Brief — GX2: src/gateway — telecom + inbound_router (routing, R5 delivery, R4 pure I/O)

**Module area**: `gateway` · **Files**: 27 · **LOC**: 20563

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/inbound_router/mod.rs` | 3031 |
| `src/gateway/telecom/public_approval.rs` | 1637 |
| `src/gateway/telecom/public_run.rs` | 1623 |
| `src/gateway/telecom/lifecycle_tools.rs` | 1579 |
| `src/gateway/telecom/private_turn.rs` | 1376 |
| `src/gateway/inbound_router/executor.rs` | 1365 |
| `src/gateway/telecom/public_history.rs` | 1111 |
| `src/gateway/telecom/sms_inbox.rs` | 1100 |
| `src/gateway/inbound_router/types.rs` | 1000 |
| `src/gateway/inbound_router/command_handler.rs` | 795 |
| `src/gateway/telecom/operator_control.rs` | 752 |
| `src/gateway/telecom/public_admission.rs` | 682 |
| `src/gateway/telecom/owner_memory_read.rs` | 566 |
| `src/gateway/telecom/protocol.rs` | 507 |
| `src/gateway/telecom/public_history_startup.rs` | 502 |
| `src/gateway/telecom/persisted_marker.rs` | 446 |
| `src/gateway/inbound_router/group_chat_handler.rs` | 438 |
| `src/gateway/telecom/lifecycle_bridge.rs` | 395 |
| `src/gateway/inbound_router/permission.rs` | 323 |
| `src/gateway/inbound_router/agent_resolver.rs` | 302 |
| `src/gateway/telecom/public_context.rs` | 301 |
| `src/gateway/telecom/private_read.rs` | 269 |
| `src/gateway/telecom/run_origin.rs` | 242 |
| `src/gateway/inbound_router/dedup.rs` | 129 |
| `src/gateway/telecom/mod.rs` | 57 |
| `src/gateway/inbound_router/approval_callback.rs` | 29 |
| `src/gateway/telecom/sms_meta.rs` | 6 |

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


### Batch-specific focus — `GX2` (telecom + inbound_router)

This is the R4/R5 boundary: routing inbound events to the Core and delivering proactive messages back. Prioritize: routing decisions that bypass Core-side policy, fan-out that duplicates a single inbound event, and delivery paths that drop messages without surfacing the failure.

## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/GX2.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/GX2.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

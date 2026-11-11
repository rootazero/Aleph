# Brief — IF3: src/gateway/interfaces — feishu + whatsapp adapters (runtime/policy/auth splits)

**Module area**: `gateway` · **Files**: 54 · **LOC**: 11537

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/feishu/feishu_inbound/events.rs` | 1125 |
| `src/gateway/interfaces/feishu/api.rs` | 931 |
| `src/gateway/interfaces/whatsapp/mod.rs` | 923 |
| `src/gateway/interfaces/feishu/mod.rs` | 752 |
| `src/gateway/interfaces/feishu/config.rs` | 669 |
| `src/gateway/interfaces/whatsapp/pairing.rs` | 627 |
| `src/gateway/interfaces/whatsapp/wa_runtime/client.rs` | 514 |
| `src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs` | 450 |
| `src/gateway/interfaces/whatsapp/wa_runtime/fake.rs` | 435 |
| `src/gateway/interfaces/feishu/feishu_outbound/streaming.rs` | 424 |
| `src/gateway/interfaces/feishu/types.rs` | 342 |
| `src/gateway/interfaces/feishu/envelope.rs` | 340 |
| `src/gateway/interfaces/feishu/api_handle.rs` | 295 |
| `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs` | 253 |
| `src/gateway/interfaces/whatsapp/wa_outbound/media.rs` | 199 |
| `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs` | 197 |
| `src/gateway/interfaces/feishu/feishu_runtime/mod.rs` | 195 |
| `src/gateway/interfaces/whatsapp/reactions.rs` | 180 |
| `src/gateway/interfaces/feishu/auth.rs` | 171 |
| `src/gateway/interfaces/whatsapp/history_buffer.rs` | 170 |
| `src/gateway/interfaces/feishu/feishu_inbound/crypto.rs` | 159 |
| `src/gateway/interfaces/whatsapp/wa_runtime/traits.rs` | 134 |
| `src/gateway/interfaces/feishu/feishu_runtime/ws_client.rs` | 133 |
| `src/gateway/interfaces/feishu/feishu_inbound/policy.rs` | 130 |
| `src/gateway/interfaces/whatsapp/media.rs` | 128 |
| `src/gateway/interfaces/feishu/feishu_inbound/user_cache.rs` | 127 |
| `src/gateway/interfaces/whatsapp/config.rs` | 119 |
| `src/gateway/interfaces/feishu/feishu_outbound/sender.rs` | 114 |
| `src/gateway/interfaces/feishu/feishu_policy/group_policy.rs` | 114 |
| `src/gateway/interfaces/feishu/feishu_inbound/dedup.rs` | 108 |
| `src/gateway/interfaces/whatsapp/wa_policy/dm_policy.rs` | 106 |
| `src/gateway/interfaces/whatsapp/wa_inbound/policy.rs` | 97 |
| `src/gateway/interfaces/whatsapp/wa_policy/group_policy.rs` | 97 |
| `src/gateway/interfaces/feishu/feishu_policy/dm_policy.rs` | 96 |
| `src/gateway/interfaces/whatsapp/wa_outbound/sender.rs` | 87 |
| `src/gateway/interfaces/whatsapp/wa_policy/pairing.rs` | 80 |
| `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs` | 74 |
| `src/gateway/interfaces/whatsapp/message.rs` | 63 |
| `src/gateway/interfaces/whatsapp/wa_runtime/http_client.rs` | 63 |
| `src/gateway/interfaces/whatsapp/wa_runtime/state.rs` | 54 |
| `src/gateway/interfaces/feishu/message_ops.rs` | 52 |
| `src/gateway/interfaces/feishu/feishu_runtime/state.rs` | 50 |
| `src/gateway/interfaces/whatsapp/types.rs` | 46 |
| `src/gateway/interfaces/feishu/feishu_outbound/reactions.rs` | 40 |
| `src/gateway/interfaces/feishu/feishu_outbound/media.rs` | 16 |
| `src/gateway/interfaces/feishu/feishu_inbound/mod.rs` | 12 |
| `src/gateway/interfaces/whatsapp/wa_runtime/mod.rs` | 10 |
| `src/gateway/interfaces/feishu/feishu_outbound/mod.rs` | 7 |
| `src/gateway/interfaces/whatsapp/wa_policy/mod.rs` | 7 |
| `src/gateway/interfaces/feishu/feishu_policy/mod.rs` | 5 |
| `src/gateway/interfaces/whatsapp/wa_inbound/mod.rs` | 5 |
| `src/gateway/interfaces/whatsapp/wa_outbound/mod.rs` | 5 |
| `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs` | 4 |
| `src/gateway/interfaces/whatsapp/wa_auth/mod.rs` | 3 |

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
   - cheap tier → `candidates/IF3.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF3.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

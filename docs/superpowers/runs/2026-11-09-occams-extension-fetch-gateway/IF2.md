# Brief — IF2: src/gateway/interfaces — discord + voice_call adapters (incl. relay tests)

**Module area**: `gateway` · **Files**: 26 · **LOC**: 12304

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/interfaces/voice_call/relay_tests.rs` | 2767 |
| `src/gateway/interfaces/discord/mod.rs` | 1357 |
| `src/gateway/interfaces/voice_call/protocol.rs` | 816 |
| `src/gateway/interfaces/discord/commands.rs` | 803 |
| `src/gateway/interfaces/voice_call/coordinator.rs` | 770 |
| `src/gateway/interfaces/discord/draft.rs` | 728 |
| `src/gateway/interfaces/voice_call/relay_tests/lifecycle_dispatch.rs` | 591 |
| `src/gateway/interfaces/discord/reconnect.rs` | 566 |
| `src/gateway/interfaces/voice_call/relay.rs` | 551 |
| `src/gateway/interfaces/discord/permissions.rs` | 398 |
| `src/gateway/interfaces/discord/security/startup_audit.rs` | 348 |
| `src/gateway/interfaces/discord/api.rs` | 288 |
| `src/gateway/interfaces/discord/security/audit_hooks.rs` | 286 |
| `src/gateway/interfaces/discord/config.rs` | 271 |
| `src/gateway/interfaces/voice_call/transport.rs` | 238 |
| `src/gateway/interfaces/discord/group_policy.rs` | 220 |
| `src/gateway/interfaces/voice_call/channel.rs` | 210 |
| `src/gateway/interfaces/voice_call/relay_tests/limits.rs` | 199 |
| `src/gateway/interfaces/discord/session_key.rs` | 191 |
| `src/gateway/interfaces/discord/resolver/strategy.rs` | 189 |
| `src/gateway/interfaces/discord/resolver/mod.rs` | 181 |
| `src/gateway/interfaces/discord/resolver/input.rs` | 130 |
| `src/gateway/interfaces/discord/resolver/error.rs` | 100 |
| `src/gateway/interfaces/voice_call/config.rs` | 40 |
| `src/gateway/interfaces/discord/security/mod.rs` | 35 |
| `src/gateway/interfaces/voice_call/mod.rs` | 31 |

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
   - cheap tier → `candidates/IF2.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/IF2.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

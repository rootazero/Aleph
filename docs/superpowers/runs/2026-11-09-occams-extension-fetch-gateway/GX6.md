# Brief — GX6: src/gateway — voice + openai_api + btw + mcp_face + agent_env + formatter

**Module area**: `gateway` · **Files**: 50 · **LOC**: 18260

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/agent_env/mod.rs` | 1280 |
| `src/gateway/btw/guard_tests.rs` | 922 |
| `src/gateway/mcp_face/http.rs` | 823 |
| `src/gateway/btw/tests.rs` | 769 |
| `src/gateway/openai_api/types.rs` | 769 |
| `src/gateway/formatter/helpers.rs` | 749 |
| `src/gateway/mcp_face/mod.rs` | 709 |
| `src/gateway/openai_api/completions/agent.rs` | 674 |
| `src/gateway/formatter/tests.rs` | 649 |
| `src/gateway/openai_api/stream.rs` | 647 |
| `src/gateway/voice/outbound.rs` | 644 |
| `src/gateway/mcp_face/protocol.rs` | 549 |
| `src/gateway/agent_env/manager_ops.rs` | 534 |
| `src/gateway/agent_env/ops.rs` | 534 |
| `src/gateway/voice/streaming/whisperlive.rs` | 534 |
| `src/gateway/openai_api/responses/sse.rs` | 480 |
| `src/gateway/btw/seed/mod.rs` | 406 |
| `src/gateway/btw/promote/tests.rs` | 398 |
| `src/gateway/voice/streaming/relay.rs` | 359 |
| `src/gateway/btw/mod.rs` | 347 |
| `src/gateway/formatter/markdown_to_platform.rs` | 336 |
| `src/gateway/mcp_face/session.rs` | 317 |
| `src/gateway/mcp_face/auth.rs` | 315 |
| `src/gateway/voice/sanitize.rs` | 315 |
| `src/gateway/voice/streaming/deepgram.rs` | 306 |
| `src/gateway/voice/voice_mode.rs` | 296 |
| `src/gateway/openai_api/auth.rs` | 291 |
| `src/gateway/mcp_face/config.rs` | 285 |
| `src/gateway/voice/local_provider.rs` | 285 |
| `src/gateway/voice/inbound/mod.rs` | 272 |
| `src/gateway/openai_api/completions/passthrough.rs` | 264 |
| `src/gateway/voice/inbound/provider.rs` | 263 |
| `src/gateway/voice/streaming/mod.rs` | 223 |
| `src/gateway/btw/promote.rs` | 199 |
| `src/gateway/openai_api/responses/mod.rs` | 181 |
| `src/gateway/voice/state.rs` | 180 |
| `src/gateway/voice/format.rs` | 160 |
| `src/gateway/voice/inbound/stt.rs` | 159 |
| `src/gateway/formatter/splitting.rs` | 150 |
| `src/gateway/btw/seed/gate.rs` | 148 |
| `src/gateway/formatter/mod.rs` | 127 |
| `src/gateway/openai_api/embeddings.rs` | 88 |
| `src/gateway/openai_api/models.rs` | 86 |
| `src/gateway/formatter/platform_to_markdown.rs` | 66 |
| `src/gateway/openai_api/completions/mod.rs` | 39 |
| `src/gateway/openai_api/router.rs` | 39 |
| `src/gateway/voice/mod.rs` | 39 |
| `src/gateway/openai_api/state.rs` | 31 |
| `src/gateway/openai_api/mod.rs` | 14 |
| `src/gateway/voice/hallucination.rs` | 10 |

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
   - cheap tier → `candidates/GX6.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/GX6.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

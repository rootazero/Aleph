# Brief — GX1: src/gateway — server + control_plane + admin_api (IPC surface, R4/R6 boundary)

**Module area**: `gateway` · **Files**: 30 · **LOC**: 16873

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/server/mod.rs` | 2392 |
| `src/gateway/server/handler.rs` | 1974 |
| `src/gateway/server/connection/mod.rs` | 1807 |
| `src/gateway/server/artifact_route.rs` | 1571 |
| `src/gateway/server/browser_viewer_route.rs` | 1524 |
| `src/gateway/server/connection/auth.rs` | 927 |
| `src/gateway/server/canvas_asset_route.rs` | 828 |
| `src/gateway/control_plane/server.rs` | 773 |
| `src/gateway/server/connection/off_loop.rs` | 455 |
| `src/gateway/server/panel_asset_http_tests.rs` | 421 |
| `src/gateway/admin_api/reconciler.rs` | 393 |
| `src/gateway/server/metrics_endpoint.rs` | 374 |
| `src/gateway/server/byte_range.rs` | 372 |
| `src/gateway/server/browser_live_runtime.rs` | 349 |
| `src/gateway/server/connection/dispatch.rs` | 339 |
| `src/gateway/server/panel_asset_route.rs` | 329 |
| `src/gateway/admin_api/secrets.rs` | 314 |
| `src/gateway/server/connection/forward.rs` | 251 |
| `src/gateway/server/connection/upgrade.rs` | 224 |
| `src/gateway/server/panel_asset_route_tests.rs` | 208 |
| `src/gateway/admin_api/mod.rs` | 204 |
| `src/gateway/server/browser_viewer_issuer_tests.rs` | 157 |
| `src/gateway/server/probe.rs` | 134 |
| `src/gateway/server/connection/cleanup.rs` | 128 |
| `src/gateway/server/per_client_buffer.rs` | 116 |
| `src/gateway/server/attachment_route.rs` | 108 |
| `src/gateway/server/flood_guard.rs` | 83 |
| `src/gateway/admin_api/resume.rs` | 80 |
| `src/gateway/control_plane/assets.rs` | 29 |
| `src/gateway/control_plane/mod.rs` | 9 |

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
   - cheap tier → `candidates/GX1.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/GX1.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.

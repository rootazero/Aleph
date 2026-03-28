# Module: a2a

## Summary
- Files reviewed: 37
- Issues found: 16
- Issues fixed: 14
- Deferred: 2 (architectural observations)

## Fixes

1. **`adapter/client/sse_stream.rs:51-58`** [Critical] UTF-8 corruption — `from_utf8_lossy` destroyed incomplete multi-byte sequences → Added separate `carry: Vec<u8>` buffer to properly preserve incomplete trailing bytes across chunks

2. **`adapter/server/routes.rs:215`** UTF-8 byte slicing — `auth[..7]` direct index → Changed to `auth.get(..7)` safe accessor

3. **`sub_agent.rs:6,31,39`** Sync primitives — `std::sync::Arc` and `std::sync::RwLock` → Changed to `crate::sync_primitives::{Arc, RwLock}`

4. **`adapter/server/bridge.rs:60`** Sync primitives — `std::sync::Arc::new(std::sync::Mutex::new(...))` → Changed to `Arc::new(crate::sync_primitives::Mutex::new(...))`

5. **`sub_agent.rs:54,59,65`** UTF-8 safety — `.len() >= 2` checks byte count not char count → Changed to `.chars().count() >= 2` for correct multi-byte char filtering

6. **`sub_agent.rs:184`** Silent error swallow — `serde_json::to_value(&task).unwrap_or_default()` → Added `unwrap_or_else` with `tracing::warn!` to log serialization failures

7. **`adapter/server/request_processor.rs:202`** Truncation — `v as usize` from u64 → Changed to `usize::try_from(v).unwrap_or(usize::MAX)`

8. **`adapter/server/request_processor.rs:259-263`** Silent param swallow — `unwrap_or_default()` hides malformed params → Changed to return `-32602 Invalid params` error on deserialization failure

9. **`adapter/server/stream_hub.rs:32`** Boundary panic — `capacity == 0` would panic in `broadcast::channel(0)` → Added `.max(1)` clamp

10. **`service/card_registry.rs:121`** Wrong error variant — `A2AError::TaskNotFound` used for agent-not-found → Changed to `A2AError::InvalidRequest`

11. **`service/llm_matcher.rs:88-101`** Unsafe cast — negative `i32` cast to `usize` wraps to `usize::MAX` → Split into separate guard (`idx < 0` early return) then safe cast

12. **`service/llm_matcher.rs:123`** Non-idiomatic naming — `__msgs` → Renamed to `msgs`

13. **`service/notification.rs:62-64`** Non-deterministic order — `HashMap::values()` iteration → Added `sort_by(task_id)` for deterministic output

14. **Test files** (sub_agent.rs, tests.rs, smart_router.rs, bridge.rs) — `std::sync::Mutex` in test mocks → Changed to `crate::sync_primitives::Mutex`; removed redundant type annotation on `PoisonError`

## Verification
- `cargo check -p alephcore --lib` — **clean** (14 warnings, 0 errors)
- `cargo test -p alephcore --lib a2a` — **228 tests passed, 0 failed**

## Notes (deferred observations, not fixed)

1. **`routes.rs` loopback fallback** — `fallback_addr()` always returns `127.0.0.1:0`, meaning all requests appear as loopback when `ConnectInfo<SocketAddr>` is not wired. This effectively disables IP-based auth (`local_bypass`). The proper fix requires using `axum::extract::ConnectInfo<SocketAddr>` as an extractor in the handler signature and failing closed if unavailable. Left as-is because this is an architectural change needing broader discussion.

2. **`smart_router.rs` Tier 2 substring matching** — `try_exact_skill` does `intent.contains(skill.name)` which is substring matching on natural language, arguably violating R8 (LLM Sovereignty). The tier should either be removed (fall directly to LLM) or restricted to exact quoted-name matching like Tier 1. Left as-is because this is a design decision.

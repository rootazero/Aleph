

Compilation confirmed. Here's the final report:

---

# Module: gateway

## Summary
- **Files reviewed**: 290 (across 6 parallel subagents)
- **Issues found**: 4
- **Issues fixed**: 4

## Fixes

1. **`execution_engine/run_loop.rs:321`** — Underflow risk: `session_history.len() - 1` panics if empty → Changed to `.saturating_sub(1)` (logically guarded by `.last()` returning `Some`, but fragile)

2. **`execution_engine/run_loop.rs:223`** — Redundant unwrap: `has_attachments` flag + `.unwrap()` on next line → Refactored to `if let` pattern matching, eliminating the unwrap entirely

3. **`handlers/session/store.rs:339`** — Underflow risk: `before_count - keep_count` could underflow → Changed to `.saturating_sub()` (guarded by early return, but defensive)

4. **`lane.rs:164`** — Production `.expect()` on hot path: `self.lanes.get(&lane).expect(...)` → Changed to `match` returning `LaneError::Congested` (P7 defensive design on every RPC request)

## Verified Clean (no changes needed)

| Category | Files | Finding |
|----------|-------|---------|
| UTF-8 byte slicing | ~30 patterns across formatter, interfaces, reply_emitter, tool_display, i18n | All indices from `.find()`, `char_indices()`, or `is_char_boundary()` — **safe** |
| Lock safety | All production lock sites | Already use `.unwrap_or_else(\|e\| e.into_inner())` |
| `.unwrap()`/`.expect()` | 1367 occurrences | All in `#[cfg(test)]` blocks except HMAC (safe by API contract), rate_limiter (logically proven), server setup (startup assertion) |
| `static mut` | 0 | None found |
| HashMap ordering | policy_engine, identity_map | Use `Vec` or `DashMap.get()` only — no order-dependent iteration |
| SQL injection | N/A | No LanceDB filters in gateway |

## Notes

- The gateway module has been well-maintained — most known bug patterns were already addressed in prior reviews.
- The `agent_init.rs` bin has a pre-existing `?` operator error (unrelated to gateway).
- The 13 compiler warnings are pre-existing (unused methods in feishu).
peline/inbound_router 等 (1 fix)
19. `bridge/supervisor.rs:40` 移除未使用的 `error` import

### Root files (4 fixes)
20. `tool_display.rs:138` `truncate_str` 用 `s.len()` 比较 `max_len` — 改为 `s.chars().count()` + `char_indices().nth()`（字符计数 vs 字节计数修正）
21. `tool_display.rs:124` `shorten_path` 同上 — 改为字符计数
22. `tool_display.rs:170` `group_paths` HashMap 迭代顺序不确定 — 添加 key 排序
23. `workspace_loader.rs:87` `load_memory_md` 的 `max_chars` 参数实际比较字节数 — 改为正确的字符计数逻辑

## Notes

**架构合规性**：gateway 模块整体遵守架构红线，没有发现违反 R1-R10 的问题。

**代码质量亮点**：
- 大部分 lock 已经使用 `.unwrap_or_else(|e| e.into_inner())`
- `i18n.rs` 的 `truncate_error` 已正确使用 `char_indices()`
- `idempotency.rs` 有正确的 RAII guard 模式
- `challenge.rs` 使用常量时间比较做 HMAC 验证
- `stream_buffer.rs` 的字节索引安全（只存储 `self.text.len()` 值）

**高危修复**：
- `tool_display.rs` 和 `workspace_loader.rs` 的字节/字符混淆是真实 bug — 处理中文内容时会截断位置错误
- `generation_providers.rs` 的 `.expect()` 是竞态条件下的潜在 panic
- `feishu/mod.rs` 的 `unreachable!()` 在生产环境会导致服务器崩溃
- 所有 `nostr`/`xmpp`/`imessage` 的字节切片在处理非 ASCII 消息时会 panic

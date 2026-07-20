All 325 dispatcher tests pass. Here's the review summary:

---

# Module: dispatcher

## Summary
- Files reviewed: 41
- Issues found: 1
- Issues fixed: 1

## Fixes
1. **`tool_index/inference.rs:131-140`** UTF-8 safety — `humanize_name` used byte-based `get_mut(0..1)` + `make_ascii_uppercase()` for first-letter capitalization. Replaced with char-based `chars().next()` + `to_uppercase()` pattern per P7 defensive design guidelines.

## Notes

**This module is exceptionally clean.** Across 41 files (~4000 lines of production code), only one minor UTF-8 safety issue was found. Key observations:

- **Lock safety**: `PendingConfirmationStore` correctly uses `.unwrap_or_else(|e| e.into_inner())` on all lock operations (6 call sites).
- **UTF-8 safety**: All truncation functions properly delegate to `crate::utils::text_format::truncate_text` — no raw `&s[..n]` byte slicing.
- **Static initialization**: `risk.rs` correctly uses `OnceLock` for lazy regex patterns — no `static mut`.
- **No SQL injection surface**: No LanceDB filter construction via `format!()` in this module.
- **No unwrap on user-facing paths**: `Regex::new().unwrap()` calls are only inside `OnceLock::get_or_init()` with compile-time-valid patterns.
- **HashMap iteration**: All query methods (`list_all_for_ui`, `list_root_commands`, `list_preset_tools`, etc.) explicitly sort results before returning — no ordering-dependent iteration.
- **TOCTOU prevention**: `register_with_conflict_resolution` performs conflict check and insertion under a single write lock.
- **Async RwLock**: Registry uses `tokio::sync::RwLock` (not `std`), appropriate for async contexts with potential contention.

**Pre-existing issue outside scope**: `bin/aleph-server/commands/start/builder/agent_init.rs:177` has a `?` operator in a function returning a non-Result type — unrelated to dispatcher.

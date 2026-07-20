

All 129 tests pass. Here's the review summary:

---

# Module: capability

## Summary
- Files reviewed: 10
- Issues found: 2
- Issues fixed: 2

## Fixes

1. **`mod.rs:52-81`** Dead code removal — Removed 6 unused fields (`ai_provider`, `memory_exclusion_set`, `use_ai_retrieval`, `ai_retrieval_timeout_ms`, `ai_retrieval_max_candidates`, `ai_retrieval_fallback_count`) and 2 dead methods (`with_ai_retrieval`, `with_memory_exclusion_set`) from `CapabilityExecutor`. These were configured via builder methods but never read by any execute method. The actual AI retrieval logic lives in `MemoryStrategy` instead.

2. **`strategy.rs:373-461`** DRY violation — `health_check_all()` and `health_check()` in `CompositeCapabilityExecutor` duplicated ~30 lines of identical `CapabilityHealth` construction logic. Extracted into a shared `build_health()` helper method.

## Clean Areas (no issues found)
- **UTF-8 safety**: `response_parser.rs` correctly uses `char_indices()` for brace matching (byte offsets), and all string slicing targets ASCII-only patterns (`{`, `}`, `` ``` ``). No `&s[..n]` on arbitrary user strings.
- **Lock safety**: No `Mutex`/`RwLock` from `std::sync` used; all locking uses `tokio::sync::RwLock` with `.await` (no `.unwrap()` on lock).
- **No `unwrap`/`expect` on user-facing paths**: All methods return `Result` or use `Option` with early returns.
- **No SQL injection**: No LanceDB filters or `format!()` SQL construction.
- **No `static mut`**: None present.
- **Architecture compliance**: Follows strategy pattern (P3 extensibility), trait-based design (P1/P4), clean separation of concerns.

## Notes
- `CapabilityExecutor` (the older monolithic executor in `mod.rs`) coexists with `CompositeCapabilityExecutor` + `CapabilitySystem` (the newer strategy-based approach). `CapabilityExecutor` is not used outside the module (only a stale comment in `search/provider.rs` references it). It could be a candidate for future removal once confirmed fully superseded.
- The pre-existing compilation error in `bin/aleph-server/commands/start/builder/agent_init.rs:177` is unrelated to this module.

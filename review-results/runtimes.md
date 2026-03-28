All 30 tests pass. Here's the review summary:

---

# Module: runtimes

## Summary
- Files reviewed: 6
- Issues found: 4
- Issues fixed: 4

## Fixes
1. **probe.rs:231** Lock poisoning — `.lock().ok()?` silently returns `None` on mutex poison, causing version detection to silently fail → Changed to `.unwrap_or_else(|e| e.into_inner())` per project convention
2. **ledger.rs:164** Non-deterministic HashMap iteration in `build_path()` — PATH order varies across runs, affecting which binary gets resolved → Sort Ready entries by name before building PATH
3. **ledger.rs:193** Non-deterministic HashMap iteration in `list_ready()` — prompt content varies across runs → Sort results by name for deterministic output
4. **bootstrap.rs:95** Potential index panic — `spec.expected_paths[0]` would panic if `expected_paths` were empty → Use `.first()` with fallback

## Notes
- Code quality is generally high — clean separation of probe/bootstrap/ledger/ensure phases
- No UTF-8 slicing issues, no SQL injection, no `static mut`, no `unwrap()` on user-facing paths
- The `ensure.rs` module correctly uses `tokio::sync::RwLock` (not std) so `.await` is the right pattern there
- The pre-existing compile error in `agent_init.rs:177` is unrelated to runtimes

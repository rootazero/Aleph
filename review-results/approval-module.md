# Module: src/approval (desktop/browser action authorization)

## Summary
- Files reviewed: 5 (`mod.rs`, `types.rs`, `config.rs`, `adapters.rs`, `policy.rs`)
- Tests run: 206 passed, 0 failed
- Issues found: 1
- Issues fixed: 1

## Findings

### [Warning] Sync primitives import rule violation
- **Location**: `src/approval/adapters.rs:18`
- **Risk**: Code imports `std::sync::Arc` directly instead of `crate::sync_primitives::Arc`, violating Aleph invariant #8. While `Arc` is currently a straight re-export from `std::sync`, using the crate-level abstraction ensures future loom compatibility and consistency with the rest of the codebase.
- **Current impact**: Low (functional behavior is identical today)
- **Fix applied**: Changed `use std::sync::Arc;` to `use crate::sync_primitives::Arc;`

## Verification
- `cargo test -p alephcore --lib approval` — 206 passed, 0 failed
- `cargo check -p alephcore --lib` — clean (3 pre-existing warnings unrelated to approval)

## Notes

**Already correct patterns observed:**
- **Fail-closed design**: `adapters.rs` returns `ApprovalOutcome::Denied` when `SESSION_ID` task-local is unset — no silent auto-approve.
- **Decision priority**: `config.rs` correctly implements blocklist > allowlist > defaults > Ask.
- **Error handling**: `load_from()` gracefully handles missing files and parse errors with fallback to safe defaults.
- **No unwrap in production**: All `.unwrap()` calls are confined to test code.
- **Regex safety**: `regex::Regex` crate uses finite automata — no catastrophic backtracking / ReDoS risk.
- **Glob semantics**: `glob_to_regex_str` correctly handles `*`, `**`, `?` and special-character escaping.
- **No lock poisoning**: Module has no `std::sync::Mutex` or `RwLock` usage.
- **No UTF-8 byte slicing**: No `&s[..n]` patterns found.
- **No `static mut`**: None found.

**Minor observations (not fixed):**
- `config.rs` silently falls back to default policy when config file parsing fails. This is documented behavior, but in a strict fail-closed model a parse error could be treated more conservatively.
- `adapters.rs:85` passes `""` as `agent_id` to `request_for_tool`; the bridge accepts this but the empty value is not semantically meaningful.

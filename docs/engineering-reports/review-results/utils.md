

# Module: utils

## Summary
- Files reviewed: 7
- Issues found: 3
- Issues fixed: 3

## Fixes

1. **json_extract.rs:104** Dead logic in find predicate → Removed redundant `|| c == '{'` condition. `{` is not whitespace, so `!c.is_whitespace()` already matches it.

2. **text_format.rs:33** Double char iteration in `truncate_text` → Rewrote to single-pass using `char_indices().nth(max_chars)` — eliminates the redundant `chars().count()` traversal.

3. **paths.rs:96, 129, 331** TOCTOU race on directory creation (3 locations) → Removed `if !dir.exists()` guards before `create_dir_all`. The function is idempotent (returns `Ok` if dir exists), so the existence check is redundant and introduces a race window between check and create.

## Verification
- `cargo check -p alephcore --lib` — compiles clean
- `cargo test -p alephcore --lib utils::` — **87/87 tests pass**
- Pre-existing compile error in `bin/aleph-server/commands/start/builder/agent_init.rs:177` is unrelated

## Notes

The utils module is well-written overall. Key observations:

- **UTF-8 safety**: All string slicing in `json_extract.rs` uses byte offsets from `find()` or `char_indices()` on ASCII delimiters (`{`, `}`, `` ` ``). Safe by construction.
- **OnceLock usage**: Both `pii.rs` and `prompt_sanitize.rs` correctly use `OnceLock` for lazy-initialized static patterns. No `static mut`.
- **Path traversal protection**: `get_agent_config_dir` properly validates against `/`, `\`, `..`, and empty strings.
- **No locks, no SQL**: This module has no mutex usage or SQL filter construction, so those checklist items don't apply.
- **Architecture compliance**: Clean utility module, no violations of architectural redlines (R1-R10) or design principles (P1-P8).

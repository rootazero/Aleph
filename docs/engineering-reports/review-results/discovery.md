All 41 discovery-related tests pass. Here's the review summary:

# Module: discovery

## Summary
- Files reviewed: 4 (mod.rs, types.rs, paths.rs, scanner.rs)
- Issues found: 4
- Issues fixed: 4

## Fixes
1. **[scanner.rs:343] Doc/code mismatch — missing `hooks/` auto-discover check** → Added `path.join("hooks").is_dir()` to `has_plugin_manifest()`, matching the doc comment that lists `hooks/` as an auto-discover trigger
2. **[scanner.rs:343] Magic string instead of constant** → Replaced hardcoded `".mcp.json"` with `MCP_CONFIG_FILE` constant already defined in `paths.rs`
3. **[scanner.rs:174,256,277] DRY violation — hidden-dir check duplicated 3 times** → Extracted `is_hidden(path: &Path) -> bool` helper function, replaced all 3 inline checks
4. **[types.rs:101-195] Dead code — `ComponentType`, `DiscoveredComponent`, `DiscoveredConfig`** → Removed 3 types + impls + tests (defined but never used outside `types.rs`)

## Notes
- No UTF-8, lock safety, SQL injection, or `static mut` issues found — this module is purely filesystem-based with no string slicing, no locks, no database queries.
- Architecture compliance is excellent: clean separation, trait-based design, proper error propagation via `DiscoveryResult<T>`.
- The `find_upward` function has a subtle edge case where `canonicalize()` could succeed for `stop` but fail for `current` (or vice versa), causing the stop comparison to never match. This is safely guarded by `max_depth` so it's not a bug, just worth noting.
- Pre-existing compile error in `agent_init.rs:177` is unrelated to this module.

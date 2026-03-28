Lib compiles cleanly. Here's the review:

# Module: clawhub

## Summary
- Files reviewed: 3 (mod.rs, client.rs, types.rs)
- Issues found: 1
- Issues fixed: 1

## Fixes
1. **[client.rs:210] Slug path separator in temp filename** — `slug` format is `owner/skill` (contains `/`). Using it directly in `format!("clawhub-{}-{}.zip", slug, ...)` creates a path like `/tmp/clawhub-owner/skill-uuid.zip`, where the parent directory `clawhub-owner` doesn't exist, causing `fs::write` to fail. Fixed by sanitizing: `slug.replace('/', "-")`.

## Notes
This module is well-written. No issues found in the other checklist categories:

- **UTF-8 safety**: No byte slicing. The `body.chars().take(200)` in `check_status` is correct.
- **Lock safety**: No Mutex/RwLock usage.
- **unwrap safety**: Only `Client::builder().build().unwrap_or_default()` which is safe. All other error paths use `?` or `map_err`.
- **Architecture**: Clean separation between raw API types and internal unified types (`From` impls). Read-only client with no auth — appropriate for public API. Follows P1 (low coupling), P2 (high cohesion), P6 (simplicity).
- **Code quality**: Functions are concise, no dead code, no DRY violations. `encode_slug_path` properly handles URL encoding with per-segment percent-encoding.
- **Edge cases**: `unix_ms_to_rfc3339` handles invalid timestamps via `unwrap_or_default()`. `is_newer_version` has documented fallback for non-semver strings. Browse has fallback to search when endpoint returns empty.

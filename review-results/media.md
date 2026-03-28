

Lib compiles clean — the error is in the `aleph-server` binary, pre-existing and unrelated to our changes.

# Module: media

## Summary
- Files reviewed: 15
- Issues found: 3
- Issues fixed: 3

## Fixes
1. **`cache.rs:255`** `&item.url[..prefix_len]` → `item.url.chars().take(30).collect()` — UTF-8 byte slice on data URL could panic on multi-byte characters
2. **`cache.rs:247`** `&id[..8]` → `id.get(..8).unwrap_or(&id)` — unsafe byte slice pattern (UUID is ASCII but sets bad precedent)
3. **`cache.rs:275`** `&id[..8]` → `id.get(..8).unwrap_or(&id)` — same fix as #2

## Notes
This module is well-written overall:
- **No lock safety issues** — no `Mutex`/`RwLock` usage in the module
- **No `static mut`** — clean
- **No SQL injection** — no LanceDB filters
- **No dead code or unused imports**
- **Error handling** is solid — graceful fallbacks everywhere (`unwrap_or_default()` on HTTP client, fallback to URL-only on download failure)
- **Architecture compliance** — clean separation: trait-based providers (P4), single responsibility (P2), proper dependency inversion (P4)
- **`cleanup_stale()`** has a smart `.created_at` marker pattern to avoid mtime drift — good defensive design (P7)

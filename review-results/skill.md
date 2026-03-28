---

# Module: skill

## Summary
- Files reviewed: 12
- Issues found: 4
- Issues fixed: 4

## Fixes

1. **[mod.rs:136-181] DRY violation — `init()` and `rebuild()` duplicated scan logic**
   → Extracted common logic into private `rescan_dirs()` method. Both `init()` and `rebuild()` now delegate to it.

2. **[mod.rs:221-273] Non-deterministic iteration — `skill_status()` and `full_status()` returned HashMap-order results**
   → Added `.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()))` to both methods for deterministic UI/API output.

3. **[installer.rs:14-18] Security — incomplete command injection denylist**
   → Replaced denylist (`!contains(';')` etc.) with strict allowlist: only `[a-zA-Z0-9\-_./:@+=~]` are permitted. This blocks spaces, quotes, redirections, backticks, `$()`, and all other shell metacharacters. Added 2 tests covering malicious and legitimate package names.

4. **[commands.rs:40-46] Non-deterministic iteration — `resolve_command()` name-match used arbitrary HashMap order**
   → Collects all matching candidates, sorts by priority descending (then by id for tie-breaking), and picks the best match. Ensures workspace skills consistently override bundled skills with the same command name.

## Notes
- **UTF-8 safety**: `split_frontmatter()` uses byte indexing (`trimmed[3..]`, `rest[..closing_pos]`), but all delimiter patterns (`---`, `\n`) are single-byte ASCII, so the indices always land on valid UTF-8 boundaries. No fix needed.
- **Lock safety**: All locks are `tokio::sync::RwLock` (no poison), used correctly with `.await`. No `std::sync::Mutex` present.
- **No `unwrap()` on user-facing paths** in production code. All `unwrap()` calls are confined to test code.
- **Architecture compliance**: Module follows P1 (low coupling via traits), P2 (high cohesion — each file has single responsibility), P6 (simplicity), and R9 (tool-oriented design). No violations found.
- The pre-existing compile error in `agent_init.rs:177` is unrelated to this module.

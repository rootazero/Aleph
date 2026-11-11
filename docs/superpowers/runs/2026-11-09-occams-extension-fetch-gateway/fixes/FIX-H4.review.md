# Fix review — H4

| ID | Verdict | Comment |
|---|---|---|
| H4-01 | FIXED | Line 516 replaces `existing_ids: HashSet<String>` with `existing_configs: HashMap<String, String>` mapping ID to original name; line 523 checks `existing_name == &name` before marking migrated, line 527–532 logs collision and skips deletion when names differ. |
| H4-02 | FIXED | Lines 140–154 collect all provider names with their keys across categories into `name_to_keys`; lines 157–169 reject migration entirely if any name has multiple distinct non-empty keys, preserving plaintext on disk. |
| H4-03 | FIXED | Lines 370–376 backup old vault values before overwriting; lines 379–383 restore backed-up values on any store failure before returning error. |
| H4-04 | FIXED | Lines 757–762 now remove the inserted config entry and return `INTERNAL_ERROR` on save failure, preventing success response when persistence fails. |
| H4-05 | FIXED | Line 123 uses `std::mem::replace` to save original; line 127 restores `cfg.general = original` before returning error on save failure. |
| H4-06 | FIXED | Lines 109–111 wrap `find_latest_log(&log_dir)` in `tokio::task::spawn_blocking`, moving synchronous directory traversal off the worker thread. |
| H4-07 | FIXED | Lines 439–464 extract config snapshot in inner scope, dropping guard before line 471 `create_channel_from_config` and line 486 `registry.register`. |

## New problems

- `src/gateway/handlers/channel.rs:889` — Added regression test `create_persists_before_creating_or_registering_a_runtime_channel` performs string search to verify ordering, but current source at line 758 already removes config and returns error before reaching creation. The test will pass because the source already persists before creating, but the test's premise is that a fix was needed. The fix agent noted this (H4-NEW-01 SKIPPED), but left the test in place anyway. This is not a functional bug, but documents a non-existent pre-fix defect.

## Batch verdict

Safe to commit. All findings fixed correctly; no regressions. The extra test is benign structural validation rather than a functional problem.
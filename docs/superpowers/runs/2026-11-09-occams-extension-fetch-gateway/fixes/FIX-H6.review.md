# Fix review — H6

| ID | Verdict | Comment |
|---|---|---|
| H6-01 | NOT_FIXED | No changes to `update.rs:222-227` or event publication; the diff does not touch the file that serializes the DTO into the event value. |
| H6-02 | FIXED | Lines `+70-80` move `cfg.save_incremental` before `vault.delete_secret`, reversing the destructive order. |
| H6-03 | NOT_FIXED | No changes to `helpers.rs:52-58` (`find_provider_type`); the lookup still infers bucket from `capabilities.first()` rather than returning the actual storage location. |
| H6-04 | FIXED | Lines `+693-700` introduce `saved_generation_probe`, which returns `Some(type_str)` only when type, base_url, and credential all match; line `+703` conditions the update on `matched`. |
| H6-05 | NOT_FIXED | No changes to `install.rs:65`; synchronous `git2::Repository::clone` remains in the async handler without `spawn_blocking`. |
| H6-06 | NOT_FIXED | No changes to `cron/real.rs:694-700`; `as_bool()` returning `None` for non-boolean still triggers the toggle branch. |
| H6-07 | NOT_FIXED | No changes to `cron/real.rs:475-486,525-526`; timezone-only updates still read schedule outside the final lock scope. |
| H6-08 | FIXED | Lines `+23-33` (`update_config_toml`) acquire `<config>.lock` before read/modify/write and call `write_atomic` (line `+36`); all four security writers now use this wrapper (lines `+454,+522,+578,+640`). |
| H6-09 | FIXED | Line `+36` delegates to `write_atomic`, which the evidence confirmed creates files with `0600`; all security writers route through `update_config_toml`. |
| H6-10 | FIXED | Line `+475` checks `i64::try_from(window.window_secs)` and returns `Err` on overflow before constructing the TOML value. |

## New problems

- `handlers.rs:+612-639` (`saved_generation_probe`) — the helper is defined inside the test module (`#[cfg(test)] mod probe_verification_tests`) at lines `+554-608`, making it unavailable to the production handler at line `+695`; the code will not compile.

## Batch verdict

**Needs rework.** H6-01, H6-03, H6-05, H6-06, H6-07 remain unaddressed, and the new `saved_generation_probe` function is unreachable from production code due to module boundaries.
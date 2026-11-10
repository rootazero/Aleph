# Fix review — H3

| ID | Verdict | Comment |
|---|---|---|
| IF4-03 | FIXED | `handle_update` now selects `current_host.clone()` when `matches_bind_address(&current_host)` is true, and only derives a canonical host after an explicit mode change, preserving custom addresses such as `192.168.1.20` and `::`. |
| H3-01 | FIXED | The new match rejects every non-null, non-string `user_id` with `INVALID_PARAMS` before ticket creation, so values such as `42` cannot silently become an unbound owner ticket. |
| H3-02 | FIXED | The fallback now obtains both `preset_entry` and `user_entry`, then applies `user.hydrate_from_preset(&preset)` before enabling the result, preserving user overrides while retaining preset defaults. |
| H3-03 | FIXED | `security_config.network_access.matches_bind_address(&current_host)` selects `current_host.clone()` for an unchanged projected mode, while an explicit mode switch still uses `to_bind_address()`. |
| H3-04 | FIXED | Scope is fully parsed into `Option<PromptScope>` before `SetEnabled` is called, and an invalid scope returns `INVALID_PARAMS` before any mutation. |
| H3-05 | FIXED | `devtools_profile` is now matched against exactly `"user"` and `"managed"`; every other value returns `INVALID_PARAMS` before shared configuration is written. |
| H3-06 | FIXED | Browser, execution, and behavior handlers save the previous section and restore it on `save_incremental` failure, with behavior restoring the complete `Option`. |
| H3-07 | FIXED | The complete path-resolution and directory scan now runs in one `tokio::task::spawn_blocking` closure, including `read_dir`, canonicalization, metadata access, denial checks, and sorting. |

## New problems

- none

## Batch verdict

Safe to commit; all listed failure modes are removed without an evident independent regression in the shown diff.
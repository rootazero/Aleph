# Fix review — E3

| ID | Verdict | Comment |
|---|---|---|
| E3-01 | FIXED | `binds_relative_script_outside_root` (consent.rs:612–633) returns `true` when `same_dir(cwd, plugin_root)` is false and `bound_script_word` resolves a relative (non-absolute, non-`~/`) token; executor.rs:846–862 calls it before spawn and returns `Err(ExtensionError::HookExecution(...))`, blocking the mismatched execution. |
| E3-02 | FIXED | The `Err(e)` arm now returns `Err(ExtensionError::HookExecution(...))` (executor.rs:1211), and the post-match `Ok(ar) if !ar.success => Err(...)` (executor.rs:1219–1222) converts non-2xx `ActionResult` to `Err`, routing both through the existing fail-closed interceptor `Err` branch instead of being parsed as empty-body decisions. |
| E3-03 | FIXED | `spill_dir` now uses `format!("{}_{}", sanitize_prefix(session_id), &hash[..16])` with a SHA-256 digest of the original session ID (output_budget.rs:126–136), so `sess:s1` and `sess_s1` produce distinct directory names and pruning no longer crosses session boundaries. |

## New problems

none

## Batch verdict

Safe to commit — all three findings are addressed, no regressions or independent issues introduced.
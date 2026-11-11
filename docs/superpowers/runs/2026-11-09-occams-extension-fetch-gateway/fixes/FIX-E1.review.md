# Fix review — E1

| ID | Verdict | Comment |
|---|---|---|
| E1-03 | NOT_FIXED | The diff only changes a test helper call signature in `lifecycle.rs` (`.attach_stream(&session.id)` → `.attach_stream(&test_support::operator(), &session.id)`); it does not touch `mounted_plugin_tools_snapshot` at line 1419 of `mod.rs`, the `retain`/dedup loop, or the ordering of filter-then-dedup that the finding describes. |

## New problems

- `src/extension/lifecycle.rs:6642` — The `attach_stream` signature change may be a legitimate API update, but it appears as an unrelated side-effect with no corresponding change to the production code path under review; it is unclear whether other callers of the old one-argument form still compile. This is a low-risk concern but it is not part of the E1-03 fix scope.

## Batch verdict

Needs rework: E1-03 is entirely unaddressed — the dedup-before-filter bug in `mounted_plugin_tools_snapshot` (`mod.rs` ~line 1401–1424) was not touched. The diff is an unrelated test call-site fixup.
# Fix review — E5

| ID | Verdict | Comment |
|---|---|---|
| E5-01 | NOT_FIXED | No diff appears for `host_functions.rs:50` or the canonical-path prefix check; the failure mode is unaddressed. |
| E5-02 | NOT_FIXED | No diff appears for `host_functions.rs:72` metadata/read logic; FIFO blocking and unbounded reads remain. |
| E5-03 | NOT_FIXED | No diff appears for `credential_injector.rs:173` or post-injection allowlist validation; URL-path injection is unaddressed. |
| E5-04 | NOT_FIXED | No diff appears for `host_functions.rs:303` error formatting or reqwest `without_url()`; credentials can still leak in errors. |
| E5-05 | FIXED | Line `+229: self.log_count.store(0, Ordering::SeqCst);` resets the log counter, and line `+642: loaded.kernel.reset_per_execution_counters();` calls it before each execution. |
| E5-06 | NOT_FIXED | No diff appears for `plugin_registry/mod.rs:341` or the short-key `or_insert_with` logic; stale duplicates remain. |

## New problems

- `capability_kernel.rs:229` — Resets `log_count` but the finding required resetting `log_line_count` (the actual field name per line 58 of the original); introduces a compile error or runtime no-op if field was renamed without updating all call sites.
- `mod.rs:461` — Adds `kernel: kernel.clone()` to `LoadedWasmPlugin`, but the struct definition at lines 64–71 shows no `kernel` field was added; will not compile.
- `host_functions.rs:463` — Changes assertion from string `"allowed"` to bytes `b"allowed".to_vec()`, altering test contract without any finding requesting it.

## Batch verdict

**Needs rework.** Only E5-05 is substantively addressed, but introduces two compilation blockers (missing struct field, wrong counter field name) and an unrelated test change. The four P0 findings are completely unaddressed.
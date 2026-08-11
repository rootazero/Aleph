# Builtin Tools Batch 2 — file_ops/* Code Review

**Date**: 2026-08-11
**Path**: `src/builtin_tools/file_ops/*` (16 files, ~9543 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    1 |     2 |   3 |    6 |

---

## Findings

### [HIGH] apply_patch.rs:151 — no upper bound on patch envelope size → memory exhaustion DoS
- **Category**: DoS / architecture
- **Description**: `ApplyPatchArgs.patch: String` is taken verbatim into `parse_patch`, which builds a `Vec<PatchOp>` plus per-op `lines: Vec<Line>`. The whole envelope is also held in memory through `execute` and the planning pass. `patch_bytes = args.patch.len()` is logged but never *bounded*. A runaway model (or a malicious prompt that asks the model to embed the entire `/usr/include` tree as `+<line>` entries) can submit a multi-hundred-MB patch that consumes process memory before any write is rejected.
- **Suggested fix**: Reject at the dispatcher: `if args.patch.len() > MAX_PATCH_BYTES` (suggest 4 MB — generous for V4A envelopes which are typically < 100 KB). Return `ToolError::InvalidArgs` with a clear message naming the cap and pointing to `apply_patch` alternatives for bulk rewrites.

### [MEDIUM] apply_patch.rs:644-789 — `PatchOp::Add.lines` count and per-line size are unbounded
- **Category**: DoS
- **Description**: The `PatchOp::Add { path, lines: Vec<Line> }` payload flows into `plan_add`, which appends each line to the in-memory `body` and writes it through `atomic_write_file`. There is no cap on `lines.len()` and no per-line length cap. A single Add op can therefore consume unbounded memory and write a file large enough to fill the disk.
- **Suggested fix**: Two checks at parse time (or in `plan_add`): `lines.len() <= MAX_PATCH_OPS_LINES` (suggest 100k) and each `Line.content.len() <= MAX_PATCH_LINE_BYTES` (suggest 1 MB). Surface clear errors naming the cap. The model rarely writes a 100k-line file with one `Add` — for those, file_write is the right tool.

### [MEDIUM] apply_patch.rs — denied-paths enforcement skips `PatchOp::Add` when the parent directory does not yet exist
- **Category**: security
- **Description**: `plan_add` resolves the target via `check_and_resolve_path`, which canonicalizes the *longest existing ancestor* and then lexically resolves the remainder. The deny check runs against the resolved path — good. But a `*** Add File: ~/.ssh/authorized_keys` op passes the resolve-and-deny check *iff* the deny list contains an entry like `<config_dir>` whose prefix match catches it; the broad `/root/.ssh` rule in `get_denied_paths` (line 100-ish) does catch it, but the failure mode is *path-prefix correctness* rather than deny-list mistakes — i.e. any future "open up the ruleset" change that drops a deny entry silently re-opens the gate. Worth a comment lock or an explicit `PatchOp::Add` deny precheck that fails closed if the resolved path's ancestor is on the deny list.
- **Suggested fix**: Add an explicit precheck in `plan_add`: if any ancestor (after canonicalization) is denied, refuse with `ToolError::InvalidArgs`. Defense in depth.

### [LOW] apply_patch.rs:1066 — `unreachable!("just initialised")`
- **Category**: quality
- **Description**: Defensively unreachable after the slot was just initialised; the `unwrap_or_else` mirrors a `match` that always lands in `Some`. Correct, but the `unreachable!()` panic surface is one refactor away from being hit.
- **Suggested fix**: Replace with `let Some(s) = slot else { unreachable!("slot is initialised immediately above") };` for a clearer intent, or refactor the surrounding function to remove the Option.

### [LOW] apply_patch.rs:1140, 1144 — `panic!("expected Add/Update/Delete, got {:?}", other)`
- **Category**: quality
- **Description**: Test-only debug assertions; guarded by `#[cfg(test)]`. Acceptable as is, but a `match` with `unimplemented!()` is the convention for "this should not happen in tests either."
- **Suggested fix**: Leave; flag only if test rewrites are scheduled.

### [LOW] path_utils.rs — `get_denied_paths()` re-reads config on every call (no caching)
- **Category**: quality
- **Description**: Called from many sites (per write/read/edit). Each call walks the env, may call `crate::utils::paths::get_config_dir()`, and rebuilds the Vec. The list is effectively static for the server's lifetime; the cost is small but the function is on every file I/O hot path.
- **Suggested fix**: Cache the denied list with `OnceLock<Vec<String>>` populated lazily on first call. Pure perf nit; no correctness impact.

---

## Strengths

The file_ops module is unusually well-defended for its size:

- `check_and_resolve_path` correctly canonicalizes, denies, rebases through `FsScope`, and refuses `..`-based traversal in `reject_unsafe_glob_pattern`. The two-path-resolvers comment in `path_utils.rs:660-680` is exactly the right discipline.
- `execute_delete` distinguishes symlinks from regular files via `symlink_metadata` and refuses to descend into a symlink's target.
- `execute_copy` threads `denied_paths` through recursive walks with a `CopyTally` that discloses *which* paths were skipped and *why* (protected vs. unresolvable) — folding those into one number would have been a quieter, less honest contract.
- `read.rs` enforces a two-limits-whichever-first rule (line count + token window), with a self-describing continuation message that avoids the offset-holes trap.
- `stats.rs` refuses to count lines in files > 16 MB (`MAX_LINE_COUNT_BYTES`), avoiding a multi-minute walk on a `/var/log`-sized file.
- `apply_patch.rs` is two-phase (plan all ops, then commit) so a hunk miss on the last op leaves the first untouched.

The single missing piece is **input size bounds** — every other surface has a cap except the patch envelope itself.

---

## Recommended Single Fix

Add a small validation layer at the top of `apply_patch.rs::run` before `parse_patch`:

```text
if args.patch.len() > MAX_PATCH_BYTES (4 MB) -> reject
if args.patch.contains("\n*** Add File:").count() > MAX_PATCH_OPS (500) -> reject
```

Plus the same `is_finite`/length guards inside `plan_add` and `plan_update` for per-op payload sizes. Estimated 15 lines, closes both HIGH and MEDIUM findings in one place.
# Module: src/utils (occams-r9 review, 2026-10-02)

## Summary

- Files reviewed: 20 (all of `src/utils/*.rs`), 9 863 lines total
- Total findings: **0 critical / 0 warning / 0 suggested test**
- Status of utils-2026-08-29 Mediums:
  - **M1** (`scratch.rs` SIGKILL on possibly-recycled PID) — **STILL DEFERRED** (still latent; API surface change still required)
  - **M2** (`reqwest_limit.rs` zero tests) — **STILL FIXED** (9 test entries: 8 `#[tokio::test]` at lines 120/166/180/196/220/245/264/288 + 1 `#[test]` at line 143)
- Status of utils-2026-08-29 Lows:
  - **L1** (`paths.rs` inline `use tracing::info;`) — **STILL FIXED** (file has top-level `use tracing::{info, warn};`; `grep "^use tracing::info"` returns no matches)
  - **L2** (`atomic_write.rs` `set_permissions` silently swallowed) — **STILL DEFERRED** (`src/utils/atomic_write.rs:77` still has `let _ = fs::set_permissions(&tmp_path, meta.permissions()).await;`)
- NEW findings (not in utils-2026-08-29): **0**
- New files since 2026-08-29: **1** — `src/utils/shell.rs` (832 lines, commit `8337e4f1f` "utils: resolve the agent's shell once, per platform, absolutely", 2026-09-05). Reviewed in full; no findings at W or higher.

## Critical

(none)

## Warning

(none)

## Suggested Test

(none — see "Per-perspective" for low-confidence test gaps that did not clear the bar.)

## Per-perspective (lower confidence)

The module is in unusually strong shape; nothing below rises to W. The following are observations, not findings:

### Security

- **`src/utils/shell.rs:414` — `cmd_shell()` trusts `%COMSPEC%` directly** without routing the candidate through `is_windows_apps_alias`. Other shell resolvers (`Bash`/`Pwsh`/`WindowsPowerShell`) route every candidate through `is_windows_apps_alias` (line 262). `cmd.exe` from a `WindowsApps` alias would be picked up here. Defense-in-depth only — `%COMSPEC%` is system-set, not user-controlled on a normal install — but the asymmetry with the other resolvers is conspicuous.
- **`src/utils/scratch.rs:80-89` — `reap()` SIGKILL risk unchanged.** The M1 finding stands exactly as documented. The fix path is now well-understood (see "Fix order proposal" below) because `process_alive::process_matches(pid, expected_start)` is the documented SSOT at `src/utils/process_alive.rs:72`. **Still deferred — no change in status.**
- **`src/utils/instance_lock.rs` is in the strongest state of any module reviewed this round.** Specifically: `a_refused_lock_is_an_error_not_a_stale_peer` (network/FUSE `ENOLCK`/`ERROR_NOT_SUPPORTED` is now an `Err`, not "orphaned — rm") closes the AGENTS.md-named vault-data-loss vector; `try_acquire_refuses_a_symlink_at_the_lock_path` defends against attacker-planted redirect; `diagnose_holder_propagates_io_error_for_unreadable_sidecar` pins the doctor-check contract. No findings.

### Logic

- **`src/utils/shell.rs` — threshold checks are byte-based, not char-based.** `STDIN_PIPE_THRESHOLD` (line 27, `32*1024`) is checked against `script.len()` at line 138; `PWSH_STDIN_THRESHOLD` (line 41, `8*1024`) against `wrapped.len()` at line 175. This is **correct** because both are defending against `MAX_ARG_STRLEN` (kernel-side, byte count). Multibyte UTF-8 doesn't introduce a hidden bug — a 32 000-byte budget admits ≈32 000 UTF-8 code points, never more. Worth noting because a future contributor might be tempted to "fix" it to `.chars().count()` and silently weaken it.
- **`src/utils/shell.rs:489-501` — `python3()` Windows arm deliberately limits fallback to `py -3 → python → python3`.** No fallback to well-known absolute paths (`C:\Python3X\Python.exe`, `%LOCALAPPDATA%\Programs\Python\…`). The docstring explicitly justifies this ("official launcher is the one name that stays right across installs"). This is a deliberate, documented scope decision — not a finding.

### Architecture

- **`src/utils/shell.rs` OnceLock caching is the right shape for the cost.** `AGENT_SHELL`/`PWSH`/`WINDOWS_POWERSHELL`/`PYTHON3` are all `OnceLock` (lines 233/234/235/451), avoiding the cost of `which::which` + `canonicalize` on every shell invocation. `resolve_in(path_var)` (line 478) and `python3_in(path_var)` (line 538) provide the uncached seams tests need. No findings.
- **`src/utils/scratch.rs:80-89` — the `atexit` SIGKILL risk compounds with `process_alive::process_matches`'s reliance on `sysinfo::System::new()`.** `sysinfo::System::new()` allocates. An atexit handler runs after the program has begun exiting — heap fragmentation is plausible. If the M1 fix is implemented, the cheap signal-safe alternative is `libc::kill(pid, 0)` for liveness (does not require sysinfo) and a recorded start-time comparison done only on the success path. The current M1 finding's suggested fix should be sharpened to prefer `kill(pid, 0)` over `process_matches` inside the atexit handler. See the `Note:` in M1's original report — this is a sharper version of that observation.

### Quality

- **`src/utils/shell.rs` test `cached_answers_are_runnable_when_the_host_has_a_runnable_candidate`** (line 854) verifies the cached answer lives under the expected roots (`under(program)`) but does **not** verify the cached program is runnable (exec bit). `pick_runnable` (line 376) checks `metadata().mode() & 0o111 != 0` AND `under_roots(path)` AND `under_roots(canonicalize(path))`; the test only verifies the second leg. A future change to `pick_runnable` that drops the exec-bit gate would slip through. Test gap, not a code bug — the gate is still in place.
- **`src/utils/atomic_io.rs` is the hardening that the rest of the workspace depends on.** `write_atomic` (line 30) creates with `O_CREAT | O_EXCL` and 0600 mode on Unix; the `is_lock_contended` predicate (line 122) deliberately uses `raw_os_error()` rather than `ErrorKind::WouldBlock` to fix the Windows fs2 misclassification (ERROR_LOCK_VIOLATION → Uncategorized). The `with_file_lock` 5-second deadline (line 137) is a hard ceiling, not a fallback to "best-effort". No findings; this module is a model for what other writers should look like.
- **`src/utils/paths.rs` meta-guard tests are the most heavily self-tested block in this module.** `HOME_JOIN_ALLOWLIST` (lines 1100–1600 range), `no_hand_rolled_aleph_home_outside_the_allowlist`, `no_link_arg_names_a_path_in_the_source_tree`, `manifest_dir_tainted` (8-pass fixpoint), `guard_predicate_sees_multi_line_spellings`, `every_exemption_still_offends`, `link_arg_guard_sees_the_shape_that_shipped`. Each of the 9 allowlist entries is asserted to **still trigger** the guard (no orphan allow-list rows). This is exemplary.
- **`src/utils/filename.rs`** consolidates two prior private copies into one `sanitize_filename` with seven invariants. Reserved-name detection cuts at the **first** dot (`CON.txt` and `con.tar.gz` both flagged — line 173-ish, the cut-at-first-dot rule is explicitly documented). 7 tests pin each invariant including prefix-not-match (`CONTACTS.txt` passes), colon/space special handling (`con:foo` → `confoo`, `"con "` → FALLBACK_FILENAME), and `MAX_FILENAME_CHARS = 200` (char count, not bytes, for UTF-8 safety). Strong module; no findings.
- **`src/utils/text_format.rs`** unified 22 private truncate helpers into 5 named contracts with a per-helper property table at lines 6–31. 13 tests, including proptest for `truncate_bytes`. Strong module; no findings.
- **`src/utils/source_scan.rs`** documents its own known gaps (F2: unlexed `starts_with("#[cfg(test)]")` could mis-fire on string payloads) in its module doc — the gaps are tracked, not lost. Strong module; no findings.
- **`src/utils/json_extract.rs`** uses four strategies (direct / ```json / generic ``` / brace-match) with 16 tests, including JSONC/JSON5 trailing-lang-id handling (`extract_from_json_code_block`, line 115) and depth-counter with string-aware escape handling (`find_matching_brace`, line 207). Strong module; no findings.
- **`src/utils/host.rs`** centralizes hostname resolution and has a meta-test (`no_other_module_hand_rolls_the_hostname_env_read`) that walks `src/` for direct env reads. Strong module; no findings.

## Conclusion

### Net delta from utils-2026-08-29

- **What holds:** the four prior findings are unchanged. M1 and L2 remain deferred for the same reasons (M1 needs an API-surface change; L2 has no live call site that would notice). M2 and L1 remain fixed.
- **What changed:** one new file (`src/utils/shell.rs`, 832 lines, 24 tests) landed on 2026-09-05. It resolves the agent's shell + `python3` exactly once per process, with byte-based thresholds, OnceLock caching, and four shells (Bash/Pwsh/WindowsPowerShell/Cmd). It is reviewed clean — no W or C findings. The instance_lock module has been substantially hardened since 2026-08-29 (added `classify_lock_failure`, `O_NOFOLLOW`, `is_lock_held`, `diagnose_holder`, `HolderDiagnostic`, `rewrite_holder_pid`, `Drop` impl, `HELD_HOLDER_RECORDS` registry). The hardening closes the AGENTS.md-named vault-data-loss vector and defends against symlink-planted lock redirects.
- **What didn't regress:** no W or C findings anywhere in the module. Test surfaces match risk levels (24 tests on shell, 16 on json_extract, 7 on filename, 13 on text_format, ~30+ on instance_lock including meta-guard coverage, 10+ on atomic_io including Windows fs2 misclassification pin).

### Fix order proposal (utils-only)

If a reviewer must pick 2–3 fixes to apply first, in priority order:

1. **M1 — sharpen and apply.** The fix path is now well-defined:
   - Change `DOOMED: Mutex<Vec<(Option<u32>, PathBuf)>>` (line 77) to `Vec<(Option<u32>, PathBuf, Option<u64>)>`.
   - Change `register_for_exit(pid: Option<u32>, dir: PathBuf)` (line 74) to `register_for_exit(pid: Option<u32>, dir: PathBuf, start: Option<u64>)`.
   - In `reap()` (line 80): replace the unconditional `libc::kill(pid, SIGKILL)` with a `libc::kill(pid, 0)` liveness probe + start-time comparison (cheaper than `process_alive::process_matches`; the latter allocates `sysinfo::System::new()` which is unsafe to call from an atexit handler on fragmented heap).
   - Call-site updates: `keep_until_exit` (line ~149) must capture `start_time` before `register_for_exit`; `reap_on_exit` (line 108 Unix arm, line 130 Windows arm) gains the new parameter. The Windows arm is unchanged in behaviour — `kill_on_this_process_exit` already handles its own correctness.
   - Test surface: at minimum, a test that a `(pid, stale_start)` tuple does **not** SIGKILL (i.e., the function short-circuits when start times mismatch); a test that `(pid, matching_start)` still SIGKILLs. The "recycled PID" test itself is essentially infeasible to trigger deterministically in CI — the cheaper property tests cover the contract.
   - **API impact: public.** The change to `register_for_exit` is breaking for any out-of-tree caller, but it is `pub(crate)`-equivalent in practice (used only by `scratch.rs` itself and `reap_on_exit`).

2. **L2 — apply.** `src/utils/atomic_write.rs:77`:
   - Replace `let _ = fs::set_permissions(&tmp_path, meta.permissions()).await;` with a logged form: `if let Err(e) = fs::set_permissions(&tmp_path, meta.permissions()).await { tracing::warn!(path = %tmp_path.display(), error = %e, "preserve_permissions_failed"); }`.
   - Use the module's existing logging style (it already uses `tracing::warn!` elsewhere — confirm by adding `use tracing::warn;` to the top of the file).
   - Test surface: `preserves_permissions_on_overwrite` already exists and passes; add `logs_when_set_permissions_fails_after_write` that flips the source to a read-only mode and asserts the warning fires. (Unix-only.)
   - **API impact: none.** One-line fix. The deferral justification was "no live call site that would notice" — this fix doesn't change that, but it makes the failure visible if one ever shows up.

3. **(Optional, defense in depth) `shell.rs:414` — route `%COMSPEC%` through `is_windows_apps_alias`.** Bring `cmd_shell()` in line with the other three resolvers. Tiny change; closes a one-line asymmetry. Test surface: extend the WindowsApps-alias test family with a `cmd_under_windows_apps_alias_is_not_returned` test using the same WindowsApps-stub trick used for the other shells.

Items 1 and 2 are the cheapest+highest-impact. Item 3 is bonus.

### What was NOT done in this review

- No `cargo check` or `cargo test` was run (per the hard constraints).
- No files were edited.
- No commits or pushes.
- No `cargo fmt -- <file>` was invoked (the constraint exists because the workspace is not byte-clean under a single rustfmt edition; running it would touch unrelated files).
- Findings are filed against utils-side code only; cross-module evidence (e.g., `instance_lock` is consumed by `aleph-server` lifecycle code) is cited as evidence, not as a finding in another module.
- The "Per-perspective" test gaps (`shell.rs` `cached_answers_are_runnable_...` not checking exec bits) did not clear the bar to ST level; they are documented here for a future contributor who may want to tighten them, but no ST was filed.
- The known gap F2 in `source_scan.rs` (unlexed `starts_with("#[cfg(test)]")` could mis-fire on string payloads) is documented in the module doc itself and is tracked there; no action taken or proposed here.

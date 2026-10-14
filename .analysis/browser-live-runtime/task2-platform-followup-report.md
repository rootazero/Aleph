# Task2 platform follow-up report (ffmpeg PATH resolution + Tokio minimum)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
Scope: narrow follow-up to `task2-i1-i4-fix-report.md`. No commit/reset/renormalize/merge; no Native/Gateway restart; unrelated dirty files untouched.

## Files changed

1. `src/browser/cdp_backend/recording.rs`
   - `which_on_path(name)` now reads PATH (and PATHEXT on Windows only) without mutating env and delegates to a pure resolver.
   - New pure helpers:
     - `parse_pathext(Option<&OsStr>) -> Vec<String>`: `;`-split, trim, drop empty/`.`, add missing leading `.`, case-insensitive dedupe keeping order; unset/empty -> `DEFAULT_PATHEXT = ".COM;.EXE;.BAT;.CMD"`.
     - `executable_names(name, Option<&[String]>) -> Vec<String>`: non-Windows -> `[name]`; Windows bare name -> `name+suffix` only (extensionless files are never accepted); Windows name with explicit extension -> as-is first, then `name+suffix` (cmd.exe order).
     - `path_probe_candidates(&OsStr, &[String]) -> Vec<PathBuf>`: directory-major order, empty PATH entries skipped (no implicit cwd).
     - `find_executable_in(path, pathext: Option<Option<&OsStr>>, name) -> Option<PathBuf>`: accepts only `is_executable` (unix: regular file + exec bit; non-unix: `is_file()`, so directories are rejected).
   - Precedence unchanged: `ALEPH_FFMPEG` -> PATH -> Playwright bundle (`ffmpeg_candidates` / `pick_ffmpeg` untouched).
   - New tests: `pathext_parsing_normalizes_and_falls_back_to_the_default`, `executable_names_follow_platform_suffix_rules`, `path_probe_candidates_are_directory_major_and_skip_empty_entries` (all platforms), and `#[cfg(windows)] windows_resolver_finds_suffixed_file_and_requires_a_real_file` (two TempDirs; decoy dir holds a directory `ffmpeg.exe` and extensionless file `ffmpeg`, real dir holds `ffmpeg.exe`; PATH/PATHEXT passed explicitly, process env never touched; also asserts explicit-extension lookup and that PATHEXT `.CMD` alone does not find `.exe`).
2. `Cargo.toml` (root `alephcore` package only)
   - `[dependencies] tokio` `1.35` -> `1.41` (+ one rationale comment line).
   - `[dev-dependencies] tokio` (`test-util`) `1.35` -> `1.41` (same package; kept consistent).
   - NOT changed: `[workspace.dependencies] tokio = "1.35"` (consumed only by `desktop/shared` via `workspace = true`, which does not use the Id APIs), `crates/aleph-cdp`, `interfaces/*`, `shared/client`, desktop crates — none use `task::Id`/`try_id`/`AbortHandle::id` (grep over src/crates/interfaces/shared/desktop).
3. `Cargo.lock`: not modified by this follow-up (sha256 `3c1800356de55dce4395750c9ce88a300ef5a91b498031df77980d54250561bb` before and after both Cargo runs; `--locked` check passed). Lock already resolves tokio 1.52.3.

## Tokio minimum rationale

Evidence: vendored registry `C:\Users\zou\scoop\persist\rustup\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.52.3\CHANGELOG.md`:
- 1.18.0: "task: add `task::Id` type" (unstable, `tokio_unstable`).
- 1.22.0: "task: add `task::id()` and `task::try_id()`" (unstable).
- 1.41.0 (Oct 22nd, 2024) "### Added": "task: stabilize `Id` apis ([#6793], [#6891])".
- In 1.52.3 source, `src/runtime/task/abort.rs:72 pub fn id(&self) -> super::Id`, `src/runtime/task/id.rs:45 id()`, `:58 try_id()`, re-exported at `src/task/mod.rs:320` without unstable gating.
So 1.41 is the minimum where these APIs exist without `--cfg tokio_unstable`. Note: alephcore already used `tokio::task::Id` before Task2 (`src/agents/subagent_tool/loop_tool.rs:988`, `src/gateway/resume_coordinator.rs:269`), so the 1.35 floor was already understated; Task2's `routes.rs:326/330/565` widened it. Tokio 1.41 MSRV (1.70) is far below workspace `rust-version = "1.95"`; MSRV policy unaffected.
Limitation: could not fetch the GitHub PR (#6793) page (proxy fake-IP block), so per-API attribution to 1.41 rests on the CHANGELOG entry, not the PR diff. Minimal-versions resolution (`-Z minimal-versions`) was not run.

## Safety audit before Cargo

Selected tests (`browser::cdp_backend::recording`): no `set_var`/`remove_var`, no keyring/SQLite/vault, no `remove_dir*`. All file writes go to `tempfile::tempdir()`. `the_default_path_lands_under_the_managed_dir_with_profile_and_tab` only computes a path via `get_config_dir()` (read-only `aleph_home()`, no create). `resolve_ffmpeg_finds_the_one_on_this_machines_path` reads env and stats files only. Encoder stub tests build a stub with rustc inside a TempDir.

## Commands (one at a time, no Cargo/rustc running beforehand, ALEPH_FFMPEG unset)

Env: `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`

| # | Command | Free mem before | Exit | Result |
|---|---------|-----------------|------|--------|
| 0 | `rustfmt --edition 2021 --check src/browser/cdp_backend/recording.rs` | - | 0 | clean |
| 0 | `git -c core.autocrlf=true diff --check -- Cargo.toml src/browser/cdp_backend/recording.rs` | - | 0 | clean |
| 1 | `cargo test --offline -p alephcore --lib browser::cdp_backend::recording -- --test-threads=1` | 16051654656 B | 0 | 23 passed, 0 failed (19 prior + 4 new incl. Windows resolver test); 0 warnings |
| 2 | `cargo check --offline --locked -p alephcore --lib` | 17310552064 B | 0 | Finished in 3m44s; 0 warnings |

Key fact: without `ALEPH_FFMPEG`, `resolve_ffmpeg_finds_the_one_on_this_machines_path` and the 8 recording pipeline tests that previously failed now pass — PATH resolution finds the scoop `ffmpeg.exe` shim.

## Not done / limitations

- Did not rerun the live 123/123 or routes 31/31 suites (out of scope; this change touches only ffmpeg PATH lookup and manifest floors).
- No `cargo clippy`, no full workspace build/test, no `-Z minimal-versions` build at tokio 1.41.
- The Windows resolver accepts any PATHEXT suffix (e.g. `.bat`/`.cmd`) as cmd.exe would; launching an ffmpeg `.cmd` wrapper via `std::process::Command` relies on Rust's batch-file argument escaping. Not separately tested.
- Non-unix `is_executable` is `is_file()`; no ACL/execute-permission check on Windows.
- Task2 overall completion is not claimed here; only these two follow-up items are verified as above.

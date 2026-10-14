# Off-loop final verification (clippy + rustfmt) — read-only audit

Worktree: panel-browser-workspace-20261007-202532-13256 (after rescan generation fence + cancellation-aware init hardening)

## Safety audit (before running cargo)
- `cargo clippy --lib` does not execute tests or library code. Only build.rs runs. Default features = [] (Cargo.toml:149), so build.rs `control-plane` trunk fallback is compiled out. build.rs touches only OUT_DIR (macOS plist staging, non-Windows).
- Home/.aleph/keyring/SQLite/remove_dir_all hits in src/skill/mod.rs (remove_dir_all :718, runtime only; others are path classification or #[cfg(test)] tests under TempDir) and src/skill/shared.rs, connection/{mod,off_loop}.rs: none are reached by clippy. No tests run.
- No cargo/rustc running beforehand; memory gate `py -B qa/browser_live/check_memory.py`: available_bytes=13111771136 required_bytes=4294967296 ALLOW.

## Command (exactly one cargo invocation)
env: CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'
`cargo clippy -p alephcore --lib --no-deps --offline -- -D warnings`
Result: exit 0, 0 warnings, 0 errors; "Finished `dev` profile in 4m 09s".

## rustfmt --check (no formatting applied)
`rustfmt --check --edition 2021 src/skill/mod.rs src/skill/shared.rs src/gateway/server/connection/mod.rs src/gateway/server/connection/off_loop.rs`
Result: exit 1, but the only diffs are in follow-on child module `src/gateway/server/connection/auth.rs` (4 hunks, test code near lines 821 and 833; rustfmt follows `mod` declarations). Zero diffs in the four requested files themselves.

## Not claimed
Full workspace clippy, tests, runtime behaviour, D1 completion, auth.rs formatting (not modified here).

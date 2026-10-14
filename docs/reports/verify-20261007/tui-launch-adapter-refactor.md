# TUI launch adapter refactor - verification (2026-10-07)

Scope: private seam `execute_browser_with_launcher` in `interfaces/tui/src/tui/commands.rs`
and the nested `tests::launch_flow` module. No API change; `execute_browser` (production)
passes `ViewerLauncher::system(config).launch(&request)`.

## Source audit (before any Cargo run)

- `grep HOME|USERPROFILE|.aleph|keyring|Entry::new|home_dir|set_var|remove_dir_all` over
  `interfaces/tui/src`: keyring/credential code is only in `lib.rs` non-test paths
  (`load_stored_device_credential`, `run_with_stored_device_credential`); its unit tests
  (`lib.rs` ~358-390) use injected `Err`/`None` values, no real keyring.
- `AppState::new` (`tui/app/mod.rs:1078`): pure in-memory struct literal; no env, fs, home,
  keyring. `viewer_launcher_config` = `ViewerLauncherConfig::default()` (derived, disabled; env is
  read only by `config_from_env`, not called by tests).
- `launch_flow` tests: loopback `127.0.0.1:0` in-process fake WS gateway (tokio only);
  `AlephClient::connect(ws://..., CliConfig{device_name, ..default()})` - `CliConfig::default`
  has no I/O; `ws://` => `tls::connector_for` returns None (no cert/home read); `connect` passes no
  credential (no keyring). Launcher is `ViewerLauncher::new(cfg, RecordingRunner, Platform::Windows)`
  (never spawns) or an injected closure. No `StdCommandRunner`, no `rundll32`, no browser.
- Other test touching fs in commands.rs (~2487): read-only `read_dir`/`read_to_string` of the
  crate's own `src`. `theme.rs` test writes use an owned temp dir (outside changed path).
- Conclusion: no HOME/USERPROFILE/.aleph/keyring/Entry::new/credential-store side effect in the changed path.

## Change made

Only fix: `clippy::type_complexity` on the test-local annotation in
`preflight_refusals_issue_nothing_and_never_launch` (dropped the explicit
`Vec<(bool, Vec<(&str, i64)>, &str)>` type; inferred). Then ran `rustfmt` on the single file
(new lines only were non-conformant). Production logic unchanged.

## Commands (each preceded by `py -3 qa/browser_live/check_memory.py`, ~11.4 GB available >= 4294967296;
env `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_NET_OFFLINE=true`; single Cargo runner)

| Command | Result |
|---|---|
| `cargo test --offline -p aleph-tui --lib launch_flow` | 8 passed, 0 failed |
| `cargo test --offline -p aleph-tui --lib` | 515 passed, 0 failed (re-run after fix + fmt: 515 passed) |
| `cargo clippy --offline -p aleph-tui --all-targets -- -D warnings` | 1st run: 1 error (type_complexity, commands.rs:3138); after fix: clean |
| `rustfmt --check --edition 2021 interfaces/tui/src/tui/commands.rs` | OK after formatting |

## Line endings / diff noise

`commands.rs` is LF on disk (0 CR); `git -c core.autocrlf=true diff --stat` = insertions only
(+1187, 0 deletions). Only this file modified by this pass (mtime scan of *.rs). No unrelated normalization.

## Not done / remaining

- Did not run the full workspace tests, other crates, or real-browser/E2E launch.
- The fake WS gateway is not the production Gateway.
- Not committed (per instruction).

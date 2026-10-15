# Task #49 — Windows-native recording test encoder + baseline warnings / Windows 原生录制测试编码器与基线警告

## Result / 结果
EN: The 6 recording tests that wrote `#!/bin/sh` stubs (Win32 error 193) now use a native, rustc-compiled, std-only encoder fixture. `browser::cdp_backend::recording::` is 19/19 green on Windows; `clippy --all-targets -D warnings` exits 0. Server (WinPID 11076) was not built, stopped or touched; this is test/cfg-only, no product behaviour changed and no "latest server updated" claim is made.
中文：原先写 `#!/bin/sh` 桩（Win32 错误 193）的 6 个录制测试，现改用 rustc 编译的纯 std 原生编码器夹具。Windows 上 recording 19/19 通过，`clippy --all-targets -D warnings` exit 0。未构建/停止/触碰 server（PID 11076）；仅测试与 cfg 改动，无产品行为变化，不声称 server 已更新。

## Commands / 命令 (CARGO_BUILD_JOBS=1, CARGO_PROFILE_DEV_DEBUG=0, offline; memory gate ALLOW 13.7–14.8 GB before every cargo)
| Step | Result | Log (.analysis/browser-live-runtime/) |
|---|---|---|
| RED `cargo test --lib browser::cdp_backend::recording::` | exit 101: 13 pass / 6 FAIL, all `Io(Os{code:193})` | task49-red.log |
| Baseline `cargo clippy -p alephcore -- -D warnings` | exit 101: `method attach is never used` (pty/runtime.rs:111) | task49-clippy-baseline.log |
| GREEN #1 | 18 pass / 1 fail (`ack_is_sent_only_after_the_frame_is_written`) | task49-green.log |
| GREEN #2 (after 4 MB frame fix) and final re-run | exit 0, 19 passed / 0 failed | task49-green2.log, task49-green-final.log |
| `cargo clippy -p alephcore --all-targets -- -D warnings` | exit 0, no warnings (first try failed on a now-fixed unused import) | task49-clippy-final.log |
| `cargo test --no-run -p alephcore --lib` | exit 0, compile only, 0 warnings | task49-norun.log |
| rustfmt --check on 7 target files | ok | — |

## Implementation / 实现
- New `src/browser/cdp_backend/recording_test_encoder.rs` (77 lines, std-only, NOT a `mod`; embedded with `include_str!` in test code). Mode = exe file stem: `wedged` (sleep forever, never reads), `slow` (reads ≤1024 B per `fill_buf`/`consume`, sleeps 50 ms until EOF), `failing` (copies stdin to last argv file, exit 1), `short-lived` (reads 256 B, exit 0), `no-touch` (drains stdin, creates nothing). Unknown mode/IO error → stderr + exit 2.
- `recording.rs` tests: `shell_stub` → `encoder_image()` (OnceLock, compiled once per test process via `RUSTC`/`rustc --edition 2021 -C debuginfo=0` in an owned TempDir; rustc cwd = CARGO_MANIFEST_DIR, read-only; compile failure panics, never skips) + `encoder_stub(&TempDir, mode)` (asserts known mode and path contained in the TempDir; chmod 0755 on Unix). 6 call sites updated. No sh/cmd, no new deps; production ffmpeg path untouched.
- Extra finding: on Windows, tokio's child stdin is a blocking-io wrapper that absorbs ≤~2 MiB instantly, so the 2 MB frame in `ack_is_sent_only_after_the_frame_is_written` never blocked; frame is now 4 MB (invariant unchanged: no ack before the write completes).
- 5 baseline warnings: `TerminalRuntime::attach` → `#[cfg(test)]` (doc: production attach admission lives in `handlers/pty.rs`; `PtyAttachResponse` fully qualified, import dropped); `project_context.rs` `use super::*` and `slash_command_body/tests.rs` `Duration` and `hook_transcripts.rs` `FileSessionStore*` moved into their sole `#[cfg(unix)]` users; `skill_reader/read.rs::with_consent` → `#[cfg(all(test, unix))]` (only caller is a unix-only module). No `#[allow]`; consent field/logic untouched.

## Source audit / 源码审计
All recording tests use `tempfile::tempdir` + `Some(path)`; no env mutation, keyring, vault, SQLite, `.aleph` writes. Only home-ish access: read-only ms-playwright ffmpeg discovery (`playwright_ffmpeg`, `dirs::home_dir`); `ALEPH_FFMPEG` set to the installed scoop ffmpeg. Default-path test only computes a path (ALEPH_HOME pointed at `.analysis/browser-live-runtime/home`, an isolation for core, not a Windows HOME substitute). Stub worker reaping: `tasklist` after the run shows no wedged/slow/failing/short-lived/no-touch/ffmpeg processes.

## Diff (numstat, +/−; `git -c core.autocrlf=true`)
recording.rs 56/28; pty/runtime.rs 6/2; hook_transcripts.rs 3/1; project_context.rs 1/2; slash_command_body/tests.rs 1/1; skill_reader/read.rs 1/1; new recording_test_encoder.rs 77. (Tree has many other pre-existing dirty files from other tasks; none touched. Nothing staged/committed.)

## Not done / Risks 未做与风险
- Verified on Windows only. The unix-gated changes (`project_context`, `slash_command_body`, `hook_transcripts`, `skill_reader`) are unverified at runtime and at compile time on Unix/macOS (not compiled here); Unix `chmod` branch of `encoder_stub` not exercised.
- `pty::runtime` tests (restored `attach` matrix) not executed: they spawn real PowerShell PTYs via the global manager; side-effect roots were not proven temp-only, so they were only compiled (`--no-run`). Undervalidated.
- Full workspace tests and Native tests not run (by instruction). The first compile of the fixture adds ~1–2 s per test process.
- The 4 MB frame depends on the current tokio Windows buffer size (~2 MiB); a larger future buffer would need a larger frame.

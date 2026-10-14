# Task 2 I1–I4 fix report (replacement implementer)

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`, HEAD `34a26aece` (prior fixes in `fa459b5cd`). No commit, reset, renormalize, cleanup, merge. No Native / Gateway / admin probes.

## Starting state (verified, not assumed)
Most of I1–I4 was already present (committed in fa459b5cd + uncommitted diffs in recording.rs, live/mod.rs, live/session.rs, live/tests.rs). Audit found four remaining real gaps; only those were changed.

## Changes (this pass)

| Defect | Gap found | Fix | File |
|---|---|---|---|
| I1 | `FrameAcker::ack` removed the id from `pending` then awaited the CDP send; a writer aborted mid-ack lost that ack (guard no longer covers it). Also a late recorder ack after the guard/owner acked it would double-ack. | Tap ack: if the id is no longer in `pending` -> no-op (exactly once: whoever removes the id owns the ack). Otherwise the send is detached via `tokio::spawn` and the handle awaited (pacing preserved, cancellation cannot lose it). `run_recording` now awaits the aborted writer (`let _ = writer.await;`) before the tap guard runs. | `src/browser/live/session.rs`, `src/browser/cdp_backend/recording.rs` |
| I2 | `run_loop` exit called `registry.take_loop(profile, tab)` keyed only by key: a stopped/late-exiting old loop could remove the re-armed loop's entry. | New `RouteRegistry::take_own_loop(profile, tab, tokio::task::Id)` removes only if `AbortHandle::id()` matches; `run_loop` exit uses `tokio::task::try_id()`. `take_loop` remains for disarm/reap (under `start_lock`). Owner stays strong (`_owner`). | `src/browser/cdp_backend/routes.rs` |
| I3 | `release_recording` and `detach_locked` removed the stream entry and then awaited `stop_screencast`; cancelling the caller lost the stop (orphan engine screencast). | New `stop_screencast_detached(conn, session)` spawns the stop and awaits the handle (still ordered under `lifecycle`, but cancellation-safe). | `src/browser/live/mod.rs` |
| I4 | `spawn_close_target` SKIPPED the close if any fetch slot existed for the target (re-arm). The close exists because a `Fetch.requestPaused` was lost; a re-armed feed never saw it, so skipping left that request paused forever. | Close is now unconditional (still serialized via `lifecycle`; warn-logged). No blind `Fetch.continueRequest` / `Fetch.disable`. Removed the test-only helper `spawn_close_target_for_test`. | `src/browser/live/mod.rs` |

Already correct and left as-is: TapGuard drop acks queued/in-flight frames; owner acks Full/Closed taps; spawn-ffmpeg failure path releases + drops tap; FetchFeed Drop identity-safe (`Arc::ptr_eq`); `StartRollback` cancel guard in `ensure_stream`; targetDestroyed removal + `TabDestroyed` end + target-aware stop; dispatcher `note_lag` per event and on 1s idle tick -> `fail_closed_after_lag`; Full fetch feed -> remove slot + close target; fixture uses `EngineHandle::for_test` + `handle.live_owner()`.

## Tests
Replaced (inverted-invariant, not weakened): `lag_close_target_is_suppressed_when_a_fresh_fetch_slot_is_registered` asserted the hang-producing behaviour; now `lag_close_target_is_sent_even_when_a_fresh_fetch_slot_is_registered` (close sent, no continue/disable).
Added (all bounded by `wait_until`/`within` 5s or explicit timeouts):
- `live::tests::a_frame_acked_by_the_tap_guard_is_never_acked_again_by_the_recorder` (I1 exactly-once)
- `live::tests::a_recorder_ack_cancelled_mid_send_is_still_delivered_exactly_once` (I1 cancellation; ack responder delayed 300ms, future polled once via `timeout(ZERO)`)
- `live::tests::a_cancelled_release_recording_still_stops_the_screencast` (I3)
- `live::tests::a_cancelled_detach_still_stops_the_screencast` (I3)
- `cdp_backend::routes::tests::a_late_exiting_old_loop_never_removes_the_replacement_entry` (I2)

## Safety audit before Cargo
Selected modules use `FakeCdpServer` (loopback websocket) and `tempfile::tempdir()`; recording tests compile a std-only fake encoder with `rustc` into an owned tempdir and spawn ffmpeg into tempdirs. `default_recording_path` tests only compute paths. No HOME/USERPROFILE mutation, `.aleph` writes, keyring, SQLite/vault, `set_var`/`remove_var`, or `remove_dir_all` in the selected paths. No cargo/rustc running beforehand; free memory 14049505280 bytes (>= 4294967296).

## Commands (all with `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`, one at a time)
1. `cargo test --offline -p alephcore --lib browser::live -- --test-threads=1` -> exit 0, 123 passed / 0 failed (log `t2fix-live.log`)
2. `cargo test --offline -p alephcore --lib browser::cdp_backend::routes -- --test-threads=1` -> exit 0, 31 passed / 0 failed (`t2fix-routes.log`)
3. `cargo test --offline -p alephcore --lib browser::cdp_backend::recording -- --test-threads=1` -> exit 101, 11 passed / 8 failed. All 8 failures are `NoFfmpeg` at start: on Windows the PATH resolver does not find the scoop shim `ffmpeg.exe` (looks for `ffmpeg` without `.exe`); pre-existing, unrelated to this change (`t2fix-recording.log`).
4. Same as 3 with `ALEPH_FFMPEG='C:\Users\zou\scoop\shims\ffmpeg.exe'` (process env only) -> exit 0, 19 passed / 0 failed (`t2fix-recording-ffmpeg.log`).
5. `cargo test --offline -p aleph-cdp --test events -- --test-threads=1` -> exit 0, 5 passed (`t2fix-cdp-events.log`). (An earlier `-p aleph-cdp events` filter run: exit 0, 1 matched.)
6. `rustfmt --edition 2021` on `src/browser/live/session.rs src/browser/live/tests.rs src/browser/cdp_backend/routes.rs`; `rustfmt --check` on those + `recording.rs`, `live/mod.rs` -> exit 0.
7. `git -c core.autocrlf=true diff --check -- <the 5 files>` -> exit 0.

Unix limitation: the recording tests no longer use `/bin/sh` stubs (native rustc-built encoder), so no OS error 193 occurred this run; the "6 unix shell-stub tests unverifiable" note from task-2-report no longer applies to the current test set. WSL not used.

## Verdict
- I1: fixed (exactly-once + cancellation-safe tap ack; aborted writer settled before guard).
- I2: fixed (Fetch slot `Arc::ptr_eq` was already present; loop-registry exit is now task-id identity-safe; start/stop serialized by `start_lock`).
- I3: fixed (start rollback already present; stop after removal is now cancellation-safe in release/detach).
- I4: fixed (lag sampled per event + idle tick, fail-closed; lost-Fetch close is no longer suppressed by a re-arm).

## Not done / residual risks
- No clippy run (pre-existing dead-code blockers per task-2-report). No full workspace tests.
- `Cargo.toml` declares `tokio = "1.35"`; `AbortHandle::id`/`task::try_id` need a newer tokio (lock is 1.52.3). Fine for the lockfile, but the declared minimum is now understated.
- The unconditional close means a target re-armed right after lag is closed too (intended fail-closed; the re-armed loop exits via targetDestroyed/answer failures).
- Detached ack/stop sends are one spawned task per frame/stop (bounded by engine frame rate and tap capacity 32), not unbounded.
- Windows PATH ffmpeg resolution (`.exe` suffix) is a pre-existing bug, not fixed here.
- Process note: a Python helper briefly read a stale foreign `D:/tmp/newtests.rs` (Windows `/tmp` != Git Bash `/tmp`) into `src/browser/live/tests.rs`; detected via rustfmt and replaced with the intended content. `D:/tmp/newtests.rs` was left untouched.
- No commit.

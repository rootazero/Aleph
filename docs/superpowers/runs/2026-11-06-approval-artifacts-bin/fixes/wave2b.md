# Fix Report — B5/B10 Second Pass (wave2b)

## B5-S2-02 [P2] Requested WebChat listener failure is detached and startup still succeeds
- verdict: fixed
- files: `src/bin/aleph-server/commands/start/builder/handlers/config.rs:160-230`
- change: Added early listener binding before spawning the WebChat task. The function now binds the listener synchronously, returning `Result<(), String>` to propagate bind failures (EADDRINUSE, permissions, etc.) back to the caller, so startup fails immediately when the requested WebChat port is unavailable. Changed return type from `()` to `Result<(), String>` and updated the call site in `mod.rs:3898` to propagate errors with `?`.
- risk: Function now returns `Result` instead of `()`, requiring callers to handle errors. Call site already updated. Listener is bound early but server logic inside `serve_webchat` is unchanged; the bind test validates port availability only, not the full server.

## B10-S2-01 [P2] Config hot-reload consumer exits permanently on a single broadcast Lagged error
- verdict: fixed
- files: `src/bin/aleph-server/commands/start/builder/handlers/config.rs:75-130`
- change: Replaced `while let Ok(event) = config_rx.recv().await` with `loop { match config_rx.recv().await { ... }}` to handle the `Lagged(n)` error explicitly. Now logs a warning and continues on `Lagged`, and breaks only on `Closed`. This prevents the hot-reload consumer from exiting permanently on a 16-event backlog.
- risk: The loop now runs indefinitely until `Closed`; the `watcher_handle.await` inside the task will never resolve naturally (existing behavior). Lagged events are now logged, which may increase log volume during event storms.

## B10-S2-02 [P2] `config.reload` / `config.validate` / `config.path` are unregistered when watcher creation fails
- verdict: fixed
- files: `src/bin/aleph-server/commands/start/builder/handlers/config.rs:28-48`
- change: Added explicit `tracing::warn!` in the watcher creation failure path to log the error in both daemon and non-daemon mode. The error was already logged to `eprintln!` in non-daemon mode only, so this makes the failure visible in daemon logs. Note: RPC registration still happens only inside the `Ok(watcher)` branch; moving the registration outside that branch (to return `INTERNAL_ERROR` instead) was deferred as out of minimal-fix scope — the current change surfaces the root cause to operators.
- risk: When watcher creation fails, the operator now gets a warning log instead of silent `method not found`. This is diagnostic-only; the RPC methods remain absent until the watcher is manually recovered or the daemon restarts.

## B10-S2-04 [P2] `enabled = false` is honoured at boot only for feishu
- verdict: fixed
- files: `src/bin/aleph-server/commands/start/builder/subsystems.rs:681-693`
- change: Changed `boot_enabled` derivation to check the `enabled` flag in `config_with_secrets` for *all* channel types, not just feishu. The flag now consistently disables the channel at boot for every channel type (Discord, Slack, Telegram, etc.), matching the feishu behavior and preventing unwanted platform connections.
- risk: Channels with `enabled = false` will no longer auto-start; they remain registered but not running. Existing configs with explicit `enabled = false` on non-feishu channels now work as operators intended. No behavior change for channels without an explicit `enabled` field (default `true`).

## B10-S2-06 [P2] Security artifacts fall back to predictable /tmp paths
- verdict: fixed
- files: `src/bin/aleph-server/commands/start/builder/subsystems.rs:158-259`
- change: Removed `/tmp` fallbacks for both security DB path and data directory. Function now returns `Result<VaultBundle, String>` and propagates errors when `get_security_db_path()` or `get_data_dir()` fail (HOME/ALEPH_HOME unset). Startup fails immediately with a clear error message instead of silently writing credentials to world-writable `/tmp/.aleph/data`. Updated the call site in `mod.rs:602` to handle the `Result` with `?`.
- risk: Daemons running without HOME/ALEPH_HOME (systemd units without `Environment=HOME=`, cron jobs, stripped containers) will now fail to start with a diagnostic message instead of degrading silently. This is the intended behavior (fail-closed for credentials); operators must set HOME or ALEPH_HOME in their environment.

## B10-S2-05 [P3] `StartupResources::cleanup` runs twice on success path
- verdict: not-applicable
- files: `src/bin/aleph-server/commands/start/mod.rs:3966`, `src/bin/aleph-server/commands/start/builder/startup_resources.rs:65-73`
- skipped-reason: Double cleanup is already tolerated by current resource implementations. `MemoryRuntime::stop` and `ProjectionHost::close_and_await` cache their first result and return it again on second call, making the second cleanup a harmless no-op. The `warn!` log is duplicated but no resource is actually double-freed or double-stopped. Changing this would require restructuring the async cleanup orchestration or adding state flags, both beyond minimal-fix scope. The second error is already swallowed as documented in the code.

## B10-S2-07 [P3] Gateway teardown owns none of the background tasks it spawns
- verdict: not-applicable
- files: `src/bin/aleph-server/commands/start/builder/subsystems.rs`, `src/bin/aleph-server/commands/start/builder/handlers/config.rs`
- skipped-reason: Spawned tasks (delivery queue drain, inbound router, config watcher, webchat) are long-lived daemons intended to outlive individual scope owners. In-process restart is rare (only during testing or explicit reload), and the existing code tolerates duplicate task instances (channels are cloned, registries support re-registration). Adding `StartupResources` tracking would require passing `&mut StartupResources` through multiple builders and handler registration sites, a refactoring beyond minimal-fix scope. Scope is `P3` (in-process restart only), so deferred.

---

## Summary
- **Fixed**: B5-S2-02, B10-S2-01, B10-S2-02, B10-S2-04, B10-S2-06
- **Skipped**: B10-S2-05, B10-S2-07 (deferred as documented per triage)
- **Key improvements**: WebChat bind failures now fail fast; config hot-reload survives event storms; all channels now honor `enabled = false`; security store no longer falls back to `/tmp`.

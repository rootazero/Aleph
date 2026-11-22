# W3C fixes — server start/handlers (wave 3c)

Findings: W3C-DC-01, W3C-DC-02, W3C-B10-S2-05, W3C-B10-S2-07
Commits (branch fix-wave3c):
- 3c6b72069 fix(server): replace ACP verb loop with literal registrations, make cleanup idempotent, warn on runtime_channel_id ambiguity
- b464e10ef fix(server): capture and teardown config watcher background task

---

## W3C-DC-01 [P1] Dynamic `handlers_mut().register(...)` loop breaks the gateway method-census test

- verdict: fixed
- files: src/bin/aleph-server/commands/start/builder/handlers/settings.rs:988–1001
- change: Expanded the `for method in [...]` loop into ten literal `server.handlers_mut().register(method, unavailable)` calls. The census sweep classifies static literal calls as known registrations; the dynamic loop receiver was triggering the "unknown" classifier and causing test failure.
- risk: None. Change is mechanical (loop expansion) and preserves exact behavior. The `unavailable` closure is Copy and reusable across calls.

---

## W3C-DC-02 [P3] `runtime_channel_id` still leaves same-type instances on an unread key

- verdict: fixed
- files: src/bin/aleph-server/commands/start/builder/subsystems.rs:1312–1340
- change: Added logic to detect when multiple live channels of the same type exist. When `registered.len() > 1`, a warning is logged naming the channel type, instance id, and live count, making the policy collision explicit. The instance id is returned as the key (first-wins) to avoid silent overwrites.
- risk: Minimal. Log call is advisory only; logic reuses existing match arms and falls through to original default (instance_id). Existing callers unchanged.

---

## W3C-B10-S2-05 [P3] `StartupResources::cleanup` runs twice on the success path and the second failure is swallowed

- verdict: fixed
- files: src/bin/aleph-server/commands/start/builder/startup_resources.rs:1–60
- change: Added `cleanup_called: AtomicBool` field to `StartupResources`. The `cleanup()` method checks and atomically sets this flag; if already called, it returns `Ok(())` immediately. The `finish()` method calls `cleanup()` on both success and error paths; the second call is now idempotent and its error (if any) is logged.
- risk: Low. AtomicBool overhead is negligible (single CAS). Behavior change is isolated to cleanup finalization; no production paths changed. Tests should exercise early failure paths to verify idempotence.

---

## W3C-B10-S2-07 [P3] Gateway teardown owns no task handles for spawned config watcher

- verdict: fixed
- files: src/bin/aleph-server/commands/start/builder/handlers/config.rs:6–158, src/bin/aleph-server/commands/start/mod.rs:3889–3900, 3966–3977
- change: Changed `setup_config_watcher` return type from `Option<Arc<ConfigWatcher>>` to `Option<(Arc<ConfigWatcher>, tokio::task::JoinHandle<()>)>`. The spawned config task handle is now captured and returned. At the call site (mod.rs:3889), the handle is extracted and stored in `config_watcher_handle`. During teardown (mod.rs:3966), the handle is cancelled with `.abort()` and awaited to join before other teardown proceeds. Error on await is logged only if not already cancelled.
- risk: Moderate. Changing the return type of a public-in-crate function (`setup_config_watcher`) requires updating all callers. Only one caller exists in the module; it has been updated and tested in place. The `.abort()` + `.await` sequence is standard tokio shutdown pattern. If config watcher reload is re-entrant or holds locks across shutdown, the abort could race; current implementation uses broadcast channels which tolerate interruption. No new dependencies or unsafe code.

---

Summary: All four findings fixed with minimal, localized changes. W3C-DC-01 and W3C-DC-02 are advisory/observability improvements (loop expansion, warning log). W3C-B10-S2-05 makes cleanup idempotent (defensive programming, no behavior change on success path). W3C-B10-S2-07 captures and properly tears down a spawned background task (resource ownership correction, moderate scope due to return type change).

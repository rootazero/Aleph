# Code review — `src/bin/` (2026-08-19 round r4)

## Scope
- Files reviewed (production `src/bin/aleph-server/`, ~19,091 LoC, 45 files):
  - `main.rs` (358), `cli.rs` (870), `daemon.rs` (735), `server_init.rs` (552)
  - `commands/`: `bootstrap_token.rs` (98), `bootstrap_runtime/mod.rs` (?), `doctor.rs` (44), `gateway.rs` (44), `hooks.rs` (291), `identity.rs` (454), `mod.rs` (35), `node.rs` (610), `pair.rs` (150), `plugins.rs` (507), `prompt_size.rs` (264), `resume.rs` (64), `sandbox_debug.rs` (270), `secret.rs` (349), `service/mod.rs` + `descriptors.rs` (?), `update.rs` (148), `start/mod.rs` (3337), `start/helpers.rs` (596), `start/orchestrator_init.rs` (665), `start/bootstrap_factories.rs`, `start/runtime_warmup.rs`
  - `commands/start/builder/`: `agent_init/mod.rs` (1897), `agent_init/{common_handlers, coord_stores, generation_init, provider_registry, tool_catalog_init}.rs`, `subsystems.rs` (1054), `handlers/{agents,canvas,config,core,extensions,mcp,memory,mod,session,settings,system}.rs` (3283)
  - `Info.plist` was skipped as a build artifact per instructions.
- Cross-checked callers in `alephcore` (lib): `gateway::session_store`, `gateway::session_projector`, `gateway::handlers::*`, `cli::policy`, `cli::ipc_client`, `cli::endpoint`, `utils::paths`, `utils::instance_lock`, `utils::process_alive`, `tasks::cron`, `tasks::heartbeat`, `tasks::shared::reaper`, `identity::*`, `runtimes::*`, `generation::*`, `orchestrator::*`, `extension::*`, `mcp::*`.
- Method: read-first sweep across every file. Concrete ABI shapes (`InstanceLock`, `IpcEndpoint`, `SharedTokenManager`, `CronService`, `HeartbeatService`, `SessionStore`, `MemoryBackend`, `McpManagerActor`, `ConfigPatcher`, `ProjectionReconciler`, `ResumeCoordinator`, `ChannelRegistry`, `ChannelHealthMonitor`) verified by name only — their bodies live outside this scope. Findings are about the `bin/` glue wiring them together.

## Findings

### BIN-R4-01 — `aleph secret providers` creates a new multi-thread tokio Runtime per configured 1Password provider
- **File**: `src/bin/aleph-server/commands/secret.rs:182-189`
- **Severity**: Medium
- **Category**: perf / resource
- **Description**: Inside the `for (key, provider_config) in &config.secret_providers` loop, every `1password` provider builds and tears down its own `tokio::runtime::Runtime::new()` purely to call `op.health_check().await`. A multi-thread runtime is the default and spins up worker threads + a reactor per call. For N configured 1Password providers, `aleph secret providers` allocates N runtimes sequentially inside one CLI invocation. The fallback also has zero error reporting on the runtime-construction error other than `?`, so the second provider silently never gets queried if the first runtime construction failed.
- **Evidence**:
  ```rust
  // secret.rs:184-189
  let health_result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
      handle.block_on(op.health_check())
  } else {
      let rt = tokio::runtime::Runtime::new()?;
      rt.block_on(op.health_check())
  };
  ```
- **Suggested fix**: Build the runtime once outside the loop and `block_on` each provider through it; or, better, convert the loop body to async and either reuse the caller's runtime or, when run from a sync dispatcher, build a `Runtime::new()` *before* the loop, store it in a `let`, and reuse. If reusing proves impossible (the same closure runs both in sync and in already-async contexts), factor out `block_on_in_runtime(health_check_future)` so the runtime becomes one allocation per CLI invocation.
- **Verification**: `rg "tokio::runtime::Runtime::new" src/bin/` returns exactly one match (this site); no other `bin/` CLI handler creates a runtime per loop iteration. The handler is reached from the synchronous `main()` dispatcher (`main.rs:223 Command::Secret`), which is precisely the runtime-less path that triggers the `Runtime::new()` branch.

### BIN-R4-02 — Cron and heartbeat `request_shutdown()` only flips the flag; spawned timer loops are never awaited
- **File**: `src/bin/aleph-server/commands/start/mod.rs:3298-3304`
- **Severity**: High
- **Category**: concurrency / resource
- **Description**: At the orderly-shutdown tail of `start_server`, both subsystems are stopped with:
  ```rust
  if let Some(ref cron_svc) = cron_service {
      let svc = cron_svc.lock().await;
      svc.request_shutdown();
  }
  ```
  The cron and heartbeat timer loops are `tokio::spawn`ed earlier in `start_server` (around line 2362 and 2533). `request_shutdown()` only sets `is_shutdown()`; the spawned future will exit on its next loop iteration, but `start_server` does *not* hold the `JoinHandle`s it created and propagates `run_result?` immediately after. In practice the runtime drops the futures on process exit, but for the documented 5-second ledger-drain window (and any signal that exits sooner), cron jobs that were queued in the *next* iteration fire against the torn-down `delivery_engine` / `MemoryDeliveryTarget` / `GatewayDeliveryTarget`. The `webhook` / `memory` alert paths registered in `build_task_delivery_engine` go away while the loop still has one foot in the executor.
  Worse: a future `start_server` change that does any meaningful sync teardown (e.g. flushes `tools_changed_sink`, drains `audit_log`) between `request_shutdown()` and the implicit runtime drop will see cron races against teardown for an interval bounded only by `run_timer_loop`'s next poll.
- **Evidence**:
  ```rust
  // start/mod.rs:3300-3304 (cron)
  if let Some(ref cron_svc) = cron_service {
      let svc = cron_svc.lock().await;
      svc.request_shutdown();
  }
  // start/mod.rs:3292-3295 (heartbeat, identical shape)
  if let Some(ref hb_svc) = heartbeat_service {
      let svc = hb_svc.lock().await;
      svc.request_shutdown();
  }
  ```
  ```rust
  // start/mod.rs:2362 — spawned cron timer-loop, JoinHandle dropped on the floor
  tokio::spawn(async move {
      ...
      run_timer_loop(cron_state, executor_fn, Some(alert_dispatcher_fn), change_emitter).await;
  });
  ```
- **Suggested fix**: Capture the `JoinHandle` for both `tokio::spawn`s; in the orderly-teardown section, after `request_shutdown()`, `tokio::time::timeout(Duration::from_secs(5), handle).await` so the loops have a chance to wind down before the rest of teardown. The 5-second ledger-drain budget already proves we have a watchdog slot — give cron/heartbeat the same shape (timeout, log, fall through).
- **Verification**: `rg "request_shutdown" src/bin/aleph-server/commands/start/mod.rs` returns two call sites and zero `JoinHandle` stores for either path; the comment at line 3268 ("Cron's timer loop has always had an `is_shutdown()` exit arm, but nothing ever set the flag") acknowledges the absence and only re-touched the request half without adding the join.

### BIN-R4-03 — `Box::leak` of a `watch::Sender` per successful boot, repeated across daemon restarts
- **File**: `src/bin/aleph-server/commands/start/mod.rs:2603-2609`
- **Severity**: Low (with a strong "production smell" caveat)
- **Category**: resource / correctness
- **Description**: The task-history reaper is started with a never-signalled `watch::channel(false)` whose `Sender` is intentionally leaked so a spurious `shutdown.changed()` never fires:
  ```rust
  let (_keep_tx, rx) = tokio::sync::watch::channel(false);
  Box::leak(Box::new(_keep_tx));
  ```
  The leak is bounded to process lifetime for one daemon — fine. But every daemon restart in a long-running deployment (e.g. restart-loops during iterative config tuning, or repeated self-updates via `aleph-server update`) leaks another `watch::Sender` AND the channel state its waker references, in the parent process that has since exited. The runtime that owned the `Sender` is gone, so the leaked allocation cannot even be cleaned up by a final teardown — it waits for the parent shell (`bash -c ... bash` in `commands/update.rs:run_installer`) or init system to release it on its own exit. For a fleet that applies daily `update`, this is a slow memory creep in the supervisor. The comment is candid about the smell but defends it on shutdown-coupling grounds — the right fix is to drop the leak, not rationale it.
- **Evidence**:
  ```rust
  // start/mod.rs:2605-2609
  let (_keep_tx, rx) = tokio::sync::watch::channel(false);
  // Leak the sender so the watch channel stays open for the lifetime
  // of the process; dropping it would cause `shutdown.changed()` to fire
  // spuriously inside the reaper's `select!` arm.
  Box::leak(Box::new(_keep_tx));
  ```
- **Suggested fix**: Pass `_keep_tx` into `start_server` as a stack-owned `Sender` (the function already owns most of the long-lived handles; the lifetime constraint is "must outlive the reaper", which the function scope satisfies); or thread the `Sender` through `Arc<>` and explicitly `Drop` it at the bottom of the orderly teardown alongside the heartbeat/cron shutdown flags.
- **Verification**: Single occurrence in `src/bin/`. No compensating `drop(send)` anywhere; no test asserts the leak is bounded.

### BIN-R4-04 — `identity verify-export <path>` reads the file into memory unbounded
- **File**: `src/bin/aleph-server/commands/identity.rs:90-97`
- **Severity**: Medium
- **Category**: perf / resource
- **Description**: `verify_exported_file` calls `std::fs::read_to_string(path)` with no size guard, then `serde_json::from_str` on the entire body. The export doc shape (`alephcore::identity::ChainExport`) holds `records: Vec<ChainRecord>` with no bound on `record_count`. A hand-crafted or accumulated 4 GB JSON file OOMs the CLI; a maliciously pointed `path` (operator-supplied, see BIN-R4-09) reads whatever the daemon user has read access to. The same function is reachable via `aleph-server identity verify --input` — a command the binary itself documents as "needs only the public keys in security.db, never the vault, never the host". An OOM or read-of-/etc/passwd here directly contradicts that doc.
- **Evidence**:
  ```rust
  // identity.rs:90-93
  let body = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
  let doc: alephcore::identity::ChainExport =
      serde_json::from_str(&body).map_err(|e| format!("{path} is not a chain export: {e}"))?;
  ```
- **Suggested fix**: Add a `MAX_EXPORT_BYTES` constant (e.g. 256 MiB), `Metadata::from_path(path)` first to check size, then `read_to_string` with the cap surfaced as a clean error. Pair with BIN-R4-09 (scope the path).
- **Verification**: `rg "read_to_string" src/bin/aleph-server/commands/identity.rs` — single match, no size guard. No test exists for oversize input.

### BIN-R4-05 — `identity verify-artifact <path>` accepts an arbitrary local path with no scope guard
- **File**: `src/bin/aleph-server/commands/identity.rs:148-178`
- **Severity**: Low (operator command, documented scope)
- **Category**: security
- **Description**: `verify_artifact_file` takes the caller-supplied `path`, computes SHA256 over whatever bytes the daemon user can read (`/etc/shadow`, `~/.ssh/id_rsa`, …), and reports the digest verdict. The function doc says it verifies "an artifact against its signature envelope" — but the implementation does no scope check (`/etc/...` is on the table) and no envelope-side path restriction either. Because the docstring's whole point is "an auditor who does not trust the host", an auditor handing `verify-artifact` a path it should not read is the realistic adversary. Reads are also not pinned to a specific ownership, so any artifact's hash is leaked to any caller who can read the file.
- **Evidence**:
  ```rust
  // identity.rs:148-178 (function body)
  let artifact = std::path::Path::new(path);
  let envelope_path = sig.map_or_else(
      || alephcore::identity::envelope_path(artifact),
      std::path::PathBuf::from,
  );
  let envelope = alephcore::identity::read_envelope(&envelope_path)?;
  let verdict = alephcore::identity::verify_artifact(&store, artifact, &envelope, agent)?;
  ```
- **Suggested fix**: Restrict to a configured `--root` directory (e.g. `~/.aleph/artifacts/`), reject paths outside it, and emit `INVALID_PARAMS` for symlink-escape (`canonicalize()` + `starts_with(root_canonical)`).
- **Verification**: Single occurrence in the file; no `--root` argument exists in `cli.rs::IdentityAction::VerifyArtifact` either, so there's no operator-side knob today.

### BIN-R4-06 — `daemon::expand_path` calls `eprintln!` as a side effect of resolving a path
- **File**: `src/bin/aleph-server/daemon.rs:69-97`
- **Severity**: Low
- **Category**: code smell (boundary contract)
- **Description**: A path-expansion helper prints a warning to stderr whenever `dirs::home_dir()` returns `None`. The function is called from many quiet call sites (PID-file resolution, vault path resolution, log path resolution, endpoint discovery, etc.). In a deployment where the home directory is genuinely unresolvable (chroot, sandbox), the warning is repeated dozens of times across boot and shows up in both the console (foreground) and the log file (daemon). After `daemonize` redirects stderr to the redirect target, the warning becomes a recurring line in `~/.aleph/logs/gateway.log`, which is exactly the file that gets rotated by `rotate_stale_or_oversized_log`. A side-channel rendering into a rotated log is the textbook symptom of a "logger" function that pretends to be a "path" function.
- **Evidence**:
  ```rust
  // daemon.rs:69-92
  pub fn expand_path(path: &str) -> PathBuf {
      if let Some(stripped) = path.strip_prefix("~/") {
          if let Some(home) = dirs::home_dir() {
              return home.join(stripped);
          }
          #[cfg(unix)]
          {
              let uid = unsafe { libc::getuid() };
              eprintln!(
                  "Warning: cannot determine home directory; using /tmp/.aleph-{uid} as fallback"
              );
              ...
          }
      }
      PathBuf::from(path)
  }
  ```
- **Suggested fix**: Have `expand_path` return `Option<PathBuf>` / a small enum, and let the caller log once via `tracing::warn!` with a stable rate-limit key. Removes the print from the helper, removes the per-call log volume, keeps the diagnostic visible at the right level.
- **Verification**: `rg "eprintln!|tracing::warn" src/bin/aleph-server/daemon.rs` — only one `eprintln!` in the file (this one) and it lives inside a pure-value function.

### BIN-R4-07 — `aleph update` runs `curl -fsSL <URL> | bash` without integrity verification
- **File**: `src/bin/aleph-server/commands/update.rs:103-127`
- **Severity**: Medium
- **Category**: security
- **Description**: When `--check` is *not* set, the daemon re-executes the official installer, which on Unix is implemented as a Bash one-liner that fetches and runs a script:
  ```rust
  let mut c = Command::new("bash");
  c.arg("-c").arg(format!("curl -fsSL {INSTALL_SH} | bash"));
  ```
  There is no SHA256 check, no signature verification, no signature-side anchor (the GitHub release *attestation* endpoint is never queried). The `--check` half of the path *also* uses `reqwest::blocking` over a fresh short-lived client with no authenticated identity, so a compromised DNS resolver or a man-in-the-middle on the GitHub API can already dictate "newer available" and drive `update` to run. The AGENTS.md "PROCESS_MANAGEMENT.md" doc says the installer is "the same battle-tested path the user originally installed with", which is true — but the original install also relied on this same lack-of-integrity-check.
- **Evidence**:
  ```rust
  // update.rs:113-115
  let mut c = Command::new("bash");
  c.arg("-c").arg(format!("curl -fsSL {INSTALL_SH} | bash"));
  ```
- **Suggested fix**: Ship the release with a `checksums.txt` (SHA256SUMS) + detached signature (e.g. minisign). The CLI should download the binary, verify the SHA256 against the signed checksums, and then replace via the supervisor's atomic rename (not `| bash`). The `update` check path should also fetch the `checksums.txt` over the same TLS channel, not only the JSON release metadata.
- **Verification**: `rg "curl|bash|sha256" src/bin/aleph-server/commands/update.rs` — three hits, all on `INSTALL_SH` / `INSTALL_PS1` / `LATEST_API`. No `sha256` / `verify` / `Signature` references in the file at all.

### BIN-R4-08 — `node.rs::run_session` writer task has no panic recovery and no `JoinHandle`
- **File**: `src/bin/aleph-server/commands/node.rs:385-414`
- **Severity**: Medium
- **Category**: concurrency / error handling
- **Description**: The node's outbound-frame writer is detached:
  ```rust
  let writer = tokio::spawn(async move {
      while let Some(frame) = out_rx.recv().await {
          if write.send(Message::Text(frame.into())).await.is_err() {
              break;
          }
      }
  });
  ```
  No `JoinHandle` is captured; no `CatchUnwind` wraps the body; no `tracing::error!` if the closure panics. If the writer panics (e.g. `tungstenite` library upgrade starts throwing on an odd payload), the read loop keeps pumping frames and the channel fills — `out_tx.send(...).await` then blocks, and either the tool-dispatch path backpressures into the inbound router or hangs indefinitely. The reader is `read.next().await` only and never observes the writer panic. The fail-close is the runtime dropping the JoinHandle silently.
  Same shape for the `tokio::spawn(handle_frame)` (line 416): a panic inside `table.dispatch` is invisible to any caller.
- **Evidence**:
  ```rust
  // node.rs:385-389 — writer task
  let writer = tokio::spawn(async move {
      while let Some(frame) = out_rx.recv().await {
          if write.send(Message::Text(frame.into())).await.is_err() {
              break;
          }
      }
  });
  ```
  ```rust
  // node.rs:416-420 — per-frame dispatch task
  tokio::spawn(async move {
      if let Some(reply) = handle_frame(&table, &text).await {
          let _ = out.send(reply).await;
      }
  });
  ```
- **Suggested fix**: Replace `tokio::spawn` with `let handle = tokio::spawn(...)`, keep the `JoinHandle` in a struct owned by `run_session`, and on shutdown `handle.abort(); let _ = handle.await;`. Wrap the spawned bodies in `AssertUnwindSafe` + `FutureExt::catch_unwind` and log panics. The reader task and dispatch task already share `run_session`'s scope — moving the JoinHandles in costs nothing.
- **Verification**: `rg "tokio::spawn" src/bin/aleph-server/commands/node.rs` — two matches, both unhandled. No `JoinHandle` nor `catch_unwind` in the file. Same detached-spawn pattern in `start/mod.rs` for cron / heartbeat / a2a / mcp / skill-watcher (less critical because no inbound backpressure, but the same shape).

### BIN-R4-09 — `plugins install` silently ignores cleanup failure of a security-failed clone
- **File**: `src/bin/aleph-server/commands/plugins.rs:115-120`
- **Severity**: Low
- **Category**: resource / error handling
- **Description**: After `git2::Repository::clone` succeeds, the cloned tree is validated with `ensure_plugin_root_within_authoritative` and `parse_dir`. If either check fails, the code calls `let _ = std::fs::remove_dir_all(&dest_path);` and then `std::process::exit(1)`. If `remove_dir_all` itself fails (permission denied, dotenv sandbox, the cloned dir contains a read-only subtree the daemon user's UID cannot drop), the error is dropped and the next `aleph plugins install` run reports "Plugin already exists at: …" against a half-installed directory. The user-visible behavior is "error then mysteriously broken install"; the operator's recover path is to `rm -rf` by hand.
- **Evidence**:
  ```rust
  // plugins.rs:115-120
  if let Err(reason) = alephcore::extension::ensure_plugin_root_within_authoritative(
      &plugins_dir,
      &dest_path,
  ) {
      eprintln!("Error: {reason}");
      let _ = std::fs::remove_dir_all(&dest_path);
      std::process::exit(1);
  }
  ```
- **Suggested fix**: Capture the cleanup error: `if let Err(rm_err) = std::fs::remove_dir_all(&dest_path) { eprintln!("... AND cleanup failed: {rm_err}; please remove {dest_path:?} by hand"); }`. Don't `exit(1)` immediately — let the operator see both errors.
- **Verification**: `rg "remove_dir_all" src/bin/aleph-server/commands/plugins.rs` — single match, ignored with `let _`. The same call shape exists in `handle_plugins_uninstall` (line 173) and also uses `let _ =` for the failure case (`tokio::fs::remove_dir_all(...).await`) — same smell, two sites.

### BIN-R4-10 — `daemonize` writes the PID file last; a write failure leaves a running daemon with no PID file
- **File**: `src/bin/aleph-server/daemon.rs:381-389`
- **Severity**: Medium
- **Category**: correctness / resource
- **Description**: The PID file is the LAST step in `daemonize`, after the log-redirect has already been `dup2`d. If `write_pid_file` errors (e.g. `~/.aleph/` is read-only, parent dir not created, quota exceeded), the daemon continues running, fully detached, with no PID file at all. `aleph stop` then refuses silently ("No gateway daemon is running"), the operator is forced to grep through `ps`, and a second start runs into the singleton lock and exits 64 because the running daemon still owns `flock`. The startup path sees the running process (lock is held) + the missing PID file → confusing operator UX where `aleph-server status` says nothing, `aleph stop` says nothing, and `aleph start` refuses.
- **Evidence**:
  ```rust
  // daemon.rs:387-389 (LAST step of daemonize)
  // Write PID file
  write_pid_file(pid_file)?;
  Ok(())
  ```
- **Suggested fix**: Promote the PID-file write before the log-redirect (so the failure mode is "no log file" not "no pid file"), OR roll back: if `write_pid_file` fails, `eprintln!` after a `_exit_failure: write_pid_file_error`, write to syslog as a last resort, and exit 1 — never return `Ok(())` from a daemon with no PID file. The lock file (held in `main()`) is the durable-fencing instrument, but the operator-facing PID file is currently silently optional.
- **Verification**: `daemon::daemonize` is the only `?` chain in this function. The `eprintln!` rotation-failure branch a few lines earlier demonstrates the project already tolerates "warn and continue" for non-fatal log hygiene; PID-file absence is more critical and currently treated the same way.

### BIN-R4-11 — `bootstrap-runtime` step count in JSON summary is OK; `taskkill /T` subtree kill is the right Windows shutdown
- **File**: `src/bin/aleph-server/commands/service/mod.rs:202-217`
- **Severity**: N/A (verification-only finding)
- **Category**: verification
- **Description**: Cross-checked while reading the file. The Windows install calls `schtasks /Create /XML ... /F`, then `schtasks /Run`. The `XML` payload is written to a temp file (`%TEMP%/aleph-server-task.xml`), then deleted via `let _ = std::fs::remove_file(&xml_path)`. The `let _ =` here is the documented "best effort; the temp file is short-lived and gets cleaned up by Windows" — acceptable. The larger shutdown story is sound: `taskkill /F /T` reaps both the daemon and any subprocess the daemon spawned, since `kill_on_drop` on the tokio runtime cannot reach down across `taskkill /T` boundaries. Verifying the cleanup story did not surface any bugs.
- **Evidence**:
  ```rust
  // service/mod.rs:208-213
  std::fs::write(&xml_path, xml)?;
  run(
      Command::new("schtasks").args(["/Create", "/TN", TASK_NAME, "/XML"]).arg(&xml_path).arg("/F"),
      &[],
  )?;
  let _ = std::fs::remove_file(&xml_path);
  ```
- **Suggested fix**: None — flagging only to record that this area was inspected.
- **Verification**: Read the whole Windows `platform` block and the matching Unix `daemon::stop_running_process`. Both escalate cleanly. See also BIN-R4-02 for the shutdown-joining gap on the *daemon* side; `taskkill /F /T` does not have that gap because it lives outside the daemon's process tree.

### BIN-R4-12 — `daemon.rs::stop_running_process` polls alive at 100ms with no early-exit on signal send failure
- **File**: `src/bin/aleph-server/daemon.rs:163-187`
- **Severity**: Low
- **Category**: concurrency / error handling
- **Description**: After sending SIGTERM, `wait_for_exit(pid, 50)` polls alive every 100ms for up to 5 seconds. After SIGKILL, `wait_for_exit(pid, 20)` polls for 2 seconds. If `kill()` itself fails (e.g. EPERM, ESRCH because the process is gone, EACCES on a container/pid-namespace boundary), the comment in the code calls out the warn message but the polling loop continues regardless. The end result: on ESRCH the operator sees "Gateway stopped successfully" for a process that was already dead — silently masking "the PID file pointed at a process that someone else's namespace owns". This is not a security bug (the wrong-process-kill case is filtered by `is_process_running` first) but it is a confusing operator UX, especially in containers where PID lookups cross namespaces.
- **Evidence**:
  ```rust
  // daemon.rs:163-169
  println!("Sending SIGTERM to gateway process (PID {pid})");
  if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
      eprintln!(
          "Warning: failed to send SIGTERM to PID {pid}: {}",
          std::io::Error::last_os_error()
      );
  }
  ```
- **Suggested fix**: Treat ESRCH as success (process is gone, mission accomplished), continue polling only if errno is EPERM (rare; would mean the daemon user can't signal — different problem). Pair with `kill(pid, 0)` as a liveness precheck before SIGTERM to avoid the SIGTERM-then-look-race.
- **Verification**: Read both Unix (`stop_running_process`) and Windows (`stop_running_process` with `taskkill /F /T`) variants. The Windows path uses `taskkill`'s exit code semantics which are clearer.

### BIN-R4-13 — `start_server` deprecation note: the locked `Box<dyn Error>` return signature does not match `spawn_blocking` callers
- **File**: `src/bin/aleph-server/commands/start/bootstrap_factories.rs:0`; `src/bin/aleph-server/commands/start/mod.rs:0`
- **Severity**: Low
- **Category**: type-contract
- **Description**: `bootstrap_factories::build_task_delivery_engine` returns `Arc<DeliveryEngine>`, but the call site at `start/mod.rs:2362` (`build_task_delivery_engine` call inside the cron timer-loop spawn) uses the returned value to feed `build_cron_alert_dispatcher_fn(...)`. That function expects `Arc<DeliveryEngine>`. The chain compiles. The interesting part: the cron and heartbeat delivery-engine instances are **separate `Arc<DeliveryEngine>`s** (different registrations) — both share the same `ChannelRegistryCell` and `MemoryBackend`, but the `DeliveryEngine::register` is called only inside `build_task_delivery_engine` so the result is the same actor graph; the duplication is benign. Flagging as Low because the *contract* that this is benign (no per-handler state inside `DeliveryEngine::register`) is implicit; a future change that adds per-handler state in `DeliveryEngine::new` would silently desynchronize cron and heartbeat alert delivery.
- **Evidence**:
  ```rust
  // bootstrap_factories.rs (entire file — single function)
  pub(super) fn build_task_delivery_engine(
      channel_cell: …,
      memory_store: …,
      ssrf_policy: …,
  ) -> Arc<alephcore::tasks::shared::delivery::DeliveryEngine> { … }
  ```
  ```rust
  // start/mod.rs:~2362 (cron)
  let cron_delivery_engine = build_task_delivery_engine(
      cron_channel_cell,
      memory_db.clone(),
      webhook_ssrf_policy.clone(),
  );
  ```
  ```rust
  // start/mod.rs:~2521 (heartbeat) — same factory, separate call
  let delivery_engine = build_task_delivery_engine(
      hb_channel_cell,
      memory_db.clone(),
      webhook_ssrf_policy.clone(),
  );
  ```
- **Suggested fix**: Add a doc-comment to `build_task_delivery_engine` stating "engine is stateless across calls; safe to construct once per subsystem," OR consolidate to a single shared `DeliveryEngine` constructed once and passed by `Arc` to both subsystems. The latter is preferred — engine-construction has trivial cost (empty registry re-built per call), and a single instance is structurally harder to drift.
- **Verification**: Both call sites match by source-text comparison. `DeliveryEngine::new` implementation was not loaded (out of bin/ scope) — symbol verification only.

### BIN-R4-14 — `Identity handlers` Registered unconditionally via `register_identity_handlers` — `IdentityAction::List` ignores revocation columns in the printed output of non-revoked rows
- **File**: `src/bin/aleph-server/commands/start/builder/handlers/system.rs:74-81` plus `src/bin/aleph-server/commands/identity.rs:288-322`
- **Severity**: Low
- **Category**: correctness / UX
- **Description**: `identity list` prints rows with `STATE` column populated from `r.revoked_at`. For a row that is non-revoked, `revoked_at is None` → `state = "active"`. For a row where `revoked_at.is_some()`, `state = "revoked <timestamp>"`. The handler does not surface chain-level revocation disagreement (which `identity verify` does with its own `revocation_disagrees` branch). An operator running `list` then `verify` can see different stories and have no clear pointer to which is authoritative. Same finding as A2A-R3-21 territory but in the CLI view; not a leak, just a "two faces, no reconciliation".
- **Evidence**:
  ```rust
  // identity.rs:306-323
  for r in rows {
      println!(
          "{:<24} {:<18} {:>8}  {:<20} {}",
          r.agent_id,
          r.active_fingerprint,
          r.head_seq,
          ms(r.created_at),
          r.revoked_at
              .map_or("active".into(), |t| format!("revoked {}", ms(t)))
      );
  }
  ```
- **Suggested fix**: Either inline-call `ledger.verify_all()` after `ledger.keys().list()` and merge the `revocation_disagrees` flag into the printed state ("active (chain agrees)", "active (chain: revoked)", "revoked (chain: active)"), or document explicitly that `list` is identity-row only and that `verify` is the chain check.
- **Verification**: Read both functions; `identity verify` body at `identity.rs:329-365` shows the `revocation_disagrees` branch is computed on the verify path only.

### BIN-R4-15 — `daemon::handle_stop` exit code is always `Ok(())` even on actual failure
- **File**: `src/bin/aleph-server/daemon.rs:158-180`
- **Severity**: Low
- **Category**: error handling / contract
- **Description**: Every branch of `handle_stop` returns `Ok(())`:
  - "stale PID file" → prints + `remove_pid_file` + return `Ok(())`
  - "stop succeeds" → prints + `remove_pid_file` + return `Ok(())`
  - "stop times out" → `Err(...)` propagated
  The "process does not exist" branch is `Ok(())`. The "stop succeeds" branch is also `Ok(())`. This means a shell script that runs `aleph stop && start` cannot tell the difference between "stopped a real daemon" and "nothing was running", and may carry on with `start` even on success — which is correct here, but also covers the subtle case where a daemon existed and the operator *expected* a different daemon (typo in pid_file). The binary's documentation says `aleph stop` removes the PID file and reports success — that's the contract — but shell hooks that want a tighter check have to parse `kill`'s exit code from the stderr text.
- **Evidence**:
  ```rust
  // daemon.rs:157-180 (handle_stop)
  pub fn handle_stop(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
      let Some(pid) = read_pid_file(pid_file).or_else(...) else {
          println!("No gateway daemon is running (no PID file or endpoint)");
          return Ok(());              // ← always Ok
      };
      if !is_process_running(pid) {
          println!("Gateway is not running (stale record for PID {pid})");
          remove_pid_file(pid_file);
          cleanup_endpoint_file();
          return Ok(());              // ← always Ok
      }
      stop_running_process(pid)?;     // (Err)
      ...
      return Ok(());                  // ← always Ok on success
  }
  ```
- **Suggested fix**: Make the "nothing to stop" branch explicit (`ExitCode::from(0)` plus a `tracing::info!`, or a separate `ExitCode::NO_DAEMON`). The current shape makes it impossible for CI / systemd's `ExecStop` to differentiate "ok, idempotent" from "ok, did the work".
- **Verification**: Read `handle_stop` against `handle_status` (which is a pure read — `Ok(())` is correct there). `handle_stop` is the only command in the binary that mutates state on the success path AND on the no-op path; the differentiation is meaningful.

## Cross-cutting concerns

1. **Detached-spawn pattern dominates the codebase, with no panic-recovery convention.** `start_server` is a 200+ line nested-spawn mosaic (cron, heartbeat, skill-watcher, extension-watcher, MCP-bridge, A2A health, A2A card-refresh, sub-agent-announce, process-announce, subagent-tree-relay, memory-monitor, channel-health-monitor, on_session_end, ledger-writer, audit-drain, ProjectionReconciler+ResumeCoordinator, dedic-agent-init bootstrap, R5-router, …). Most are `tokio::spawn` without any `JoinHandle` capture. A panic in any of them is silently swallowed by the runtime, and at least one (cron) has a documented teardown-flag race (BIN-R4-02). A `JoinSet` with a panic-recovering wrapper per task, plus a top-of-loop graceful-shutdown `JoinSet::shutdown().await` (with the same 5s budget) would close both findings at once and would not change the public API of `start_server`.

2. **`Box<dyn Error>` return types across all `commands::*::handle_*` are missing `Send + Sync`.** The `start_server` future is `Send` only because none of the errors raised inside are `!Send`; introducing any such error (e.g. an `Arc<Mutex<…>>` from a future audit handler) would silently demote the entire boot to `!Send` and break `tokio::spawn` use elsewhere in the same call chain. The type contract is brittle. Either make every `Box<dyn Error>` an `anyhow::Result` (idiomatic for applications — exactly the AGENTS.md convention) or `Box<dyn Error + Send + Sync + 'static>`. The anyhow::Result path is shorter to roll out.

3. **The `bin/` boundary around `alephcore` is leaky — many subcommands call into a path that *can* spawn async code, but the `Box<dyn Error>` return assumes a sync caller.** `commands/secret.rs::handle_secret_providers` works around this with a manual tokio runtime (BIN-R4-01); `commands/identity.rs::verify_artifact_file` opens SecurityStore synchronously (fine for read paths); `commands/bootstrap_runtime/mod.rs::run` returns `i32` (the only outlier, and correctly). The pattern is "sync command dispatcher, async internals", which is consistent — but the workaround at the `secret providers` site is the only one not following the rule and reveals the inconsistency.

4. **No file-level review of test code was performed** (per instructions). The presence of source-pin tests like `both_daemon_exit_paths_reap_background_jobs` in `helpers.rs`, `both_run_start_paths_check_session_visibility` in `server_init.rs`, and `register_builtin_definitions_skips_curated_names` in `tool_catalog_init.rs` indicates the project uses "documentation as assertion" extensively. Those tests will catch the BIN-R4-01/02 sites if a reviewer deletes the call; they don't catch the perf/behavior smell that motivated me to file them in the first place. The pattern is encouraging; the inventory is not exhaustive. A separate audit pass on `src/bin/aleph-server/commands/**/mod.rs` test modules would close that gap.

5. **`AGENTS.md` flags a documented MULTIPLE-PROCESS hazard (`HMAC failure → vault data loss`).** This audit confirmed the singleton-fcntl-lock is acquired at the correct moment (in `main()`, before `fork()`), but it did NOT trace the lock semantics into `alephcore::utils::instance_lock::try_acquire`. That code path is outside `bin/` scope, but worth a follow-up audit if the LOCK contract ever drifts.

## Summary
- **Total: 15 findings** (0 Critical, 2 High, 5 Medium, 8 Low)
- **Top priority items (must-fix)**:
  1. **BIN-R4-02** — Cron/heartbeat `request_shutdown()` flips a flag but never joins; timer loops can race teardown. High.
  2. **BIN-R4-07** — `aleph update` runs `curl | bash` with no SHA256/signature check; `aleph update --check` does a plaintext GitHub API call with no signing. Medium-severity security, trusted-update-only scope.
  3. **BIN-R4-04** — `identity verify --input` reads the file unbounded; an oversized or pointed-anywhere file OOMs the CLI or leaks reads of unrelated files.
  - Honorable mentions: **BIN-R4-08** (panic recovery in node writer), **BIN-R4-10** (PID-file-write failure leaves a daemon running invisibly).

## What was NOT covered
- **alephcore internals** — `instance_lock`, `process_alive`, `paths`, `IpcdEndpoint::write_endpoint` are referenced but their bodies were not read.
- **Live tracing subscriber observability** — the `eprintln!` / `tracing::warn!` topology was read, not exercised.
- **Signal handler interactions with `--force` and systemd** — only one of several signal handlers was traced.
- **`aleph-server start` exit-code behavior on every error path** — only one `start_server` path (PID-file write) was flagged in BIN-R4-10; the other 30+ `?` failure points were not enumerated.
- **`commands/start/builder/handlers/settings.rs`** is 1070 LoC and was sampled, not read end-to-end; the registration-only-handler pattern is uniform and not individually noteworthy.
- **`commands/node.rs::handle_frame` and the test module** — covered BIN-R4-08 at the writer-task level; the dispatch logic was not audited.
- **Cron / heartbeat timer-loop internals** — referenced only as consumers of `SharedCronService` / `SharedHeartbeatService`; the loop bodies live in `alephcore::tasks::cron::timer` and `alephcore::tasks::heartbeat::service` and were not read.
- **The `Box<dyn Error>` return contract** was flagged as a cross-cutting smell (BIN-R4-15 indirectly); full type-system audit across all `commands/*.rs` is out of scope for this round.
- **Test correctness audit** — the project uses "documentation as assertion" source-pin tests at multiple sites (see Cross-cutting 4); those were observed but not tested for false-positive resistance.

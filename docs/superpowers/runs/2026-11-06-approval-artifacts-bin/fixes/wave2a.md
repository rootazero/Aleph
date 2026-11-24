# Fix report — wave2a (second-pass findings, batches B3 + B5)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-wave2a` · branch `fix-wave2a` (base `main@9cc85a599`)
Scope of this wave: B3-S2-01, B5-S2-01, B5-S2-03. B5-S2-02 and B10-S2-03 were explicitly out of scope.

## B3-S2-01 [P1] Node requester omits Ok(0) check, silently parks approval for full timeout
- verdict: fixed
- files: `src/approval/node_requester.rs:130` (publish arm), `src/approval/node_requester.rs:139` (`Ok(0)` arm), `src/approval/node_requester.rs:196` (new `deny_unavailable` helper)
- change: `publish_frame` now returns the number of subscribers that received the frame, and `Ok(0)` means the
  `ApprovalRequested` card reached nobody — no operator surface was listening, or every subscriber lagged the
  broadcast ring. The `if let Err(e)` pattern match only covered the error case, so `Ok(0)` fell through to the
  approval await and the node parked for the full `DEFAULT_APPROVAL_TIMEOUT_MS` against a request nobody was ever
  asked about. The match now has three arms: `Ok(0)` denies the pending approval and returns the shared
  "unavailable" wire outcome, `Ok(_)` proceeds, `Err(e)` keeps the pre-existing deny behaviour (moved verbatim into
  the same helper). The deny/resolution body was extracted into `deny_unavailable(manager, approval_id)` so both
  failure arms share one implementation, mirroring `operator_requester::request_approval` (APPROVAL-R4-01) and its
  `deny_unavailable` (src/approval/operator_requester.rs:257, :416-425), which the first round already reworked.
- risk: behaviour change for the node path only — an approval request published while no operator surface is
  connected is now denied immediately with reason "unavailable" instead of waiting out the timeout. Callers of
  `run_node_approval` already handle the `"unavailable"` outcome (it existed for the publish-error case), so no new
  outcome is introduced. `ExecApprovalManager::resolve` removes the pending entry, so the node's `rx` is resolved
  and the await cannot park. Risk: a subscriber that is present but lagged now denies instead of being waited for;
  that is the same fail-closed contract the operator path already uses.

## B5-S2-01 [P1] Daemon parent reports success before daemon setup succeeds
- verdict: fixed
- files: `src/bin/aleph-server/daemon.rs:24` (`READY_OK`), `src/bin/aleph-server/daemon.rs:28` (`READY_ERROR`),
  `src/bin/aleph-server/daemon.rs:35` (`DAEMON_READY_TIMEOUT_MS`), `src/bin/aleph-server/daemon.rs:432` (unix
  `daemonize`), `src/bin/aleph-server/daemon.rs:556` (`setup_daemon_after_fork`), `src/bin/aleph-server/daemon.rs:670`
  (`create_readiness_pipe`), `src/bin/aleph-server/daemon.rs:690` (`close_fd`), `src/bin/aleph-server/daemon.rs:702`
  (`report_daemon_setup_failure`), `src/bin/aleph-server/daemon.rs:719` (`wait_for_daemon_readiness`)
- change: the invoking parent used to `exit(0)` right after the first `fork()`, while the daemon could still fail
  `setsid()`, the second `fork()`, `chdir`, the log redirect or the PID-file write — the shell recorded a successful
  start although no server existed, and those failures were only visible in the daemon's own (redirected) stderr.
  A readiness pipe (both ends `FD_CLOEXEC`) now connects the invoking parent to the final daemon: the daemon writes
  `ok` after its post-fork setup completes, or `error: <message>` and exits non-zero when setup fails. The parent
  blocks on the pipe (30s cap) and only then reports success — `exit(0)` on `ok`, a propagated `Err` (→ non-zero
  exit of `aleph-server start`, since `main.rs` calls `daemonize(...)?`) on an error report, on an empty report
  (daemon died without reporting), or on a timeout. The daemon-side setup body was moved verbatim into
  `setup_daemon_after_fork` so a failure inside it can be reported instead of returning an error the already-forked
  parent would never see.
- risk: `aleph-server start --daemon` now blocks until the daemon finished its setup (local file I/O, normally
  milliseconds) instead of returning instantly — that is the intended semantic change, but it changes the perceived
  latency of the command and could surface a previously hidden setup failure as a non-zero exit. The invoking parent
  still never continues into server startup (`exit(0)` on success), so no second server can be started.
  `setup_daemon_after_fork` keeps the pre-existing semantics verbatim: log rotation and `cleanup_old_logs` stay
  best-effort and the PID-file write stays WARN-only per the BIN-R4-10 decision, so those two do not fail the
  handshake. The handshake covers daemon setup only — the later socket bind happens after `daemonize` returns and is
  still outside it. The intermediate parent (child of the first fork) still exits 0 and no longer holds the write end
  of the pipe, so the parent's read sees EOF exactly once.

## B5-S2-03 [P2] Failed installer launch leaks downloaded update files
- verdict: fixed
- files: `src/bin/aleph-server/commands/update.rs:252` (guard binding), `src/bin/aleph-server/commands/update.rs:316`
  (`TempDirGuard`), `src/bin/aleph-server/commands/update.rs:320` (`Drop` impl), `src/bin/aleph-server/commands/update.rs:304`
  (removed the single explicit cleanup call)
- change: the private temp directory (`aleph-update-<pid>-<nonce>`) was removed by one explicit
  `std::fs::remove_dir_all` placed after the installer launch, so every earlier return — a failed secure write, or a
  failed `Command::status()` launch — returned before the cleanup and left the downloaded release script (plus its
  SHA-verified bytes) under the system temp directory. Cleanup is now tied to the directory's lifetime through a
  `TempDirGuard` whose `Drop` removes the directory; the guard is created immediately after the directory is created,
  so write failure, launch failure, non-zero installer status and success all clean up. The explicit cleanup call was
  removed to keep a single removal path.
- risk: the removal now happens when `run_installer` returns instead of a few lines earlier, i.e. after the
  `if !status.success()` check — the installer has already exited (`.status()` waits for it), so the file is not
  needed any more and the timing difference is not observable. Cleanup failures stay swallowed (`let _ = ...`), so a
  locked file on Windows cannot turn a successful install into a failure. `run_installer`'s signature and the
  installer's exit-status handling are unchanged.

## Skipped findings (out of scope for this wave)
- B5-S2-02 [P2] Requested WebChat listener failure is detached and startup still succeeds — skipped-too-risky:
  handled in the other wave2 branch (explicitly excluded from this worker's scope).
- B10-S2-03 — skipped-too-risky: deferred by triage (explicitly excluded from this worker's scope).

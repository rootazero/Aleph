# Wave 4c fixes

## B3-003 — aborted attended approval leaves a zombie registration

- verdict: FIXED
- files: `src/exec/manager.rs`
- change: Added a private, generation-bound `PendingRegistration` drop guard to `RegisteredApprovalReceiver`. If an attended wait is cancelled while the receiver is still pending, the guard removes only the matching `(approval_id, generation)` entry immediately. The receiver disarms the guard when its oneshot becomes ready so `await_registered` retains ownership of the normal completion path and can harvest `deny_reason` before removal. Duplicate registrations retain no cleanup token. Cleanup only removes the pending entry; it does not resolve, deny, cascade, or publish a decision.
- risk: The cleanup guard takes the existing pending write lock during receiver drop, so it follows the manager's existing lock and can block briefly behind another resolver. `try_recv` has no production callers in the inspected tree and remains unchanged; no test or build command was run by contract.

## B3-004 — operator fallback is lost after channel teardown

- verdict: FIXED
- files: `src/approval/adapters.rs`
- change: `FallbackApprovalRequester` now retains the channel response and falls back to the operator requester only when the channel returns `ApprovalOutcome::Unavailable` after the reachability preflight. Delivered decisions, denials, and timeouts are returned directly and are never retried through the operator path.
- risk: A channel that returns `Unavailable` after partially performing an external side effect is treated according to the existing transport contract as an undelivered request; the change does not add cross-transport deduplication. No test or build command was run by contract.

## B10-003 — separate WebChat server is detached from gateway shutdown

- verdict: FIXED
- files: `src/bin/aleph-server/server_init.rs`, `src/bin/aleph-server/commands/start/builder/handlers/config.rs`, `src/bin/aleph-server/commands/start/builder/handlers/mod.rs`, `src/bin/aleph-server/commands/start/mod.rs`
- change: Added a crate-visible listener-based WebChat serving helper while preserving the existing public `serve_webchat(SocketAddr, PathBuf)` signature. Startup now retains the prebound listener and spawned `JoinHandle`; after `run_until_shutdown` returns it aborts and awaits that task before the existing startup-resource and config-watcher teardown. The task logs unexpected join failures while treating cancellation as normal.
- risk: WebChat is stopped by task cancellation rather than a separate graceful HTTP drain, matching the existing gateway shutdown ownership and preventing the discarded-task/rebind race. The helper was source-inspected only; no build, test, or live restart validation was run by contract.

## Verification

- `git diff --check` passed.
- Source inspection confirmed the old WebChat call path is gone, the public address-based helper remains, and the new listener helper has one startup caller.
- Cargo, clippy, rustfmt, tests, and live runtime checks were intentionally not run; the orchestrator owns unified compilation and test verification.

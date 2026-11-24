# Fix report — wave 4b (B5-001: `aleph-server stop` must not signal a recycled PID)

Commit: `90b57f0b3` — `fix(daemon): verify the recorded server identity before signalling a pid`
Branch: `fix-w4b` (base `main@777f37616`)

## B5-001 [P1] Stop can SIGTERM/SIGKILL a recycled unrelated PID

- verdict: fixed

- files:
  - `src/cli/endpoint.rs:39-77` — `IpcEndpoint` gains the `start_time: Option<u64>` field; `IpcEndpoint::current()` records the writer's own process start time.
  - `src/bin/aleph-server/daemon.rs:183-232` — `handle_stop` restructured: the endpoint record is now the only path that may authorise a signal, and it must pass the identity check first.
  - `src/bin/aleph-server/daemon.rs:234-282` — new `verify_endpoint_identity(endpoint, pid, live_start_time) -> Result<(), String>` (pure; the live start time is injected so it is testable without spawning processes).
  - `src/bin/aleph-server/daemon.rs:939-947` — existing `status_tests::endpoint` helper updated for the new field.
  - `src/bin/aleph-server/daemon.rs:1020-1065` — new `stop_identity_tests` module (4 tests: matching start time proves identity; recycled PID refused; record without a start time refused; unreadable live start time refused).

- change:
  The endpoint record (`.ipc-endpoint.json`) now binds the server's PID to the start time of the process that wrote it (`start_time`, seconds since the Unix epoch, taken from the existing `alephcore::utils::process_alive::process_start_time` — the same anti-PID-reuse signal the singleton lock already uses). `handle_stop` no longer treats "the PID is alive" as proof of identity: before any SIGTERM/SIGKILL it compares the recorded start time with the live process's start time, and only an exact match authorises the signal. A mismatch (recycled PID), a record without a start time, an unusable `0` value, or a start time the platform will not report all fail closed — no signal is sent, the error is returned to `main` (stderr message + non-zero exit). The endpoint-absent (PID-file-only) path keeps the pre-existing refusal ("no matching IPC endpoint proves it is an Aleph server"), and the stale-record path still cleans up the PID file / endpoint file without signalling.

- risk:
  - Behaviour change (upgrade window): an endpoint file written by a build that predates this change carries no `start_time`, so `aleph-server stop` now refuses to stop that server ("the IPC endpoint record carries no process start time…") and the operator must stop it manually or restart it with the current build. This is the intended fail-closed behaviour, but it is a visible change for a server started by an older binary.
  - The endpoint file format gained an optional field; `version` was deliberately **not** bumped, so an older binary still parses the file (serde ignores unknown fields here). Old readers simply ignore `start_time`.
  - Start-time granularity is one second (sysinfo): a false match would require the OS to recycle the exact PID to a process started within the same wall-clock second as the original server — not reachable in practice, but it is the residual weakness of this signal.
  - `process_start_time` returns `None` when the platform cannot report a start time; the check then refuses (fail closed), so on such a platform `stop` would never signal.
  - `handle_stop`'s public signature and the CLI surface are unchanged; its only caller (`src/bin/aleph-server/main.rs:172`) is unaffected.
  - Not verified by compilation: per the brief, no `cargo` / `clippy` / `rustfmt` / test run was performed (the orchestrator runs the unified `cargo check`). The new code was hand-matched to the surrounding rustfmt style.

## What was NOT done

- No fallback to the singleton-lock sidecar (`aleph.lock.pid`, which also records `pid` + start time) for endpoint records written by an older build — that would live outside this batch's file scope (`src/utils/instance_lock.rs`).
- No IPC/HTTP handshake fallback (would need a PID-reporting route plus client plumbing).
- No `/proc/<pid>/stat` hand-parsing for a higher-resolution start time (Linux-only); the existing cross-platform `sysinfo` helper was reused instead.
- No change to `daemonize`'s existing endpoint/PID-file guard, to `handle_status`, or to the instance-lock mechanism.

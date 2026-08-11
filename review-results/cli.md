# Module: src/cli

- Path: `src/cli/`
- Files scanned: 3 (endpoint.rs, ipc_client.rs, policy.rs) + 1 mod.rs
- Total production LOC: ~285 (excluding tests)
- Review date: 2026-08-12
- Reviewer: static (4-perspective: security / logic / architecture / quality)
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary

| Severity | Count |
|----------|------:|
| critical | 0     |
| high     | 2     |
| medium   | 2     |
| low      | 2     |
| **Total**| **6** |

## Findings

### High

#### H1 — `danger_accept_invalid_certs(true)` unconditionally accepts all TLS certs
- **File:** `src/cli/ipc_client.rs:79-81`
- **Perspective:** Security
- **Description:** `forward_to_server` builds an HTTP client with
  `danger_accept_invalid_certs(true)` regardless of whether the endpoint URL is
  `http://` or `https://`. `build_endpoint_url` will emit an `https://` URL when
  the server is configured with TLS (`endpoint.rs:24-26`), so an attacker with
  any position on the routing path can MITM the admin IPC channel. The existing
  doc comment on `build_endpoint_url` ("Self-signed cert trust is the caller's
  problem") is correct in principle, but the only caller (`ipc_client.rs`)
  ignores the problem entirely.
- **Live consumers (read-before-write check):**
  `src/bin/aleph-server/commands/secret.rs:226,288` and
  `src/bin/aleph-server/commands/resume.rs:?` both call `forward_to_server` —
  this is a **load-bearing** wire.
- **Decision:** **CONNECT** — restrict the insecure override to loopback
  addresses only (`127.0.0.1`, `::1`); bail on any other HTTPS host. Local
  self-signed TLS is a common, expected setup, and pinning/CA-validation is
  out of scope for a one-shot CLI; the loopback gate narrows the trust window
  from "any host" to "same machine".
- **Fix:** When the URL scheme is `https://`, parse the host and only apply
  `danger_accept_invalid_certs(true)` if the host is loopback; otherwise refuse
  the connection with a clear error.

#### H2 — Endpoint file left with world-readable permissions on chmod failure
- **File:** `src/cli/endpoint.rs:60-75`
- **Perspective:** Security
- **Description:** `write_endpoint` writes the file atomically then calls
  `set_permissions(0o600)`. If `set_permissions` fails (e.g. on a filesystem
  with restrictive ACLs that block the mode set, or a sandbox that doesn't
  support chmod), the endpoint URL and PID are now on disk in a possibly
  world-readable state. The function returns an error but never cleans up.
- **Live consumers (read-before-write check):**
  `src/bin/aleph-server/commands/start/mod.rs:3108` and
  `src/bin/aleph-server/daemon.rs:397` both call `write_endpoint` /
  `remove_endpoint`. **Load-bearing.**
- **Decision:** **CONNECT** — on `set_permissions` failure, remove the just-
  written file and return the error so the operator is not left with a
  quietly-permissionless file.

### Medium

#### M1 — `LockOrIpc` TOCTOU race between lock-held check and IPC forward
- **File:** `src/cli/policy.rs:130-141`
- **Perspective:** Logic
- **Description:** When `acquire_or_held` returns `LockHeldError`, we call
  `forward_to_server` to talk to the running server. Between the
  `try_acquire` call and the HTTP request landing, the lock holder may shut
  down (or release its lock), leaving the user with a confusing
  "server is initializing or crashed" error and no fallback. The local
  lock is now free and the command could have run safely.
- **Live consumers:** `src/bin/aleph-server/commands/secret.rs::Init, Set`
  and `resume.rs`. **Load-bearing.**
- **Decision:** **CONNECT** — when IPC forward fails, retry `acquire_or_held`
  once. If the lock is now free, run `local`. Only the second failure is
  surfaced.

#### M2 — `LockHeldError { pid: 0, orphaned: false }` from unreadable sidecar
- **File:** `src/cli/policy.rs:89-103`
- **Perspective:** Logic / operability
- **Description:** `instance_lock::try_acquire` returns
  `HeldByLive { pid: 0, lock_path }` when the sidecar is unreadable (per
  `instance_lock.rs:127-130` fallback). `acquire_or_held` maps that to
  `LockHeldError { pid: 0, orphaned: false }`, which the Display impl prints
  as "server is running (PID 0)" — PID 0 is never a valid user-space process,
  so this is actively misleading for the operator.
- **Decision:** **CONNECT** — treat `pid == 0` as "holder unknown / sidecar
  unreadable" and surface it as orphaned (the safe interpretation: we don't
  know if a live process holds the lock, so we treat the lock as effectively
  orphaned). The operator message becomes "orphaned lock detected; no live
  server" instead of "server is running (PID 0)".

### Low

#### L1 — Server response body embedded verbatim into user-facing errors
- **File:** `src/cli/ipc_client.rs:55-58, 95-98`
- **Perspective:** Security / quality
- **Description:** `finalize` and the 401 handler both call `resp.text()` and
  embed the raw body in the error message. A misbehaving server that returns
  a stack trace in the body (e.g. debug-mode HTTP error) would leak internal
  paths, module names, or backtraces to the CLI user. The body is also not
  bounded, so a large payload inflates log output.
- **Decision:** **CONNECT** — truncate the body to 256 chars before splicing
  into the error. Keeps operators informed but bounded.

#### L2 — `#[cfg(not(windows))]` test guard is no longer needed
- **File:** `src/cli/policy.rs:208`
- **Perspective:** Test coverage
- **Description:** The test comment says "Windows LockFileEx blocks the held-
  lock PID readback" — but `instance_lock.rs` was refactored to read the PID
  from an *unlocked* sidecar, so `try_acquire` returns `HeldByLive { pid }`
  cross-platform. The sibling tests in `instance_lock.rs`
  (`second_acquire_in_same_process_returns_held_by_live`,
  `diagnose_holder_returns_pid_when_held`) already run on Windows without
  guards. The `try_with_policy_lock_only_returns_err_when_held` test should
  follow.
- **Decision:** **CONNECT** — remove the `#[cfg(not(windows))]` guard.

## DECIDE (deferred)

- **D1.** `write_atomic` creates the temp file before permission tightening is
  re-asserted on the final path. (`atomic_io.rs:write_atomic` already creates
  a `tempfile::Builder::new()` without an explicit `.permissions(0o600)`,
  relying on the library default. `endpoint.rs:60-75` re-tightens post-rename
  so the persistent state is always 0o600, but a brief race window exists.)
  Resolving cleanly requires changing `atomic_io::write_atomic`'s signature to
  accept permissions, which touches 8+ call sites. **Deferred** to a wider
  atomic_io pass; the current code is correct on every filesystem that
  respects `tempfile`'s documented default.
- **D2.** `with_policy` calls `std::process::exit(64)` on lock contention
  (`policy.rs:155-160`). This is a deliberate UX choice (clean stderr + exit
  code) documented in the function's docstring and reviewed at Task 11. Not a
  defect; flagged for completeness only.
- **D3.** No Windows ACL tightening equivalent for `.ipc-endpoint.json`.
  Windows is not a supported production target for the IPC channel; flagged
  for the portability audit pass.

## CUT (acknowledged dead / non-issues, no change)

- `remove_endpoint` both logs and returns an error — caller can choose
  to log again or not. Single responsibility holds (caller decides).
- `with_policy` delegates to `try_with_policy` for everything except
  `LockOnly` — the "duplicate" is by design (the `LockOnly` arm needs the
  `eprintln` + `exit(64)` UX that the try variant intentionally omits).
- `try_with_policy` non-LockHeldError path returns `Err(e)` directly —
  correct.
- `IpcEndpoint` schema is a single source of truth (struct + version
  constant); no drift possible.
- `build_endpoint_url` IPv6 bracket handling is unit-tested on multiple
  concrete addresses; no parser bugs found.

## Architecture Compliance

| Redline | Status | Notes |
|---------|--------|-------|
| R1 (core no platform APIs) | ✓ | All `#[cfg(unix)]` blocks are inside `endpoint.rs` permission tightening, well-bounded. |
| R4 (interface layer pure I/O) | ✓ | `ipc_client` is pure HTTP forwarding; `endpoint` is pure file I/O; `policy` is dispatch only. |
| R7 (one core, many shells) | ✓ | This module is the wire between the `aleph-server` binary and the live server. The producer (CLI) and the consumer (admin HTTP API) are both live. |
| R8 (regex for machine formats only) | ✓ | No regex usage in this module. |
| R10 (intelligence in prompt) | ✓ | No prompt engineering here. |

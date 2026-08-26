# Logic Review Report — src/cli/

**Module**: cli
**Scope**: `src/cli/mod.rs` (3 LOC), `src/cli/endpoint.rs` (238 LOC), `src/cli/ipc_client.rs` (265 LOC), `src/cli/policy.rs` (347 LOC) — 853 LOC total
**Date**: 2026-08-26
**Mode**: normal
**Worktree**: `.worktrees/rust-logic-audit-2026-08-26`
**Branch**: `rust-logic-audit/2026-08-26`

## Context (Phase 1)

The `src/cli/` module exposes CLI-side helpers that talk to a running
daemon:

- `endpoint.rs` — read/write/remove `.ipc-endpoint.json` (a
  server-supplied `IpcEndpoint { version, url, pid, started_at }`) under
  the user's data dir. Owns the URL format (loopback mapping for `0.0.0.0`
  and `::`).
- `ipc_client.rs` — reads bearer token from `security.db` (read-only
  WAL) and forwards a CLI request to the daemon's `/v1/admin/*` HTTP
  endpoint. One-shot retry on `401` to handle token rotation.
- `policy.rs` — `run_no_lock` / `try_with_policy` / `with_policy`
  dispatch helpers. `LockOrIpc` retries local acquisition once when
  IPC forward fails (the holder may have released between the lock
  check and the IPC send).
- `mod.rs` — submodule wiring only.

Prior audits (`review-results/cli.md` 2026-08-12, `cli-batch-1/REPORT.md`
2026-08-22) found and FIXED 6 issues (H1/H2/M1/M2/L1/L2). This audit is a
deeper semantic pass on what survived.

Cross-module context (via `graphify explain`):

- `cli::endpoint::write_endpoint` ← consumed by `start/mod.rs:3368`
  (boot path) and `cli::endpoint::remove_endpoint` ← `start/mod.rs:3416`
  + `daemon.rs:397`.
- `cli::endpoint::read_endpoint` ← consumed by
  `cli::ipc_client::forward_to_server` and `daemon.rs:349`
  (`read_endpoint_best_effort`, status probe).
- `cli::ipc_client::forward_to_server` ← consumed by
  `cli::policy::try_with_policy` (`LockOrIpc` arm) and directly by
  `commands/resume.rs::handle_resume_command`.
- `cli::policy::with_policy` / `try_with_policy` / `run_no_lock` ←
  consumed by `commands/secret.rs` (5 subcommands), `commands/gateway.rs`,
  `commands/plugins.rs`, `commands/bootstrap_runtime/mod.rs`.

The `instance_lock::try_acquire` it depends on already refuses symlinks at
the lock path with `O_NOFOLLOW` (Unix) and uses `write_atomic` for the
holder sidecar — so lock-layer TOCTOU is well-defended. The CLI module
adds no new lock primitives and uses no in-process `Mutex` (the lock
hierarchy redline does not bind here — the CLI is a one-shot process).

## Findings

### [Warning] Endpoint file URL host is not validated — bearer token follows whatever URL the daemon wrote
- **Location**: `src/cli/ipc_client.rs:34-37`, `src/cli/endpoint.rs:36-54`
- **Trigger condition**: An attacker who can write to `~/.aleph/data/.ipc-endpoint.json`
  (which is mode 0600, so the attack is bounded by "another process as the
  same UID") replaces the daemon's `url` with an attacker-controlled URL.
  The CLI reads the endpoint, reads the bearer token from `security.db`,
  and POSTs to the attacker's URL with `Authorization: Bearer <token>`.
- **Expected**: The CLI refuses to forward the bearer token to non-loopback
  hosts (analogous to the `is_loopback_host` gate that already exists for
  HTTPS).
- **Actual**: For `http://` (the default), `build_client` performs no host
  check at all — `danger_accept_invalid_certs` is the only guard, and it
  only fires for `https://`. The comment in `build_client` explicitly notes
  this is intentional: "the bearer token is the only authentication".
  That assumption only holds when the URL is loopback.
- **Risk**: Token exfiltration to an attacker-controlled server. Bearer
  token grants `/v1/admin/*`, so a leaked token is full admin compromise
  for the daemon's IPC channel.
- **Current impact**: low-medium (bounded by filesystem write access
  as same user), but worth defense-in-depth because the endpoint file
  is reachable by anything running as the user (extensions, shell
  snippets, compromised subprocesses).
- **Suggestion**: Add a `is_loopback_url(&str) -> bool` helper and reject
  the request (or require an opt-in env var) when the host is non-loopback,
  matching the existing HTTPS posture. See suggested test below.

### [Warning] IPv6 HTTPS loopback URL is incorrectly rejected
- **Location**: `src/cli/ipc_client.rs:122-129`
- **Trigger condition**: Daemon binds on `::1` with TLS enabled
  (`gateway.tls.enabled = true`). Endpoint URL becomes
  `https://[::1]:18790`. CLI tries to build the client.
- **Expected**: `build_client` accepts `https://[::1]:port` (same way
  `http://[::1]:port` is accepted), since `::1` is loopback.
- **Actual**: The host extraction
  `url.trim_start_matches("https://").split([':', '/', '?']).next()`
  splits `[::1]:18790` on `:` first, yielding `[` as the first segment.
  After `trim_start_matches('[').trim_end_matches(']')`, the host is
  the empty string `""`, and `is_loopback_host("")` returns `false`.
  The function bails with "TLS IPC endpoint is not on loopback".
- **Risk**: Real daemon bind to `::1` with TLS is a supported
  configuration per `build_endpoint_url`'s IPv6 handling. The CLI silently
  rejects it. The existing test
  `build_client_accepts_loopback_https` only covers IPv4.
- **Current impact**: medium (breaks IPv6 HTTPS loopback setup).
- **Suggestion**: Use `url::Url::parse(url).ok().and_then(|u| u.host_str())`
  to get the host correctly for both v4 and v6, then run
  `is_loopback_host` on the result. Add the missing test.

### [Warning] `read_endpoint` silently treats unknown version as "missing endpoint"
- **Location**: `src/cli/endpoint.rs:118-126`
- **Trigger condition**: Daemon upgraded to version N+1 (writes endpoint
  with `version = N+1`), CLI is still at version N. CLI calls
  `read_endpoint`.
- **Expected**: A clear error message telling the operator the CLI is
  outdated ("CLI version X is too old; server requires version Y; please
  upgrade").
- **Actual**: The function returns `Ok(None)`. The CLI then bubbles up:
  > "server is initializing or crashed (no .ipc-endpoint.json at <path>).
  > Try again or run `aleph stop` first."
  The operator is misled into believing the server is broken and may run
  `aleph stop` — which removes the file and starts a fresh daemon,
  potentially losing the bearer-token/HMAC relationship to upstream
  data.
- **Risk**: Operator action based on misleading error.
- **Current impact**: low (the comment says it's deliberate, but the
  error message doesn't carry the version hint).
- **Suggestion**: Return a distinct error variant
  (`Ok(None)` already collapses "missing" and "wrong-version" into one)
  OR surface a debug-level warning that names the version. Even keeping
  `Ok(None)` is acceptable, but `read_endpoint_best_effort` in
  `daemon.rs:347` (which also calls `read_endpoint`) would benefit from
  a structured signal.

### [Warning] `forward_to_server` URL construction is naive string concatenation
- **Location**: `src/cli/ipc_client.rs:34-37`
- **Trigger condition**: The endpoint file contains a URL with an
  unexpected path or query string (e.g., a future server configuration
  adds a reverse-proxy prefix or `?token=...` for some reason).
- **Expected**: Robust URL joining (or at least a `Url::parse` +
  `set_path`).
- **Actual**: `format!("{}/{}", endpoint.url.trim_end_matches('/'),
  route.trim_start_matches('/'))`. If `endpoint.url` ends with `?q=v`,
  the route is appended after the query, making the route a query
  parameter of the original URL. Today the daemon only writes
  `scheme://host:port` so this is dormant, but it's a brittle
  invariant.
- **Risk**: Latent URL parsing bug if endpoint URL format evolves.
- **Current impact**: low (no exploitable path today).
- **Suggestion**: Use `url::Url::parse(endpoint.url)` then
  `url.join(route)` for path joining, with `Url::set_query(None)` to
  clear any inherited query string. Validate the final URL host is
  loopback (ties into finding above).

### [Warning] HTTP response body is not size-bounded — only the embedded error is
- **Location**: `src/cli/ipc_client.rs:156-167`
- **Trigger condition**: A buggy or malicious server returns a very
  large `application/json` body to a successful (200) admin request.
- **Expected**: Either `Content-Length` check or `take(N)` on the body
  stream before `resp.json::<T>()`. Existing precedent: `read_endpoint`
  already uses `take(MAX + 1)` precisely to avoid OOM from a corrupt
  large file.
- **Actual**: `resp.json::<T>()` will buffer the entire body into
  memory before deserializing. `reqwest`'s default body size is bounded
  by the OS socket buffer + memory, not by an application cap.
- **Risk**: Memory pressure on the CLI process (small in practice —
  the CLI is short-lived, but still avoidable).
- **Current impact**: low.
- **Suggestion**: Either accept the small risk and document, or
  configure `reqwest::blocking::ClientBuilder::max_body_size` if the
  version supports it (reqwest 0.12+ does).

### [Warning] `with_policy`'s `LockOrIpc` retry discards the second lock acquisition error
- **Location**: `src/cli/policy.rs:153-170`
- **Trigger condition**: First `acquire_or_held` returns `LockHeldError`,
  IPC forward fails (`fwd_err`). Retry `acquire_or_held` returns a
  DIFFERENT error (e.g., `PermissionDenied` because `data_dir` mode
  changed between the two acquires, or `NotFound` because `data_dir`
  was unmounted).
- **Expected**: Surface the more informative one (or at least log the
  lock error before discarding it).
- **Actual**: Code returns `Err(fwd_err)` and the lock error is dropped
  silently. The comment says "only the second failure is surfaced",
  which is the intent, but it ignores the case where the lock error is
  strictly more informative than the IPC error.
- **Risk**: Operator sees a confusing "connection refused" instead of
  "data dir is no longer accessible".
- **Current impact**: low.
- **Suggestion**: Add `tracing::warn!(?lock_err, "lock state changed
  between IPC failure and retry; surfacing IPC error")` before
  returning `fwd_err`. Or surface the lock error when its
  `ErrorKind != LockHeldError` (e.g., PermissionDenied).

### [Warning] `forward_to_server` 401-retry only handles one rotation
- **Location**: `src/cli/ipc_client.rs:42-56`
- **Trigger condition**: The token rotates twice between `read_token`
  and `call_once`. The second rotation replaces the value with
  `fresh`, but a third rotation (during the second `call_once`)
  could put us back in the same position. (Today's `set_shared_token_with_secret`
  is `DELETE+INSERT` — every rotation invalidates the prior token
  immediately, so this only matters if the rotation interval is shorter
  than a round-trip.)
- **Expected**: Bounded retry loop with a clear upper bound (say, 2-3
  attempts).
- **Actual**: Single retry. After the retry, the function returns the
  error to the caller.
- **Risk**: Low — the rotation cadence is operator-driven, not
  high-frequency. But a malicious or buggy operator-side script could
  rotate every 100ms and reliably break all CLI commands.
- **Current impact**: low.
- **Suggestion**: Either accept the risk or document the constraint
  ("bearer token must not rotate more than once per CLI invocation").

### [Warning] `truncate_error_body` uses byte length, not char count
- **Location**: `src/cli/ipc_client.rs:67-86`
- **Trigger condition**: The server returns an error body whose first
  256 bytes contain 200+ multi-byte UTF-8 characters (e.g., Chinese
  or emoji).
- **Expected**: The truncation point is at a char boundary (already
  handled), and the **displayed message length** is roughly the cap.
- **Actual**: The cap is byte-based. A string of 64 multi-byte chars
  could be 256 bytes, so a short readable error gets cut off
  prematurely (only 64 chars visible).
- **Risk**: Cosmetic only — but inconsistent with what the constant
  name `MAX_ERROR_BODY_CHARS` suggests.
- **Current impact**: low (UX).
- **Suggestion**: Either rename to `MAX_ERROR_BODY_BYTES` (more
  accurate) or compute the cap in chars using `s.char_indices().nth(N)`
  and use the resulting byte offset. Tiny refactor; existing char
  boundary check is preserved.

### [Warning] `write_endpoint` chmod is redundant but masquerades as security-critical
- **Location**: `src/cli/endpoint.rs:73-82`
- **Trigger condition**: The `tempfile` crate creates the staging
  file with mode 0600 by default on Unix
  (`tempfile-3.27.0/src/file/imp/unix.rs:23`:
  `open_options.mode(... .unwrap_or(0o600))`).
- **Expected**: Comment should explain WHY the post-rename chmod is
  retained (defense in depth + Windows ACL story).
- **Actual**: The comment claims a window exists between rename and
  chmod during which the file might be world-readable. In practice
  the staging file was already 0600, so the window is harmless on
  Unix. On Windows, the chmod block is `#[cfg(unix)]`-gated so it
  doesn't run; the file inherits its ACL from the parent directory.
- **Risk**: Misleading comment makes future readers think the chmod
  is load-bearing and may waste effort auditing it.
- **Current impact**: low (documentation).
- **Suggestion**: Tighten the comment to clarify the chmod is
  belt-and-suspenders and what it actually defends against (e.g.,
  a custom `tempfile::Builder::permissions()` from a future caller
  that forgets, or a downgraded `tempfile` crate).

### [Warning] Bearer token stored in plaintext in `security.db`
- **Location**: `src/cli/ipc_client.rs:85-91` (consumer),
  `src/gateway/security/store/tokens.rs:60-82` (producer)
- **Trigger condition**: A backup of `~/.aleph/data/security.db`
  leaks the secret-vault master key (it's also the bearer token for
  `/v1/admin/*`).
- **Expected**: Either the bearer token is derived from an
  encrypted-at-rest secret, or operators are warned about plaintext
  storage.
- **Actual**: Plaintext column (`plaintext_token TEXT`). This is the
  documented design — the comment in `tokens.rs` calls it "the
  secret-vault master key" — but it's also the only credential for
  the admin IPC channel.
- **Risk**: Whoever can read `security.db` can talk to `/v1/admin/*`.
  Bounded by filesystem permissions today.
- **Current impact**: medium (single secret = both vault key and admin
  auth).
- **Suggestion**: Either separate the bearer token from the vault
  master key (different secrets, different rotation policies) or
  document the coupling explicitly in operator docs.

## Suggested Tests

### Test 1 — Reject non-loopback HTTP endpoint URL

```rust
#[test]
fn build_client_or_url_validation_refuses_non_loopback_http() {
    // Only meaningful once the host-validation gate from
    // ipc_client.rs is added. Today HTTP has no gate.
    let bad = std::path::PathBuf::from("/tmp");
    let _ = bad;
    // After fix:
    // let res = forward_to_server::<serde_json::Value>(
    //     &bad, HttpMethod::Get, "/v1/admin/whatever", serde_json::Value::Null,
    // );
    // assert!(matches!(res, Err(e) if e.to_string().contains("loopback")));
}
```

### Test 2 — IPv6 loopback HTTPS is accepted

```rust
#[test]
fn build_client_accepts_loopback_https_ipv6() {
    let client = build_client("https://[::1]:9000/admin")
        .expect("loopback https ipv6 should be allowed after host-extraction fix");
    drop(client);
}
```

### Test 3 — Endpoint version mismatch returns an informative signal

```rust
#[test]
fn read_endpoint_indicates_version_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    // Write a JSON file with a newer version than the CLI knows.
    let path = dir.path().join(".ipc-endpoint.json");
    std::fs::write(
        &path,
        r#"{"version": 999, "url": "http://127.0.0.1:1", "pid": 1, "started_at": "2026-01-01T00:00:00Z"}"#,
    ).unwrap();
    let res = read_endpoint(dir.path()).unwrap();
    // After fix: assert the function distinguishes "missing" from
    // "wrong-version". Easiest: have read_endpoint return a tagged
    // enum or a dedicated error variant.
    assert!(res.is_none(), "today: silently treated as missing");
}
```

### Test 4 — `forward_to_server` URL join with endpoint URL containing path

```rust
#[test]
fn forward_to_server_handles_endpoint_url_with_path_prefix() {
    use alephcore::cli::endpoint::{write_endpoint, IpcEndpoint};
    use alephcore::cli::policy::HttpMethod;
    let dir = tempfile::tempdir().unwrap();
    write_endpoint(
        dir.path(),
        &IpcEndpoint::current("http://127.0.0.1:1/api"),
    ).unwrap();
    let res: anyhow::Result<serde_json::Value> = forward_to_server(
        dir.path(),
        HttpMethod::Get,
        "/v1/admin/whatever",
        serde_json::Value::Null,
    );
    // Today the URL is built as "http://127.0.0.1:1/api/v1/admin/whatever"
    // — a connection refused, not a malformed URL. After fix with
    // url::Url::join, behavior should be deterministic.
    assert!(res.is_err());
}
```

### Test 5 — `forward_to_server` retry path: token rotates twice

```rust
#[test]
fn forward_to_server_401_retry_handles_double_rotation() {
    // Existing 401 retry test rotates once. A "rotates twice" version
    // would rotate between read_token and the first call_once,
    // then again between the second read_token and the second
    // call_once. Today this would surface as an auth error. After
    // a bounded retry loop fix it should succeed.
    todo!("requires mock server that can rotate twice within one request");
}
```

### Test 6 — LockOrIpc retry surfaces PermissionDenied, not IPC error

```rust
#[test]
fn lock_or_ipc_retry_surfaces_permission_error_over_ipc_error() {
    let dir = tempfile::tempdir().unwrap();
    let _hold = match instance_lock::try_acquire(dir.path()).unwrap() {
        AcquireOutcome::Acquired(g) => g,
        _ => panic!(),
    };
    // Seed security.db and an endpoint pointing at an unbound port
    // (existing M1 test pattern).
    // ...

    // Drop the holder so retry acquire succeeds. After it succeeds,
    // immediately chmod the data_dir to 0000 so the second acquire
    // fails with PermissionDenied.
    drop(_hold);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

    let result: anyhow::Result<i32> = try_with_policy(
        CommandPolicy::LockOrIpc { route: "/x", method: HttpMethod::Get },
        dir.path(),
        |_lock| Ok(7),
        serde_json::Value::Null,
    );
    // Today: result.is_err() with the IPC "connection refused" message.
    // After fix: result.unwrap_err() should mention PermissionDenied or
    // the data_dir path is unreachable.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap(); // cleanup
    assert!(result.is_err());
}
```

## Summary

| Level        | Count |
|--------------|-------|
| Critical     | 0     |
| Warning      | 10    |
| Suggested Test | 6   |

## Cross-Module Observations

- **Local privilege escalation surface**: Narrow. Endpoint file is mode
  0600 (verified by `write_endpoint_sets_owner_only_permissions`
  test), and `instance_lock::try_acquire` uses `O_NOFOLLOW` to refuse
  symlinks at the lock path. The CLI module inherits these protections
  correctly. The remaining gap is "same-UID attacker writes the
  endpoint URL" — see Warning 1.

- **Policy bypass paths**: None. Every CLI command declares its
  `CommandPolicy` and dispatches through `run_no_lock` / `with_policy`
  / `try_with_policy`. The dispatch is exhaustive on
  `CommandPolicy` (`NoLock` → error in `try_with_policy` because
  it's wrong-tier, `LockOnly` and `LockOrIpc` → handled). Wiring
  completeness verified by listing all callers
  (`commands/secret.rs` × 5, `commands/gateway.rs`,
  `commands/plugins.rs` × 3, `commands/bootstrap_runtime/mod.rs`,
  `commands/resume.rs`). No `pub fn` in `src/cli/` lacks a caller.

- **IPC client race conditions**: Two of note.
  - **Holder-dies-between-check-and-IPC**: handled by the M1 retry
    path (already fixed in prior batch). The retry acquires the
    lock again and runs `local` if free. Verified by
    `lock_or_ipc_retries_local_acquire_when_forward_fails`.
  - **Token-rotation-during-request**: handled by the 401 retry arm.
    Verified by `tests/spec_c_cli_token_rotation.rs`. Multiple
    rotations in one invocation are not handled (Warning 7).

- **Sync primitives**: None. The CLI is a one-shot process; cross-
  process coordination goes through `instance_lock` (fs2) and the
  atomic file I/O helpers. No `Mutex` / `RwLock` use in `src/cli/`
  — the lock hierarchy redline doesn't bind here.

- **Architecture redlines**:
  - **R1** (core no platform APIs): ✓. The only `#[cfg(unix)]` blocks
    are in `endpoint.rs` permission tightening, `is_loopback_host`
    path parsing, and `instance_lock.rs`. All are bounded and
    reviewed.
  - **R4** (interface layer pure I/O): ✓. `ipc_client` is pure HTTP
    forwarding; `endpoint` is pure file I/O; `policy` is dispatch
    only.
  - **R7** (one core, many shells): ✓. This module IS the seam.
  - **R8 / R10**: N/A for this module (no prompt/regex usage).

## What I Did NOT Do

- I did not run `cargo check`, `cargo clippy`, or any cargo command
  (per protocol).
- I did not modify any source file (per protocol).
- I did not write to `TODO.md` or any tracking file inside the repo.
- I did not exhaustively review every consumer of `src/cli/` (e.g.,
  `commands/secret.rs` validation was glanced at, not deeply audited;
  the in-scope files are the three production `.rs` files listed in
  the task).
- I did not test the network behavior under real load (no proptest /
  loom sketch executed; suggested in test stubs above).
- I did not verify the `tempfile-3.27.0` default-mode claim against
  the actual dependency graph for older lockfiles — I read the
  crate source under `~/.cargo/registry/...` as evidence, not as a
  pinned-version guarantee.
- I did not assess Windows ACL behavior for `.ipc-endpoint.json`
  (out of scope; `#[cfg(unix)]` gates the chmod).
- I did not propose changes to `forward_to_server`'s
  `read_token → call_once → on-401 → read_token → call_once` pattern
  even though it is racy across >1 rotation; the suggestion is
  documented (Warning 7) but the patch would touch the public API.
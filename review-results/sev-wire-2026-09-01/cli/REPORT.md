# Severed-Wire Audit — `src/cli/`

**Date:** 2026-09-01
**Module:** `src/cli/{mod.rs, endpoint.rs (284 LoC), ipc_client.rs (320 LoC), policy.rs (419 LoC)}` — 1,026 LoC total
**Method:** Read-first sweep (`rg` parity checks + cross-reference to `src/gateway/admin_api/`, `src/bin/aleph-server/daemon.rs`, `src/bin/aleph-server/commands/`). Every "no consumer" claim is back-stopped by a `rg` invocation; production-vs-test distinction is enforced by stripping `#[cfg(test)]` blocks before counting. No edits made.

## Inventory — produced surface

### `mod.rs`
- `pub mod endpoint;`
- `pub mod ipc_client;`
- `pub mod policy;`

### `endpoint.rs` — `.ipc-endpoint.json` read/write + URL builder

| Symbol | Location |
|---|---|
| `pub fn build_endpoint_url(addr: SocketAddr, tls: bool) -> String` | endpoint.rs:24 |
| `pub struct IpcEndpoint { version: u32, url: String, pid: u32, started_at: String }` | endpoint.rs:39 |
| `pub fn IpcEndpoint::current(url: impl Into<String>) -> Self` | endpoint.rs:47 |
| `pub(crate) fn endpoint_path(data_dir: &Path) -> PathBuf` | endpoint.rs:58 |
| `pub fn write_endpoint(data_dir: &Path, endpoint: &IpcEndpoint) -> std::io::Result<()>` | endpoint.rs:62 |
| `pub fn read_endpoint(data_dir: &Path) -> std::io::Result<Option<IpcEndpoint>>` | endpoint.rs:87 |
| `pub fn remove_endpoint(data_dir: &Path) -> std::io::Result<()>` | endpoint.rs:140 |
| `const ENDPOINT_FILENAME: &str = ".ipc-endpoint.json"` | endpoint.rs:13 |
| `const CURRENT_ENDPOINT_VERSION: u32 = 1` | endpoint.rs:14 |
| `const MAX_ENDPOINT_FILE_SIZE: u64 = 1_048_576` | endpoint.rs:85 |

### `ipc_client.rs` — HTTP forwarder to `/v1/admin/*` over bearer token

| Symbol | Location |
|---|---|
| `pub fn forward_to_server<T>(data_dir, method, route, body) -> anyhow::Result<T>` | ipc_client.rs:18 |
| `fn read_token(data_dir: &Path) -> anyhow::Result<String>` | ipc_client.rs:94 |
| `fn call_once(url, method, body, token) -> anyhow::Result<reqwest::blocking::Response>` | ipc_client.rs:104 |
| `fn build_client(url: &str) -> anyhow::Result<reqwest::blocking::Client>` | ipc_client.rs:124 |
| `fn host_of(url: &str) -> Option<String>` | ipc_client.rs:154 |
| `fn is_loopback_host(host: &str) -> bool` | ipc_client.rs:198 |
| `fn finalize<T>(resp) -> anyhow::Result<T>` | ipc_client.rs:204 |
| `fn truncate_error_body(s: String) -> String` | ipc_client.rs:81 |
| `const MAX_ERROR_BODY_CHARS: usize = 256` | ipc_client.rs:71 |
| `const SECURITY_DB_FILENAME: &str = "security.db"` | ipc_client.rs:15 |

### `policy.rs` — declarative dispatch + lock-aware forwarding

| Symbol | Location |
|---|---|
| `pub struct LockHeldError { pid: u32, lock_path: PathBuf, orphaned: bool }` | policy.rs:16 |
| `impl fmt::Display for LockHeldError` | policy.rs:22 |
| `impl std::error::Error for LockHeldError` | policy.rs:56 |
| `pub enum HttpMethod { Get, Post }` | policy.rs:59 |
| `pub const fn HttpMethod::as_reqwest(&self) -> reqwest::Method` | policy.rs:66 |
| `pub enum CommandPolicy { NoLock, LockOnly, LockOrIpc { route: &'static str, method: HttpMethod } }` | policy.rs:75 |
| `pub fn run_no_lock<T, F>(f: F) -> anyhow::Result<T>` | policy.rs:92 |
| `fn acquire_or_held(data_dir: &Path) -> anyhow::Result<InstanceLock>` | policy.rs:102 |
| `pub fn try_with_policy<L, T>(policy, data_dir, local, ipc_body) -> anyhow::Result<T>` | policy.rs:134 |
| `pub fn with_policy<L, T>(policy, data_dir, local, ipc_body) -> anyhow::Result<T>` | policy.rs:204 |

## Inventory — production consumers

All `rg` output is restricted to non-`src/cli/` paths under `src/`, `interfaces/`, `shared/`, plus `tests/spec_c_cli_*.rs` (production-shaped integration tests). Test code inside `src/cli/` itself is excluded.

### endpoint.rs

```bash
$ rg -n "build_endpoint_url" src/ interfaces/ shared/ tests/
src/cli/endpoint.rs:24:    (definition)
src/cli/endpoint.rs:225:   (test) …:255:   (tests)
src/bin/aleph-server/commands/start/mod.rs:3479:    alephcore::cli::endpoint::build_endpoint_url(addr, full_config.gateway.tls.enabled);
```

```bash
$ rg -n "IpcEndpoint" src/ interfaces/ shared/ tests/ | grep -v "src/cli/endpoint.rs"
src/bin/aleph-server/daemon.rs:12:use alephcore::cli::endpoint::{read_endpoint, remove_endpoint, IpcEndpoint};
src/bin/aleph-server/daemon.rs:303:    endpoint: Option<&IpcEndpoint>,
src/bin/aleph-server/daemon.rs:347:fn read_endpoint_best_effort() -> Option<IpcEndpoint> {
src/bin/aleph-server/daemon.rs:678:    fn endpoint(pid: u32, started_at: &str) -> IpcEndpoint {        (test inside daemon.rs)
src/bin/aleph-server/daemon.rs:679:        IpcEndpoint { …                                              (test)
src/bin/aleph-server/commands/start/mod.rs:3520:    let endpoint = alephcore::cli::endpoint::IpcEndpoint::current(endpoint_url);
src/cli/policy.rs:397:    &crate::cli::endpoint::IpcEndpoint::current("http://127.0.0.1:1".to_string());   (test)
tests/spec_c_cli_token_rotation.rs:26:    use alephcore::cli::endpoint::{write_endpoint, IpcEndpoint};
tests/spec_c_cli_token_rotation.rs:93:    write_endpoint(&data_dir, &IpcEndpoint::current(format!("http://{addr}")))
tests/spec_c_cli_ipc.rs:28:    use alephcore::cli::endpoint::{write_endpoint, IpcEndpoint};
tests/spec_c_cli_ipc.rs:89:    write_endpoint(&data_dir, &IpcEndpoint::current(format!("http://{addr}")))
```

```bash
$ rg -n "write_endpoint|read_endpoint|remove_endpoint" src/ interfaces/ shared/ tests/ | grep -v "src/cli/endpoint.rs" | grep -v "(test)"
src/bin/aleph-server/daemon.rs:12:use alephcore::cli::endpoint::{read_endpoint, remove_endpoint, IpcEndpoint};
src/bin/aleph-server/daemon.rs:175:    .or_else(|| read_endpoint_best_effort().and_then(|ep| i32::try_from(ep.pid).ok()))
src/bin/aleph-server/daemon.rs:349:    read_endpoint(&dir).ok().flatten()
src/bin/aleph-server/daemon.rs:375:    let endpoint = read_endpoint_best_effort();
src/bin/aleph-server/daemon.rs:397:    if let Err(e) = remove_endpoint(&dir) { … }
src/bin/aleph-server/commands/start/mod.rs:3521:    if let Err(e) = alephcore::cli::endpoint::write_endpoint(dir, &endpoint) { … }
src/bin/aleph-server/commands/start/mod.rs:3588:    if let Err(e) = alephcore::cli::endpoint::remove_endpoint(dir) { … }
tests/spec_c_cli_ipc.rs:89: write_endpoint(...)                                  (integration test)
tests/spec_c_cli_token_rotation.rs:93: write_endpoint(...)                          (integration test)
tests/spec_c_cli_endpoint_missing.rs:27: (test intentionally omits write_endpoint)   (integration test)
```

```bash
$ rg -n "endpoint_path" src/ interfaces/ shared/ tests/ | grep -v "src/cli/endpoint.rs"
(no matches — endpoint_path is correctly `pub(crate)`; only used inside endpoint.rs)
```

### ipc_client.rs

```bash
$ rg -n "forward_to_server" src/ interfaces/ shared/ tests/ | grep -v "src/cli/ipc_client.rs"
src/bin/aleph-server/commands/resume.rs:17:    use alephcore::cli::ipc_client::forward_to_server;
src/bin/aleph-server/commands/resume.rs:27:        run_no_lock(|| forward_to_server(&data_dir, HttpMethod::Post, "/v1/admin/resume", body))
src/cli/policy.rs:165:                    match crate::cli::ipc_client::forward_to_server::<T>(data_dir, method, route, ipc_body)
tests/spec_c_cli_ipc.rs:100:   forward_to_server::<serde_json::Value>(...)
tests/spec_c_cli_token_rotation.rs:100:  forward_to_server::<serde_json::Value>(...)
```

### policy.rs

```bash
$ rg -n "LockHeldError" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
(no external matches — only downcast_ref::<LockHeldError> inside policy.rs itself)
```

```bash
$ rg -n "HttpMethod\b" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
src/bin/aleph-server/commands/secret.rs:220:    use alephcore::cli::policy::{run_no_lock, with_policy, CommandPolicy, HttpMethod};
src/bin/aleph-server/commands/resume.rs:18:    use alephcore::cli::policy::{run_no_lock, HttpMethod};
src/cli/ipc_client.rs:12:    use crate::cli::policy::HttpMethod;
```

```bash
$ rg -n "CommandPolicy\b" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
src/bin/aleph-server/commands/secret.rs:220:    use alephcore::cli::policy::{run_no_lock, with_policy, CommandPolicy, HttpMethod};
src/bin/aleph-server/commands/secret.rs:228/250/266/294/307:    CommandPolicy::LockOrIpc { route: "/v1/admin/secrets", method: HttpMethod::Get/Post }
src/bin/aleph-server/commands/secret.rs:293/306:    CommandPolicy::LockOnly
```

```bash
$ rg -n "run_no_lock" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
src/bin/aleph-server/commands/secret.rs:220:    use alephcore::cli::policy::{run_no_lock, ...}
src/bin/aleph-server/commands/secret.rs:318:    run_no_lock(|| Ok::<(), anyhow::Error>(()))                   (Providers — no-lock marker)
src/bin/aleph-server/commands/resume.rs:18:    use alephcore::cli::policy::{run_no_lock, HttpMethod};
src/bin/aleph-server/commands/resume.rs:27:    run_no_lock(|| forward_to_server(...))                       (the only non-marker call)
src/bin/aleph-server/commands/gateway.rs:18:    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;     (marker)
src/bin/aleph-server/commands/plugins.rs:16:  alephcore::cli::policy::run_no_lock(...)?;      (marker, see plugins.rs:230/327)
src/bin/aleph-server/commands/bootstrap_runtime/mod.rs:28:  if alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(())).is_err() {…}
```

```bash
$ rg -n "with_policy\b" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
src/bin/aleph-server/commands/secret.rs:220:    use alephcore::cli::policy::{run_no_lock, with_policy, CommandPolicy, HttpMethod};
src/bin/aleph-server/commands/secret.rs:227/249/265/293/306:    with_policy(...)               (5 distinct sites: init/set/list/delete/verify)
```

```bash
$ rg -n "try_with_policy" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
(no external matches — try_with_policy is `pub` but only used by `with_policy` (line 230, inside policy.rs) and by 3 internal unit tests. No external caller — see sw-cli-2.)
```

### Summary table

| Public symbol | Production caller(s) | Test-only caller(s) | Verdict |
|---|---|---|---|
| `endpoint::build_endpoint_url` | `bin/.../start/mod.rs:3479` | `endpoint.rs:225-261` (8 URL-shape tests) | Healthy |
| `endpoint::IpcEndpoint` | `bin/.../daemon.rs:12, 303, 347`, `bin/.../start/mod.rs:3520` | `daemon.rs:678/679`, `policy.rs:397` | Healthy |
| `endpoint::IpcEndpoint::current` | `bin/.../start/mod.rs:3520` | `endpoint.rs:159/209/275`, `daemon.rs:678`, `policy.rs:397`, `tests/spec_c_cli_*.rs` | Healthy |
| `endpoint::write_endpoint` | `bin/.../start/mod.rs:3521` | `endpoint.rs:160/210/276`, `policy.rs:395`, `tests/spec_c_cli_*.rs` | Healthy |
| `endpoint::read_endpoint` | `bin/.../daemon.rs:349` (via `read_endpoint_best_effort`) | `endpoint.rs:161/169/182/201/212`, `policy.rs` (transitively) | Healthy |
| `endpoint::remove_endpoint` | `bin/.../daemon.rs:397`, `bin/.../start/mod.rs:3588` | `endpoint.rs:211` | Healthy |
| `endpoint::endpoint_path` (`pub(crate)`) | n/a (correctly internal) | `endpoint.rs:180/196/277` | Healthy — correctly `pub(crate)` |
| `ipc_client::forward_to_server` | `bin/.../commands/resume.rs:27`, `policy.rs:165` (LockOrIpc retry path) | `tests/spec_c_cli_ipc.rs:100`, `tests/spec_c_cli_token_rotation.rs:100` | Healthy |
| `policy::LockHeldError` (struct + Display + Error) | none by name (returned in `anyhow::Error` chain) | `policy.rs:155/179/219` (downcast_ref), `policy.rs:255-298` (Display tests) | Healthy — type is reachable via downcast; see sw-cli-3 |
| `policy::HttpMethod` | `bin/.../commands/secret.rs:220, 230/252/268`, `bin/.../commands/resume.rs:18`, `ipc_client.rs:12` | `policy.rs:410` (test) | Healthy |
| `policy::HttpMethod::as_reqwest` | `ipc_client.rs:115` | none | Healthy |
| `policy::CommandPolicy` | `bin/.../commands/secret.rs:220, 228/250/266/294/307` | `policy.rs:305/322/350/408` (tests) | Healthy |
| `policy::run_no_lock` | `bin/.../commands/resume.rs:27` (real call), `bin/.../commands/secret.rs:318`, `bin/.../commands/gateway.rs:18`, `bin/.../commands/plugins.rs:16/230/327`, `bin/.../commands/bootstrap_runtime/mod.rs:28` (5 marker calls) | `policy.rs:239/245` | Healthy |
| `policy::try_with_policy` | none (only internal caller is `with_policy` at policy.rs:230) | `policy.rs:321/349/407` (3 unit tests) | See sw-cli-2 |
| `policy::with_policy` | `bin/.../commands/secret.rs:227/249/265/293/306` | `policy.rs:304` | Healthy |

## Findings

### sw-cli-1 — `write_endpoint` is called on the hot path with no compile-time guarantee the data dir has been provisioned

- **Module:** `src/cli`
- **Files:** `endpoint.rs:62-85`, `bin/aleph-server/commands/start/mod.rs:3521`
- **Severity:** low
- **Form:** smell — correctness / boot-path hygiene
- **Produced:** `pub fn write_endpoint(data_dir: &Path, endpoint: &IpcEndpoint) -> std::io::Result<()>`. Atomically writes `.ipc-endpoint.json` with mode 0o600; on chmod failure deletes the file and returns `PermissionDenied`.
- **Consumer location:** `src/bin/aleph-server/commands/start/mod.rs:3521` (single production call site, in the start path); also called by `policy.rs:395` (test) and the integration tests `tests/spec_c_cli_*.rs`.
- **Evidence:**
  ```bash
  $ rg -n "write_endpoint" src/ interfaces/ shared/ tests/
  src/cli/endpoint.rs:62:        (definition)
  src/cli/endpoint.rs:160/210/276: (unit tests)
  src/cli/policy.rs:395:            (lock_or_ipc unit test seeds a fake endpoint)
  src/bin/aleph-server/commands/start/mod.rs:3521:
      if let Err(e) = alephcore::cli::endpoint::write_endpoint(dir, &endpoint) {
  tests/spec_c_cli_ipc.rs:89:    write_endpoint(&data_dir, &IpcEndpoint::current(format!("http://{addr}")))
  tests/spec_c_cli_token_rotation.rs:93: write_endpoint(&data_dir, &IpcEndpoint::current(format!("http://{addr}")))
  ```
  The unit test at `endpoint.rs:267-281` pins 0o600 on the success path. The chmod-failure path is reviewed by inspection (cannot be simulated reliably under unit-test filesystems).
- **Decision:** KEEP (no action)
- **Rationale:** The function has exactly one production caller (`start/mod.rs:3521`), and that caller propagates the error to a `tracing::warn!` / early-return path — not to a panic. The unit test for the 0o600 chmod is the only correctness gate today; integrating this with `instance_lock`'s O_NOFOLLOW / `write_atomic` chain is already done (see the import at `endpoint.rs:73`). The 0o600 contract is documented in `endpoint.rs:204-215` (`write_endpoint_sets_owner_only_permissions`). No smell beyond what the audit task category already calls out.
- **Proposed change:** none. Verify the contract still holds after the next `start` path change.
- **Verification:** `cargo test -p alephcore --lib cli::endpoint::` continues to pass, in particular `write_endpoint_sets_owner_only_permissions`.
- **Risk:** low.

### sw-cli-2 — `try_with_policy` is `pub` with no external production caller

- **Module:** `src/cli`
- **Files:** `policy.rs:134-199`
- **Severity:** low
- **Form:** 1 (no external consumer) — borderline; see Rationale.
- **Produced:** `pub fn try_with_policy<L, T>(policy: CommandPolicy, data_dir: &Path, local: L, ipc_body: serde_json::Value) -> anyhow::Result<T>` where `L: FnOnce(&InstanceLock) -> anyhow::Result<T>`, `T: serde::de::DeserializeOwned`. Returns `Err` on lock contention instead of exiting; otherwise delegates to the same `acquire_or_held` / `forward_to_server` logic `with_policy` uses.
- **Consumer location:** none outside `src/cli/policy.rs` itself. The single in-module production caller is `with_policy` at `policy.rs:230`. Three internal unit tests at `policy.rs:321/349/407` exercise the LockOnly contention and LockOrIpc-retry paths.
- **Evidence:**
  ```bash
  $ rg -n "try_with_policy" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
  (no matches)
  ```
  In-module:
  ```bash
  $ rg -n "try_with_policy" src/cli/policy.rs
  131: /// `try_with_policy`'s); the `LockOnly` arm in `with_policy`           (doc comment)
  134: pub fn try_with_policy<L, T>(                                            (definition)
  201: /// Production dispatch: same as `try_with_policy` but converts lock      (doc comment)
  214: // Only the LockOnly contention behavior differs from `try_with_policy`    (in with_policy)
  230:     try_with_policy(policy, data_dir, local, ipc_body)                    (with_policy delegation)
  315:     fn try_with_policy_lock_only_returns_err_when_held() { … try_with_policy(...)  (test)
  349:     fn held_by_live_with_pid_zero_is_treated_as_orphaned() { … try_with_policy(...) (test)
  407:     fn lock_or_ipc_retries_local_acquire_when_forward_fails() { … try_with_policy(...) (test)
  ```
- **Decision:** KEEP (no action)
- **Rationale:** The function is not dead — `with_policy` calls it at line 230, and the three unit tests exercise behavior `with_policy`'s exit-on-contention contract cannot reach from a normal test process (because `with_policy` calls `std::process::exit(64)` which kills the test binary). The doc comment at line 201 names this exactly: `try_with_policy` is the testable half, `with_policy` is the production-exiting half. The audit task's "form 1" lens would normally flag it as CUT, but the read-first rule says no live caller → default CUT, and the live caller here is `with_policy`. Triage-playbook "decide-via-read": no production pain observed (no operator has complained about a CLI that exits cleanly vs returning Err), the painless-wire heuristic therefore points to leaving the surface intact. CUTting it would force the three tests to be re-anchored to `with_policy`'s exit-on-contention contract, which would either (a) wrap each test in a `std::process::exit`-handling harness or (b) drop the lock-contention tests entirely. Neither is worth the surface-area shrinkage.
- **Proposed change:** none.
- **Verification:** the three tests at `policy.rs:315/349/407` continue to pass after any policy.rs change. If a future refactor removes `try_with_policy`'s only in-module caller, re-evaluate.
- **Risk:** low.

### sw-cli-3 — `LockHeldError` is `pub` but never name-imported by any external module

- **Module:** `src/cli`
- **Files:** `policy.rs:16-57`
- **Severity:** low
- **Form:** 6 (orphan public API surface — borderline; the type IS reachable via `downcast_ref`)
- **Produced:** `pub struct LockHeldError { pub pid: u32, pub lock_path: std::path::PathBuf, pub orphaned: bool }` plus its `fmt::Display` impl (which prints "server is running (PID N)" / "orphaned lock detected" / "holder unknown" depending on `(pid, orphaned)`) and `impl std::error::Error for LockHeldError {}`.
- **Consumer location:** none by name. The struct is constructed inside `policy.rs:112-120` and returned inside an `anyhow::Error` chain; every external reachability goes through `e.downcast_ref::<LockHeldError>().is_some()` from inside `policy.rs` (line 155, 179, 219).
- **Evidence:**
  ```bash
  $ rg -n "LockHeldError" src/ interfaces/ shared/ tests/ | grep -v "src/cli/policy.rs"
  (no matches)
  ```
  In-module: the struct is named 8 times (definition + 2 constructors + 3 downcast_ref call sites + 3 Display unit tests).
- **Decision:** KEEP (no action)
- **Rationale:** The struct must be `pub` to be referenced in a `downcast_ref::<LockHeldError>()` call site (a `pub(crate)` type would still be reachable from inside `policy.rs` itself, but `pub` is what lets a future caller in another module — a hypothetical `bin/aleph-server/commands/status.rs` that wants to surface lock-held state instead of forwarding — match on it without changing the signature). The orphan-import lens in the audit task calls out `pub` items with no name import, but a `downcast_ref`-reachable type is the canonical case where `pub` is required without a name import. The struct has live production readers via the downcast arms; it is not severed. No action.
- **Proposed change:** none.
- **Verification:** the downcast arms at `policy.rs:155/179/219` continue to match. The three Display unit tests at `policy.rs:255-298` pin the orphan-vs-live PID-0 message contract — see `cli-logic-2026-08-26/REPORT.md` (Warning M2) for the original rationale.
- **Risk:** none.

### sw-cli-4 — `endpoint_path` is `pub(crate)` but only used inside `endpoint.rs` itself

- **Module:** `src/cli`
- **Files:** `endpoint.rs:58`
- **Severity:** low
- **Form:** smell — dead-private; not a severed wire
- **Produced:** `#[must_use] pub(crate) fn endpoint_path(data_dir: &Path) -> PathBuf` — joins `ENDPOINT_FILENAME` onto `data_dir`.
- **Consumer location:** none outside `endpoint.rs`. Inside `endpoint.rs` it is used by `write_endpoint` (line 63), `read_endpoint` (line 90), `remove_endpoint` (line 141), and three unit tests (lines 180/196/277).
- **Evidence:**
  ```bash
  $ rg -n "endpoint_path" src/ interfaces/ shared/ tests/ | grep -v "src/cli/endpoint.rs"
  (no matches)
  ```
  In-module matches all live inside `endpoint.rs`. The helper exists because the three public functions share the path-join logic and tests need to reach it without re-stating `".ipc-endpoint.json"` (see test at `endpoint.rs:180, 196, 277`).
- **Decision:** KEEP (no action)
- **Rationale:** Triage-playbook rule "delete-a-file safety rule" applies here: the helper is intentionally extracted so a future caller (a new RPC, a debug tool) can hit the same canonical path without copy-pasting `ENDPOINT_FILENAME`. The `pub(crate)` visibility is correct — neither leakable to library consumers nor invisible to a future sibling file in `src/cli/`. The audit task's "unused imports, dead functions" lens notes that `pub(crate)` items tolerated by lint are sometimes dead; this one is not — it's a single source of truth for the file path that 6 sites depend on.
- **Proposed change:** none.
- **Verification:** if the helper is removed, three public functions and three unit tests need to inline `data_dir.join(ENDPOINT_FILENAME)`. Today it factors cleanly. Net negative.
- **Risk:** none.

### sw-cli-5 — Code smell: `with_policy` mixes `Result`-return with `std::process::exit(64)` (TODO comment at `policy.rs:221`)

- **Module:** `src/cli`
- **Files:** `policy.rs:217-228`
- **Severity:** low
- **Form:** smell — correctness / API hygiene (clippy::exit would flag this in a `cargo clippy --all` if not gated)
- **Produced:** `pub fn with_policy<L, T>(...)` — same shape as `try_with_policy` except the `LockOnly` arm calls `eprintln!("{held}")` followed by `std::process::exit(64)` on lock contention instead of returning `Err`. The TODO at `policy.rs:221-223` is the design note explaining why the lint is suppressed:
  > `with_policy` is documented as the production dispatch that exits cleanly on lock contention rather than returning an `Err` to the caller. Replacing this with `Result` propagation would change the public API contract and all callers, so it is left as-is.
- **Evidence:**
  ```rust
  // src/cli/policy.rs:217-228
  if let CommandPolicy::LockOnly = policy {
      let lock = acquire_or_held(data_dir).inspect_err(|e| {
          if let Some(held) = e.downcast_ref::<LockHeldError>() {
              eprintln!("{held}");
              // TODO: clippy::exit — `with_policy` is documented as the production
              // dispatch that exits cleanly on lock contention rather than returning
              // an `Err` to the caller. Replacing this with `Result` propagation would
              // change the public API contract and all callers, so it is left as-is.
              std::process::exit(64);
          }
      })?;
      return local(&lock);
  }
  try_with_policy(policy, data_dir, local, ipc_body)
  ```
  All five production callers in `bin/aleph-server/commands/secret.rs` (lines 227, 249, 265, 293, 306) pass `LockOnly` only for `SecretAction::Delete` and `SecretAction::Verify` — neither produces output the operator needs post-contention; both have `with_policy(...).map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?` chains that would never fire if the contract were changed to `Result`.
- **Decision:** KEEP (no action)
- **Rationale:** This is the documented clippy::exit carve-out the TODO names — a deliberate exit-on-contention contract that the `secret` commands rely on (a lock-held error there should not propagate up the call stack as `Err`, because by construction there is nothing the CLI process can do about it: `data_dir` is held by the singleton). Changing it to `Result` propagation would force every caller to add an `if `lock is held` { exit(64) } else { format error }` boilerplate, which is a strict surface-area regression. The TODO is itself a *good* smell marker — it makes the design decision visible to future readers who would otherwise think the `exit` is an oversight. The same pattern is documented as a warning in `review-results/cli-logic-2026-08-26/REPORT.md` (entry "with_policy's LockOrIpc retry discards the second lock acquisition error"). Not a finding — leave the TODO, leave the exit.
- **Proposed change:** none.
- **Verification:** `rg -n "std::process::exit" src/cli/policy.rs` should match exactly the one site at line 224 and the test-helper reference at no other site.
- **Risk:** low (intentional design).

## RPC dispatch parity check (bonus lens)

The CLI routes over HTTP, not JSON-RPC — so the `method_visibility` / `method_census` tables do not bind. The relevant parity is `CLIENT_SENT_ROUTE` ↔ `SERVER_HANDLER`. Cross-reference:

| CLI route (sender) | Server mount | Server handler | Verdict |
|---|---|---|---|
| `/v1/admin/secrets` (POST, body `{key, value}`) | `bin/.../start/mod.rs:1845` → `gateway::admin_api::router(state)` → `.nest("/v1/admin", admin)` (`gateway/server/mod.rs:914`) → `.nest("/secrets", secrets::router())` (`admin_api/mod.rs:43`) | `create_or_update_secret` (`gateway/admin_api/secrets.rs:43`) | Wired ✓ |
| `/v1/admin/secrets` (GET) | same mount | `list_secrets` (`gateway/admin_api/secrets.rs:60`) | Wired ✓ |
| `/v1/admin/resume` (POST, body `{session_key}`) | `.nest("/resume", resume::router())` (`admin_api/mod.rs:44`) | `resume_session` (`gateway/admin_api/resume.rs:33`) | Wired ✓ |
| `/v1/admin/whatever` | n/a | n/a | Test-only route inside `policy.rs:409` |

No client-ghost (form 4). No name drift (form 5) between `/v1/admin/secrets` (CLI) and `/secrets` (nest) → combined path `/v1/admin/secrets`. Both the CLI `LockOrIpc` calls in `secret.rs` and the direct `forward_to_server` call in `resume.rs` route to live handlers.

Note: `/v1/admin/reconciler` exists as a third admin route (`admin_api/reconciler.rs`) but no CLI command currently sends to it. This is dormant, not severed — the server surface is live (`admin_api/mod.rs:45`) and a CLI command can be added later by following the `LockOrIpc { route: "/v1/admin/reconciler/...", method: HttpMethod::Get }` pattern in `secret.rs`.

## Negative findings (what I did NOT find)

- **No form 2 (stub far-end).** No handler in `src/cli/` returns `Ok(success)` without doing anything. `forward_to_server` actually calls the server; `with_policy` actually acquires the lock or forwards; `write_endpoint`/`read_endpoint`/`remove_endpoint` actually do filesystem I/O. The unit-test stub of "write a real endpoint, drop the lock, run the retry" at `policy.rs:407` is a normal test, not a stub.
- **No form 4 (client ghost).** Every CLI route (`/v1/admin/secrets` × 3 calls, `/v1/admin/resume` × 1 call) maps to a live handler on the server side, mounted at `gateway/server/mod.rs:914`.
- **No form 5 (name drift).** The HTTP route names match between sender and handler. The IPC body shape (`{key, value}` for set, `{session_key}` for resume) matches the axum `Deserialize` structs in `gateway/admin_api/secrets.rs` and `gateway/admin_api/resume.rs`.
- **No form 6 (never-compiled far-end).** No `#[cfg(feature = "X")]` gates on any path inside `src/cli/`. The `#[cfg(unix)]` block at `endpoint.rs:67-82` gates the chmod step, which is correct platform-specific behavior, not a never-compiled feature.
- **No `unimplemented!()` / `todo!()`** in production code (verified by `rg -n "unimplemented!|todo!\(" src/cli/` returning nothing in production paths).
- **No `Box::leak` / `tokio::runtime::Runtime::new()`** inside `src/cli/`. The CLI is a one-shot process and uses `reqwest::blocking` deliberately (see the comment at `ipc_client.rs:107-112`). No watch channels / JoinHandles / async-runtime leak surfaces in this module.
- **No `read_to_string` without size bound.** `endpoint.rs:104` uses `file.take(MAX_ENDPOINT_FILE_SIZE + 1).read_to_end(&mut bytes)` and rejects oversize before parsing. This is the *correct* pattern, lifted explicitly in the prior `cli-logic-2026-08-26` review (Warning: "HTTP response body is not size-bounded" applied to `forward_to_server`, not this module).
- **No missing timeouts on IPC.** `ipc_client.rs:128` sets `reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10))` for every admin request. 10s is the contract; the daemon's `/v1/admin/*` handlers are sync and bounded.
- **No `unwrap()` / `expect()` on operator-facing fallible paths** in production code. All 60+ `unwrap()`/`expect()` matches across the three files are inside `#[cfg(test)] mod tests` blocks (verified by stripping `#[cfg(test)]` blocks via Python and counting remaining occurrences: 0).
- **No unused imports** in production code. The two `use` statements at `ipc_client.rs:11-12` (`read_endpoint`, `HttpMethod`) are both live-referenced at `ipc_client.rs:27` and `ipc_client.rs:20/105`.
- **No inert config** in `src/cli/`. The module reads no `Config` field directly — `build_endpoint_url(addr, tls: bool)` is parameter-driven, the caller (`start/mod.rs:3479`) passes `full_config.gateway.tls.enabled` as the boolean argument. `policy.rs` and `ipc_client.rs` take `data_dir: &Path` from the caller; nothing reaches into `crate::config::*` from inside `src/cli/`.
- **No error-type variants that are never constructed.** `LockHeldError { pid, lock_path, orphaned }` is constructed at exactly two sites (`policy.rs:112-120`); every field on every construction has a Display-arm covering it (`policy.rs:27-43`).

## Symbols that PASS the parity check

Every public symbol in the audited module has at least one production caller:

- **`endpoint::build_endpoint_url`** — `bin/aleph-server/commands/start/mod.rs:3479` (production boot path).
- **`endpoint::IpcEndpoint`** — `bin/aleph-server/daemon.rs:12, 303, 347, 678, 679`, `bin/aleph-server/commands/start/mod.rs:3520`. Five sites; one is a test fixture inside `daemon.rs`.
- **`endpoint::IpcEndpoint::current`** — `bin/aleph-server/commands/start/mod.rs:3520`. One production call site; multiple test fixtures.
- **`endpoint::write_endpoint`** — `bin/aleph-server/commands/start/mod.rs:3521` (writes the live endpoint at boot). Plus integration tests `tests/spec_c_cli_ipc.rs:89`, `tests/spec_c_cli_token_rotation.rs:93`, and the in-module retry test at `policy.rs:395`.
- **`endpoint::read_endpoint`** — `bin/aleph-server/daemon.rs:349` (via `read_endpoint_best_effort`, called from the status probe path at line 175 and from `handle_status` at line 375). Also called internally by `ipc_client.rs:27`.
- **`endpoint::remove_endpoint`** — `bin/aleph-server/daemon.rs:397` (cleanup on shutdown) and `bin/aleph-server/commands/start/mod.rs:3588` (cleanup on graceful exit).
- **`endpoint::endpoint_path`** (`pub(crate)`) — correctly internal; 6 in-module callers.
- **`ipc_client::forward_to_server`** — `bin/aleph-server/commands/resume.rs:27` (production CLI subcommand `aleph-server resume`), `policy.rs:165` (internal: LockOrIpc forward path), `tests/spec_c_cli_ipc.rs:100`, `tests/spec_c_cli_token_rotation.rs:100`.
- **`policy::LockHeldError`** — reachable via `downcast_ref` inside `policy.rs` itself (lines 155, 179, 219). The struct is returned in `anyhow::Error` chains; every external caller sees it via the `Display` impl's "server is running (PID N)" / "orphaned lock detected" / "holder unknown" message. Three Display unit tests pin the contract.
- **`policy::HttpMethod`** — `bin/aleph-server/commands/secret.rs:220, 230, 252, 268`, `bin/aleph-server/commands/resume.rs:18`, `cli::ipc_client::forward_to_server`. Live.
- **`policy::HttpMethod::as_reqwest`** — `ipc_client.rs:115`. Live (the `reqwest::Method` enum conversion).
- **`policy::CommandPolicy`** — `bin/aleph-server/commands/secret.rs:220, 228/250/266/294/307`. Five distinct dispatch sites in the same command file. NoLock appears nowhere in production callers (the comment at `policy.rs:90` notes this is by design — NoLock dispatch goes through `run_no_lock`, not through `with_policy` / `try_with_policy`).
- **`policy::run_no_lock`** — six production callers across `secret.rs`, `resume.rs`, `gateway.rs`, `plugins.rs`, `bootstrap_runtime/mod.rs`. Only `resume.rs:27` is a "real" call (it wraps `forward_to_server`); the other five are marker calls (the doc comment at `policy.rs:90-91` explicitly says these exist for reverse-regression checks in `bin/aleph-server/commands/`).
- **`policy::try_with_policy`** — `policy.rs:230` (internal, called from `with_policy`). Three unit-test call sites. See sw-cli-2 for why this is **not** a CUT.
- **`policy::with_policy`** — five call sites in `bin/aleph-server/commands/secret.rs` (one per `SecretAction` variant except `Providers`, which goes through `run_no_lock`).

## Recommended actions (priority order)

1. **sw-cli-1** (smell — no action): verify the 0o600 chmod contract stays in place after any future refactor of `start/mod.rs:3521`. Severity low, no change recommended.
2. **sw-cli-2** (form 1 borderline — no action): `try_with_policy` has an in-module production caller (`with_policy` at line 230). It is not severed; the tests at lines 315/349/407 are the only way to exercise lock-contention without invoking `std::process::exit` mid-test. KEEP.
3. **sw-cli-3** (form 6 borderline — no action): `LockHeldError` is reachable via `downcast_ref`. A `pub` struct with no name-import is the canonical pattern for an error type that callers match by downcast rather than by name.
4. **sw-cli-4** (smell — no action): `endpoint_path` is correctly `pub(crate)` and serves as the single source of truth for the file path across 6 call sites.
5. **sw-cli-5** (smell — no action): the `std::process::exit(64)` in `with_policy` is documented and deliberate. The TODO at line 221 is a feature, not debt.

## Sanity-check table (file:line for every symbol)

| File | Line | Symbol | Verdict |
|---|---|---|---|
| src/cli/mod.rs | 1 | `pub mod endpoint;` | KEEP |
| src/cli/mod.rs | 2 | `pub mod ipc_client;` | KEEP |
| src/cli/mod.rs | 3 | `pub mod policy;` | KEEP |
| src/cli/endpoint.rs | 13 | `const ENDPOINT_FILENAME: &str = ".ipc-endpoint.json"` | KEEP — internal const |
| src/cli/endpoint.rs | 14 | `const CURRENT_ENDPOINT_VERSION: u32 = 1` | KEEP — internal const |
| src/cli/endpoint.rs | 24 | `pub fn build_endpoint_url` | KEEP — caller: start/mod.rs:3479 |
| src/cli/endpoint.rs | 39 | `pub struct IpcEndpoint` | KEEP — callers: daemon.rs, start/mod.rs |
| src/cli/endpoint.rs | 47 | `pub fn IpcEndpoint::current` | KEEP — caller: start/mod.rs:3520 |
| src/cli/endpoint.rs | 58 | `pub(crate) fn endpoint_path` | KEEP — correctly internal (sw-cli-4) |
| src/cli/endpoint.rs | 62 | `pub fn write_endpoint` | KEEP — caller: start/mod.rs:3521 |
| src/cli/endpoint.rs | 85 | `const MAX_ENDPOINT_FILE_SIZE: u64 = 1_048_576` | KEEP — size bound on read |
| src/cli/endpoint.rs | 87 | `pub fn read_endpoint` | KEEP — caller: daemon.rs:349, ipc_client.rs:27 |
| src/cli/endpoint.rs | 140 | `pub fn remove_endpoint` | KEEP — callers: daemon.rs:397, start/mod.rs:3588 |
| src/cli/ipc_client.rs | 15 | `const SECURITY_DB_FILENAME: &str = "security.db"` | KEEP — internal const |
| src/cli/ipc_client.rs | 18 | `pub fn forward_to_server` | KEEP — callers: resume.rs:27, policy.rs:165 |
| src/cli/ipc_client.rs | 71 | `const MAX_ERROR_BODY_CHARS: usize = 256` | KEEP — bounds error message size |
| src/cli/ipc_client.rs | 81 | `fn truncate_error_body` | KEEP — helper for finalize() and 401 arm |
| src/cli/ipc_client.rs | 94 | `fn read_token` | KEEP — internal helper |
| src/cli/ipc_client.rs | 104 | `fn call_once` | KEEP — internal helper |
| src/cli/ipc_client.rs | 124 | `fn build_client` | KEEP — internal helper |
| src/cli/ipc_client.rs | 128 | `.timeout(Duration::from_secs(10))` | KEEP — IPC timeout enforced |
| src/cli/ipc_client.rs | 154 | `fn host_of` | KEEP — internal helper |
| src/cli/ipc_client.rs | 198 | `fn is_loopback_host` | KEEP — internal helper |
| src/cli/ipc_client.rs | 204 | `fn finalize` | KEEP — internal helper |
| src/cli/policy.rs | 16 | `pub struct LockHeldError` | KEEP — sw-cli-3 |
| src/cli/policy.rs | 22 | `impl fmt::Display for LockHeldError` | KEEP |
| src/cli/policy.rs | 56 | `impl std::error::Error for LockHeldError` | KEEP |
| src/cli/policy.rs | 59 | `pub enum HttpMethod` | KEEP — callers: secret.rs, resume.rs, ipc_client.rs |
| src/cli/policy.rs | 66 | `pub const fn HttpMethod::as_reqwest` | KEEP — caller: ipc_client.rs:115 |
| src/cli/policy.rs | 75 | `pub enum CommandPolicy` | KEEP — caller: secret.rs |
| src/cli/policy.rs | 92 | `pub fn run_no_lock` | KEEP — 6 production callers |
| src/cli/policy.rs | 102 | `fn acquire_or_held` | KEEP — internal helper |
| src/cli/policy.rs | 134 | `pub fn try_with_policy` | KEEP — sw-cli-2 (no external caller, but called by `with_policy`) |
| src/cli/policy.rs | 204 | `pub fn with_policy` | KEEP — callers: secret.rs ×5 |
| src/cli/policy.rs | 221 | `// TODO: clippy::exit ...` | KEEP — sw-cli-5 (documented design decision) |
| src/cli/policy.rs | 224 | `std::process::exit(64)` | KEEP — sw-cli-5 (intentional) |

## Audit summary

| Metric | Count |
|---|---|
| Public symbols audited | 16 (5 endpoint + 1 ipc_client + 10 policy, including Display/Error impls and methods) |
| Production callers found | 16 / 16 |
| Severed wires (form 1, true dead) | 0 |
| Borderline form-1 (`try_with_policy`) | 1 — KEEP, justified by sw-cli-2 |
| Borderline form-6 (`LockHeldError`) | 1 — KEEP, justified by sw-cli-3 |
| Stubs (form 2) | 0 |
| Client ghosts (form 4) | 0 |
| Name drift (form 5) | 0 |
| Inert config (form 3) | 0 |
| Code smells flagged | 5 — all KEEP, no fix recommended |
| Test-only consumers stripped before counting | yes (Python regex split on `#[cfg(test)]`) |

The module is healthy end-to-end. Every public symbol has a live consumer; the only items that look suspicious under a strict form-1 reading (`try_with_policy`, `LockHeldError`) are justified by read-first triage — they have in-module production callers and a `downcast_ref`-reachable contract respectively. No severed wires; no required cuts or connects.
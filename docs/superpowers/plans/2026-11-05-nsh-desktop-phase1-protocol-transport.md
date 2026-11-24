# NSH Desktop Phase 1 — Protocol 传输层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish `desktop/nsh-shell` → `shared/client` → `aleph-server` connection pipeline: daemon supervisor, `system.hello` handshake, event subscription, session list RPC — no business UI.

**Architecture:** NSH AppKit event loop on main thread; tokio multi-thread runtime on a background thread; the two communicate via `tokio::sync::mpsc`. The daemon supervisor (ported from `desktop/shell/src/daemon.rs`) ensures `aleph-server` is up on `127.0.0.1:18790`. The connection layer uses `AlephClient::connect` (which already does the `connect` auth handshake) then calls `system.hello`, `events.subscribe`, `sessions.list`. A new `system.hello` server handler is added; it is protocol-layer only (no auth, no gate) and coexists with the existing `connect` auth handshake.

**Tech Stack:** Rust, tokio 1.53.3, nsh v0.4.0 (path dep at commit `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9`), aleph-client, aleph-protocol, serde_json, uuid.

**Spec:** `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md` · `docs/research/NSH_PROTOCOL_V1.md` · `docs/research/NSH_ADR-001~009.md` · `desktop/nsh-spike/FINDINGS.md`

## Global Constraints

- **NSH path dep:** `nsh = { path = "/Volumes/TBU4/Github/nsh", rev = "2ab3b6bc27f151afd5d932c63746a8b4b15a00e9" }` — no crates.io, no following head.
- **Daemon port:** `127.0.0.1:18790`, ready probe `/ready`, ready timeout 40 s, poll interval 400 ms (from `desktop/shell/src/daemon.rs`).
- **`client_name`:** `"aleph-nsh-desktop"` in every `system.hello` call (Protocol V1 §4.2, design doc Phase 1).
- **Protocol version:** `major: 1, minor: 0` in handshake params.
- **Crate name:** `aleph-nsh-shell` (matches workspace convention: `aleph-` prefix).
- **Zero business logic in `desktop/nsh-shell`:** only OS integration + daemon supervisor + connection plumbing (R2/R10).
- **macOS only this phase:** `#[cfg(target_os = "macos")]` gate the NSH code path; non-mac returns `ExitCode::SUCCESS` (same as spike).
- **`system.hello` is not a gate:** Phase 1 server accepts other RPCs whether or not `system.hello` was called. The "server rejects business RPC before hello" rule (Protocol V1 §4.2) is a Phase 2+ concern.
- **Commit prefix:** `feat(nsh):` for nsh-shell work, `feat(server):` for the hello handler.

## Review Focus

Five input classes the spec implies but no task's happy path exercises. Each gets a test pinned to its owning task.

1. **`system.hello` with `protocol.major` ≠ 1** — server should return an error result, not a success with a mismatched version. (Protocol V1 §18: "主版本不兼容".) → Task 1
2. **`system.hello` with an unrecognized `features` entry** — server should echo only the features it knows, not reject the whole handshake. (Protocol V1 §4.2: server returns narrowed `features`.) → Task 1
3. **Daemon binary not found** — `resolve_daemon_binary()` returns `None`; supervisor should report a clear error, not panic or spawn a null process. (From `daemon.rs` `stop_daemon` path.) → Task 3
4. **Port 18790 held by a foreign (non-aleph-server) process** — supervisor must fail fast with an actionable message, not burn the ready timeout polling a process that will never answer `/ready`. (`daemon.rs` `PortOccupant::Foreign`.) → Task 3
5. **Connection drops after `events.subscribe` succeeds** — reconnect must re-issue `connect` → `system.hello` → `events.subscribe` in that order; a re-subscribe without a re-handshake would attach to a session the server considers new. (`AlephClient::reconnect` re-does `connect`; `system.hello` + `events.subscribe` must ride along.) → Task 4

---

## File Structure

### New crate: `desktop/nsh-shell/`
| File | Responsibility |
|------|---------------|
| `Cargo.toml` | Crate manifest; NSH path dep, aleph-client, aleph-protocol, tokio, uuid. |
| `src/main.rs` | Entry point. `run_app(AppSource::Rust(Root))`, minimal status window. Wires supervisor + connection. |
| `src/supervisor.rs` | Daemon discovery, launch, `/ready` probe, exponential backoff. Ported from `desktop/shell/src/daemon.rs`. |
| `src/runtime.rs` | Tokio runtime background-thread bootstrap (from spike `start_gateway_probe` pattern). |
| `src/handshake.rs` | `system.hello` client-side call; builds `HelloParams`, parses `HelloResult`. |
| `src/connection.rs` | `AlephClient` lifecycle: connect → hello → subscribe → session.list. Reconnect loop. mpsc status channel to NSH thread. |

### Server side (protocol-layer addition only)
| File | Responsibility |
|------|---------------|
| `shared/protocol/src/system_hello.rs` | DTO: `HelloParams`, `HelloResult`, `ProtocolVersion`, `ClientHello`, `ServerHello`, `Feature`. Single source of truth for the wire shape. |
| `shared/protocol/src/lib.rs` | Add `pub mod system_hello;` |
| `src/gateway/handlers/system_hello.rs` | `handle(request: JsonRpcRequest) -> JsonRpcResponse`. Pure, no side effects, no auth, no gate. |
| `src/gateway/handlers/mod.rs` | Register `system.hello`. |

### Workspace
| File | Change |
|------|--------|
| `Cargo.toml` (root) | Add `desktop/nsh-shell` to `[workspace.members]`. |

**Builder call logged:** adding DTOs to `shared/protocol` (design doc §5.1 marks it "不变"). Rationale: the wire shape must have one source (Shape Index #1/#10 — two copies of the same shape cancel each other's safety). The "不变" guidance is about not disrupting existing types; adding a new self-contained module doesn't touch them.

---

### Task 1: `system.hello` DTO + server handler

**Files:**
- Create: `shared/protocol/src/system_hello.rs`
- Modify: `shared/protocol/src/lib.rs` (add `pub mod system_hello;`)
- Create: `src/gateway/handlers/system_hello.rs`
- Modify: `src/gateway/handlers/mod.rs` (register `system.hello`)

**Interfaces:**
- Consumes: `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcResponse::success` / `JsonRpcResponse::error` from `crate::gateway::protocol`; `env!("ALEPH_VERSION")` for server version.
- Produces:
  - `aleph_protocol::system_hello::{HelloParams, HelloResult, ProtocolVersion, ClientHello, ServerHello, Feature}` — serde types used by both server handler and Task 4's client.
  - `pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse` — same signature shape as `system_info::handle`.

- [ ] **Step 1: Write the failing tests for DTO serde round-trip**

Create `shared/protocol/src/system_hello.rs` with the type definitions and a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_params_round_trip() {
        let params = HelloParams {
            protocol: ProtocolVersion { major: 1, minor: 0 },
            client: ClientHello {
                name: "aleph-nsh-desktop".to_string(),
                version: "0.1.0".to_string(),
                platform: "macos".to_string(),
                ui_runtime: "nsh".to_string(),
            },
            features: vec![Feature::EventReplay, Feature::Snapshot],
        };
        let json = serde_json::to_value(&params).unwrap();
        let back: HelloParams = serde_json::from_value(json).unwrap();
        assert_eq!(back.protocol.major, 1);
        assert_eq!(back.client.name, "aleph-nsh-desktop");
        assert_eq!(back.features.len(), 2);
    }

    #[test]
    fn hello_result_round_trip() {
        let result = HelloResult {
            protocol: ProtocolVersion { major: 1, minor: 0 },
            server: ServerHello { name: "aleph-server".to_string(), version: "26.9.1".to_string() },
            connection_id: "conn-001".to_string(),
            features: vec![Feature::EventReplay],
            limits: HelloLimits { max_message_bytes: 8_388_608, max_inflight_requests: 128 },
        };
        let json = serde_json::to_value(&result).unwrap();
        let back: HelloResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.server.name, "aleph-server");
        assert_eq!(back.connection_id, "conn-001");
    }
}
```

`Feature` is an enum with `#[serde(rename_all = "snake_case")]` variants: `EventReplay`, `Snapshot`, `BinaryChannel`. Unknown variants deserialize to `Feature::Unknown(String)` (use `#[serde(other)]` is not compatible with a String-carrying variant; use a custom deserializer or `#[serde(untagged)]` fallback — the implementer picks the simplest that passes the test).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-protocol system_hello`
Expected: FAIL — module not found / types not defined.

- [ ] **Step 3: Implement the DTO types**

Define `ProtocolVersion { major: u32, minor: u32 }`, `ClientHello { name, version, platform, ui_runtime: String }`, `ServerHello { name, version: String }`, `HelloLimits { max_message_bytes: u64, max_inflight_requests: u32 }`, `Feature` (serde enum, unknown-tolerant), `HelloParams { protocol, client, features: Vec<Feature> }`, `HelloResult { protocol, server, connection_id, features, limits }`. Add `pub mod system_hello;` to `shared/protocol/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-protocol system_hello`
Expected: PASS.

- [ ] **Step 5: Write the failing test for the server handler**

Create `src/gateway/handlers/system_hello.rs` with the handler and a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;
    use serde_json::json;

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id("system.hello", Some(params), json!(1))
    }

    #[tokio::test]
    async fn happy_path_returns_server_version_and_echoed_protocol() {
        let params = json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": ["event_replay", "snapshot"]
        });
        let resp = handle(req(params)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocol"]["major"], 1);
        assert_eq!(result["server"]["name"], "aleph-server");
        assert!(result["connection_id"].as_str().unwrap().starts_with("conn-"));
        assert!(result["features"].as_array().unwrap().contains(&json!("event_replay")));
    }

    #[tokio::test]
    async fn major_version_mismatch_returns_error_not_success() {
        let params = json!({
            "protocol": {"major": 2, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": []
        });
        let resp = handle(req(params)).await;
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn unknown_feature_is_narrowed_not_rejected() {
        let params = json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": ["event_replay", "nonexistent_feature"]
        });
        let resp = handle(req(params)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let features = result["features"].as_array().unwrap();
        assert!(features.contains(&json!("event_replay")));
        assert!(!features.contains(&json!("nonexistent_feature")));
    }
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p aleph-server system_hello`
Expected: FAIL — handler not found.

- [ ] **Step 7: Implement `handle` in `src/gateway/handlers/system_hello.rs`**

Signature: `pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse`. Parse `request.params` as `HelloParams`. If `protocol.major != 1`, return `JsonRpcResponse::error(request.id, INVALID_PARAMS, "incompatible protocol major version")`. Otherwise build `HelloResult`: echo `protocol`, `server.name = "aleph-server"`, `server.version = env!("ALEPH_VERSION")`, `connection_id = format!("conn-{}", uuid::Uuid::new_v4())`, `features` = intersection of requested and known, `limits` = `{max_message_bytes: 8_388_608, max_inflight_requests: 128}`. Return `JsonRpcResponse::success(request.id, serde_json::to_value(&result))`.

Add `pub mod system_hello;` to `src/gateway/handlers/mod.rs` and register: `registry.register("system.hello", system_hello::handle);` next to the `system.info` registration.

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p aleph-server system_hello`
Expected: PASS.

- [ ] **Step 9: Verify the whole crate still builds and existing tests pass**

Run: `cargo check -p aleph-server && cargo test -p aleph-server system_info`
Expected: build OK, existing `system.info` tests still pass.

- [ ] **Step 10: Commit**

```bash
git add shared/protocol/src/system_hello.rs shared/protocol/src/lib.rs \
  src/gateway/handlers/system_hello.rs src/gateway/handlers/mod.rs
git commit -m "feat(server): add system.hello protocol handshake handler

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `nsh-shell` crate skeleton

**Files:**
- Create: `desktop/nsh-shell/Cargo.toml`
- Create: `desktop/nsh-shell/src/main.rs`
- Modify: `Cargo.toml` (root — add to `[workspace.members]`)

**Interfaces:**
- Consumes: NSH `nsh-platform-macos` (`run_app`, `AppOptions`, `AppSource`, `Root`), `nsh-core` (`#[component]`, `signal`, `spawn`, `text`).
- Produces: a binary `aleph-nsh-shell` that opens a window. Task 3 and Task 4 plug into `main.rs`.

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "aleph-nsh-shell"
version = "0.1.0"
edition = "2021"

[dependencies]
nsh-core = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-core" }
nsh-platform-macos = { path = "/Volumes/TBU4/Github/nsh/crates/nsh-platform-macos" }
aleph-client = { path = "../../shared/client" }
aleph-protocol = { path = "../../shared/protocol" }
tokio = { version = "1", features = ["rt-multi-thread", "sync", "macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }

# Match the workspace tokio version (spike verified 1.53.3 compatible)
```

Note: the implementer verifies the exact `nsh-core` / `nsh-platform-macos` crate names and the `Root::of::<App>()` import path against the spike's `Cargo.toml` at `desktop/nsh-spike/Cargo.toml` and copies the dep spellings verbatim. The NSH rev is pinned via the path; if the path clone moves, the implementer re-pins by checking out `2ab3b6bc27f151afd5d932c63746a8b4b15a00e9` in `/Volumes/TBU4/Github/nsh`.

- [ ] **Step 2: Create `src/main.rs` (minimal window, from spike)**

Minimal NSH window that displays a placeholder string. `#[cfg(target_os = "macos")]` gate; non-mac returns `ExitCode::SUCCESS`. Uses the spike's exact `run_app` + `AppOptions` + `#[component] fn App()` + `signal` + `text` pattern from `desktop/nsh-spike/src/main.rs`.

- [ ] **Step 3: Add to workspace members**

In root `Cargo.toml`, add `"desktop/nsh-shell"` to the `[workspace.members]` array (alphabetical, after `desktop/nsh-spike` if present, otherwise after `desktop/shell`).

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p aleph-nsh-shell`
Expected: builds with zero errors.

- [ ] **Step 5: Verify it runs and opens a window**

Run: `cargo run -p aleph-nsh-shell`
Expected: a window titled "Aleph NSH" opens showing the placeholder text. Close it manually.

- [ ] **Step 6: Commit**

```bash
git add desktop/nsh-shell/ Cargo.toml
git commit -m "feat(nsh): add aleph-nsh-shell crate skeleton with minimal NSH window

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Daemon supervisor

**Files:**
- Create: `desktop/nsh-shell/src/supervisor.rs`
- Modify: `desktop/nsh-shell/src/main.rs` (call supervisor before opening window — wired in Task 5)

**Interfaces:**
- Consumes: `std::process::Command`, `tokio::net::TcpStream`, `tokio::io::{AsyncReadExt, AsyncWriteExt}`. Ported from `desktop/shell/src/daemon.rs`.
- Produces: `pub async fn ensure_ready() -> Result<(), String>` — guarantees `aleph-server` is up and `/ready` returns 200 on `127.0.0.1:18790`, or an actionable error. `pub fn resolve_daemon_binary() -> Option<PathBuf>`.

- [ ] **Step 1: Write the failing tests**

Create `desktop/nsh-shell/src/supervisor.rs` with a `#[cfg(test)]` module. Port the probe logic from `desktop/shell/src/daemon.rs` and write tests that exercise the three `PortOccupant` branches and the binary-not-found path:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_free_port_returns_free() {
        // Bind a TcpListener on 127.0.0.1:0, probe that port (not 18790),
        // assert PortOccupant::Free. Use a helper that takes a port arg.
    }

    #[tokio::test]
    async fn probe_foreign_process_returns_foreign() {
        // Bind a TcpListener that accepts but does not answer /ready.
        // Assert PortOccupant::Foreign.
    }

    #[test]
    fn resolve_daemon_binary_returns_none_when_not_on_path() {
        // Temporarily set PATH to empty, assert resolve_daemon_binary() is None.
        // (Or: assert it finds the binary when PATH includes the build dir —
        //  the implementer picks the test that is robust to the local env.)
    }
}
```

Note: `ensure_ready` launches a real process, so it is NOT unit-tested here; it is exercised in Task 5's end-to-end run. The unit tests cover `probe_port` (pure-ish, takes a port) and `resolve_daemon_binary`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-nsh-shell supervisor`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `supervisor.rs`**

Port from `desktop/shell/src/daemon.rs`: `DAEMON_HOST = "127.0.0.1"`, `DAEMON_PORT = 18790`, `READY_TIMEOUT = 40s`, `POLL_INTERVAL = 400ms`, `PROBE_TIMEOUT = 2s`. `PortOccupant { Free, DaemonReady, DaemonBooting, Foreign }`. `probe_port(port) -> PortOccupant` (parameterize the port so tests can use an ephemeral port). `resolve_daemon_binary() -> Option<PathBuf>` — search `PATH` + the cargo build dir relative to the current exe. `launch_detached() -> Result<(), String>`. `wait_until_ready() -> Result<(), String>`. `ensure_ready() -> Result<(), String>` — the dispatch from `probe_port`: `DaemonReady` → Ok, `DaemonBooting` → wait, `Free` → launch + wait, `Foreign` → Err. Add exponential backoff to `wait_until_ready`: start at `POLL_INTERVAL`, double on each miss, cap at 2 s (deviation from the original's fixed interval — the design doc Phase 1 deliverable calls for "指数退避重连").

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aleph-nsh-shell supervisor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add desktop/nsh-shell/src/supervisor.rs
git commit -m "feat(nsh): port daemon supervisor with exponential backoff

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Connection layer (handshake + subscribe + session list)

**Files:**
- Create: `desktop/nsh-shell/src/runtime.rs`
- Create: `desktop/nsh-shell/src/handshake.rs`
- Create: `desktop/nsh-shell/src/connection.rs`

**Interfaces:**
- Consumes: `aleph_client::{AlephClient, CliConfig, StreamEvent}`, `aleph_protocol::system_hello::{HelloParams, HelloResult, ProtocolVersion, ClientHello, Feature}`, `crate::supervisor::ensure_ready`.
- Produces:
  - `pub enum ShellStatus { Connecting, Connected { server_version: String, session_count: usize }, Error(String) }` — the status pushed to the NSH thread.
  - `pub fn spawn_connection_loop(status_tx: mpsc::Sender<ShellStatus>) -> JoinHandle` — starts the tokio runtime thread, runs the connect→hello→subscribe→session.list loop, pushes status updates, and reconnects on drop.

- [ ] **Step 1: Write the failing tests for the handshake builder**

Create `desktop/nsh-shell/src/handshake.rs` with a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hello_params_uses_nsh_desktop_client_name() {
        let params = build_hello_params();
        assert_eq!(params.client.name, "aleph-nsh-desktop");
        assert_eq!(params.protocol.major, 1);
        assert_eq!(params.protocol.minor, 0);
        assert!(params.features.contains(&Feature::EventReplay));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-nsh-shell handshake`
Expected: FAIL.

- [ ] **Step 3: Implement `handshake.rs`**

`pub fn build_hello_params() -> HelloParams` — returns `HelloParams { protocol: ProtocolVersion { major: 1, minor: 0 }, client: ClientHello { name: "aleph-nsh-desktop".into(), version: env!("CARGO_PKG_VERSION").into(), platform: std::env::consts::OS.into(), ui_runtime: "nsh".into() }, features: vec![Feature::EventReplay, Feature::Snapshot, Feature::BinaryChannel] }`. `pub async fn perform_handshake(client: &AlephClient) -> Result<HelloResult, String>` — calls `client.call::<HelloParams, HelloResult>("system.hello", Some(build_hello_params())).await`, maps error to string.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-nsh-shell handshake`
Expected: PASS.

- [ ] **Step 5: Write the failing test for the connection lifecycle**

Create `desktop/nsh-shell/src/connection.rs` with a `#[cfg(test)]` module. The lifecycle (connect → hello → subscribe → session.list) is exercised against a mock server in an integration test; the unit test here covers the status sequencing and the reconnect-re-handshake ordering:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reconnect_re_issues_hello_then_subscribe_in_order() {
        // Use a mock that records the method names it receives.
        // Drive one successful cycle, then drop the connection,
        // then drive a reconnect. Assert the recorded sequence is
        // ["connect", "system.hello", "events.subscribe", "sessions.list",
        //  "connect", "system.hello", "events.subscribe", "sessions.list"].
        // (connect is implicit in AlephClient::connect; record the explicit
        //  RPCs: system.hello, events.subscribe, sessions.list, then again
        //  after reconnect: system.hello, events.subscribe, sessions.list.)
    }
}
```

The implementer builds the mock server with `tokio_tungstenite` the same way the spike's `worker_delivers_version_even_when_response_takes_longer_than_startup` test does (see `desktop/nsh-spike/src/main.rs` test module). The mock answers `system.hello` with a valid `HelloResult`, `events.subscribe` with a subscription_id, `sessions.list` with an empty array.

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p aleph-nsh-shell connection`
Expected: FAIL.

- [ ] **Step 7: Implement `runtime.rs` and `connection.rs`**

`runtime.rs`: `pub fn spawn_tokio_worker<F>(future: F) -> JoinHandle<F::Output>` — `std::thread::spawn` + `tokio::runtime::Builder::new_multi_thread().enable_all().build()` + `rt.block_on(future)`, mirroring the spike's `start_gateway_probe` thread pattern.

`connection.rs`: `ShellStatus` enum. `spawn_connection_loop(status_tx: mpsc::Sender<ShellStatus>) -> JoinHandle`:
1. `status_tx.send(ShellStatus::Connecting).await`
2. `ensure_ready().await?` (supervisor)
3. `let (client, events_rx) = AlephClient::connect("ws://127.0.0.1:18790", &CliConfig::default()).await?`
4. `let hello = perform_handshake(&client).await?`
5. `client.call("events.subscribe", Some(json!({"scope": {"type": "global"}}))).await?`
6. `let sessions: serde_json::Value = client.call("sessions.list", None).await?` ; `session_count = sessions.as_array().map(|a| a.len()).unwrap_or(0)`
7. `status_tx.send(ShellStatus::Connected { server_version: hello.server.version, session_count }).await`
8. Await `events_rx` in a loop (keep the subscription alive; drain into a log or discard for Phase 1 — Phase 2's UI consumes them).
9. On disconnect: `client.reconnect(&config).await?`, then go to step 4 (re-handshake + re-subscribe). Backoff between attempts using the same exponential schedule as the supervisor.

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p aleph-nsh-shell connection`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add desktop/nsh-shell/src/runtime.rs desktop/nsh-shell/src/handshake.rs desktop/nsh-shell/src/connection.rs
git commit -m "feat(nsh): add connection layer with hello, subscribe, session list

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Integration — NSH window shows connection status

**Files:**
- Modify: `desktop/nsh-shell/src/main.rs`

**Interfaces:**
- Consumes: `crate::supervisor::ensure_ready`, `crate::connection::{spawn_connection_loop, ShellStatus}`.
- Produces: a running app whose window reflects `ShellStatus` via an NSH `signal`.

- [ ] **Step 1: Wire the connection loop into `main.rs`**

Replace the placeholder `#[component] fn App()` with one that:
1. Creates a `signal(ShellStatus::Connecting)`.
2. Spawns an `mpsc::channel::<ShellStatus>(8)`.
3. Calls `spawn_connection_loop(tx)` (on the tokio worker thread from `runtime.rs`).
4. In an NSH `spawn`, receives from `rx` and `status.set(received)` on each update.
5. Renders `text(move || match status.get() { Connecting => "Connecting...", Connected { server_version, session_count } => format!("Connected: server v{}, {} sessions", server_version, session_count), Error(e) => format!("Error: {e}") })`.

- [ ] **Step 2: Build**

Run: `cargo build -p aleph-nsh-shell`
Expected: builds.

- [ ] **Step 3: End-to-end run against a real or simulated server**

If `aleph-server` is running locally: `cargo run -p aleph-nsh-shell` → window shows "Connecting..." then "Connected: server v{version}, N sessions".

If no `aleph-server` is available: run `cargo run -p aleph-server -- start` in one terminal (or let the supervisor launch it), then `cargo run -p aleph-nsh-shell`. The supervisor should launch the daemon if it's not running. The window should transition to Connected.

Verify the `system.hello` was actually called by checking the server logs for a `system.hello` dispatch line (or by temporarily adding a `tracing::info!` in the handler during this step and removing it before commit).

- [ ] **Step 4: Commit**

```bash
git add desktop/nsh-shell/src/main.rs
git commit -m "feat(nsh): wire supervisor + connection into NSH status window

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## State the Negative

What this plan does **not** do:

- **No UI components.** The window shows a single status string. Session list rendering, chat, composer, settings — all Phase 2+.
- **No `system.hello` gate.** Server accepts business RPCs whether or not hello was called. Enforcing "no business RPC before hello" (Protocol V1 §4.2) is Phase 2.
- **No reconnect persistence across app restarts.** Reconnect is in-process only; the connection state is not saved.
- **No event consumption.** `events_rx` is drained and discarded. Phase 2's UI consumes events.
- **No non-macOS support.** `#[cfg(target_os = "macos")]` gate; other platforms return immediately.
- **No `interfaces/nsh-desktop` crate.** That's Phase 2. This plan only touches `desktop/nsh-shell`.
- **No removal of `desktop/shell`.** The Tauri shell stays; both crates exist in parallel.
- **No ADR-003/004 confirmation.** Those are Phase 2 prerequisites; out of scope here.
- **No verification of the `nsh-core`/`nsh-platform-macos` crate names or import paths beyond the spike.** The implementer copies dep spellings from `desktop/nsh-spike/Cargo.toml`; if the spike's manifest changed since this plan was written, the implementer re-verifies.
- **Not validated:** whether `CliConfig::default()` is the right config for a loopback connect with no credential. The implementer checks `shared/client/src/config.rs` and uses the loopback-friendly constructor (the original daemon's `ensure_ready` connects without a credential; `AlephClient::connect` on `ws://127.0.0.1:18790` should resolve to operator role via the loopback path documented in `connection.rs`).

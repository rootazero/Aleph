# NSH Desktop Phase 2 — Connection Layer Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Implement connection lifecycle + protocol handshake for NSH desktop client (Tasks 4–5 from Phase 1 plan).

**Starting Point:** `main` branch. Phase 1 complete (system.hello protocol + supervisor daemon).

**References:**
- Design: `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md` (§4–7)
- Protocol: `/Volumes/TBU4/Workspace/Aleph/docs/research/NSH_PROTOCOL_V1.md` (handshake sequence, system.hello)
- Phase 1 Evidence: `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/runs/2026-11-03-nsh-desktop-phase1-evidence.md`

## Architecture Summary

```
desktop/nsh-shell/src/
├── main.rs              ← Task 5: NSH window + status display
├── supervisor.rs        ← Task 3: daemon lifecycle (Phase 1 complete)
├── connection.rs        ← Task 4: NEW — lifecycle + handshake
└── runtime.rs           ← Task 4: tokio background thread

shared/client            ← reuse AlephClient::connect
shared/protocol          ← system.hello DTO (Phase 1 complete)
```

**Tech Stack:** Rust, tokio 1.53.3, nsh v0.4.0 (`2ab3b6bc27`), aleph-client, aleph-protocol.

**Phase 1 Completion Evidence:**
- `desktop/nsh-shell/` crate exists with supervisor
- `shared/protocol/src/system_hello.rs` DTOs defined
- `src/gateway/handlers/system_hello.rs` handler registered
- All verified in commit `d897c376b` (2026-11-03)

---

## Global Constraints

- **NSH path dep:** locked to `/Volumes/TBU4/Github/nsh` @ `2ab3b6bc27` — no crates.io, no following head
- **Daemon port:** `127.0.0.1:18790`, ready probe `/ready`, 40s timeout, 400ms poll (from Phase 1 supervisor)
- **Client name:** `"aleph-nsh-desktop"` in every `system.hello` call
- **Protocol version:** `major: 1, minor: 0`
- **Crate name:** `aleph-nsh-shell` (workspace convention)
- **Zero business logic in nsh-shell:** only OS integration + connection plumbing (R2/R10)
- **macOS only:** `#[cfg(target_os = "macos")]` gate; non-mac returns `ExitCode::SUCCESS`
- **Commit prefix:** `feat(nsh):` for nsh-shell work

---

## Review Focus — Five Edge Cases

1. **`system.hello` with `protocol.major` ≠ 1** — server returns error, not success (Protocol V1 §18)
2. **Unknown `features` entry** — server narrows, doesn't reject (Protocol V1 §4.2)
3. **Daemon binary not found** — `supervisor::resolve_daemon_binary()` returns `None`, must fail gracefully
4. **Port 18790 held by non-aleph process** — fail fast with actionable message (from Phase 1 `PortOccupant::Foreign`)
5. **Connection drops after subscribe** — reconnect must re-do: `connect` → `system.hello` → `events.subscribe` in that order

---

## File Structure

### New files (Task 4)
| File | Responsibility |
|------|---------------|
| `desktop/nsh-shell/src/runtime.rs` | Tokio runtime on background thread |
| `desktop/nsh-shell/src/connection.rs` | Connection lifecycle: connect → hello → subscribe → session.list |

### Modified files (Task 5)
| File | Change |
|------|--------|
| `desktop/nsh-shell/src/main.rs` | Wire supervisor + connection into NSH status window |

### Phase 1 completed (no changes needed)
| File | Status |
|------|--------|
| `shared/protocol/src/system_hello.rs` | DTO defined ✅ |
| `src/gateway/handlers/system_hello.rs` | Handler registered ✅ |
| `desktop/nsh-shell/src/supervisor.rs` | Daemon supervisor complete ✅ |

---

## Task 4: Connection Layer

**Files:**
- Create: `desktop/nsh-shell/src/runtime.rs`
- Create: `desktop/nsh-shell/src/connection.rs`

**Interfaces:**
- Consumes: `aleph_client::AlephClient`, `aleph_protocol::system_hello::{HelloParams, HelloResult, Feature}`, `crate::supervisor::ensure_ready`
- Produces:
  - `pub enum ShellStatus { Connecting, Connected { server_version: String, session_count: usize }, Error(String) }`
  - `pub fn spawn_connection_loop(status_tx: mpsc::Sender<ShellStatus>) -> JoinHandle`

### Step 4.1: Write failing tests for handshake builder

**File:** `desktop/nsh-shell/src/connection.rs` (new)

Create test module:

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

    #[tokio::test]
    async fn reconnect_re_issues_hello_then_subscribe_in_order() {
        // Mock server that records method names
        // Drive one cycle → drop → reconnect
        // Assert sequence: ["system.hello", "events.subscribe", "sessions.list",
        //                   "system.hello", "events.subscribe", "sessions.list"]
        todo!("implement mock server");
    }
}
```

**Expected:** FAIL — module not found.

**Verification:**
```bash
cargo test -p aleph-nsh-shell connection
```

### Step 4.2: Implement runtime.rs

**File:** `desktop/nsh-shell/src/runtime.rs` (new)

```rust
use std::thread;
use tokio::task::JoinHandle;

/// Spawn tokio multi-thread runtime on background thread
pub fn spawn_tokio_worker<F>(future: F) -> JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = rt.block_on(future);
        let _ = tx.send(result);
    });
    
    // Convert std channel to tokio JoinHandle pattern
    tokio::task::spawn_blocking(move || rx.recv().expect("worker thread panicked"))
}
```

**Pattern:** Mirrors Phase 1 spike `start_gateway_probe` pattern (from `desktop/nsh-spike/src/main.rs`).

### Step 4.3: Implement connection.rs — handshake builder

**File:** `desktop/nsh-shell/src/connection.rs` (new)

```rust
use aleph_client::AlephClient;
use aleph_protocol::system_hello::{HelloParams, HelloResult, ProtocolVersion, ClientHello, Feature};
use tokio::sync::mpsc;
use std::time::Duration;

pub enum ShellStatus {
    Connecting,
    Connected { server_version: String, session_count: usize },
    Error(String),
}

pub fn build_hello_params() -> HelloParams {
    HelloParams {
        protocol: ProtocolVersion { major: 1, minor: 0 },
        client: ClientHello {
            name: "aleph-nsh-desktop".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            ui_runtime: "nsh".into(),
        },
        features: vec![Feature::EventReplay, Feature::Snapshot, Feature::BinaryChannel],
    }
}

pub async fn perform_handshake(client: &AlephClient) -> Result<HelloResult, String> {
    client.call::<HelloParams, HelloResult>("system.hello", Some(build_hello_params()))
        .await
        .map_err(|e| format!("handshake failed: {e}"))
}
```

**Verification:**
```bash
cargo test -p aleph-nsh-shell connection::tests::build_hello_params_uses_nsh_desktop_client_name
```
Expected: PASS

### Step 4.4: Implement connection loop with reconnect

**File:** `desktop/nsh-shell/src/connection.rs` (extend)

```rust
pub fn spawn_connection_loop(status_tx: mpsc::Sender<ShellStatus>) -> std::thread::JoinHandle<()> {
    crate::runtime::spawn_tokio_worker(async move {
        loop {
            if let Err(e) = run_connection_cycle(&status_tx).await {
                let _ = status_tx.send(ShellStatus::Error(e.clone())).await;
                // Exponential backoff before reconnect
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    })
}

async fn run_connection_cycle(status_tx: &mpsc::Sender<ShellStatus>) -> Result<(), String> {
    // 1. Ensure daemon ready
    status_tx.send(ShellStatus::Connecting).await.ok();
    crate::supervisor::ensure_ready().await?;
    
    // 2. Connect via AlephClient
    let (client, events_rx) = AlephClient::connect(
        "ws://127.0.0.1:18790",
        &aleph_client::CliConfig::default()
    ).await.map_err(|e| format!("connect failed: {e}"))?;
    
    // 3. Handshake
    let hello = perform_handshake(&client).await?;
    
    // 4. Subscribe to events
    client.call::<serde_json::Value, serde_json::Value>(
        "events.subscribe",
        Some(serde_json::json!({"scope": {"type": "global"}}))
    ).await.map_err(|e| format!("subscribe failed: {e}"))?;
    
    // 5. Get session list
    let sessions: serde_json::Value = client.call("sessions.list", None)
        .await.map_err(|e| format!("sessions.list failed: {e}"))?;
    let session_count = sessions.as_array().map(|a| a.len()).unwrap_or(0);
    
    // 6. Report connected
    status_tx.send(ShellStatus::Connected {
        server_version: hello.server.version,
        session_count,
    }).await.ok();
    
    // 7. Keep connection alive by draining events
    let mut events = events_rx;
    while let Some(_event) = events.recv().await {
        // Phase 2: just drain. Phase 3+ will process events
    }
    
    Err("connection closed".into())
}
```

**Verification:**
```bash
cargo test -p aleph-nsh-shell connection
```
Expected: Both tests PASS

### Step 4.5: Add connection.rs to module tree

**File:** `desktop/nsh-shell/src/main.rs` (modify)

Add at top:
```rust
mod supervisor;
mod runtime;
mod connection;  // NEW
```

**Verification:**
```bash
cargo check -p aleph-nsh-shell
```
Expected: compiles

---

## Task 5: NSH Window Integration

**File:** `desktop/nsh-shell/src/main.rs` (modify)

**Interfaces:**
- Consumes: `crate::connection::{spawn_connection_loop, ShellStatus}`
- Produces: running app whose window reflects `ShellStatus` via NSH `signal`

### Step 5.1: Wire connection loop into App component

Replace the placeholder `App` component:

```rust
use nsh_core::prelude::*;

#[component]
fn App() -> impl IntoElement {
    let status = signal(ShellStatus::Connecting);
    
    // Spawn connection loop on background thread
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ShellStatus>(8);
    let _handle = connection::spawn_connection_loop(tx);
    
    // Update status signal from connection events
    spawn(async move {
        while let Some(new_status) = rx.recv().await {
            status.set(new_status);
        }
    });
    
    // Render status
    text(move || match status.get() {
        ShellStatus::Connecting => "Connecting to Aleph server...".to_string(),
        ShellStatus::Connected { server_version, session_count } => {
            format!("Connected: server v{}, {} sessions", server_version, session_count)
        }
        ShellStatus::Error(e) => format!("Error: {}", e),
    })
}
```

**Pattern:** Matches Phase 1 spike's `signal` + `spawn` pattern for async updates (from `desktop/nsh-spike/src/main.rs`).

### Step 5.2: Build and verify

**Verification:**
```bash
cargo build -p aleph-nsh-shell
```
Expected: builds with zero errors

---

## End-to-End Verification

### QA Checklist (all must pass before commit)

1. **Happy path:**
   ```bash
   # Terminal 1: start server
   cargo run -p aleph-server -- start
   
   # Terminal 2: run NSH shell
   cargo run -p aleph-nsh-shell
   ```
   Expected: window shows "Connecting..." → "Connected: server v26.11.5, N sessions"

2. **Server not running:**
   ```bash
   # Ensure no server on 18790
   lsof -i :18790  # should be empty
   
   cargo run -p aleph-nsh-shell
   ```
   Expected: supervisor launches daemon automatically, window transitions to Connected

3. **Foreign process on port 18790:**
   ```bash
   # Occupy port with netcat
   nc -l 18790 &
   NC_PID=$!
   
   cargo run -p aleph-nsh-shell
   ```
   Expected: window shows "Error: port 18790 occupied by non-aleph process"
   ```bash
   kill $NC_PID
   ```

4. **Server logs show system.hello:**
   ```bash
   # In server terminal, grep for handshake
   grep "system.hello" <server-log>
   ```
   Expected: see `method="system.hello" client="aleph-nsh-desktop"`

5. **Unit tests:**
   ```bash
   cargo test -p aleph-nsh-shell
   ```
   Expected: all tests PASS

---

## State the Negative

What Phase 2 does **not** do:

- **No UI components.** Window shows a single status string. Session list rendering, chat, composer — all Phase 3+.
- **No `system.hello` gate.** Server accepts business RPCs before/after hello. Enforcing "no RPC before hello" is Phase 2+.
- **No reconnect persistence.** Reconnect is in-process only; state not saved across app restarts.
- **No event consumption.** `events_rx` is drained and discarded. Phase 3's UI consumes events.
- **No non-macOS support.** `#[cfg(target_os = "macos")]` gate; other platforms return immediately.
- **No `interfaces/nsh-desktop` crate.** That's Phase 3. This plan only touches `desktop/nsh-shell`.
- **No removal of `desktop/shell`.** Tauri shell stays; both coexist in parallel.

---

## Commit Checklist

Before marking complete:

- [ ] `cargo build -p aleph-nsh-shell` — compiles
- [ ] `cargo test -p aleph-nsh-shell` — all tests pass
- [ ] `cargo clippy -p aleph-nsh-shell` — no warnings
- [ ] QA checklist 1-5 — all scenarios verified
- [ ] Server logs confirm `system.hello` handshake
- [ ] Window transitions: Connecting → Connected → (on drop) Error

**Commit message template:**
```
feat(nsh): implement Phase 2 connection layer

- Add connection.rs: lifecycle + system.hello handshake
- Add runtime.rs: tokio background thread
- Wire supervisor + connection into NSH status window
- Verify: connect → handshake → subscribe → session.list

Phase 1 supervisor reused; AlephClient::connect handles auth.
Protocol: major=1, minor=0, client="aleph-nsh-desktop".

QA: happy path + daemon launch + foreign port + server logs verified.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Known Limitations (defer to Phase 3+)

- Swift bridge integration (截图/语音/摄像头) — not in Phase 2 scope (design doc §5.4)
- Session UI rendering — Phase 3
- Memory integration — Phase 4
- Plugin UI lifecycle — Phase 5
- Windows/Linux platform parity — parallel track

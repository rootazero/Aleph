# Social Connectivity Evolution — Phase 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform Aleph's channel system from hardcoded initialization to a manifest-driven plugin architecture with multi-instance support, dynamic config, and external bridge process management.

**Architecture:** Introduce `LinkManager` as the central orchestrator that scans `~/.aleph/links/` YAML configs and `~/.aleph/bridges/` manifests to instantiate channels at runtime. External bridges communicate via a `Transport` trait abstraction (Unix Socket or Stdio). A `BridgeSupervisor` manages external process lifecycle with heartbeat monitoring. Native channels (Telegram, Discord) register as "builtin" bridge types, presenting a unified interface to users.

**Tech Stack:** Rust, Tokio, serde_yaml, jsonschema, notify (file watcher), existing Channel trait

**Design Doc:** `docs/plans/2026-02-23-social-connectivity-evolution-design.md`

---

## Task 1: Data Models — Bridge & Link Types

Define the core data types that represent bridge type definitions (from `bridge.yaml`) and link instance configs (from `link.yaml`).

**Files:**
- Create: `core/src/gateway/bridge/mod.rs`
- Create: `core/src/gateway/bridge/types.rs`
- Create: `core/src/gateway/link/mod.rs`
- Create: `core/src/gateway/link/types.rs`
- Test: `core/src/gateway/bridge/types.rs` (inline tests)
- Test: `core/src/gateway/link/types.rs` (inline tests)

**Step 1: Write failing test for BridgeDefinition deserialization**

In `core/src/gateway/bridge/types.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_bridge_definition_deserialize() {
        let yaml = r#"
spec_version: "1.0"
id: "telegram-native"
name: "Telegram"
version: "0.1.0"
author: "Aleph Team"
description: "Native Telegram bot integration"
runtime:
  type: builtin
capabilities:
  messaging:
    - send_text
    - receive_text
settings_schema:
  type: object
  properties:
    token:
      type: string
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id.as_str(), "telegram-native");
        assert!(matches!(def.runtime, BridgeRuntime::Builtin));
    }

    #[test]
    fn test_process_bridge_definition_deserialize() {
        let yaml = r#"
spec_version: "1.0"
id: "whatsapp-go"
name: "WhatsApp"
version: "1.0.0"
runtime:
  type: process
  executable: "./bin/whatsapp-bridge"
  args: ["--mode", "jsonrpc"]
  transport: unix-socket
  health_check_interval_secs: 30
  max_restarts: 5
  restart_delay_secs: 3
capabilities:
  messaging:
    - send_text
  lifecycle:
    - pairing_qr
"#;
        let def: BridgeDefinition = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(def.id.as_str(), "whatsapp-go");
        match &def.runtime {
            BridgeRuntime::Process { executable, transport, max_restarts, .. } => {
                assert_eq!(executable.to_str().unwrap(), "./bin/whatsapp-bridge");
                assert!(matches!(transport, TransportType::UnixSocket));
                assert_eq!(*max_restarts, 5);
            }
            _ => panic!("Expected Process runtime"),
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::bridge::types::tests -- --nocapture 2>&1 | head -30`
Expected: FAIL — module not found

**Step 3: Implement BridgeDefinition types**

In `core/src/gateway/bridge/types.rs`:
```rust
use std::path::PathBuf;
use std::time::Duration;
use serde::{Deserialize, Serialize};

/// Unique identifier for a bridge type
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BridgeId(String);

impl BridgeId {
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for BridgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Bridge type definition — parsed from bridge.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeDefinition {
    pub spec_version: String,
    pub id: BridgeId,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub runtime: BridgeRuntime,
    #[serde(default)]
    pub capabilities: BridgeCapabilities,
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
}

/// How the bridge runs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BridgeRuntime {
    /// Compiled into Core binary (Telegram, Discord, iMessage)
    Builtin,
    /// External OS process
    Process {
        executable: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_transport")]
        transport: TransportType,
        #[serde(default = "default_health_interval")]
        health_check_interval_secs: u64,
        #[serde(default = "default_max_restarts")]
        max_restarts: u32,
        #[serde(default = "default_restart_delay")]
        restart_delay_secs: u64,
    },
}

fn default_transport() -> TransportType { TransportType::UnixSocket }
fn default_health_interval() -> u64 { 30 }
fn default_max_restarts() -> u32 { 5 }
fn default_restart_delay() -> u64 { 3 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    #[default]
    UnixSocket,
    Stdio,
}

/// Capabilities declared by a bridge
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeCapabilities {
    #[serde(default)]
    pub messaging: Vec<String>,
    #[serde(default)]
    pub interactions: Vec<String>,
    #[serde(default)]
    pub lifecycle: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}
```

In `core/src/gateway/bridge/mod.rs`:
```rust
mod types;
pub use types::*;
```

**Step 4: Write failing test for LinkConfig deserialization**

In `core/src/gateway/link/types.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_config_deserialize() {
        let yaml = r#"
spec_version: "1.0"
id: "my-personal-telegram"
bridge: "telegram-native"
name: "My Personal Bot"
enabled: true
settings:
  token: "fake-token"
  allowed_users: [12345678]
routing:
  agent: "main"
  dm_policy: pairing
  group_policy: disabled
"#;
        let link: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(link.id.as_str(), "my-personal-telegram");
        assert_eq!(link.bridge.as_str(), "telegram-native");
        assert!(link.enabled);
        assert_eq!(link.routing.agent, "main");
    }

    #[test]
    fn test_link_config_env_var_in_settings() {
        let yaml = r#"
spec_version: "1.0"
id: "test"
bridge: "telegram-native"
name: "Test"
enabled: true
settings:
  token: "${env.TELEGRAM_TOKEN}"
"#;
        let link: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        let token = link.settings.get("token").unwrap().as_str().unwrap();
        assert_eq!(token, "${env.TELEGRAM_TOKEN}");
    }

    #[test]
    fn test_link_config_defaults() {
        let yaml = r#"
spec_version: "1.0"
id: "minimal"
bridge: "test"
name: "Minimal"
settings: {}
"#;
        let link: LinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!link.enabled); // default false
        assert_eq!(link.routing.agent, "main"); // default
    }
}
```

**Step 5: Implement LinkConfig types**

In `core/src/gateway/link/types.rs`:
```rust
use serde::{Deserialize, Serialize};
use crate::gateway::bridge::BridgeId;

/// Unique identifier for a link instance
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinkId(String);

impl LinkId {
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for LinkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Link instance configuration — parsed from link.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkConfig {
    pub spec_version: String,
    pub id: LinkId,
    pub bridge: BridgeId,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_settings")]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub routing: LinkRoutingConfig,
}

fn default_settings() -> serde_json::Value { serde_json::Value::Object(Default::default()) }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkRoutingConfig {
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub dm_policy: DmPolicyConfig,
    #[serde(default)]
    pub group_policy: GroupPolicyConfig,
}

impl Default for LinkRoutingConfig {
    fn default() -> Self {
        Self {
            agent: "main".to_string(),
            dm_policy: DmPolicyConfig::default(),
            group_policy: GroupPolicyConfig::default(),
        }
    }
}

fn default_agent() -> String { "main".to_string() }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicyConfig {
    Open,
    #[default]
    Pairing,
    Allowlist,
    Disabled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicyConfig {
    Open,
    Allowlist,
    #[default]
    Disabled,
}
```

In `core/src/gateway/link/mod.rs`:
```rust
mod types;
pub use types::*;
```

**Step 6: Wire modules into gateway/mod.rs**

Add to `core/src/gateway/mod.rs`:
```rust
pub mod bridge;
pub mod link;
```

**Step 7: Run tests to verify pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::bridge::types::tests gateway::link::types::tests -- --nocapture`
Expected: All tests PASS

**Step 8: Add serde_yaml dependency if missing**

Check: `grep serde_yaml core/Cargo.toml`
If missing: Add `serde_yaml = "0.9"` to `[dependencies]`

**Step 9: Commit**

```bash
git add core/src/gateway/bridge/ core/src/gateway/link/ core/src/gateway/mod.rs
git commit -m "feat(gateway): add Bridge and Link data models for social connectivity evolution"
```

---

## Task 2: Transport Trait Abstraction

Define the `Transport` trait and extract `UnixSocketTransport` from the existing WhatsApp RPC client.

**Files:**
- Create: `core/src/gateway/transport/mod.rs`
- Create: `core/src/gateway/transport/traits.rs`
- Create: `core/src/gateway/transport/unix_socket.rs`
- Modify: `core/src/gateway/mod.rs` (add `pub mod transport`)
- Reference: `core/src/gateway/interfaces/whatsapp/rpc_client.rs` (extraction source)
- Reference: `core/src/gateway/interfaces/whatsapp/bridge_protocol.rs` (event types)

**Step 1: Write failing test for Transport trait**

In `core/src/gateway/transport/traits.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Verify the trait is object-safe (can be used as dyn Transport)
    fn _assert_object_safe(_: &dyn Transport) {}

    #[tokio::test]
    async fn test_bridge_event_variants() {
        // Ensure BridgeEvent can represent all standard events
        let events = vec![
            BridgeEvent::Ready,
            BridgeEvent::StatusChange { status: "connected".into() },
            BridgeEvent::PairingUpdate(PairingEvent::QrCode {
                qr_data: "test".into(),
                expires_in_secs: 60,
            }),
            BridgeEvent::Message {
                from: "user1".into(),
                conversation_id: "chat1".into(),
                text: "hello".into(),
                message_id: "msg1".into(),
                timestamp: 1234567890,
                is_group: false,
                attachments: vec![],
                reply_to: None,
                sender_name: None,
            },
            BridgeEvent::Receipt {
                message_id: "msg1".into(),
                receipt_type: "read".into(),
            },
        ];
        assert_eq!(events.len(), 5);
    }
}
```

**Step 2: Implement Transport trait and event types**

In `core/src/gateway/transport/traits.rs`:
```rust
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Standardized bridge events (platform-independent)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    Ready,
    StatusChange { status: String },
    PairingUpdate(PairingEvent),
    Message {
        from: String,
        #[serde(default)]
        sender_name: Option<String>,
        conversation_id: String,
        text: String,
        message_id: String,
        timestamp: i64,
        is_group: bool,
        #[serde(default)]
        attachments: Vec<AttachmentPayload>,
        #[serde(default)]
        reply_to: Option<String>,
    },
    Receipt {
        message_id: String,
        receipt_type: String,
    },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PairingEvent {
    QrCode { qr_data: String, expires_in_secs: u64 },
    QrExpired,
    Scanned,
    Syncing { progress: f32 },
    Connected { device_name: String, identifier: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPayload {
    pub mime_type: String,
    /// Base64-encoded data
    pub data: String,
    pub filename: Option<String>,
}

/// Transport error types
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Request failed: {0}")]
    RequestFailed(String),
    #[error("Request timed out")]
    Timeout,
    #[error("Not connected")]
    NotConnected,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Abstract IPC transport for communicating with external bridge processes.
///
/// Implementations:
/// - `UnixSocketTransport`: Unix domain socket (production, extracted from WhatsApp)
/// - `StdioTransport`: stdin/stdout (for simple bridges, LSP/MCP-style)
#[async_trait]
pub trait Transport: Send + Sync + fmt::Debug {
    /// Send a JSON-RPC request and wait for the response
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError>;

    /// Receive the next event notification from the bridge
    async fn next_event(&self) -> Option<BridgeEvent>;

    /// Close the transport connection
    async fn close(&self) -> Result<(), TransportError>;

    /// Check if the transport is connected
    fn is_connected(&self) -> bool;
}
```

In `core/src/gateway/transport/mod.rs`:
```rust
mod traits;
pub mod unix_socket;

pub use traits::*;
```

**Step 3: Extract UnixSocketTransport from WhatsApp RPC client**

This is the largest extraction. Read `core/src/gateway/interfaces/whatsapp/rpc_client.rs` and generalize it.

In `core/src/gateway/transport/unix_socket.rs`:
```rust
use super::{Transport, TransportError, BridgeEvent};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcMessage {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Unix Socket transport implementing JSON-RPC 2.0 over newline-delimited JSON.
///
/// Extracted and generalized from the WhatsApp bridge RPC client.
pub struct UnixSocketTransport {
    socket_path: PathBuf,
    writer: Mutex<Option<WriteHalf<UnixStream>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    next_id: AtomicU64,
    event_tx: mpsc::Sender<BridgeEvent>,
    event_rx: Mutex<Option<mpsc::Receiver<BridgeEvent>>>,
    connected: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for UnixSocketTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixSocketTransport")
            .field("socket_path", &self.socket_path)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish()
    }
}

impl UnixSocketTransport {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            writer: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            connected: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Connect to the Unix socket with retry logic
    pub async fn connect(
        &self,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<(), TransportError> {
        let mut last_error = String::new();

        for attempt in 0..=max_retries {
            match UnixStream::connect(&self.socket_path).await {
                Ok(stream) => {
                    let (reader, writer) = tokio::io::split(stream);
                    *self.writer.lock().await = Some(writer);
                    self.connected.store(true, Ordering::Release);

                    // Spawn background read loop
                    let pending = self.pending.clone();
                    let event_tx = self.event_tx.clone();
                    let connected = self.connected.clone();
                    tokio::spawn(async move {
                        Self::read_loop(reader, pending, event_tx, connected).await;
                    });

                    info!(
                        socket = %self.socket_path.display(),
                        attempt = attempt + 1,
                        "Unix socket transport connected"
                    );
                    return Ok(());
                }
                Err(e) => {
                    last_error = e.to_string();
                    if attempt < max_retries {
                        debug!(
                            socket = %self.socket_path.display(),
                            attempt = attempt + 1,
                            error = %e,
                            "Connection attempt failed, retrying..."
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                    }
                }
            }
        }

        Err(TransportError::ConnectionFailed(format!(
            "Failed to connect to {} after {} attempts: {}",
            self.socket_path.display(),
            max_retries + 1,
            last_error
        )))
    }

    async fn read_loop(
        reader: tokio::io::ReadHalf<UnixStream>,
        pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
        event_tx: mpsc::Sender<BridgeEvent>,
        connected: std::sync::atomic::AtomicBool,
    ) {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    info!("Bridge process closed connection");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                        Ok(msg) => {
                            if let Some(id) = msg.id {
                                // Response to a pending request
                                let mut pending = pending.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    if let Some(error) = msg.error {
                                        let _ = tx.send(Err(error.message));
                                    } else {
                                        let _ = tx.send(Ok(
                                            msg.result.unwrap_or(serde_json::Value::Null)
                                        ));
                                    }
                                }
                            } else if let Some(params) = msg.params {
                                // Push notification (event)
                                match serde_json::from_value::<BridgeEvent>(params) {
                                    Ok(event) => {
                                        if event_tx.send(event).await.is_err() {
                                            warn!("Event channel closed");
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        debug!("Failed to parse bridge event: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug!("Failed to parse JSON-RPC message: {}: {}", e, trimmed);
                        }
                    }
                }
                Err(e) => {
                    error!("Read error: {}", e);
                    break;
                }
            }
        }

        connected.store(false, Ordering::Release);
        info!("Unix socket read loop terminated");
    }
}

#[async_trait]
impl Transport for UnixSocketTransport {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params: if params.is_null() { None } else { Some(params) },
        };

        let json = serde_json::to_string(&request)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;

        {
            let mut writer = self.writer.lock().await;
            let writer = writer.as_mut().ok_or(TransportError::NotConnected)?;
            writer
                .write_all(json.as_bytes())
                .await
                .map_err(TransportError::Io)?;
            writer
                .write_all(b"\n")
                .await
                .map_err(TransportError::Io)?;
            writer.flush().await.map_err(TransportError::Io)?;
        }

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(TransportError::RequestFailed(e)),
            Err(_) => Err(TransportError::RequestFailed(
                "Response channel closed".to_string(),
            )),
        }
    }

    async fn next_event(&self) -> Option<BridgeEvent> {
        let mut rx_guard = self.event_rx.lock().await;
        if let Some(rx) = rx_guard.as_mut() {
            rx.recv().await
        } else {
            None
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.connected.store(false, Ordering::Release);
        let mut writer = self.writer.lock().await;
        *writer = None;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}
```

**Step 4: Wire transport module into gateway**

Add to `core/src/gateway/mod.rs`:
```rust
pub mod transport;
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::transport -- --nocapture`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/gateway/transport/ core/src/gateway/mod.rs
git commit -m "feat(gateway): add Transport trait and UnixSocketTransport"
```

---

## Task 3: StdioTransport

Implement stdin/stdout transport for simple bridges (Python scripts, etc.).

**Files:**
- Create: `core/src/gateway/transport/stdio.rs`
- Modify: `core/src/gateway/transport/mod.rs` (add `pub mod stdio`)

**Step 1: Write failing test**

In `core/src/gateway/transport/stdio.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdio_transport_creation() {
        // Create a pair of pipes to simulate stdin/stdout
        let (child_stdin_read, child_stdin_write) = tokio::io::duplex(1024);
        let (child_stdout_read, child_stdout_write) = tokio::io::duplex(1024);

        let transport = StdioTransport::from_streams(child_stdin_write, child_stdout_read);
        assert!(!transport.is_connected());
    }
}
```

**Step 2: Implement StdioTransport**

In `core/src/gateway/transport/stdio.rs`:
```rust
use super::{BridgeEvent, Transport, TransportError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

/// Stdio transport implementing JSON-RPC 2.0 over stdin/stdout.
///
/// Suitable for simple bridges written in any language.
/// The bridge reads from stdin and writes to stdout (newline-delimited JSON).
pub struct StdioTransport {
    writer: Mutex<Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    next_id: AtomicU64,
    event_tx: mpsc::Sender<BridgeEvent>,
    event_rx: Mutex<Option<mpsc::Receiver<BridgeEvent>>>,
    connected: AtomicBool,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport")
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish()
    }
}

impl StdioTransport {
    /// Create from a child process's stdin and stdout
    pub fn from_child(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let transport = Self {
            writer: Mutex::new(Some(Box::new(stdin))),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            connected: AtomicBool::new(false),
        };

        // Spawn read loop for stdout
        let pending = transport.pending.clone();
        let event_tx_clone = transport.event_tx.clone();
        let connected = transport.connected.clone();
        tokio::spawn(async move {
            Self::read_loop(stdout, pending, event_tx_clone, connected).await;
        });

        transport
    }

    /// Create from generic async streams (for testing)
    pub fn from_streams(
        writer: impl tokio::io::AsyncWrite + Send + Unpin + 'static,
        reader: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        let transport = Self {
            writer: Mutex::new(Some(Box::new(writer))),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            connected: AtomicBool::new(false),
        };

        let pending = transport.pending.clone();
        let event_tx_clone = transport.event_tx.clone();
        let connected = transport.connected.clone();
        tokio::spawn(async move {
            Self::read_loop(reader, pending, event_tx_clone, connected).await;
        });

        transport
    }

    /// Mark transport as connected (call after handshake succeeds)
    pub fn set_connected(&self) {
        self.connected.store(true, Ordering::Release);
    }

    async fn read_loop(
        reader: impl tokio::io::AsyncRead + Send + Unpin,
        pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
        event_tx: mpsc::Sender<BridgeEvent>,
        connected: AtomicBool,
    ) {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() { continue; }

                    #[derive(Deserialize)]
                    struct JsonRpcMsg {
                        #[serde(default)]
                        id: Option<u64>,
                        #[serde(default)]
                        result: Option<serde_json::Value>,
                        #[serde(default)]
                        error: Option<JsonRpcErr>,
                        #[serde(default)]
                        params: Option<serde_json::Value>,
                    }

                    #[derive(Deserialize)]
                    struct JsonRpcErr { message: String }

                    match serde_json::from_str::<JsonRpcMsg>(trimmed) {
                        Ok(msg) => {
                            if let Some(id) = msg.id {
                                let mut pending = pending.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    if let Some(error) = msg.error {
                                        let _ = tx.send(Err(error.message));
                                    } else {
                                        let _ = tx.send(Ok(
                                            msg.result.unwrap_or(serde_json::Value::Null)
                                        ));
                                    }
                                }
                            } else if let Some(params) = msg.params {
                                if let Ok(event) = serde_json::from_value::<BridgeEvent>(params) {
                                    let _ = event_tx.send(event).await;
                                }
                            }
                        }
                        Err(e) => {
                            debug!("Stdio: failed to parse JSON-RPC: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Stdio read error: {}", e);
                    break;
                }
            }
        }

        connected.store(false, Ordering::Release);
        info!("Stdio transport read loop terminated");
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id, tx);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let json = serde_json::to_string(&request)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;

        {
            let mut writer = self.writer.lock().await;
            let writer = writer.as_mut().ok_or(TransportError::NotConnected)?;
            writer.write_all(json.as_bytes()).await.map_err(TransportError::Io)?;
            writer.write_all(b"\n").await.map_err(TransportError::Io)?;
            writer.flush().await.map_err(TransportError::Io)?;
        }

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(TransportError::RequestFailed(e)),
            Err(_) => Err(TransportError::RequestFailed("Channel closed".into())),
        }
    }

    async fn next_event(&self) -> Option<BridgeEvent> {
        let mut rx = self.event_rx.lock().await;
        if let Some(rx) = rx.as_mut() {
            rx.recv().await
        } else {
            None
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.connected.store(false, Ordering::Release);
        *self.writer.lock().await = None;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}
```

**Step 3: Update transport/mod.rs**

```rust
mod traits;
pub mod unix_socket;
pub mod stdio;

pub use traits::*;
pub use unix_socket::UnixSocketTransport;
pub use stdio::StdioTransport;
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::transport -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/transport/
git commit -m "feat(gateway): add StdioTransport for simple bridge processes"
```

---

## Task 4: BridgedChannel — Channel Proxy for External Bridges

Create a generic Channel implementation that delegates all operations to a Transport.

**Files:**
- Create: `core/src/gateway/bridge/bridged_channel.rs`
- Modify: `core/src/gateway/bridge/mod.rs`

**Step 1: Write failing test**

In `core/src/gateway/bridge/bridged_channel.rs`, add tests at the bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::transport::BridgeEvent;

    #[test]
    fn test_bridged_channel_creation() {
        let channel = BridgedChannel::new(
            "test-whatsapp",
            "WhatsApp Test",
            "whatsapp-go",
        );
        assert_eq!(channel.id().as_str(), "test-whatsapp");
        assert_eq!(channel.channel_type(), "bridged");
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }
}
```

**Step 2: Implement BridgedChannel**

In `core/src/gateway/bridge/bridged_channel.rs`:
```rust
use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelId, ChannelInfo, ChannelResult, ChannelStatus,
    ConversationId, InboundMessage, MessageId, OutboundMessage, PairingData, SendResult, UserId,
};
use crate::gateway::transport::{BridgeEvent, PairingEvent, Transport};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};

/// A Channel implementation that proxies all operations to an external bridge
/// process via a Transport (Unix Socket or Stdio).
///
/// This is the core abstraction that makes external bridges "look like" native channels
/// to the rest of the system.
pub struct BridgedChannel {
    info: RwLock<ChannelInfo>,
    bridge_id: String,
    transport: Option<Arc<dyn Transport>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl BridgedChannel {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        bridge_id: impl Into<String>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: name.into(),
            channel_type: "bridged".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: ChannelCapabilities::default(),
        };
        Self {
            info: RwLock::new(info),
            bridge_id: bridge_id.into(),
            transport: None,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            shutdown_tx: None,
        }
    }

    /// Set the transport (called by BridgeSupervisor after process is spawned)
    pub fn set_transport(&mut self, transport: Arc<dyn Transport>) {
        self.transport = Some(transport);
    }

    /// Set capabilities (from bridge.yaml)
    pub async fn set_capabilities(&self, caps: ChannelCapabilities) {
        self.info.write().await.capabilities = caps;
    }

    async fn set_status(&self, status: ChannelStatus) {
        self.info.write().await.status = status;
    }

    /// Start the event forwarding loop
    fn start_event_loop(
        &self,
        transport: Arc<dyn Transport>,
        channel_id: ChannelId,
    ) -> oneshot::Sender<()> {
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let inbound_tx = self.inbound_tx.clone();
        let info = self.info.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = transport.next_event() => {
                        match event {
                            Some(BridgeEvent::Message {
                                from, sender_name, conversation_id, text,
                                message_id, timestamp, is_group, attachments, reply_to,
                            }) => {
                                let msg = InboundMessage {
                                    channel_id: channel_id.clone(),
                                    sender_id: UserId::new(&from),
                                    sender_name,
                                    conversation_id: ConversationId::new(&conversation_id),
                                    text,
                                    message_id: MessageId::new(&message_id),
                                    timestamp,
                                    is_group,
                                    attachments: vec![], // TODO: convert attachments
                                    reply_to: reply_to.map(MessageId::new),
                                };
                                if inbound_tx.send(msg).await.is_err() {
                                    warn!("Inbound channel closed for {}", channel_id);
                                    break;
                                }
                            }
                            Some(BridgeEvent::StatusChange { status }) => {
                                let new_status = match status.as_str() {
                                    "connected" => ChannelStatus::Connected,
                                    "disconnected" => ChannelStatus::Disconnected,
                                    _ => ChannelStatus::Error(status),
                                };
                                info.write().await.status = new_status;
                            }
                            Some(BridgeEvent::PairingUpdate(pairing)) => {
                                match &pairing {
                                    PairingEvent::Connected { .. } => {
                                        info.write().await.status = ChannelStatus::Connected;
                                    }
                                    PairingEvent::Failed { .. } => {
                                        info.write().await.status = ChannelStatus::Error(
                                            "Pairing failed".to_string()
                                        );
                                    }
                                    _ => {}
                                }
                                debug!("Pairing update for {}: {:?}", channel_id, pairing);
                            }
                            Some(BridgeEvent::Error { message }) => {
                                error!("Bridge error for {}: {}", channel_id, message);
                            }
                            Some(_) => {} // Ready, Receipt
                            None => {
                                info!("Event stream ended for {}", channel_id);
                                info.write().await.status = ChannelStatus::Disconnected;
                                break;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        info!("Shutdown signal received for {}", channel_id);
                        break;
                    }
                }
            }
        });

        shutdown_tx
    }
}

#[async_trait]
impl Channel for BridgedChannel {
    fn info(&self) -> &ChannelInfo {
        // This is a workaround since info is behind RwLock
        // In practice, we'd need to adjust the trait or use a different pattern
        // For now, we'll need to handle this at integration time
        unimplemented!("Use async info access pattern")
    }

    async fn start(&mut self) -> ChannelResult<()> {
        let transport = self.transport.as_ref()
            .ok_or_else(|| crate::gateway::channel::ChannelError::Other(
                "Transport not set".into()
            ))?
            .clone();

        self.set_status(ChannelStatus::Connecting).await;

        // Send start command to bridge
        transport
            .request("aleph.link.start", serde_json::json!({}))
            .await
            .map_err(|e| crate::gateway::channel::ChannelError::Other(e.to_string()))?;

        // Start event forwarding
        let channel_id = self.info.read().await.id.clone();
        let shutdown_tx = self.start_event_loop(transport, channel_id);
        self.shutdown_tx = Some(shutdown_tx);

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        // Signal event loop to stop
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Send stop command to bridge
        if let Some(transport) = &self.transport {
            let _ = transport
                .request("aleph.link.stop", serde_json::json!({}))
                .await;
            let _ = transport.close().await;
        }

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let transport = self.transport.as_ref()
            .ok_or_else(|| crate::gateway::channel::ChannelError::Other(
                "Transport not set".into()
            ))?;

        let params = serde_json::json!({
            "conversation_id": message.conversation_id.as_str(),
            "text": message.text,
        });

        let resp = transport
            .request("aleph.link.send", params)
            .await
            .map_err(|e| crate::gateway::channel::ChannelError::Other(e.to_string()))?;

        let message_id = resp
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(SendResult {
            message_id: MessageId::new(message_id),
            timestamp: chrono::Utc::now().timestamp(),
        })
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        // This follows the same pattern as WhatsApp channel
        // The receiver is taken once by ChannelRegistry
        None // Taken at construction via separate method
    }

    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        if let Some(transport) = &self.transport {
            let resp = transport
                .request("aleph.link.get_pairing", serde_json::json!({}))
                .await
                .map_err(|e| crate::gateway::channel::ChannelError::Other(e.to_string()))?;

            if let Some(qr) = resp.get("qr_data").and_then(|v| v.as_str()) {
                Ok(PairingData::QrCode(qr.to_string()))
            } else if let Some(code) = resp.get("code").and_then(|v| v.as_str()) {
                Ok(PairingData::Code(code.to_string()))
            } else {
                Ok(PairingData::None)
            }
        } else {
            Ok(PairingData::None)
        }
    }
}
```

**Step 3: Update bridge/mod.rs**

```rust
mod types;
pub mod bridged_channel;

pub use types::*;
pub use bridged_channel::BridgedChannel;
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::bridge -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/bridge/
git commit -m "feat(gateway): add BridgedChannel proxy for external bridges"
```

---

## Task 5: BridgeSupervisor — Process Lifecycle Manager

Extract and generalize the WhatsApp BridgeManager into a supervisor that manages multiple external bridge processes.

**Files:**
- Create: `core/src/gateway/bridge/supervisor.rs`
- Modify: `core/src/gateway/bridge/mod.rs`
- Reference: `core/src/gateway/interfaces/whatsapp/bridge_manager.rs`

**Step 1: Write failing test**

In `core/src/gateway/bridge/supervisor.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_supervisor_creation() {
        let tmp = TempDir::new().unwrap();
        let supervisor = BridgeSupervisor::new(tmp.path().to_path_buf());
        assert!(supervisor.list_processes().is_empty());
    }

    #[test]
    fn test_managed_process_config() {
        let config = ManagedProcessConfig {
            executable: PathBuf::from("/usr/local/bin/whatsapp-bridge"),
            args: vec!["--mode".into(), "jsonrpc".into()],
            transport_type: TransportType::UnixSocket,
            max_restarts: 5,
            restart_delay: Duration::from_secs(3),
            health_check_interval: Duration::from_secs(30),
            env_vars: HashMap::new(),
        };
        assert_eq!(config.max_restarts, 5);
    }
}
```

**Step 2: Implement BridgeSupervisor**

In `core/src/gateway/bridge/supervisor.rs`:
```rust
use super::types::{BridgeId, BridgeRuntime, TransportType};
use crate::gateway::link::LinkId;
use crate::gateway::transport::{Transport, UnixSocketTransport, StdioTransport};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Configuration for spawning a bridge process
#[derive(Debug, Clone)]
pub struct ManagedProcessConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub transport_type: TransportType,
    pub max_restarts: u32,
    pub restart_delay: Duration,
    pub health_check_interval: Duration,
    pub env_vars: HashMap<String, String>,
}

impl ManagedProcessConfig {
    pub fn from_runtime(runtime: &BridgeRuntime) -> Option<Self> {
        match runtime {
            BridgeRuntime::Process {
                executable, args, transport,
                health_check_interval_secs, max_restarts, restart_delay_secs,
            } => Some(Self {
                executable: executable.clone(),
                args: args.clone(),
                transport_type: transport.clone(),
                max_restarts: *max_restarts,
                restart_delay: Duration::from_secs(*restart_delay_secs),
                health_check_interval: Duration::from_secs(*health_check_interval_secs),
                env_vars: HashMap::new(),
            }),
            BridgeRuntime::Builtin => None,
        }
    }
}

/// Status of a managed bridge process
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    Starting,
    Running,
    Unhealthy,
    Restarting,
    Stopped,
    Failed(String),
}

/// A running bridge process managed by the supervisor
struct ManagedProcess {
    link_id: LinkId,
    child: Child,
    transport: Arc<dyn Transport>,
    config: ManagedProcessConfig,
    restart_count: u32,
    last_heartbeat: Instant,
    status: ProcessStatus,
}

/// Manages external bridge process lifecycle with heartbeat monitoring
/// and automatic restart on failure.
pub struct BridgeSupervisor {
    processes: RwLock<HashMap<LinkId, ManagedProcess>>,
    run_dir: PathBuf,
}

impl BridgeSupervisor {
    pub fn new(run_dir: PathBuf) -> Self {
        Self {
            processes: RwLock::new(HashMap::new()),
            run_dir,
        }
    }

    /// List all managed process IDs and their status
    pub fn list_processes(&self) -> Vec<(LinkId, ProcessStatus)> {
        // Synchronous snapshot for non-async contexts
        vec![]
    }

    /// List all managed processes (async)
    pub async fn list_processes_async(&self) -> Vec<(LinkId, ProcessStatus)> {
        self.processes
            .read()
            .await
            .iter()
            .map(|(id, p)| (id.clone(), p.status.clone()))
            .collect()
    }

    /// Spawn an external bridge process and return its Transport
    pub async fn spawn(
        &self,
        link_id: &LinkId,
        config: ManagedProcessConfig,
    ) -> Result<Arc<dyn Transport>, BridgeSupervisorError> {
        // Ensure run directory exists
        tokio::fs::create_dir_all(&self.run_dir).await
            .map_err(|e| BridgeSupervisorError::IoError(e.to_string()))?;

        let socket_path = self.run_dir.join(format!("{}.sock", link_id.as_str()));

        // Clean up stale socket
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await
                .map_err(|e| BridgeSupervisorError::IoError(e.to_string()))?;
        }

        // Validate binary exists
        let binary = &config.executable;
        if binary.is_absolute() && !binary.exists() {
            return Err(BridgeSupervisorError::BinaryNotFound(
                binary.display().to_string(),
            ));
        } else if !binary.is_absolute() {
            which::which(binary).map_err(|_| {
                BridgeSupervisorError::BinaryNotFound(binary.display().to_string())
            })?;
        }

        // Build command
        let mut cmd = Command::new(&config.executable);
        cmd.args(&config.args)
            .env("ALEPH_INSTANCE_ID", link_id.as_str())
            .env("ALEPH_SOCKET_PATH", &socket_path)
            .env("ALEPH_LOG_LEVEL", "info")
            .kill_on_drop(true);

        // Add custom env vars
        for (k, v) in &config.env_vars {
            cmd.env(k, v);
        }

        // Configure stdio for StdioTransport
        if matches!(config.transport_type, TransportType::Stdio) {
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit());
        }

        // Spawn process
        let mut child = cmd.spawn()
            .map_err(|e| BridgeSupervisorError::SpawnFailed(e.to_string()))?;

        info!(link_id = %link_id, pid = ?child.id(), "Bridge process spawned");

        // Create transport
        let transport: Arc<dyn Transport> = match config.transport_type {
            TransportType::UnixSocket => {
                // Wait for socket file to appear
                Self::wait_for_socket(&socket_path, Duration::from_secs(10)).await?;
                let t = UnixSocketTransport::new(&socket_path);
                t.connect(5, 500).await
                    .map_err(|e| BridgeSupervisorError::ConnectionFailed(e.to_string()))?;
                Arc::new(t)
            }
            TransportType::Stdio => {
                let stdin = child.stdin.take()
                    .ok_or_else(|| BridgeSupervisorError::SpawnFailed(
                        "Failed to capture stdin".into()
                    ))?;
                let stdout = child.stdout.take()
                    .ok_or_else(|| BridgeSupervisorError::SpawnFailed(
                        "Failed to capture stdout".into()
                    ))?;
                let t = StdioTransport::from_child(stdin, stdout);
                t.set_connected();
                Arc::new(t)
            }
        };

        // Perform handshake
        let handshake_result = transport
            .request("aleph.handshake", serde_json::json!({
                "protocol_version": "1.0",
                "instance_id": link_id.as_str(),
            }))
            .await
            .map_err(|e| BridgeSupervisorError::HandshakeFailed(e.to_string()))?;

        info!(
            link_id = %link_id,
            response = %handshake_result,
            "Bridge handshake completed"
        );

        // Store managed process
        let managed = ManagedProcess {
            link_id: link_id.clone(),
            child,
            transport: transport.clone(),
            config: config.clone(),
            restart_count: 0,
            last_heartbeat: Instant::now(),
            status: ProcessStatus::Running,
        };
        self.processes.write().await.insert(link_id.clone(), managed);

        // Start health monitoring
        self.start_health_monitor(link_id.clone(), config.health_check_interval);

        Ok(transport)
    }

    /// Stop and remove a managed process
    pub async fn stop(&self, link_id: &LinkId) -> Result<(), BridgeSupervisorError> {
        let mut processes = self.processes.write().await;
        if let Some(mut process) = processes.remove(link_id) {
            let _ = process.transport.close().await;
            process.child.kill().await
                .map_err(|e| BridgeSupervisorError::IoError(e.to_string()))?;

            // Clean up socket file
            let socket_path = self.run_dir.join(format!("{}.sock", link_id.as_str()));
            if socket_path.exists() {
                let _ = tokio::fs::remove_file(&socket_path).await;
            }

            info!(link_id = %link_id, "Bridge process stopped");
            Ok(())
        } else {
            Ok(()) // Already stopped
        }
    }

    /// Stop all managed processes
    pub async fn stop_all(&self) {
        let link_ids: Vec<LinkId> = self.processes.read().await.keys().cloned().collect();
        for id in link_ids {
            if let Err(e) = self.stop(&id).await {
                error!(link_id = %id, error = %e, "Failed to stop bridge");
            }
        }
    }

    fn start_health_monitor(&self, link_id: LinkId, interval: Duration) {
        let processes = self.processes.clone();
        let run_dir = self.run_dir.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let transport = {
                    let procs = processes.read().await;
                    match procs.get(&link_id) {
                        Some(p) if p.status == ProcessStatus::Running => {
                            Some(p.transport.clone())
                        }
                        Some(_) => continue,
                        None => break, // Process removed
                    }
                };

                if let Some(transport) = transport {
                    match transport.request("system.ping", serde_json::json!({})).await {
                        Ok(_) => {
                            let mut procs = processes.write().await;
                            if let Some(p) = procs.get_mut(&link_id) {
                                p.last_heartbeat = Instant::now();
                            }
                        }
                        Err(e) => {
                            warn!(
                                link_id = %link_id,
                                error = %e,
                                "Heartbeat failed"
                            );
                            let mut procs = processes.write().await;
                            if let Some(p) = procs.get_mut(&link_id) {
                                p.status = ProcessStatus::Unhealthy;
                                // TODO: trigger restart logic
                            }
                        }
                    }
                }
            }
        });
    }

    async fn wait_for_socket(
        path: &Path,
        timeout: Duration,
    ) -> Result<(), BridgeSupervisorError> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if path.exists() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(BridgeSupervisorError::ConnectionFailed(format!(
            "Socket {} did not appear within {:?}",
            path.display(),
            timeout
        )))
    }
}

impl Drop for BridgeSupervisor {
    fn drop(&mut self) {
        // Best-effort cleanup: tokio tasks will be cancelled when runtime drops
        info!("BridgeSupervisor dropped, managed processes will be cleaned up");
    }
}

/// Errors from the BridgeSupervisor
#[derive(Debug, thiserror::Error)]
pub enum BridgeSupervisorError {
    #[error("Binary not found: {0}")]
    BinaryNotFound(String),
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Max restarts exceeded: {0}")]
    MaxRestartsExceeded(u32),
}
```

**Step 3: Update bridge/mod.rs**

```rust
mod types;
pub mod bridged_channel;
pub mod supervisor;

pub use types::*;
pub use bridged_channel::BridgedChannel;
pub use supervisor::{BridgeSupervisor, BridgeSupervisorError, ManagedProcessConfig, ProcessStatus};
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::bridge::supervisor::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/bridge/
git commit -m "feat(gateway): add BridgeSupervisor for external bridge process management"
```

---

## Task 6: LinkManager — Configuration Scanner and Lifecycle Orchestrator

The central component that ties everything together.

**Files:**
- Create: `core/src/gateway/link/manager.rs`
- Modify: `core/src/gateway/link/mod.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Write failing test**

In `core/src/gateway/link/manager.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_scan_link_configs() {
        let tmp = TempDir::new().unwrap();
        let links_dir = tmp.path().join("links");
        std::fs::create_dir_all(&links_dir).unwrap();

        // Write a test link.yaml
        std::fs::write(
            links_dir.join("test-telegram.yaml"),
            r#"
spec_version: "1.0"
id: "test-telegram"
bridge: "telegram-native"
name: "Test Bot"
enabled: true
settings:
  token: "fake"
routing:
  agent: "main"
"#,
        ).unwrap();

        let configs = scan_link_configs(&links_dir).await.unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].id.as_str(), "test-telegram");
        assert!(configs[0].enabled);
    }

    #[tokio::test]
    async fn test_scan_bridge_definitions() {
        let tmp = TempDir::new().unwrap();
        let bridges_dir = tmp.path().join("bridges");
        let whatsapp_dir = bridges_dir.join("whatsapp-go");
        std::fs::create_dir_all(&whatsapp_dir).unwrap();

        std::fs::write(
            whatsapp_dir.join("bridge.yaml"),
            r#"
spec_version: "1.0"
id: "whatsapp-go"
name: "WhatsApp"
version: "1.0.0"
runtime:
  type: process
  executable: "./bin/whatsapp-bridge"
  transport: unix-socket
capabilities:
  messaging:
    - send_text
"#,
        ).unwrap();

        let defs = scan_bridge_definitions(&bridges_dir).await.unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id.as_str(), "whatsapp-go");
    }

    #[tokio::test]
    async fn test_expand_env_vars() {
        std::env::set_var("TEST_TOKEN_123", "secret-value");
        let settings = serde_json::json!({
            "token": "${env.TEST_TOKEN_123}",
            "name": "no-expansion",
        });
        let expanded = expand_env_vars(&settings);
        assert_eq!(expanded.get("token").unwrap().as_str().unwrap(), "secret-value");
        assert_eq!(expanded.get("name").unwrap().as_str().unwrap(), "no-expansion");
        std::env::remove_var("TEST_TOKEN_123");
    }
}
```

**Step 2: Implement LinkManager**

In `core/src/gateway/link/manager.rs`:
```rust
use super::types::{LinkConfig, LinkId};
use crate::gateway::bridge::{
    BridgeDefinition, BridgeId, BridgeRuntime, BridgeSupervisor, BridgedChannel,
    ManagedProcessConfig,
};
use crate::gateway::channel::{ChannelFactory, ChannelRegistry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Scan ~/.aleph/links/ for link.yaml files
pub async fn scan_link_configs(dir: &Path) -> Result<Vec<LinkConfig>, LinkManagerError> {
    let mut configs = Vec::new();

    if !dir.exists() {
        return Ok(configs);
    }

    let mut entries = tokio::fs::read_dir(dir).await
        .map_err(|e| LinkManagerError::IoError(e.to_string()))?;

    while let Some(entry) = entries.next_entry().await
        .map_err(|e| LinkManagerError::IoError(e.to_string()))?
    {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => match serde_yaml::from_str::<LinkConfig>(&content) {
                    Ok(config) => {
                        info!(path = %path.display(), id = %config.id, "Loaded link config");
                        configs.push(config);
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to parse link config");
                    }
                },
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to read link config");
                }
            }
        }
    }

    Ok(configs)
}

/// Scan ~/.aleph/bridges/ for bridge.yaml files
pub async fn scan_bridge_definitions(dir: &Path) -> Result<Vec<BridgeDefinition>, LinkManagerError> {
    let mut defs = Vec::new();

    if !dir.exists() {
        return Ok(defs);
    }

    let mut entries = tokio::fs::read_dir(dir).await
        .map_err(|e| LinkManagerError::IoError(e.to_string()))?;

    while let Some(entry) = entries.next_entry().await
        .map_err(|e| LinkManagerError::IoError(e.to_string()))?
    {
        let path = entry.path();
        if path.is_dir() {
            let bridge_yaml = path.join("bridge.yaml");
            if bridge_yaml.exists() {
                match tokio::fs::read_to_string(&bridge_yaml).await {
                    Ok(content) => match serde_yaml::from_str::<BridgeDefinition>(&content) {
                        Ok(def) => {
                            info!(path = %bridge_yaml.display(), id = %def.id, "Loaded bridge definition");
                            defs.push(def);
                        }
                        Err(e) => {
                            warn!(path = %bridge_yaml.display(), error = %e, "Failed to parse bridge.yaml");
                        }
                    },
                    Err(e) => {
                        warn!(path = %bridge_yaml.display(), error = %e, "Failed to read bridge.yaml");
                    }
                }
            }
        }
    }

    Ok(defs)
}

/// Expand ${env.VAR_NAME} references in settings
pub fn expand_env_vars(settings: &serde_json::Value) -> serde_json::Value {
    match settings {
        serde_json::Value::String(s) => {
            if let Some(var_name) = s.strip_prefix("${env.")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                match std::env::var(var_name) {
                    Ok(val) => serde_json::Value::String(val),
                    Err(_) => {
                        warn!("Environment variable {} not set", var_name);
                        settings.clone()
                    }
                }
            } else {
                settings.clone()
            }
        }
        serde_json::Value::Object(map) => {
            let expanded: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), expand_env_vars(v)))
                .collect();
            serde_json::Value::Object(expanded)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(expand_env_vars).collect())
        }
        _ => settings.clone(),
    }
}

/// Central orchestrator for the social connectivity plugin system
pub struct LinkManager {
    /// Registered bridge type definitions
    bridge_registry: RwLock<HashMap<BridgeId, BridgeDefinition>>,

    /// Builtin channel factories (Telegram, Discord, iMessage)
    builtin_factories: RwLock<HashMap<BridgeId, Arc<dyn ChannelFactory>>>,

    /// Active channel instances
    channel_registry: Arc<ChannelRegistry>,

    /// External bridge process supervisor
    bridge_supervisor: Arc<BridgeSupervisor>,

    /// Configuration directory paths
    base_dir: PathBuf,
}

impl LinkManager {
    pub fn new(channel_registry: Arc<ChannelRegistry>, base_dir: PathBuf) -> Self {
        let run_dir = base_dir.join("run");
        Self {
            bridge_registry: RwLock::new(HashMap::new()),
            builtin_factories: RwLock::new(HashMap::new()),
            channel_registry,
            bridge_supervisor: Arc::new(BridgeSupervisor::new(run_dir)),
            base_dir,
        }
    }

    /// Register a builtin bridge type with its factory
    pub async fn register_builtin(
        &self,
        definition: BridgeDefinition,
        factory: Arc<dyn ChannelFactory>,
    ) {
        let id = definition.id.clone();
        self.bridge_registry.write().await.insert(id.clone(), definition);
        self.builtin_factories.write().await.insert(id.clone(), factory);
        info!(bridge_id = %id, "Registered builtin bridge");
    }

    /// Full startup sequence
    pub async fn start(&self) -> Result<(), LinkManagerError> {
        // 1. Scan external bridge definitions
        let bridges_dir = self.base_dir.join("bridges");
        let external_defs = scan_bridge_definitions(&bridges_dir).await?;
        for def in external_defs {
            let id = def.id.clone();
            self.bridge_registry.write().await.insert(id.clone(), def);
            info!(bridge_id = %id, "Registered external bridge");
        }

        // 2. Scan link configs
        let links_dir = self.base_dir.join("links");
        let link_configs = scan_link_configs(&links_dir).await?;

        // 3. Instantiate and start enabled links
        for link in link_configs {
            if !link.enabled {
                info!(link_id = %link.id, "Skipping disabled link");
                continue;
            }

            if let Err(e) = self.create_and_start_link(&link).await {
                error!(
                    link_id = %link.id,
                    bridge = %link.bridge,
                    error = %e,
                    "Failed to start link"
                );
            }
        }

        info!("LinkManager startup complete");
        Ok(())
    }

    async fn create_and_start_link(&self, link: &LinkConfig) -> Result<(), LinkManagerError> {
        let bridge_registry = self.bridge_registry.read().await;
        let bridge = bridge_registry.get(&link.bridge)
            .ok_or_else(|| LinkManagerError::BridgeNotFound(link.bridge.to_string()))?;

        // Expand environment variables in settings
        let expanded_settings = expand_env_vars(&link.settings);

        match &bridge.runtime {
            BridgeRuntime::Builtin => {
                let factories = self.builtin_factories.read().await;
                let factory = factories.get(&link.bridge)
                    .ok_or_else(|| LinkManagerError::FactoryNotFound(link.bridge.to_string()))?;

                let channel = factory.create(expanded_settings).await
                    .map_err(|e| LinkManagerError::ChannelCreationFailed(e.to_string()))?;

                let channel_id = self.channel_registry.register(channel).await;
                self.channel_registry.start_channel(&channel_id).await
                    .map_err(|e| LinkManagerError::ChannelStartFailed(e.to_string()))?;

                info!(link_id = %link.id, channel_id = %channel_id, "Started builtin link");
            }
            BridgeRuntime::Process { .. } => {
                let process_config = ManagedProcessConfig::from_runtime(&bridge.runtime)
                    .ok_or_else(|| LinkManagerError::InvalidRuntime("Expected process runtime".into()))?;

                let transport = self.bridge_supervisor
                    .spawn(&link.id, process_config)
                    .await
                    .map_err(|e| LinkManagerError::BridgeSpawnFailed(e.to_string()))?;

                let mut bridged = BridgedChannel::new(
                    link.id.as_str(),
                    &link.name,
                    link.bridge.as_str(),
                );
                bridged.set_transport(transport);

                let channel_id = self.channel_registry.register(Box::new(bridged)).await;
                self.channel_registry.start_channel(&channel_id).await
                    .map_err(|e| LinkManagerError::ChannelStartFailed(e.to_string()))?;

                info!(link_id = %link.id, channel_id = %channel_id, "Started external bridge link");
            }
        }

        Ok(())
    }

    /// Stop all links and bridge processes
    pub async fn stop(&self) {
        self.bridge_supervisor.stop_all().await;
        self.channel_registry.stop_all().await;
        info!("LinkManager stopped all links");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LinkManagerError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Bridge not found: {0}")]
    BridgeNotFound(String),
    #[error("Factory not found: {0}")]
    FactoryNotFound(String),
    #[error("Invalid runtime: {0}")]
    InvalidRuntime(String),
    #[error("Channel creation failed: {0}")]
    ChannelCreationFailed(String),
    #[error("Channel start failed: {0}")]
    ChannelStartFailed(String),
    #[error("Bridge spawn failed: {0}")]
    BridgeSpawnFailed(String),
    #[error("Config parse error: {0}")]
    ConfigParseError(String),
}
```

**Step 3: Update link/mod.rs**

```rust
mod types;
pub mod manager;

pub use types::*;
pub use manager::{LinkManager, LinkManagerError};
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::link::manager::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/link/
git commit -m "feat(gateway): add LinkManager for config scanning and lifecycle orchestration"
```

---

## Task 7: Refactor Server Startup

Replace hardcoded channel initialization in `start.rs` with LinkManager.

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start.rs` (lines ~617-680)
- Modify: `core/src/gateway/mod.rs` (add re-exports)

**Step 1: Read current start.rs to confirm exact line numbers**

Read: `core/src/bin/aleph_server/commands/start.rs` lines 610-740

**Step 2: Replace hardcoded channel init with LinkManager**

In `start.rs`, replace the channel initialization block (the `#[cfg(feature = "...")]` blocks) with:

```rust
// ── Channel System: LinkManager ──
let link_manager = {
    let base_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph");
    Arc::new(LinkManager::new(channel_registry.clone(), base_dir))
};

// Register builtin bridge types
#[cfg(feature = "telegram")]
{
    use crate::gateway::channels::telegram::TelegramChannelFactory;
    let bridge_def = BridgeDefinition {
        spec_version: "1.0".into(),
        id: BridgeId::new("telegram-native"),
        name: "Telegram".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        author: Some("Aleph Team".into()),
        description: Some("Native Telegram bot integration".into()),
        runtime: BridgeRuntime::Builtin,
        capabilities: Default::default(),
        settings_schema: None,
    };
    link_manager.register_builtin(bridge_def, Arc::new(TelegramChannelFactory)).await;
}

#[cfg(feature = "discord")]
{
    use crate::gateway::channels::discord::DiscordChannelFactory;
    let bridge_def = BridgeDefinition {
        spec_version: "1.0".into(),
        id: BridgeId::new("discord-native"),
        name: "Discord".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        runtime: BridgeRuntime::Builtin,
        ..Default::default()  // Only if Default is derived
    };
    link_manager.register_builtin(bridge_def, Arc::new(DiscordChannelFactory)).await;
}

#[cfg(target_os = "macos")]
{
    use crate::gateway::channels::imessage::IMessageChannelFactory;
    let bridge_def = BridgeDefinition {
        spec_version: "1.0".into(),
        id: BridgeId::new("imessage-native"),
        name: "iMessage".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        runtime: BridgeRuntime::Builtin,
        ..Default::default()
    };
    link_manager.register_builtin(bridge_def, Arc::new(IMessageChannelFactory)).await;
}

// Start LinkManager (scans ~/.aleph/links/ and ~/.aleph/bridges/)
if let Err(e) = link_manager.start().await {
    error!("LinkManager startup failed: {}", e);
}
```

**Step 3: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo build -p alephcore 2>&1 | tail -20`
Expected: Build succeeds (possibly with warnings)

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/commands/start.rs core/src/gateway/mod.rs
git commit -m "refactor(server): replace hardcoded channel init with LinkManager"
```

---

## Task 8: Add serde_yaml and Required Dependencies

Ensure all new dependencies are in Cargo.toml.

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Check existing deps**

Run: `grep -E "serde_yaml|notify|jsonschema|which|thiserror|tempfile" core/Cargo.toml`

**Step 2: Add missing dependencies**

If `serde_yaml` is missing:
```toml
serde_yaml = "0.9"
```

If `notify` is missing (for hot-reload, Phase 1 optional):
```toml
notify = { version = "7", optional = true }
```

If `which` is missing (for binary path resolution):
```toml
which = "7"
```

If `thiserror` is missing:
```toml
thiserror = "2"
```

Dev dependency for tests:
```toml
[dev-dependencies]
tempfile = "3"
```

**Step 3: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo build -p alephcore 2>&1 | tail -20`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add serde_yaml, which, thiserror for social connectivity"
```

---

## Task 9: Integration Test — Full Link Lifecycle

Write an integration test that creates a LinkManager, registers a mock builtin bridge, and verifies the full lifecycle.

**Files:**
- Create: `core/tests/integration/link_manager_test.rs` (or inline in manager.rs)

**Step 1: Write integration test**

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::TempDir;

    /// A simple mock channel factory for testing
    struct MockChannelFactory;

    #[async_trait::async_trait]
    impl ChannelFactory for MockChannelFactory {
        fn channel_type(&self) -> &str { "mock" }

        async fn create(
            &self,
            config: serde_json::Value,
        ) -> crate::gateway::channel::ChannelResult<Box<dyn crate::gateway::channel::Channel>> {
            // Return a mock channel
            todo!("Implement mock channel for testing")
        }
    }

    #[tokio::test]
    async fn test_link_manager_full_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let base_dir = tmp.path().to_path_buf();

        // Create directory structure
        let links_dir = base_dir.join("links");
        let bridges_dir = base_dir.join("bridges");
        tokio::fs::create_dir_all(&links_dir).await.unwrap();
        tokio::fs::create_dir_all(&bridges_dir).await.unwrap();

        // Write a link config
        tokio::fs::write(
            links_dir.join("test.yaml"),
            r#"
spec_version: "1.0"
id: "test-instance"
bridge: "mock-builtin"
name: "Test"
enabled: true
settings:
  key: "value"
"#,
        ).await.unwrap();

        // Create LinkManager
        let registry = Arc::new(ChannelRegistry::new());
        let manager = LinkManager::new(registry.clone(), base_dir);

        // Register mock builtin bridge
        let bridge_def = BridgeDefinition {
            spec_version: "1.0".into(),
            id: BridgeId::new("mock-builtin"),
            name: "Mock".into(),
            version: "0.1.0".into(),
            author: None,
            description: None,
            runtime: BridgeRuntime::Builtin,
            capabilities: Default::default(),
            settings_schema: None,
        };
        manager.register_builtin(bridge_def, Arc::new(MockChannelFactory)).await;

        // Start (will try to create channel from link config)
        // This will fail at factory.create() since MockChannelFactory is todo!()
        // But the scanning and registration logic will be validated
        let result = manager.start().await;
        // Expected: Error from mock factory, but scanning succeeded
    }
}
```

**Step 2: Run test**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore link_manager -- --nocapture`

**Step 3: Commit**

```bash
git add core/tests/ core/src/gateway/link/manager.rs
git commit -m "test: add LinkManager integration test"
```

---

## Task 10: Create Default Link Configs for Existing Channels

Create example/default `link.yaml` files so existing users can migrate.

**Files:**
- Create: `core/resources/default-links/telegram.yaml`
- Create: `core/resources/default-links/discord.yaml`
- Create: `core/resources/default-links/whatsapp.yaml`
- Create: `core/resources/default-links/imessage.yaml`

**Step 1: Create default link configs**

`core/resources/default-links/telegram.yaml`:
```yaml
spec_version: "1.0"
id: "telegram"
bridge: "telegram-native"
name: "Telegram Bot"
enabled: false

settings:
  token: "${env.TELEGRAM_BOT_TOKEN}"
  allowed_users: []
  polling_mode: "long_polling"

routing:
  agent: "main"
  dm_policy: "pairing"
  group_policy: "disabled"
```

`core/resources/default-links/whatsapp.yaml`:
```yaml
spec_version: "1.0"
id: "whatsapp"
bridge: "whatsapp-go"
name: "WhatsApp"
enabled: false

settings:
  phone_number: ""
  max_restarts: 5

routing:
  agent: "main"
  dm_policy: "pairing"
  group_policy: "disabled"
```

`core/resources/default-links/discord.yaml`:
```yaml
spec_version: "1.0"
id: "discord"
bridge: "discord-native"
name: "Discord Bot"
enabled: false

settings:
  token: "${env.DISCORD_BOT_TOKEN}"
  allowed_guilds: []

routing:
  agent: "main"
  dm_policy: "open"
  group_policy: "allowlist"
```

`core/resources/default-links/imessage.yaml`:
```yaml
spec_version: "1.0"
id: "imessage"
bridge: "imessage-native"
name: "iMessage"
enabled: false

settings:
  poll_interval_secs: 5

routing:
  agent: "main"
  dm_policy: "pairing"
  group_policy: "disabled"
```

**Step 2: Commit**

```bash
git add core/resources/default-links/
git commit -m "feat: add default link.yaml configs for existing channels"
```

---

## Task 11: Documentation Update

Update CLAUDE.md and create BRIDGED_PLATFORM.md.

**Files:**
- Modify: `CLAUDE.md` (add Social Link section to architecture)
- Create: `docs/SOCIAL_CONNECTIVITY.md`

**Step 1: Write Social Connectivity doc**

Brief overview doc referencing the design document for details. Cover: how to add a new bridge, how to configure a link, directory structure.

**Step 2: Update CLAUDE.md architecture section**

Add to the architecture overview table:
```
| **Social Connectivity** | LinkManager, BridgeSupervisor, bridge.yaml/link.yaml 插件系统 | [Social Connectivity](docs/SOCIAL_CONNECTIVITY.md) |
```

**Step 3: Commit**

```bash
git add docs/SOCIAL_CONNECTIVITY.md CLAUDE.md
git commit -m "docs: add social connectivity architecture documentation"
```

---

## Summary of All Tasks

| # | Task | Files | Estimated Steps |
|---|------|-------|----------------|
| 1 | Data Models (BridgeDefinition, LinkConfig) | 4 new files | 9 steps |
| 2 | Transport Trait + UnixSocketTransport | 3 new files | 6 steps |
| 3 | StdioTransport | 1 new file | 5 steps |
| 4 | BridgedChannel | 1 new file | 5 steps |
| 5 | BridgeSupervisor | 1 new file | 5 steps |
| 6 | LinkManager | 1 new file | 5 steps |
| 7 | Refactor Server Startup | 2 modified files | 4 steps |
| 8 | Dependencies | 1 modified file | 4 steps |
| 9 | Integration Tests | 1 new file | 3 steps |
| 10 | Default Link Configs | 4 new files | 2 steps |
| 11 | Documentation | 2 files | 3 steps |

**Total: ~51 steps, 11 commits**

**Critical path:** Tasks 1→2→3→4→5→6→7 (sequential, each builds on the previous)
**Parallelizable:** Tasks 8, 10, 11 can run alongside any task

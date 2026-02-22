# WhatsApp Bridge Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the WhatsApp stub with a real protocol adapter using a whatsmeow Go Sidecar, managed as a child process with JSON-RPC over Unix Socket communication.

**Architecture:** Thin Go Sidecar (whatsmeow wrapper, < 800 LOC) + Rich Rust Adapter (BridgeManager, BridgeRpcClient, PairingStateMachine). Go process is auto-spawned/restarted by Rust. All business logic lives in Rust.

**Tech Stack:** Rust (tokio, serde, async-trait), Go (whatsmeow, go.mau.fi/whatsmeow), JSON-RPC 2.0 over Unix Domain Socket, Leptos (Dashboard WASM UI)

**Design Doc:** `docs/plans/2026-02-22-whatsapp-bridge-design.md`

---

## Task 1: PairingState Enum and State Machine

The pairing state machine is pure Rust with no external dependencies — ideal to build first with TDD.

**Files:**
- Create: `core/src/gateway/channels/whatsapp/pairing.rs`
- Modify: `core/src/gateway/channels/whatsapp/mod.rs:24` (add `pub mod pairing;`)

**Step 1: Write the PairingState enum and transition tests**

```rust
// core/src/gateway/channels/whatsapp/pairing.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Fine-grained pairing lifecycle state for WhatsApp
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingState {
    /// Bridge not started
    Idle,
    /// Bridge process is starting
    Initializing,
    /// QR code generated, waiting for user scan
    WaitingQr {
        qr_data: String,
        expires_at: DateTime<Utc>,
    },
    /// QR code expired, waiting for refresh
    QrExpired,
    /// User scanned, waiting for phone confirmation
    Scanned,
    /// Syncing encryption keys and contacts
    Syncing { progress: f32 },
    /// Fully connected
    Connected {
        device_name: String,
        phone_number: String,
    },
    /// Disconnected (reconnectable)
    Disconnected { reason: String },
    /// Unrecoverable error
    Failed { error: String },
}

impl PairingState {
    /// Map to coarse-grained ChannelStatus
    pub fn to_channel_status(&self) -> crate::gateway::channel::ChannelStatus {
        use crate::gateway::channel::ChannelStatus;
        match self {
            PairingState::Idle => ChannelStatus::Disconnected,
            PairingState::Initializing
            | PairingState::WaitingQr { .. }
            | PairingState::QrExpired
            | PairingState::Scanned
            | PairingState::Syncing { .. } => ChannelStatus::Connecting,
            PairingState::Connected { .. } => ChannelStatus::Connected,
            PairingState::Disconnected { .. } => ChannelStatus::Disconnected,
            PairingState::Failed { .. } => ChannelStatus::Error,
        }
    }

    /// Check if this state allows receiving messages
    pub fn is_connected(&self) -> bool {
        matches!(self, PairingState::Connected { .. })
    }

    /// Human-readable status description
    pub fn description(&self) -> &str {
        match self {
            PairingState::Idle => "Not started",
            PairingState::Initializing => "Starting bridge...",
            PairingState::WaitingQr { .. } => "Scan QR code to connect",
            PairingState::QrExpired => "QR code expired, refreshing...",
            PairingState::Scanned => "Scanned! Waiting for confirmation...",
            PairingState::Syncing { .. } => "Syncing encryption keys...",
            PairingState::Connected { .. } => "Connected",
            PairingState::Disconnected { .. } => "Disconnected",
            PairingState::Failed { .. } => "Connection failed",
        }
    }
}

/// Validates state transitions. Returns Ok(()) if transition is valid.
pub fn validate_transition(from: &PairingState, to: &PairingState) -> Result<(), String> {
    use PairingState::*;
    let valid = match (from, to) {
        // From Idle
        (Idle, Initializing) => true,
        // From Initializing
        (Initializing, WaitingQr { .. }) => true,
        (Initializing, Failed { .. }) => true,
        // From WaitingQr
        (WaitingQr { .. }, Scanned) => true,
        (WaitingQr { .. }, QrExpired) => true,
        (WaitingQr { .. }, Failed { .. }) => true,
        // From QrExpired
        (QrExpired, WaitingQr { .. }) => true,
        (QrExpired, Failed { .. }) => true,
        // From Scanned
        (Scanned, Syncing { .. }) => true,
        (Scanned, Failed { .. }) => true,
        // From Syncing
        (Syncing { .. }, Syncing { .. }) => true, // progress update
        (Syncing { .. }, Connected { .. }) => true,
        (Syncing { .. }, Failed { .. }) => true,
        // From Connected
        (Connected { .. }, Disconnected { .. }) => true,
        // From Disconnected
        (Disconnected { .. }, Initializing) => true,
        (Disconnected { .. }, Idle) => true,
        // From Failed
        (Failed { .. }, Idle) => true,
        // Everything else is invalid
        _ => false,
    };

    if valid {
        Ok(())
    } else {
        Err(format!(
            "Invalid state transition: {:?} -> {:?}",
            std::mem::discriminant(from),
            std::mem::discriminant(to)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_channel_status_mapping() {
        use crate::gateway::channel::ChannelStatus;

        assert_eq!(PairingState::Idle.to_channel_status(), ChannelStatus::Disconnected);
        assert_eq!(PairingState::Initializing.to_channel_status(), ChannelStatus::Connecting);
        assert_eq!(
            PairingState::WaitingQr {
                qr_data: "test".into(),
                expires_at: Utc::now(),
            }
            .to_channel_status(),
            ChannelStatus::Connecting
        );
        assert_eq!(PairingState::QrExpired.to_channel_status(), ChannelStatus::Connecting);
        assert_eq!(PairingState::Scanned.to_channel_status(), ChannelStatus::Connecting);
        assert_eq!(
            PairingState::Syncing { progress: 0.5 }.to_channel_status(),
            ChannelStatus::Connecting
        );
        assert_eq!(
            PairingState::Connected {
                device_name: "iPhone".into(),
                phone_number: "+1234".into(),
            }
            .to_channel_status(),
            ChannelStatus::Connected
        );
        assert_eq!(
            PairingState::Disconnected {
                reason: "timeout".into(),
            }
            .to_channel_status(),
            ChannelStatus::Disconnected
        );
        assert_eq!(
            PairingState::Failed {
                error: "auth".into(),
            }
            .to_channel_status(),
            ChannelStatus::Error
        );
    }

    #[test]
    fn test_valid_transitions() {
        let now = Utc::now();
        // Happy path: Idle -> Initializing -> WaitingQr -> Scanned -> Syncing -> Connected
        assert!(validate_transition(&PairingState::Idle, &PairingState::Initializing).is_ok());
        assert!(validate_transition(
            &PairingState::Initializing,
            &PairingState::WaitingQr {
                qr_data: "qr".into(),
                expires_at: now,
            }
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::WaitingQr {
                qr_data: "qr".into(),
                expires_at: now,
            },
            &PairingState::Scanned,
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::Scanned,
            &PairingState::Syncing { progress: 0.0 },
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::Syncing { progress: 0.5 },
            &PairingState::Syncing { progress: 0.8 },
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::Syncing { progress: 1.0 },
            &PairingState::Connected {
                device_name: "iPhone".into(),
                phone_number: "+1".into(),
            },
        )
        .is_ok());
    }

    #[test]
    fn test_qr_expired_refresh_cycle() {
        let now = Utc::now();
        // WaitingQr -> QrExpired -> WaitingQr (refresh)
        assert!(validate_transition(
            &PairingState::WaitingQr {
                qr_data: "old".into(),
                expires_at: now,
            },
            &PairingState::QrExpired,
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::QrExpired,
            &PairingState::WaitingQr {
                qr_data: "new".into(),
                expires_at: now,
            },
        )
        .is_ok());
    }

    #[test]
    fn test_reconnection_path() {
        // Connected -> Disconnected -> Initializing
        assert!(validate_transition(
            &PairingState::Connected {
                device_name: "x".into(),
                phone_number: "y".into(),
            },
            &PairingState::Disconnected {
                reason: "timeout".into(),
            },
        )
        .is_ok());
        assert!(validate_transition(
            &PairingState::Disconnected {
                reason: "timeout".into(),
            },
            &PairingState::Initializing,
        )
        .is_ok());
    }

    #[test]
    fn test_invalid_transitions() {
        // Cannot go directly from Idle to Connected
        assert!(validate_transition(
            &PairingState::Idle,
            &PairingState::Connected {
                device_name: "x".into(),
                phone_number: "y".into(),
            },
        )
        .is_err());
        // Cannot go from Connected to Initializing (must go through Disconnected)
        assert!(validate_transition(
            &PairingState::Connected {
                device_name: "x".into(),
                phone_number: "y".into(),
            },
            &PairingState::Initializing,
        )
        .is_err());
        // Cannot go from Scanned back to WaitingQr
        assert!(validate_transition(
            &PairingState::Scanned,
            &PairingState::WaitingQr {
                qr_data: "x".into(),
                expires_at: Utc::now(),
            },
        )
        .is_err());
    }

    #[test]
    fn test_is_connected() {
        assert!(!PairingState::Idle.is_connected());
        assert!(!PairingState::Syncing { progress: 0.9 }.is_connected());
        assert!(PairingState::Connected {
            device_name: "x".into(),
            phone_number: "y".into(),
        }
        .is_connected());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let state = PairingState::WaitingQr {
            qr_data: "base64data".into(),
            expires_at: Utc::now(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: PairingState = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, PairingState::WaitingQr { .. }));
    }
}
```

**Step 2: Register the module**

Add `pub mod pairing;` to `core/src/gateway/channels/whatsapp/mod.rs` after line 24.

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp::pairing`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/channels/whatsapp/pairing.rs core/src/gateway/channels/whatsapp/mod.rs
git commit -m "whatsapp: add PairingState enum and state machine with tests"
```

---

## Task 2: Bridge RPC Protocol Types

Define the JSON-RPC message types used between Rust and Go.

**Files:**
- Create: `core/src/gateway/channels/whatsapp/bridge_protocol.rs`
- Modify: `core/src/gateway/channels/whatsapp/mod.rs` (add `pub mod bridge_protocol;`)

**Step 1: Write protocol types and serialization tests**

```rust
// core/src/gateway/channels/whatsapp/bridge_protocol.rs

//! JSON-RPC protocol types for Rust <-> Go Bridge communication.

use serde::{Deserialize, Serialize};

// --- Rust -> Go Commands ---

/// Connect to WhatsApp (start pairing)
#[derive(Debug, Serialize)]
pub struct ConnectRequest {}

/// Disconnect from WhatsApp
#[derive(Debug, Serialize)]
pub struct DisconnectRequest {}

/// Send a message
#[derive(Debug, Serialize)]
pub struct SendRequest {
    pub to: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Media attachment for sending
#[derive(Debug, Serialize, Deserialize)]
pub struct MediaPayload {
    pub mime_type: String,
    pub data: String, // base64 encoded
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Health check ping
#[derive(Debug, Serialize)]
pub struct PingRequest {}

/// Query bridge status
#[derive(Debug, Serialize)]
pub struct StatusRequest {}

// --- Go -> Rust Responses ---

/// Generic OK response
#[derive(Debug, Deserialize)]
pub struct OkResponse {
    pub ok: bool,
}

/// Send message response
#[derive(Debug, Deserialize)]
pub struct SendResponse {
    pub id: String,
}

/// Ping response
#[derive(Debug, Deserialize)]
pub struct PingResponse {
    pub pong: bool,
    pub rtt_ms: Option<u64>,
}

/// Status response
#[derive(Debug, Deserialize)]
pub struct BridgeStatusResponse {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
}

// --- Go -> Rust Event Push ---

/// Events pushed from the Go bridge to Rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// New QR code generated
    Qr {
        qr_data: String,
        expires_in_secs: u64,
    },
    /// QR code expired
    QrExpired,
    /// User scanned the QR code
    Scanned,
    /// Syncing in progress
    Syncing { progress: f32 },
    /// Connection established
    Connected {
        device_name: String,
        phone_number: String,
    },
    /// Connection lost
    Disconnected { reason: String },
    /// Inbound message received
    Message {
        from: String,
        from_name: Option<String>,
        chat_id: String,
        text: String,
        media: Option<MediaPayload>,
        timestamp: i64,
        message_id: String,
        is_group: bool,
        reply_to: Option<String>,
    },
    /// Read/delivered receipt
    Receipt {
        message_id: String,
        receipt_type: String, // "read", "delivered"
    },
    /// Bridge ready (started successfully)
    Ready,
    /// Bridge error
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_request_serialization() {
        let req = SendRequest {
            to: "1234567890@s.whatsapp.net".into(),
            text: "Hello!".into(),
            media: None,
            reply_to: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["to"], "1234567890@s.whatsapp.net");
        assert_eq!(json["text"], "Hello!");
        assert!(json.get("media").is_none()); // skipped when None
    }

    #[test]
    fn test_send_request_with_media() {
        let req = SendRequest {
            to: "123@s.whatsapp.net".into(),
            text: "See this".into(),
            media: Some(MediaPayload {
                mime_type: "image/png".into(),
                data: "base64data".into(),
                filename: Some("photo.png".into()),
            }),
            reply_to: Some("msg-456".into()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["media"]["mime_type"], "image/png");
        assert_eq!(json["reply_to"], "msg-456");
    }

    #[test]
    fn test_bridge_event_qr_deserialization() {
        let json = r#"{"type": "qr", "qr_data": "base64qr", "expires_in_secs": 60}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Qr { qr_data, expires_in_secs } => {
                assert_eq!(qr_data, "base64qr");
                assert_eq!(expires_in_secs, 60);
            }
            _ => panic!("Expected Qr event"),
        }
    }

    #[test]
    fn test_bridge_event_connected_deserialization() {
        let json = r#"{"type": "connected", "device_name": "iPhone 15", "phone_number": "+86138"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Connected { device_name, phone_number } => {
                assert_eq!(device_name, "iPhone 15");
                assert_eq!(phone_number, "+86138");
            }
            _ => panic!("Expected Connected event"),
        }
    }

    #[test]
    fn test_bridge_event_message_deserialization() {
        let json = r#"{
            "type": "message",
            "from": "123@s.whatsapp.net",
            "from_name": "Alice",
            "chat_id": "123@s.whatsapp.net",
            "text": "Hello",
            "media": null,
            "timestamp": 1740000000,
            "message_id": "msg-123",
            "is_group": false,
            "reply_to": null
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Message { from, text, is_group, .. } => {
                assert_eq!(from, "123@s.whatsapp.net");
                assert_eq!(text, "Hello");
                assert!(!is_group);
            }
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_bridge_event_all_simple_variants() {
        let cases = vec![
            (r#"{"type": "qr_expired"}"#, "QrExpired"),
            (r#"{"type": "scanned"}"#, "Scanned"),
            (r#"{"type": "syncing", "progress": 0.75}"#, "Syncing"),
            (r#"{"type": "disconnected", "reason": "timeout"}"#, "Disconnected"),
            (r#"{"type": "ready"}"#, "Ready"),
            (r#"{"type": "error", "message": "auth failed"}"#, "Error"),
        ];
        for (json, name) in cases {
            let event: BridgeEvent = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("Failed to deserialize {}: {}", name, e));
            // Just ensure it deserializes without panic
            let _ = format!("{:?}", event);
        }
    }

    #[test]
    fn test_ok_response() {
        let json = r#"{"ok": true}"#;
        let resp: OkResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
    }

    #[test]
    fn test_send_response() {
        let json = r#"{"id": "wa-msg-abc123"}"#;
        let resp: SendResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "wa-msg-abc123");
    }

    #[test]
    fn test_ping_response() {
        let json = r#"{"pong": true, "rtt_ms": 42}"#;
        let resp: PingResponse = serde_json::from_str(json).unwrap();
        assert!(resp.pong);
        assert_eq!(resp.rtt_ms, Some(42));
    }

    #[test]
    fn test_bridge_status_response() {
        let json = r#"{"connected": true, "device_name": "Pixel 8", "phone_number": "+1555"}"#;
        let resp: BridgeStatusResponse = serde_json::from_str(json).unwrap();
        assert!(resp.connected);
        assert_eq!(resp.device_name.unwrap(), "Pixel 8");
    }
}
```

**Step 2: Register the module**

Add `pub mod bridge_protocol;` to `core/src/gateway/channels/whatsapp/mod.rs`.

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp::bridge_protocol`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/channels/whatsapp/bridge_protocol.rs core/src/gateway/channels/whatsapp/mod.rs
git commit -m "whatsapp: add Bridge RPC protocol types with serialization tests"
```

---

## Task 3: BridgeManager (Child Process Lifecycle)

Manages the Go whatsapp-bridge child process: spawn, health check, auto-restart, graceful shutdown.

**Files:**
- Create: `core/src/gateway/channels/whatsapp/bridge_manager.rs`
- Modify: `core/src/gateway/channels/whatsapp/mod.rs` (add `pub mod bridge_manager;`)

**Step 1: Write BridgeManager with tests**

```rust
// core/src/gateway/channels/whatsapp/bridge_manager.rs

//! Go Bridge child process lifecycle manager.
//!
//! Spawns and manages the whatsapp-bridge Go binary as a child process,
//! with auto-restart on crash and graceful shutdown.

use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tracing::{error, info, warn};

/// Configuration for the bridge manager
#[derive(Debug, Clone)]
pub struct BridgeManagerConfig {
    /// Path to the whatsapp-bridge binary
    pub binary_path: PathBuf,
    /// Path for the Unix socket
    pub socket_path: PathBuf,
    /// Path for session data storage
    pub data_dir: PathBuf,
    /// Maximum restart attempts before giving up
    pub max_restarts: u32,
    /// Delay between restart attempts (seconds)
    pub restart_delay_secs: u64,
}

impl Default for BridgeManagerConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            binary_path: PathBuf::from("whatsapp-bridge"),
            socket_path: home.join(".aleph/whatsapp/bridge.sock"),
            data_dir: home.join(".aleph/whatsapp"),
            max_restarts: 5,
            restart_delay_secs: 3,
        }
    }
}

/// Manages the lifecycle of the Go whatsapp-bridge process
pub struct BridgeManager {
    config: BridgeManagerConfig,
    child: Option<Child>,
    restart_count: u32,
}

impl BridgeManager {
    /// Create a new bridge manager
    pub fn new(config: BridgeManagerConfig) -> Self {
        Self {
            config,
            child: None,
            restart_count: 0,
        }
    }

    /// Start the bridge process
    pub async fn start(&mut self) -> Result<(), BridgeError> {
        // Ensure data directory exists
        tokio::fs::create_dir_all(&self.config.data_dir)
            .await
            .map_err(|e| BridgeError::IoError(format!("Failed to create data dir: {}", e)))?;

        // Clean up stale socket if exists
        let _ = tokio::fs::remove_file(&self.config.socket_path).await;

        self.spawn_process().await
    }

    /// Spawn the Go bridge binary
    async fn spawn_process(&mut self) -> Result<(), BridgeError> {
        let binary = &self.config.binary_path;

        // Check if binary exists
        if !binary.exists() {
            return Err(BridgeError::BinaryNotFound(
                binary.display().to_string(),
            ));
        }

        info!(
            binary = %binary.display(),
            socket = %self.config.socket_path.display(),
            "Spawning whatsapp-bridge process"
        );

        let child = Command::new(binary)
            .arg("--socket")
            .arg(&self.config.socket_path)
            .arg("--data-dir")
            .arg(&self.config.data_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| BridgeError::SpawnFailed(format!("{}", e)))?;

        self.child = Some(child);
        self.restart_count = 0;
        Ok(())
    }

    /// Stop the bridge process gracefully
    pub async fn stop(&mut self) -> Result<(), BridgeError> {
        if let Some(ref mut child) = self.child {
            info!("Stopping whatsapp-bridge process");
            // Send SIGTERM first
            let _ = child.kill().await;
            self.child = None;
        }

        // Clean up socket
        let _ = tokio::fs::remove_file(&self.config.socket_path).await;
        Ok(())
    }

    /// Check if the bridge process is running
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(None) => true,  // Still running
                Ok(Some(_)) => {
                    self.child = None;
                    false
                }
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Attempt to restart the bridge process (with retry limits)
    pub async fn restart(&mut self) -> Result<(), BridgeError> {
        if self.restart_count >= self.config.max_restarts {
            return Err(BridgeError::MaxRestartsExceeded(self.config.max_restarts));
        }

        self.restart_count += 1;
        warn!(
            attempt = self.restart_count,
            max = self.config.max_restarts,
            "Restarting whatsapp-bridge"
        );

        // Stop existing process
        self.stop().await?;

        // Wait before restart
        tokio::time::sleep(std::time::Duration::from_secs(
            self.config.restart_delay_secs,
        ))
        .await;

        self.spawn_process().await
    }

    /// Get socket path for RPC client to connect
    pub fn socket_path(&self) -> &PathBuf {
        &self.config.socket_path
    }

    /// Reset restart counter (call after successful connection)
    pub fn reset_restart_count(&mut self) {
        self.restart_count = 0;
    }

    /// Get current restart count
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }
}

impl Drop for BridgeManager {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            // Best-effort kill on drop
            let _ = child.start_kill();
        }
    }
}

/// Errors from bridge management
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Bridge binary not found: {0}")]
    BinaryNotFound(String),

    #[error("Failed to spawn bridge: {0}")]
    SpawnFailed(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Max restarts exceeded ({0} attempts)")]
    MaxRestartsExceeded(u32),

    #[error("Bridge process exited unexpectedly: {0}")]
    UnexpectedExit(String),

    #[error("Socket connection failed: {0}")]
    SocketError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> BridgeManagerConfig {
        BridgeManagerConfig {
            binary_path: PathBuf::from("/nonexistent/whatsapp-bridge"),
            socket_path: PathBuf::from("/tmp/aleph-wa-test.sock"),
            data_dir: PathBuf::from("/tmp/aleph-wa-test"),
            max_restarts: 3,
            restart_delay_secs: 0, // No delay in tests
        }
    }

    #[test]
    fn test_bridge_manager_creation() {
        let config = test_config();
        let manager = BridgeManager::new(config.clone());
        assert!(!manager.is_running());
        assert_eq!(manager.restart_count(), 0);
        assert_eq!(manager.socket_path(), &config.socket_path);
    }

    #[tokio::test]
    async fn test_start_with_missing_binary() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);
        let result = manager.start().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::BinaryNotFound(_) => {}
            other => panic!("Expected BinaryNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stop_when_not_running() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);
        // Stop should not error when nothing is running
        assert!(manager.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_restart_limit() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);
        // Manually set restart count to max
        manager.restart_count = 3;
        let result = manager.restart().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::MaxRestartsExceeded(3) => {}
            other => panic!("Expected MaxRestartsExceeded, got: {:?}", other),
        }
    }

    #[test]
    fn test_reset_restart_count() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);
        manager.restart_count = 4;
        manager.reset_restart_count();
        assert_eq!(manager.restart_count(), 0);
    }

    #[test]
    fn test_default_config() {
        let config = BridgeManagerConfig::default();
        assert_eq!(config.max_restarts, 5);
        assert_eq!(config.restart_delay_secs, 3);
        assert!(config.socket_path.to_string_lossy().contains(".aleph/whatsapp"));
    }

    #[test]
    fn test_is_running_no_child() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);
        assert!(!manager.is_running());
    }
}
```

**Step 2: Register the module**

Add `pub mod bridge_manager;` to `core/src/gateway/channels/whatsapp/mod.rs`.

**Step 3: Add `dirs` dependency if not already present**

Check `core/Cargo.toml` for `dirs` crate. If missing, add: `dirs = "5"` under `[dependencies]`.

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp::bridge_manager`

Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/gateway/channels/whatsapp/bridge_manager.rs core/src/gateway/channels/whatsapp/mod.rs core/Cargo.toml
git commit -m "whatsapp: add BridgeManager for child process lifecycle"
```

---

## Task 4: BridgeRpcClient (JSON-RPC over Unix Socket)

Async JSON-RPC client connecting to the Go bridge via Unix socket.

**Files:**
- Create: `core/src/gateway/channels/whatsapp/rpc_client.rs`
- Modify: `core/src/gateway/channels/whatsapp/mod.rs` (add `pub mod rpc_client;`)

**Step 1: Write the RPC client with tests**

```rust
// core/src/gateway/channels/whatsapp/rpc_client.rs

//! JSON-RPC client for communicating with the Go whatsapp-bridge
//! over a Unix domain socket.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, warn};

use super::bridge_manager::BridgeError;
use super::bridge_protocol::BridgeEvent;

/// JSON-RPC 2.0 request
#[derive(Debug, Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<RpcError>,
    /// For server-push notifications (no id)
    method: Option<String>,
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Client for communicating with the Go bridge
pub struct BridgeRpcClient {
    socket_path: PathBuf,
    writer: Arc<Mutex<Option<tokio::io::WriteHalf<UnixStream>>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    next_id: AtomicU64,
    event_tx: mpsc::Sender<BridgeEvent>,
}

impl BridgeRpcClient {
    /// Create a new RPC client (does not connect yet)
    pub fn new(socket_path: PathBuf, event_tx: mpsc::Sender<BridgeEvent>) -> Self {
        Self {
            socket_path,
            writer: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        }
    }

    /// Connect to the bridge Unix socket.
    /// Retries up to `max_retries` times with `retry_delay_ms` between attempts,
    /// since the bridge process needs time to start and create the socket.
    pub async fn connect(&self, max_retries: u32, retry_delay_ms: u64) -> Result<(), BridgeError> {
        let mut last_err = String::new();

        for attempt in 0..max_retries {
            match UnixStream::connect(&self.socket_path).await {
                Ok(stream) => {
                    let (reader, writer) = tokio::io::split(stream);
                    *self.writer.lock().await = Some(writer);

                    // Spawn reader task
                    let pending = self.pending.clone();
                    let event_tx = self.event_tx.clone();
                    tokio::spawn(async move {
                        Self::read_loop(reader, pending, event_tx).await;
                    });

                    debug!("Connected to whatsapp-bridge socket");
                    return Ok(());
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < max_retries - 1 {
                        debug!(
                            attempt = attempt + 1,
                            max = max_retries,
                            "Socket not ready, retrying..."
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                    }
                }
            }
        }

        Err(BridgeError::SocketError(format!(
            "Failed to connect after {} attempts: {}",
            max_retries, last_err
        )))
    }

    /// Read loop: processes responses and event notifications from the bridge
    async fn read_loop(
        reader: tokio::io::ReadHalf<UnixStream>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>,
        event_tx: mpsc::Sender<BridgeEvent>,
    ) {
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => {
                    warn!("Bridge socket closed (EOF)");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<RpcResponse>(trimmed) {
                        Ok(resp) => {
                            if let Some(id) = resp.id {
                                // This is a response to a request
                                let mut pending_map = pending.lock().await;
                                if let Some(sender) = pending_map.remove(&id) {
                                    if let Some(error) = resp.error {
                                        let _ = sender.send(Err(format!(
                                            "RPC error {}: {}",
                                            error.code, error.message
                                        )));
                                    } else {
                                        let _ = sender.send(Ok(
                                            resp.result.unwrap_or(serde_json::Value::Null)
                                        ));
                                    }
                                }
                            } else if resp.method.is_some() {
                                // This is an event notification (no id)
                                if let Some(params) = resp.params {
                                    match serde_json::from_value::<BridgeEvent>(params) {
                                        Ok(event) => {
                                            if event_tx.send(event).await.is_err() {
                                                warn!("Event receiver dropped");
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Failed to parse bridge event: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(line = trimmed, "Failed to parse bridge message: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Error reading from bridge socket: {}", e);
                    break;
                }
            }
        }
    }

    /// Send an RPC call and wait for the response
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T, BridgeError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let request = RpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut json = serde_json::to_string(&request)
            .map_err(|e| BridgeError::IoError(format!("Serialize failed: {}", e)))?;
        json.push('\n'); // newline-delimited JSON

        // Register pending response
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        // Send request
        {
            let mut writer_guard = self.writer.lock().await;
            match writer_guard.as_mut() {
                Some(writer) => {
                    writer
                        .write_all(json.as_bytes())
                        .await
                        .map_err(|e| BridgeError::SocketError(format!("Write failed: {}", e)))?;
                    writer
                        .flush()
                        .await
                        .map_err(|e| BridgeError::SocketError(format!("Flush failed: {}", e)))?;
                }
                None => {
                    return Err(BridgeError::SocketError("Not connected".into()));
                }
            }
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| BridgeError::SocketError("RPC call timed out".into()))?
            .map_err(|_| BridgeError::SocketError("Response channel dropped".into()))?
            .map_err(|e| BridgeError::SocketError(e))?;

        serde_json::from_value(result)
            .map_err(|e| BridgeError::SocketError(format!("Deserialize response failed: {}", e)))
    }

    /// Check if connected to the bridge socket
    pub async fn is_connected(&self) -> bool {
        self.writer.lock().await.is_some()
    }

    /// Disconnect from the bridge socket
    pub async fn disconnect(&self) {
        *self.writer.lock().await = None;
        self.pending.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_serialization() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "bridge.connect".into(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"bridge.connect\""));
        assert!(!json.contains("params")); // skipped when None
    }

    #[test]
    fn test_rpc_request_with_params() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "bridge.send".into(),
            params: Some(serde_json::json!({"to": "123", "text": "hi"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"params\""));
    }

    #[test]
    fn test_rpc_response_success() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_rpc_response_error() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "error": {"code": -32600, "message": "Invalid request"}}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32600);
    }

    #[test]
    fn test_rpc_notification() {
        let json = r#"{"jsonrpc": "2.0", "method": "event", "params": {"type": "scanned"}}"#;
        let resp: RpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.id.is_none()); // notification has no id
        assert_eq!(resp.method, Some("event".into()));
        assert!(resp.params.is_some());
    }

    #[tokio::test]
    async fn test_client_not_connected() {
        let (tx, _rx) = mpsc::channel(10);
        let client = BridgeRpcClient::new(PathBuf::from("/nonexistent.sock"), tx);
        assert!(!client.is_connected().await);
    }

    #[tokio::test]
    async fn test_connect_to_nonexistent_socket() {
        let (tx, _rx) = mpsc::channel(10);
        let client = BridgeRpcClient::new(PathBuf::from("/tmp/nonexistent-aleph-test.sock"), tx);
        let result = client.connect(2, 50).await; // 2 retries, 50ms delay
        assert!(result.is_err());
    }
}
```

**Step 2: Register the module**

Add `pub mod rpc_client;` to `core/src/gateway/channels/whatsapp/mod.rs`.

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp::rpc_client`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/channels/whatsapp/rpc_client.rs core/src/gateway/channels/whatsapp/mod.rs
git commit -m "whatsapp: add BridgeRpcClient for JSON-RPC over Unix Socket"
```

---

## Task 5: Message Converter

Convert between Bridge protocol messages and Aleph's `InboundMessage`/`OutboundMessage` types.

**Files:**
- Create: `core/src/gateway/channels/whatsapp/message.rs`
- Modify: `core/src/gateway/channels/whatsapp/mod.rs` (add `pub mod message;`)

**Step 1: Write converter with tests**

```rust
// core/src/gateway/channels/whatsapp/message.rs

//! Message format conversion between Bridge protocol and Aleph types.

use chrono::{DateTime, TimeZone, Utc};

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, OutboundMessage, UserId,
};
use super::bridge_protocol::{BridgeEvent, MediaPayload, SendRequest};

/// Convert a BridgeEvent::Message into an Aleph InboundMessage
pub fn bridge_message_to_inbound(
    event: &BridgeEvent,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    match event {
        BridgeEvent::Message {
            from,
            from_name,
            chat_id,
            text,
            media,
            timestamp,
            message_id,
            is_group,
            reply_to,
        } => {
            let mut attachments = Vec::new();
            if let Some(media) = media {
                attachments.push(Attachment {
                    id: uuid::Uuid::new_v4().to_string(),
                    mime_type: media.mime_type.clone(),
                    filename: media.filename.clone(),
                    size: None,
                    url: None,
                    path: None,
                    data: Some(
                        base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            &media.data,
                        )
                        .unwrap_or_default(),
                    ),
                });
            }

            let ts = Utc.timestamp_opt(*timestamp, 0)
                .single()
                .unwrap_or_else(Utc::now);

            Some(InboundMessage {
                id: MessageId::new(message_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(chat_id),
                sender_id: UserId::new(from),
                sender_name: from_name.clone(),
                text: text.clone(),
                attachments,
                timestamp: ts,
                reply_to: reply_to.as_ref().map(|r| MessageId::new(r)),
                is_group: *is_group,
                raw: None,
            })
        }
        _ => None,
    }
}

/// Convert an Aleph OutboundMessage into a Bridge SendRequest
pub fn outbound_to_send_request(message: &OutboundMessage) -> SendRequest {
    let media = message.attachments.first().map(|att| {
        let data = att
            .data
            .as_ref()
            .map(|bytes| {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(bytes)
            })
            .unwrap_or_default();

        MediaPayload {
            mime_type: att.mime_type.clone(),
            data,
            filename: att.filename.clone(),
        }
    });

    SendRequest {
        to: message.conversation_id.as_str().to_string(),
        text: message.text.clone(),
        media,
        reply_to: message.reply_to.as_ref().map(|id| id.as_str().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::ChannelId;
    use super::super::bridge_protocol::BridgeEvent;

    #[test]
    fn test_bridge_message_to_inbound() {
        let event = BridgeEvent::Message {
            from: "123@s.whatsapp.net".into(),
            from_name: Some("Alice".into()),
            chat_id: "123@s.whatsapp.net".into(),
            text: "Hello Aleph!".into(),
            media: None,
            timestamp: 1740000000,
            message_id: "wa-msg-001".into(),
            is_group: false,
            reply_to: None,
        };

        let channel_id = ChannelId::new("whatsapp");
        let inbound = bridge_message_to_inbound(&event, &channel_id).unwrap();

        assert_eq!(inbound.id.as_str(), "wa-msg-001");
        assert_eq!(inbound.channel_id.as_str(), "whatsapp");
        assert_eq!(inbound.sender_id.as_str(), "123@s.whatsapp.net");
        assert_eq!(inbound.sender_name.as_deref(), Some("Alice"));
        assert_eq!(inbound.text, "Hello Aleph!");
        assert!(!inbound.is_group);
        assert!(inbound.attachments.is_empty());
    }

    #[test]
    fn test_bridge_message_with_reply() {
        let event = BridgeEvent::Message {
            from: "456@s.whatsapp.net".into(),
            from_name: None,
            chat_id: "group@g.us".into(),
            text: "Reply".into(),
            media: None,
            timestamp: 1740000000,
            message_id: "wa-msg-002".into(),
            is_group: true,
            reply_to: Some("wa-msg-001".into()),
        };

        let channel_id = ChannelId::new("whatsapp");
        let inbound = bridge_message_to_inbound(&event, &channel_id).unwrap();

        assert!(inbound.is_group);
        assert_eq!(inbound.reply_to.as_ref().unwrap().as_str(), "wa-msg-001");
    }

    #[test]
    fn test_non_message_event_returns_none() {
        let event = BridgeEvent::Scanned;
        let channel_id = ChannelId::new("whatsapp");
        assert!(bridge_message_to_inbound(&event, &channel_id).is_none());
    }

    #[test]
    fn test_outbound_to_send_request_simple() {
        let msg = OutboundMessage::text("123@s.whatsapp.net", "Hi there!");
        let req = outbound_to_send_request(&msg);

        assert_eq!(req.to, "123@s.whatsapp.net");
        assert_eq!(req.text, "Hi there!");
        assert!(req.media.is_none());
        assert!(req.reply_to.is_none());
    }

    #[test]
    fn test_outbound_to_send_request_with_reply() {
        let msg = OutboundMessage::text("123@s.whatsapp.net", "Reply")
            .with_reply_to("wa-msg-original");
        let req = outbound_to_send_request(&msg);

        assert_eq!(req.reply_to.as_deref(), Some("wa-msg-original"));
    }
}
```

**Step 2: Add `base64` dependency if not already present**

Check `core/Cargo.toml` for `base64` crate. If missing, add: `base64 = "0.22"`.

**Step 3: Register the module and run tests**

Add `pub mod message;` to `core/src/gateway/channels/whatsapp/mod.rs`.

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp::message`

**Step 4: Commit**

```bash
git add core/src/gateway/channels/whatsapp/message.rs core/src/gateway/channels/whatsapp/mod.rs core/Cargo.toml
git commit -m "whatsapp: add message converter between Bridge and Aleph types"
```

---

## Task 6: Upgrade WhatsAppChannel to Integrate Components

Replace the stub implementation with the real bridge-backed channel.

**Files:**
- Modify: `core/src/gateway/channels/whatsapp/mod.rs` (complete rewrite)
- Modify: `core/src/gateway/channels/whatsapp/config.rs` (add bridge config fields)

**Step 1: Update config with bridge settings**

Add to `WhatsAppConfig` in `core/src/gateway/channels/whatsapp/config.rs`:

```rust
    /// Path to the whatsapp-bridge binary (auto-detected if not set)
    #[serde(default)]
    pub bridge_binary: Option<String>,
    /// Max restart attempts for the bridge process
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,

// Add after default_true():
fn default_max_restarts() -> u32 {
    5
}
```

**Step 2: Rewrite WhatsAppChannel**

Replace the entire `WhatsAppChannel` struct and impl in `core/src/gateway/channels/whatsapp/mod.rs` with the bridge-integrated version. Key changes:

- Add `BridgeManager` and `BridgeRpcClient` fields
- Add `PairingState` tracking with `Arc<RwLock<PairingState>>`
- `start()` now spawns Go bridge, connects RPC client, spawns event loop
- `send()` routes through `bridge.send` RPC
- `get_pairing_data()` reads from `PairingState`
- Event loop task converts `BridgeEvent` to `PairingState` transitions and `InboundMessage` forwarding
- `stop()` gracefully shuts down bridge

**Step 3: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::channels::whatsapp`

**Step 4: Run compilation check for the entire crate**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`

**Step 5: Commit**

```bash
git add core/src/gateway/channels/whatsapp/
git commit -m "whatsapp: integrate BridgeManager, RpcClient, PairingState into WhatsAppChannel"
```

---

## Task 7: Go Bridge Binary (whatsapp-bridge)

Create the Go sidecar binary that wraps whatsmeow.

**Files:**
- Create: `bridges/whatsapp/go.mod`
- Create: `bridges/whatsapp/main.go`
- Create: `bridges/whatsapp/rpc_server.go`
- Create: `bridges/whatsapp/wa_client.go`

**Step 1: Initialize Go module**

```bash
mkdir -p bridges/whatsapp
cd bridges/whatsapp
go mod init github.com/anthropic/aleph/bridges/whatsapp
go get go.mau.fi/whatsmeow@latest
go get go.mau.fi/whatsmeow/store/sqlstore@latest
```

**Step 2: Write main.go**

Entry point: parse flags (--socket, --data-dir), start Unix socket listener, run RPC server.

**Step 3: Write wa_client.go**

Thin wrapper around whatsmeow.Client:
- `NewClient(dataDir string)`: create whatsmeow client with SQLite store
- `Connect()`: start QR login, emit events
- `Disconnect()`: cleanup
- `Send(to, text, media)`: send message
- Event handler that converts whatsmeow events to BridgeEvent JSON

**Step 4: Write rpc_server.go**

JSON-RPC server over Unix socket:
- Accept connections, read newline-delimited JSON
- Route methods: `bridge.connect`, `bridge.disconnect`, `bridge.send`, `bridge.status`, `bridge.ping`
- Push events back as JSON-RPC notifications (no id field)

**Step 5: Build and test**

```bash
cd bridges/whatsapp
go build -o whatsapp-bridge .
./whatsapp-bridge --help
```

**Step 6: Commit**

```bash
git add bridges/whatsapp/
git commit -m "whatsapp: add Go bridge binary wrapping whatsmeow"
```

---

## Task 8: Fix Dashboard PairingData Serialization Mismatch

The dashboard model uses `QrCode { data: String }` but the server sends `QrCode(String)`.

**Files:**
- Modify: `clients/dashboard/src/models.rs`

**Step 1: Fix PairingData enum**

In `clients/dashboard/src/models.rs`, change `PairingData::QrCode { data: String }` to `PairingData::QrCode(String)` to match the server's serialization format:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PairingData {
    None,
    Code(String),
    QrCode(String),  // was: QrCode { data: String }
}
```

**Step 2: Update WhatsAppPanel to use the new format**

In `clients/dashboard/src/views/social_connections.rs`, change line 78:

```rust
// Before:
PairingData::QrCode { data } => set_qr_code.set(Some(data)),
// After:
PairingData::QrCode(data) => set_qr_code.set(Some(data)),
```

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/clients/dashboard && cargo check --target wasm32-unknown-unknown`

**Step 4: Commit**

```bash
git add clients/dashboard/src/models.rs clients/dashboard/src/views/social_connections.rs
git commit -m "dashboard: fix PairingData serialization mismatch with server"
```

---

## Task 9: Upgrade Dashboard WhatsApp Panel

Upgrade the WhatsAppPanel to support real-time pairing lifecycle display.

**Files:**
- Modify: `clients/dashboard/src/views/social_connections.rs`

**Step 1: Add PairingState model to dashboard**

Add to `clients/dashboard/src/models.rs`:

```rust
/// Fine-grained pairing state (mirrors server's PairingState)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingState {
    Idle,
    Initializing,
    WaitingQr { qr_data: String, expires_at: String },
    QrExpired,
    Scanned,
    Syncing { progress: f32 },
    Connected { device_name: String, phone_number: String },
    Disconnected { reason: String },
    Failed { error: String },
}
```

**Step 2: Rewrite WhatsAppPanel**

Replace the `WhatsAppPanel` component with an upgraded version that:
- Subscribes to pairing events via WebSocket (if the dashboard supports topic subscriptions)
- Falls back to polling `channels.status` + `channel.pairing_data` RPC
- Shows different UI for each `PairingState`:
  - `Idle`: "Start" button
  - `Initializing`: spinner + "Starting bridge..."
  - `WaitingQr`: QR code image + countdown timer + pairing code alternative
  - `QrExpired`: grayed QR + "Refreshing..." message
  - `Scanned`: success checkmark + "Waiting for confirmation..."
  - `Syncing`: progress bar with percentage
  - `Connected`: device name, phone number, uptime, disconnect/re-pair buttons
  - `Disconnected`/`Failed`: error message + reconnect button

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/clients/dashboard && cargo check --target wasm32-unknown-unknown`

**Step 4: Commit**

```bash
git add clients/dashboard/src/views/social_connections.rs clients/dashboard/src/models.rs
git commit -m "dashboard: upgrade WhatsApp panel with full pairing lifecycle UI"
```

---

## Task 10: End-to-End Integration Test

Verify the complete flow works: Rust spawns Go bridge, QR code appears in Dashboard.

**Step 1: Build Go bridge**

```bash
cd bridges/whatsapp && go build -o whatsapp-bridge .
```

**Step 2: Update config to point to bridge binary**

Create/update `~/.aleph/config.toml`:

```toml
[[channels]]
id = "whatsapp"
channel_type = "whatsapp"
enabled = true

[channels.config]
bridge_binary = "/path/to/bridges/whatsapp/whatsapp-bridge"
```

**Step 3: Start Aleph server**

```bash
cd /Users/zouguojun/Workspace/Aleph
cargo run --bin aleph-server --features whatsapp
```

**Step 4: Verify via RPC**

```bash
# Start the WhatsApp channel
wscat -c ws://127.0.0.1:18789 -x '{"jsonrpc":"2.0","id":1,"method":"channel.start","params":{"channel_id":"whatsapp"}}'

# Get pairing data (should return real QR code)
wscat -c ws://127.0.0.1:18789 -x '{"jsonrpc":"2.0","id":2,"method":"channel.pairing_data","params":{"channel_id":"whatsapp"}}'
```

**Step 5: Verify Dashboard**

Open `http://127.0.0.1:18790` in browser, navigate to Social Connections > WhatsApp. Verify QR code appears and states transition correctly.

**Step 6: Final commit**

```bash
git add -A
git commit -m "whatsapp: complete WhatsApp Bridge integration (end-to-end)"
```

---

## Summary

| Task | Component | LOC (est.) | Dependencies |
|------|-----------|-----------|--------------|
| 1 | PairingState state machine | ~200 | None |
| 2 | Bridge RPC protocol types | ~180 | None |
| 3 | BridgeManager | ~200 | dirs crate |
| 4 | BridgeRpcClient | ~250 | None |
| 5 | Message converter | ~120 | base64 crate |
| 6 | WhatsAppChannel upgrade | ~300 | Tasks 1-5 |
| 7 | Go Bridge binary | ~600 | whatsmeow |
| 8 | Dashboard serialization fix | ~10 | None |
| 9 | Dashboard panel upgrade | ~200 | Task 8 |
| 10 | E2E integration test | ~50 | All above |

**Total estimated: ~2100 LOC** (Rust ~1260, Go ~600, Leptos ~240)

**Task dependency graph:**
```
Tasks 1,2 (parallel) → Task 3 → Task 4 → Task 5 → Task 6
                                                       ↓
Task 7 (Go bridge, parallel with 1-6) ──────────→ Task 10
                                                       ↑
Task 8 → Task 9 ──────────────────────────────────────┘
```

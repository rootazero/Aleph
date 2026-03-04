//! Hello Snapshot — server state sent to clients after successful auth.
//!
//! When a client connects and authenticates, the server sends a `HelloSnapshot`
//! so the client immediately knows server state without extra RPC round-trips.

use serde::Serialize;

use super::presence::PresenceEntry;
use super::state_version::StateVersion;

// ---------------------------------------------------------------------------
// ConnectionLimits
// ---------------------------------------------------------------------------

/// Per-server connection limits sent inside the hello snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionLimits {
    /// Maximum allowed concurrent WebSocket connections.
    pub max_connections: u32,
    /// Number of connections currently active (including the new one).
    pub current_connections: u32,
}

// ---------------------------------------------------------------------------
// HelloSnapshot
// ---------------------------------------------------------------------------

/// Snapshot of server state sent to a client right after authentication.
///
/// This eliminates the need for follow-up RPC calls like `getPresence`,
/// `getStateVersion`, etc. — the client has everything it needs in one shot.
#[derive(Debug, Clone, Serialize)]
pub struct HelloSnapshot {
    /// Unique identifier of this server instance (stable across reconnects).
    pub server_id: String,
    /// Server uptime in milliseconds since it started.
    pub uptime_ms: u64,
    /// Current state version counter for optimistic concurrency.
    pub state_version: StateVersion,
    /// List of currently connected / recently-seen peers.
    pub presence: Vec<PresenceEntry>,
    /// Connection limits for this server.
    pub limits: ConnectionLimits,
    /// Capabilities advertised by this server (e.g. `"memory"`, `"tools"`, `"mcp"`).
    pub capabilities: Vec<String>,
    /// The workspace the user was last active in, if any.
    pub active_workspace: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal ConnectionLimits for testing.
    fn sample_limits() -> ConnectionLimits {
        ConnectionLimits {
            max_connections: 64,
            current_connections: 3,
        }
    }

    /// Helper: build a minimal HelloSnapshot for testing.
    fn sample_snapshot() -> HelloSnapshot {
        use chrono::Utc;

        let now = Utc::now();
        HelloSnapshot {
            server_id: "srv-abc123".to_string(),
            uptime_ms: 42_000,
            state_version: StateVersion {
                presence: 0,
                health: 0,
                config: 0,
            },
            presence: vec![PresenceEntry {
                conn_id: "conn-1".to_string(),
                device_id: None,
                device_name: "MacBook Pro".to_string(),
                platform: "macos".to_string(),
                connected_at: now,
                last_heartbeat: now,
            }],
            limits: sample_limits(),
            capabilities: vec!["memory".to_string(), "tools".to_string()],
            active_workspace: Some("project-x".to_string()),
        }
    }

    #[test]
    fn test_hello_snapshot_serializes() {
        let snapshot = sample_snapshot();
        let json = serde_json::to_value(&snapshot).expect("serialize HelloSnapshot");

        assert_eq!(json["server_id"], "srv-abc123");
        assert_eq!(json["uptime_ms"], 42_000);
        assert!(json["state_version"].is_object() || json["state_version"].is_number());
        assert!(json["presence"].is_array());
        assert_eq!(json["presence"].as_array().expect("presence should be array").len(), 1);
        assert_eq!(json["limits"]["max_connections"], 64);
        assert_eq!(json["limits"]["current_connections"], 3);
        assert_eq!(json["capabilities"].as_array().expect("capabilities should be array").len(), 2);
        assert_eq!(json["active_workspace"], "project-x");
    }

    #[test]
    fn test_connection_limits_serializes() {
        let limits = sample_limits();
        let json = serde_json::to_value(&limits).expect("serialize ConnectionLimits");

        assert_eq!(json["max_connections"], 64);
        assert_eq!(json["current_connections"], 3);
    }
}

//! Tool Execution Policy
//!
//! Determines where tools should be executed in Server-Client architecture.

use serde::{Deserialize, Serialize};

/// Determines where a tool should be executed in Server-Client mode.
///
/// This policy drives the routing decision when both Server and Client
/// could potentially execute a tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPolicy {
    /// Tool MUST execute on Server (e.g., internal database access).
    /// If Server lacks capability, returns error.
    ServerOnly,

    /// Tool MUST execute on Client (e.g., screenshots, system notifications).
    /// If Client lacks capability, returns error.
    ClientOnly,

    /// Prefer Server execution, fall back to Client if Server unavailable.
    #[default]
    PreferServer,

    /// Prefer Client execution, fall back to Server if Client unavailable.
    /// Best for local file operations, shell commands.
    PreferClient,
}

impl ExecutionPolicy {
    /// Returns true if this policy allows Server execution.
    pub fn allows_server(&self) -> bool {
        !matches!(self, Self::ClientOnly)
    }

    /// Returns true if this policy allows Client execution.
    pub fn allows_client(&self) -> bool {
        !matches!(self, Self::ServerOnly)
    }

    /// Returns true if this policy prefers Client over Server.
    pub fn prefers_client(&self) -> bool {
        matches!(self, Self::PreferClient | Self::ClientOnly)
    }

    /// Returns true if this policy prefers Server over Client.
    pub fn prefers_server(&self) -> bool {
        matches!(self, Self::PreferServer | Self::ServerOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_prefer_server() {
        assert_eq!(ExecutionPolicy::default(), ExecutionPolicy::PreferServer);
    }

    #[test]
    fn test_allows_server() {
        assert!(ExecutionPolicy::ServerOnly.allows_server());
        assert!(ExecutionPolicy::PreferServer.allows_server());
        assert!(ExecutionPolicy::PreferClient.allows_server());
        assert!(!ExecutionPolicy::ClientOnly.allows_server());
    }

    #[test]
    fn test_allows_client() {
        assert!(!ExecutionPolicy::ServerOnly.allows_client());
        assert!(ExecutionPolicy::PreferServer.allows_client());
        assert!(ExecutionPolicy::PreferClient.allows_client());
        assert!(ExecutionPolicy::ClientOnly.allows_client());
    }

    #[test]
    fn test_prefers_client() {
        assert!(!ExecutionPolicy::ServerOnly.prefers_client());
        assert!(!ExecutionPolicy::PreferServer.prefers_client());
        assert!(ExecutionPolicy::PreferClient.prefers_client());
        assert!(ExecutionPolicy::ClientOnly.prefers_client());
    }

    #[test]
    fn test_prefers_server() {
        assert!(ExecutionPolicy::ServerOnly.prefers_server());
        assert!(ExecutionPolicy::PreferServer.prefers_server());
        assert!(!ExecutionPolicy::PreferClient.prefers_server());
        assert!(!ExecutionPolicy::ClientOnly.prefers_server());
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = ExecutionPolicy::PreferClient;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, "\"prefer_client\"");
        let parsed: ExecutionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }
}

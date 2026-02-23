//! Tool execution location policy for Server-Client architecture.
//!
//! Re-exports types from `aleph-protocol` for backward compatibility.

// Re-export ExecutionPolicy from aleph-protocol
pub use aleph_protocol::policy::ExecutionPolicy;

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
    fn test_serde_roundtrip() {
        let policy = ExecutionPolicy::PreferClient;
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, "\"prefer_client\"");
        let parsed: ExecutionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }
}

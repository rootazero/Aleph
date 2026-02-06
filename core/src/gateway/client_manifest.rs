//! Client capability manifest for Server-Client architecture.
//!
//! Re-exports types from `aleph-protocol` for backward compatibility.

// Re-export all manifest types from aleph-protocol
pub use aleph_protocol::manifest::{
    ClientCapabilities, ClientEnvironment, ClientManifest, ExecutionConstraints,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_tool_by_category() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string(), "file_system".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(manifest.supports_tool("shell"));
        assert!(manifest.supports_tool("file_system:read"));
        assert!(!manifest.supports_tool("network:fetch"));
    }

    #[test]
    fn test_supports_tool_by_specific() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                specific_tools: vec!["applescript:run".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("applescript:run"));
        assert!(!manifest.supports_tool("applescript:compile"));
    }

    #[test]
    fn test_excluded_tools_override() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                excluded_tools: vec!["shell:sudo".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(!manifest.supports_tool("shell:sudo"));
    }

    #[test]
    fn test_default_constraints() {
        let constraints = ExecutionConstraints::default();
        assert_eq!(constraints.max_concurrent_tools, 3);
        assert_eq!(constraints.timeout_ms, 30000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let manifest = ClientManifest {
            client_type: "macos_native".to_string(),
            client_version: "1.0.0".to_string(),
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                constraints: ExecutionConstraints {
                    max_concurrent_tools: 5,
                    timeout_ms: 60000,
                },
                ..Default::default()
            },
            environment: ClientEnvironment {
                os: "macos".to_string(),
                arch: "arm64".to_string(),
                sandbox: false,
            },
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ClientManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.client_type, "macos_native");
        assert_eq!(parsed.capabilities.constraints.max_concurrent_tools, 5);
    }
}

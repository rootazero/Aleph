//! WASM plugin permission checking
//!
//! Thin facade over WasmCapabilities for quick capability presence checks.
//! Actual enforcement is in WasmCapabilityKernel.

use super::capabilities::WasmCapabilities;

/// Checks whether WASM capabilities are declared.
///
/// This is a lightweight query layer. Actual enforcement
/// (prefix checks, path traversal, leak detection, etc.)
/// is handled by [`WasmCapabilityKernel`].
#[derive(Debug, Clone, Default)]
pub struct PermissionChecker {
    capabilities: Option<WasmCapabilities>,
}

impl PermissionChecker {
    pub fn new(capabilities: Option<WasmCapabilities>) -> Self {
        Self { capabilities }
    }

    /// Whether HTTP capability is declared
    pub fn has_http(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|c| c.http.is_some())
            .unwrap_or(false)
    }

    /// Whether workspace capability is declared
    pub fn has_workspace(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|c| c.workspace.is_some())
            .unwrap_or(false)
    }

    /// Whether tool_invoke capability is declared
    pub fn has_tool_invoke(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|c| c.tool_invoke.is_some())
            .unwrap_or(false)
    }

    /// Whether secrets capability is declared
    pub fn has_secrets(&self) -> bool {
        self.capabilities
            .as_ref()
            .map(|c| c.secrets.is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::*;
    use std::collections::HashMap;

    #[test]
    fn test_no_capabilities() {
        let checker = PermissionChecker::new(None);
        assert!(!checker.has_http());
        assert!(!checker.has_workspace());
        assert!(!checker.has_tool_invoke());
        assert!(!checker.has_secrets());
    }

    #[test]
    fn test_default_capabilities_empty() {
        let checker = PermissionChecker::new(Some(WasmCapabilities::default()));
        assert!(!checker.has_http());
        assert!(!checker.has_workspace());
        assert!(!checker.has_tool_invoke());
        assert!(!checker.has_secrets());
    }

    #[test]
    fn test_with_http_capability() {
        let caps = WasmCapabilities {
            http: Some(HttpCapability {
                allowlist: vec![],
                credentials: vec![],
                rate_limit: None,
                timeout_secs: 30,
                max_request_bytes: 1_048_576,
                max_response_bytes: 10_485_760,
            }),
            ..Default::default()
        };
        let checker = PermissionChecker::new(Some(caps));
        assert!(checker.has_http());
        assert!(!checker.has_workspace());
    }

    #[test]
    fn test_with_workspace_capability() {
        let caps = WasmCapabilities {
            workspace: Some(WorkspaceCapability {
                allowed_prefixes: vec!["docs/".to_string()],
            }),
            ..Default::default()
        };
        let checker = PermissionChecker::new(Some(caps));
        assert!(checker.has_workspace());
        assert!(!checker.has_http());
    }

    #[test]
    fn test_with_tool_invoke_capability() {
        let caps = WasmCapabilities {
            tool_invoke: Some(ToolInvokeCapability {
                aliases: HashMap::new(),
                max_per_execution: 10,
            }),
            ..Default::default()
        };
        let checker = PermissionChecker::new(Some(caps));
        assert!(checker.has_tool_invoke());
    }

    #[test]
    fn test_with_secrets_capability() {
        let caps = WasmCapabilities {
            secrets: Some(SecretsCapability {
                allowed_patterns: vec!["slack_*".to_string()],
            }),
            ..Default::default()
        };
        let checker = PermissionChecker::new(Some(caps));
        assert!(checker.has_secrets());
    }
}

//! Tool routing decision engine for Server-Client architecture.
//!
//! Determines whether a tool should execute on Server or Client based on:
//! 1. Tool's ExecutionPolicy
//! 2. Client's declared capabilities (Manifest)
//! 3. Configuration overrides

use crate::dispatcher::ExecutionPolicy;
#[cfg(feature = "gateway")]
use crate::gateway::ClientManifest;
use std::collections::HashMap;

/// Result of routing decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Execute tool on Server locally
    ExecuteLocal,

    /// Route tool execution to Client
    RouteToClient,

    /// Tool cannot be executed (neither Server nor Client capable)
    CannotExecute { reason: String },
}

/// Tool routing decision engine.
pub struct ToolRouter {
    /// Configuration overrides: tool_name -> forced policy
    config_overrides: HashMap<String, ExecutionPolicy>,

    /// Tools available on Server
    server_tools: HashMap<String, bool>,
}

impl ToolRouter {
    /// Create a new ToolRouter.
    pub fn new() -> Self {
        Self {
            config_overrides: HashMap::new(),
            server_tools: HashMap::new(),
        }
    }

    /// Add a configuration override for a tool.
    pub fn add_override(&mut self, tool_name: impl Into<String>, policy: ExecutionPolicy) {
        self.config_overrides.insert(tool_name.into(), policy);
    }

    /// Register a tool as available on Server.
    pub fn register_server_tool(&mut self, tool_name: impl Into<String>) {
        self.server_tools.insert(tool_name.into(), true);
    }

    /// Check if Server has a tool.
    ///
    /// Returns true if:
    /// - Exact tool name is registered, OR
    /// - Tool's category (prefix before ':') is registered
    pub fn server_has_tool(&self, tool_name: &str) -> bool {
        // Check exact match first
        if self.server_tools.contains_key(tool_name) {
            return true;
        }

        // Check category match (tool_name format: "category:action" or just "category")
        let category = tool_name.split(':').next().unwrap_or(tool_name);
        self.server_tools.contains_key(category)
    }

    /// Resolve routing decision for a tool.
    #[cfg(feature = "gateway")]
    pub fn resolve(
        &self,
        tool_name: &str,
        tool_policy: ExecutionPolicy,
        client_manifest: Option<&ClientManifest>,
    ) -> RoutingDecision {
        // 1. Check configuration override (highest priority)
        let effective_policy = self
            .config_overrides
            .get(tool_name)
            .copied()
            .unwrap_or(tool_policy);

        // 2. Check capabilities
        let server_capable = self.server_has_tool(tool_name);
        let client_capable = client_manifest
            .map(|m| m.supports_tool(tool_name))
            .unwrap_or(false);

        // 3. Route based on policy
        match effective_policy {
            ExecutionPolicy::ServerOnly => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!(
                            "Tool '{}' requires Server but Server lacks capability",
                            tool_name
                        ),
                    }
                }
            }

            ExecutionPolicy::ClientOnly => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!(
                            "Tool '{}' requires Client but Client lacks capability",
                            tool_name
                        ),
                    }
                }
            }

            ExecutionPolicy::PreferServer => {
                if server_capable {
                    RoutingDecision::ExecuteLocal
                } else if client_capable {
                    RoutingDecision::RouteToClient
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!(
                            "Tool '{}' unavailable on both Server and Client",
                            tool_name
                        ),
                    }
                }
            }

            ExecutionPolicy::PreferClient => {
                if client_capable {
                    RoutingDecision::RouteToClient
                } else if server_capable {
                    RoutingDecision::ExecuteLocal
                } else {
                    RoutingDecision::CannotExecute {
                        reason: format!(
                            "Tool '{}' unavailable on both Client and Server",
                            tool_name
                        ),
                    }
                }
            }
        }
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "gateway"))]
mod tests {
    use super::*;
    use crate::gateway::{ClientCapabilities, ClientEnvironment};

    fn make_manifest(categories: Vec<&str>) -> ClientManifest {
        ClientManifest {
            client_type: "test".to_string(),
            client_version: "1.0.0".to_string(),
            capabilities: ClientCapabilities {
                tool_categories: categories.into_iter().map(String::from).collect(),
                ..Default::default()
            },
            environment: ClientEnvironment::default(),
        }
    }

    #[test]
    fn test_server_only_with_server_capability() {
        let mut router = ToolRouter::new();
        router.register_server_tool("database:query");

        let decision = router.resolve("database:query", ExecutionPolicy::ServerOnly, None);
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_server_only_without_capability() {
        let router = ToolRouter::new();

        let decision = router.resolve("database:query", ExecutionPolicy::ServerOnly, None);
        assert!(matches!(decision, RoutingDecision::CannotExecute { .. }));
    }

    #[test]
    fn test_client_only_with_client_capability() {
        let router = ToolRouter::new();
        let manifest = make_manifest(vec!["shell"]);

        let decision = router.resolve("shell:exec", ExecutionPolicy::ClientOnly, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_client_only_without_capability() {
        let router = ToolRouter::new();

        let decision = router.resolve("shell:exec", ExecutionPolicy::ClientOnly, None);
        assert!(matches!(decision, RoutingDecision::CannotExecute { .. }));
    }

    #[test]
    fn test_prefer_server_uses_server() {
        let mut router = ToolRouter::new();
        router.register_server_tool("search");
        let manifest = make_manifest(vec!["search"]);

        let decision = router.resolve("search", ExecutionPolicy::PreferServer, Some(&manifest));
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_prefer_server_fallback_to_client() {
        let router = ToolRouter::new();
        let manifest = make_manifest(vec!["search"]);

        let decision = router.resolve("search", ExecutionPolicy::PreferServer, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_prefer_client_uses_client() {
        let mut router = ToolRouter::new();
        router.register_server_tool("file_system");
        let manifest = make_manifest(vec!["file_system"]);

        let decision =
            router.resolve("file_system:read", ExecutionPolicy::PreferClient, Some(&manifest));
        assert_eq!(decision, RoutingDecision::RouteToClient);
    }

    #[test]
    fn test_prefer_client_fallback_to_server() {
        let mut router = ToolRouter::new();
        router.register_server_tool("file_system");

        let decision = router.resolve("file_system:read", ExecutionPolicy::PreferClient, None);
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }

    #[test]
    fn test_config_override_takes_precedence() {
        let mut router = ToolRouter::new();
        router.register_server_tool("shell");
        router.add_override("shell:exec", ExecutionPolicy::ServerOnly);

        let manifest = make_manifest(vec!["shell"]);

        // Tool declares PreferClient, but config forces ServerOnly
        let decision =
            router.resolve("shell:exec", ExecutionPolicy::PreferClient, Some(&manifest));
        assert_eq!(decision, RoutingDecision::ExecuteLocal);
    }
}

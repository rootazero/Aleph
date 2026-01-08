//! System Info MCP Service
//!
//! Wraps `services::system_info::MacOsSystemInfo` with MCP protocol adaptation.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use super::BuiltinMcpService;
use crate::error::Result;
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::services::system_info::{MacOsSystemInfo, SystemInfoProvider};

/// System info MCP service
///
/// Provides system information queries (os info, active app, window title).
pub struct SystemInfoService {
    provider: Arc<dyn SystemInfoProvider>,
}

impl SystemInfoService {
    /// Create a new SystemInfoService with default MacOsSystemInfo implementation
    pub fn new() -> Self {
        Self {
            provider: Arc::new(MacOsSystemInfo::new()),
        }
    }

    /// Create a new SystemInfoService with custom provider (for testing)
    pub fn with_provider(provider: Arc<dyn SystemInfoProvider>) -> Self {
        Self { provider }
    }
}

impl Default for SystemInfoService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BuiltinMcpService for SystemInfoService {
    fn name(&self) -> &str {
        "builtin:system"
    }

    fn description(&self) -> &str {
        "System information queries (OS, memory, active app)"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![
            McpResource {
                uri: "system://info".to_string(),
                name: "System Info".to_string(),
                description: Some("Current system information".to_string()),
                mime_type: Some("application/json".to_string()),
            },
        ])
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        match uri {
            "system://info" => {
                let info = self.provider.get_info().await?;
                Ok(serde_json::to_string_pretty(&json!({
                    "os_name": info.os_name,
                    "os_version": info.os_version,
                    "hostname": info.hostname,
                    "username": info.username,
                    "home_dir": info.home_dir,
                    "cpu_arch": info.cpu_arch,
                    "memory_total_gb": info.memory_total as f64 / 1024.0 / 1024.0 / 1024.0,
                    "memory_available_gb": info.memory_available as f64 / 1024.0 / 1024.0 / 1024.0,
                }))?)
            }
            _ => Err(crate::error::AetherError::NotFound(uri.to_string())),
        }
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "sys_info".to_string(),
                description: "Get comprehensive system information".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "active_app".to_string(),
                description: "Get the name of the frontmost application".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "active_window".to_string(),
                description: "Get the title of the active window".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                requires_confirmation: false,
            },
        ]
    }

    async fn call_tool(&self, name: &str, _args: Value) -> Result<McpToolResult> {
        match name {
            "sys_info" => {
                let info = self.provider.get_info().await?;
                Ok(McpToolResult::success(json!({
                    "os_name": info.os_name,
                    "os_version": info.os_version,
                    "hostname": info.hostname,
                    "username": info.username,
                    "home_dir": info.home_dir,
                    "cpu_arch": info.cpu_arch,
                    "memory_total": info.memory_total,
                    "memory_available": info.memory_available,
                })))
            }

            "active_app" => {
                let app = self.provider.active_application().await?;
                Ok(McpToolResult::success(json!({
                    "application": app,
                })))
            }

            "active_window" => {
                let title = self.provider.active_window_title().await?;
                Ok(McpToolResult::success(json!({
                    "title": title,
                })))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, _tool_name: &str) -> bool {
        // System info is read-only, never needs confirmation
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::system_info::SystemInfo;

    /// Mock provider for testing
    struct MockProvider {
        info: SystemInfo,
    }

    impl Default for MockProvider {
        fn default() -> Self {
            Self {
                info: SystemInfo {
                    os_name: "TestOS".to_string(),
                    os_version: "1.0".to_string(),
                    hostname: "test-host".to_string(),
                    username: "testuser".to_string(),
                    home_dir: "/home/testuser".to_string(),
                    cpu_arch: "x86_64".to_string(),
                    memory_total: 16 * 1024 * 1024 * 1024,
                    memory_available: 8 * 1024 * 1024 * 1024,
                },
            }
        }
    }

    #[async_trait]
    impl SystemInfoProvider for MockProvider {
        async fn get_info(&self) -> Result<SystemInfo> {
            Ok(self.info.clone())
        }

        async fn active_application(&self) -> Result<String> {
            Ok("TestApp".to_string())
        }

        async fn active_window_title(&self) -> Result<String> {
            Ok("Test Window".to_string())
        }
    }

    #[tokio::test]
    async fn test_sys_info() {
        let service = SystemInfoService::with_provider(Arc::new(MockProvider::default()));

        let result = service.call_tool("sys_info", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["os_name"], "TestOS");
        assert_eq!(result.content["cpu_arch"], "x86_64");
    }

    #[tokio::test]
    async fn test_active_app() {
        let service = SystemInfoService::with_provider(Arc::new(MockProvider::default()));

        let result = service.call_tool("active_app", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["application"], "TestApp");
    }

    #[tokio::test]
    async fn test_active_window() {
        let service = SystemInfoService::with_provider(Arc::new(MockProvider::default()));

        let result = service.call_tool("active_window", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["title"], "Test Window");
    }
}

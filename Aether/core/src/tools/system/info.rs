//! System Info Tool
//!
//! Provides system information queries via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::error::Result;
use crate::services::system_info::{MacOsSystemInfo, SystemInfoProvider};
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// System tools context
///
/// Provides shared access to system information provider.
#[derive(Clone)]
pub struct SystemContext {
    /// System information provider
    pub provider: Arc<dyn SystemInfoProvider>,
}

impl SystemContext {
    /// Create a new context with default MacOsSystemInfo implementation
    pub fn new() -> Self {
        Self {
            provider: Arc::new(MacOsSystemInfo::new()),
        }
    }

    /// Create a new context with custom provider (for testing)
    pub fn with_provider(provider: Arc<dyn SystemInfoProvider>) -> Self {
        Self { provider }
    }
}

impl Default for SystemContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for sys_info tool
#[derive(Debug, Deserialize)]
struct SysInfoParams {
    /// Type of information to retrieve: "all", "os", "memory", "app", "window"
    #[serde(default = "default_info_type")]
    info_type: String,
}

fn default_info_type() -> String {
    "all".to_string()
}

/// System info tool
///
/// Provides comprehensive system information including:
/// - OS name and version
/// - Hostname and username
/// - CPU architecture
/// - Memory usage
/// - Active application and window
pub struct SystemInfoTool {
    ctx: SystemContext,
}

impl SystemInfoTool {
    /// Create a new SystemInfoTool with the given context
    pub fn new(ctx: SystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for SystemInfoTool {
    fn name(&self) -> &str {
        "sys_info"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "sys_info",
            "Get system information including OS, memory, active application, and window.",
            json!({
                "type": "object",
                "properties": {
                    "info_type": {
                        "type": "string",
                        "enum": ["all", "os", "memory", "app", "window"],
                        "description": "Type of information to retrieve (default: all)",
                        "default": "all"
                    }
                }
            }),
            ToolCategory::Builtin,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: SysInfoParams = serde_json::from_str(args).unwrap_or(SysInfoParams {
            info_type: "all".to_string(),
        });

        match params.info_type.as_str() {
            "os" => {
                let info = self.ctx.provider.get_info().await?;
                Ok(ToolResult::success_with_data(
                    format!(
                        "{} {} ({}) - {} @ {}",
                        info.os_name, info.os_version, info.cpu_arch, info.username, info.hostname
                    ),
                    json!({
                        "os_name": info.os_name,
                        "os_version": info.os_version,
                        "hostname": info.hostname,
                        "username": info.username,
                        "home_dir": info.home_dir,
                        "cpu_arch": info.cpu_arch,
                    }),
                ))
            }

            "memory" => {
                let info = self.ctx.provider.get_info().await?;
                let total_gb = info.memory_total as f64 / 1024.0 / 1024.0 / 1024.0;
                let available_gb = info.memory_available as f64 / 1024.0 / 1024.0 / 1024.0;
                let used_gb = total_gb - available_gb;
                let usage_percent = (used_gb / total_gb * 100.0).round();

                Ok(ToolResult::success_with_data(
                    format!(
                        "Memory: {:.1}GB used / {:.1}GB total ({:.0}% usage)",
                        used_gb, total_gb, usage_percent
                    ),
                    json!({
                        "memory_total_bytes": info.memory_total,
                        "memory_available_bytes": info.memory_available,
                        "memory_total_gb": total_gb,
                        "memory_available_gb": available_gb,
                        "memory_used_gb": used_gb,
                        "usage_percent": usage_percent,
                    }),
                ))
            }

            "app" => {
                let app = self.ctx.provider.active_application().await?;
                Ok(ToolResult::success_with_data(
                    format!("Active application: {}", app),
                    json!({
                        "application": app,
                    }),
                ))
            }

            "window" => {
                let title = self.ctx.provider.active_window_title().await?;
                Ok(ToolResult::success_with_data(
                    format!("Active window: {}", title),
                    json!({
                        "title": title,
                    }),
                ))
            }

            _ => {
                // "all" - return everything
                let info = self.ctx.provider.get_info().await?;
                let app = self.ctx.provider.active_application().await.ok();
                let window = self.ctx.provider.active_window_title().await.ok();

                let total_gb = info.memory_total as f64 / 1024.0 / 1024.0 / 1024.0;
                let available_gb = info.memory_available as f64 / 1024.0 / 1024.0 / 1024.0;

                let content = format!(
                    "{} {} ({}) - {} @ {}\nMemory: {:.1}GB / {:.1}GB{}{}",
                    info.os_name,
                    info.os_version,
                    info.cpu_arch,
                    info.username,
                    info.hostname,
                    available_gb,
                    total_gb,
                    app.as_ref()
                        .map(|a| format!("\nActive app: {}", a))
                        .unwrap_or_default(),
                    window
                        .as_ref()
                        .map(|w| format!("\nActive window: {}", w))
                        .unwrap_or_default(),
                );

                Ok(ToolResult::success_with_data(
                    content,
                    json!({
                        "os_name": info.os_name,
                        "os_version": info.os_version,
                        "hostname": info.hostname,
                        "username": info.username,
                        "home_dir": info.home_dir,
                        "cpu_arch": info.cpu_arch,
                        "memory_total": info.memory_total,
                        "memory_available": info.memory_available,
                        "active_application": app,
                        "active_window": window,
                    }),
                ))
            }
        }
    }

    fn requires_confirmation(&self) -> bool {
        false // Read-only operation
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Builtin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::system_info::SystemInfo;

    /// Mock provider for testing
    struct MockProvider {
        info: SystemInfo,
        active_app: String,
        window_title: String,
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
                    memory_total: 16 * 1024 * 1024 * 1024,     // 16GB
                    memory_available: 8 * 1024 * 1024 * 1024, // 8GB
                },
                active_app: "TestApp".to_string(),
                window_title: "Test Window".to_string(),
            }
        }
    }

    #[async_trait]
    impl SystemInfoProvider for MockProvider {
        async fn get_info(&self) -> Result<SystemInfo> {
            Ok(self.info.clone())
        }

        async fn active_application(&self) -> Result<String> {
            Ok(self.active_app.clone())
        }

        async fn active_window_title(&self) -> Result<String> {
            Ok(self.window_title.clone())
        }
    }

    fn create_test_tool() -> SystemInfoTool {
        let ctx = SystemContext::with_provider(Arc::new(MockProvider::default()));
        SystemInfoTool::new(ctx)
    }

    #[tokio::test]
    async fn test_sys_info_all() {
        let tool = create_test_tool();

        let args = json!({}).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("TestOS"));
        assert!(result.content.contains("testuser"));

        if let Some(data) = &result.data {
            assert_eq!(data["os_name"], "TestOS");
            assert_eq!(data["cpu_arch"], "x86_64");
        }
    }

    #[tokio::test]
    async fn test_sys_info_os() {
        let tool = create_test_tool();

        let args = json!({ "info_type": "os" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("TestOS"));

        if let Some(data) = &result.data {
            assert_eq!(data["os_name"], "TestOS");
            assert!(data.get("memory_total").is_none());
        }
    }

    #[tokio::test]
    async fn test_sys_info_memory() {
        let tool = create_test_tool();

        let args = json!({ "info_type": "memory" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Memory"));
        assert!(result.content.contains("16.0GB"));

        if let Some(data) = &result.data {
            assert_eq!(data["memory_total_gb"], 16.0);
            assert_eq!(data["usage_percent"], 50.0);
        }
    }

    #[tokio::test]
    async fn test_sys_info_app() {
        let tool = create_test_tool();

        let args = json!({ "info_type": "app" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("TestApp"));

        if let Some(data) = &result.data {
            assert_eq!(data["application"], "TestApp");
        }
    }

    #[tokio::test]
    async fn test_sys_info_window() {
        let tool = create_test_tool();

        let args = json!({ "info_type": "window" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Test Window"));

        if let Some(data) = &result.data {
            assert_eq!(data["title"], "Test Window");
        }
    }

    #[test]
    fn test_sys_info_metadata() {
        let tool = create_test_tool();

        assert_eq!(tool.name(), "sys_info");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Builtin);
    }
}

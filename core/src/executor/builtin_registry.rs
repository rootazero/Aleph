//! Builtin Tool Registry for Agent Loop
//!
//! This module provides a `BuiltinToolRegistry` that implements the `ToolRegistry` trait,
//! allowing the Agent Loop's SingleStepExecutor to directly invoke builtin tools without
//! going through rig's agent framework.
//!
//! # Safety Features
//!
//! The registry integrates with the Three-Layer Control architecture's CapabilityGate
//! to enforce capability-based access control on tool execution.
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::executor::{BuiltinToolRegistry, SingleStepExecutor};
//! use aethecore::three_layer::{Capability, CapabilityGate};
//!
//! // Create registry with capability restrictions
//! let gate = CapabilityGate::new(vec![
//!     Capability::FileRead,
//!     Capability::WebSearch,
//! ]);
//! let registry = BuiltinToolRegistry::with_gate(gate);
//! let executor = SingleStepExecutor::new(Arc::new(registry));
//! ```

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use rig::tool::Tool;
use serde_json::Value;
use tracing::{debug, error, info};

use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::error::{AetherError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::{CodeExecTool, FileOpsTool, PdfGenerateTool, SearchTool, WebFetchTool, YouTubeTool};
use crate::three_layer::{Capability, CapabilityGate};

use super::ToolRegistry;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
}

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
/// Integrates with CapabilityGate for security enforcement.
pub struct BuiltinToolRegistry {
    /// Search tool instance
    search_tool: SearchTool,
    /// Web fetch tool instance
    web_fetch_tool: WebFetchTool,
    /// YouTube tool instance
    youtube_tool: YouTubeTool,
    /// File operations tool instance
    file_ops_tool: FileOpsTool,
    /// Code execution tool instance
    code_exec_tool: CodeExecTool,
    /// PDF generation tool instance
    pdf_generate_tool: PdfGenerateTool,
    /// Tool metadata for lookup
    tools: HashMap<String, UnifiedTool>,
    /// Capability gate for security enforcement
    capability_gate: CapabilityGate,
}

impl BuiltinToolRegistry {
    /// Create a new registry with default configuration
    ///
    /// Uses a permissive CapabilityGate that allows safe operations by default.
    pub fn new() -> Self {
        Self::with_config(BuiltinToolConfig::default())
    }

    /// Create a new registry with custom configuration
    ///
    /// Uses a permissive CapabilityGate that allows full AI Agent operations.
    /// Aether is designed as a powerful AI Agent that needs to perform complex
    /// multi-step tasks including file operations and code execution.
    ///
    /// # Enabled Capabilities
    /// - File operations: read, list, write, delete
    /// - Network: web search, web fetch
    /// - Code execution: shell commands, process spawning
    /// - LLM calls
    ///
    /// # Safety Notes
    /// - Dangerous commands are still blocked by CommandChecker (rm -rf /, sudo, etc.)
    /// - File operations are sandboxed by PathPermissionChecker
    pub fn with_config(config: BuiltinToolConfig) -> Self {
        // Full AI Agent capabilities - Aether is a super-powered agent
        let gate = CapabilityGate::new(vec![
            // File system operations
            Capability::FileRead,
            Capability::FileList,
            Capability::FileWrite,
            Capability::FileDelete, // Enabled for cleanup operations
            // Network operations
            Capability::WebSearch,
            Capability::WebFetch,
            // Code execution
            Capability::ShellExec,    // Enabled for code execution
            Capability::ProcessSpawn, // Enabled for spawning processes
            // LLM
            Capability::LlmCall,
        ]);
        Self::with_config_and_gate(config, gate)
    }

    /// Create a new registry with custom capability gate
    ///
    /// Allows fine-grained control over which operations are permitted.
    pub fn with_gate(gate: CapabilityGate) -> Self {
        Self::with_config_and_gate(BuiltinToolConfig::default(), gate)
    }

    /// Create a new registry with custom configuration and capability gate
    pub fn with_config_and_gate(config: BuiltinToolConfig, capability_gate: CapabilityGate) -> Self {
        let search_tool = SearchTool::with_api_key(config.tavily_api_key);
        let web_fetch_tool = WebFetchTool::new();
        let youtube_tool = YouTubeTool::new();
        let file_ops_tool = FileOpsTool::new();
        let code_exec_tool = CodeExecTool::new();
        let pdf_generate_tool = PdfGenerateTool::new();

        // Build tool metadata
        let mut tools = HashMap::new();

        tools.insert(
            "search".to_string(),
            UnifiedTool::new(
                "builtin:search",
                "search",
                SearchTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "web_fetch".to_string(),
            UnifiedTool::new(
                "builtin:web_fetch",
                "web_fetch",
                "Fetch and read content from a URL",
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "youtube".to_string(),
            UnifiedTool::new(
                "builtin:youtube",
                "youtube",
                "Get information about YouTube videos",
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "file_ops".to_string(),
            UnifiedTool::new(
                "builtin:file_ops",
                "file_ops",
                "File system operations - list, read, write, move, copy, delete, etc.",
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "code_exec".to_string(),
            UnifiedTool::new(
                "builtin:code_exec",
                "code_exec",
                CodeExecTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "pdf_generate".to_string(),
            UnifiedTool::new(
                "builtin:pdf_generate",
                "pdf_generate",
                PdfGenerateTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        // TODO: Add image generation tool when generation_registry is provided

        Self {
            search_tool,
            web_fetch_tool,
            youtube_tool,
            file_ops_tool,
            code_exec_tool,
            pdf_generate_tool,
            tools,
            capability_gate,
        }
    }

    /// Get the required capability for a tool
    fn required_capability(&self, tool_name: &str, arguments: &Value) -> Option<Capability> {
        match tool_name {
            "search" => Some(Capability::WebSearch),
            "web_fetch" => Some(Capability::WebFetch),
            "youtube" => Some(Capability::WebFetch), // YouTube fetches from web
            "file_ops" => {
                // Determine capability based on operation type
                if let Some(op) = arguments.get("operation").and_then(|v| v.as_str()) {
                    match op {
                        "list" | "search" => Some(Capability::FileList),
                        "read" => Some(Capability::FileRead),
                        "write" | "move" | "copy" | "mkdir" | "organize" | "batch_move" => {
                            Some(Capability::FileWrite)
                        }
                        "delete" => Some(Capability::FileDelete),
                        _ => Some(Capability::FileRead), // Default to read for unknown ops
                    }
                } else {
                    Some(Capability::FileRead)
                }
            }
            "code_exec" => Some(Capability::ShellExec), // Code execution requires shell capability
            "pdf_generate" => Some(Capability::FileWrite), // PDF generation writes files
            _ => None,
        }
    }

    /// Check if an operation is permitted by the capability gate
    fn check_capability(&self, tool_name: &str, arguments: &Value) -> Result<()> {
        if let Some(required) = self.required_capability(tool_name, arguments) {
            self.capability_gate.check(&required).map_err(|denied| {
                info!(
                    tool = tool_name,
                    capability = %denied.required,
                    "Capability check failed"
                );
                AetherError::permission_denied(format!(
                    "Operation requires '{}' capability which is not granted",
                    denied.required
                ))
            })
        } else {
            Ok(())
        }
    }

    /// Execute the search tool
    async fn execute_search(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::SearchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid search arguments: {}", e))
            })?;

        let result = self.search_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Search failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the web fetch tool
    async fn execute_web_fetch(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::WebFetchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid web_fetch arguments: {}", e))
            })?;

        let result = self.web_fetch_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Web fetch failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the YouTube tool
    async fn execute_youtube(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::YouTubeArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid youtube arguments: {}", e))
            })?;

        let result = self.youtube_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("YouTube tool failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the file operations tool
    async fn execute_file_ops(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::FileOpsArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid file_ops arguments: {}", e))
            })?;

        let result = self.file_ops_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("File operations failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the code execution tool
    async fn execute_code_exec(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::CodeExecArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid code_exec arguments: {}", e))
            })?;

        let result = self.code_exec_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Code execution failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the PDF generation tool
    async fn execute_pdf_generate(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::PdfGenerateArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid pdf_generate arguments: {}", e))
            })?;

        let result = self.pdf_generate_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("PDF generation failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }
}

impl Default for BuiltinToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry for BuiltinToolRegistry {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
        self.tools.get(name)
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        debug!(tool = tool_name, "Executing builtin tool");

        // Check capability before execution
        if let Err(e) = self.check_capability(tool_name, &arguments) {
            return Box::pin(async move { Err(e) });
        }

        // Match tool name before creating future to avoid lifetime issues
        match tool_name {
            "search" => Box::pin(async move { self.execute_search(arguments).await }),
            "web_fetch" => Box::pin(async move { self.execute_web_fetch(arguments).await }),
            "youtube" => Box::pin(async move { self.execute_youtube(arguments).await }),
            "file_ops" => Box::pin(async move { self.execute_file_ops(arguments).await }),
            "code_exec" => Box::pin(async move { self.execute_code_exec(arguments).await }),
            "pdf_generate" => Box::pin(async move { self.execute_pdf_generate(arguments).await }),
            _ => {
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move {
                    Err(AetherError::tool(format!("Unknown tool: {}", tool)))
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = BuiltinToolRegistry::new();

        // Verify all tools are registered
        assert!(registry.get_tool("search").is_some());
        assert!(registry.get_tool("web_fetch").is_some());
        assert!(registry.get_tool("youtube").is_some());
        assert!(registry.get_tool("file_ops").is_some());
        assert!(registry.get_tool("code_exec").is_some());
        assert!(registry.get_tool("pdf_generate").is_some());

        // Verify unknown tool returns None
        assert!(registry.get_tool("unknown").is_none());
    }

    #[test]
    fn test_tool_metadata() {
        let registry = BuiltinToolRegistry::new();

        let search = registry.get_tool("search").unwrap();
        assert_eq!(search.name, "search");
        assert_eq!(search.id, "builtin:search");
        assert!(matches!(search.source, ToolSource::Builtin));
    }

    #[tokio::test]
    async fn test_unknown_tool_execution() {
        let registry = BuiltinToolRegistry::new();

        let result = registry
            .execute_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"));
    }

    #[test]
    fn test_required_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // Search requires WebSearch
        assert_eq!(
            registry.required_capability("search", &serde_json::json!({})),
            Some(Capability::WebSearch)
        );

        // Web fetch requires WebFetch
        assert_eq!(
            registry.required_capability("web_fetch", &serde_json::json!({})),
            Some(Capability::WebFetch)
        );

        // File ops - read operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "read"})),
            Some(Capability::FileRead)
        );

        // File ops - list operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "list"})),
            Some(Capability::FileList)
        );

        // File ops - write operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "write"})),
            Some(Capability::FileWrite)
        );

        // File ops - delete operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "delete"})),
            Some(Capability::FileDelete)
        );
    }

    #[tokio::test]
    async fn test_capability_check_denied() {
        // Create registry with only WebSearch capability
        let gate = CapabilityGate::new(vec![Capability::WebSearch]);
        let registry = BuiltinToolRegistry::with_gate(gate);

        // Search should work (WebSearch granted)
        let search_result = registry.check_capability("search", &serde_json::json!({}));
        assert!(search_result.is_ok());

        // File ops read should fail (FileRead not granted)
        let file_result =
            registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(file_result.is_err());
        let err_msg = file_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Permission denied") || err_msg.contains("capability"),
            "Expected permission error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_file_delete_allowed_by_default() {
        // Default registry now grants FileDelete for super-powered AI Agent
        let registry = BuiltinToolRegistry::new();

        // Delete capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "delete"}));
        assert!(check.is_ok(), "FileDelete should be allowed by default");
    }

    #[tokio::test]
    async fn test_code_exec_allowed_by_default() {
        // Default registry grants ShellExec for code execution
        let registry = BuiltinToolRegistry::new();

        // Code execution capability check should pass
        let check = registry.check_capability("code_exec", &serde_json::json!({}));
        assert!(check.is_ok(), "ShellExec should be allowed by default");
    }

    #[test]
    fn test_code_exec_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // Code exec requires ShellExec
        assert_eq!(
            registry.required_capability("code_exec", &serde_json::json!({})),
            Some(Capability::ShellExec)
        );
    }

    #[tokio::test]
    async fn test_file_read_allowed_by_default() {
        // Default registry grants FileRead
        let registry = BuiltinToolRegistry::new();

        // Read capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(check.is_ok());
    }

    #[tokio::test]
    async fn test_file_write_allowed_by_default() {
        // Default registry grants FileWrite for AI Agent tasks
        let registry = BuiltinToolRegistry::new();

        // Write capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "write"}));
        assert!(check.is_ok());

        // Other write-like operations should also pass
        let check_mkdir = registry.check_capability("file_ops", &serde_json::json!({"operation": "mkdir"}));
        assert!(check_mkdir.is_ok());

        let check_copy = registry.check_capability("file_ops", &serde_json::json!({"operation": "copy"}));
        assert!(check_copy.is_ok());
    }

    #[test]
    fn test_pdf_generate_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // PDF generate requires FileWrite
        assert_eq!(
            registry.required_capability("pdf_generate", &serde_json::json!({})),
            Some(Capability::FileWrite)
        );
    }

    #[tokio::test]
    async fn test_pdf_generate_allowed_by_default() {
        // Default registry grants FileWrite for PDF generation
        let registry = BuiltinToolRegistry::new();

        // PDF generate capability check should pass
        let check = registry.check_capability("pdf_generate", &serde_json::json!({}));
        assert!(check.is_ok(), "FileWrite should be allowed for pdf_generate");
    }
}

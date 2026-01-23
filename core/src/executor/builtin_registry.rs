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

use crate::agents::sub_agents::{DelegateTool, DelegateArgs, SubAgentDispatcher};
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AetherError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::{CodeExecTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool, SearchTool, WebFetchTool, YouTubeTool};
use crate::rig_tools::meta_tools::{ListToolsTool, GetToolSchemaTool, ListToolsArgs, GetToolSchemaArgs};
use crate::three_layer::{Capability, CapabilityGate};
use tokio::sync::RwLock;

use super::ToolRegistry;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    pub sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
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
    /// Image generation tool instance (optional - requires generation registry)
    image_generate_tool: Option<ImageGenerateTool>,
    /// Generation provider registry for video/audio generation
    generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
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

        // Create image generation tool if generation registry is provided
        let image_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating ImageGenerateTool with generation registry");
            ImageGenerateTool::new(Arc::clone(registry))
        });

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

        // Add generation tools if registry is available
        let generation_registry = config.generation_registry.clone();
        if let Some(ref registry) = generation_registry {
            // Add image generation tool
            if image_generate_tool.is_some() {
                tools.insert(
                    "generate_image".to_string(),
                    UnifiedTool::new(
                        "builtin:generate_image",
                        "generate_image",
                        ImageGenerateTool::DESCRIPTION,
                        ToolSource::Builtin,
                    ),
                );
                info!("Registered generate_image tool in BuiltinToolRegistry");
            }

            // Check if video generation providers are available and add tool
            if let Ok(reg) = registry.read() {
                use crate::generation::GenerationType;

                // Add video generation tool if providers are available
                if reg.first_for_type(GenerationType::Video).is_some() {
                    tools.insert(
                        "generate_video".to_string(),
                        UnifiedTool::new(
                            "builtin:generate_video",
                            "generate_video",
                            "Generate videos from text descriptions",
                            ToolSource::Builtin,
                        ),
                    );
                    info!("Registered generate_video tool in BuiltinToolRegistry");
                }

                // Add audio generation tool if providers are available
                if reg.first_for_type(GenerationType::Audio).is_some() {
                    tools.insert(
                        "generate_audio".to_string(),
                        UnifiedTool::new(
                            "builtin:generate_audio",
                            "generate_audio",
                            "Generate audio/music from text descriptions",
                            ToolSource::Builtin,
                        ),
                    );
                    info!("Registered generate_audio tool in BuiltinToolRegistry");
                }
            }
        }

        // Add meta tools for smart tool discovery (if dispatcher registry is provided)
        let dispatcher_registry = config.dispatcher_registry.clone();
        if dispatcher_registry.is_some() {
            tools.insert(
                "list_tools".to_string(),
                UnifiedTool::new(
                    "builtin:list_tools",
                    "list_tools",
                    ListToolsTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );

            tools.insert(
                "get_tool_schema".to_string(),
                UnifiedTool::new(
                    "builtin:get_tool_schema",
                    "get_tool_schema",
                    GetToolSchemaTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );

            info!("Registered meta tools (list_tools, get_tool_schema) in BuiltinToolRegistry");
        }

        // Add delegate tool for sub-agent delegation (if sub_agent_dispatcher is provided)
        let sub_agent_dispatcher = config.sub_agent_dispatcher.clone();
        if sub_agent_dispatcher.is_some() {
            tools.insert(
                "delegate".to_string(),
                UnifiedTool::new(
                    "builtin:delegate",
                    "delegate",
                    "Delegate a task to a specialized sub-agent for tool discovery (MCP tools or skill workflows)",
                    ToolSource::Builtin,
                ),
            );
            info!("Registered delegate tool in BuiltinToolRegistry");
        }

        Self {
            search_tool,
            web_fetch_tool,
            youtube_tool,
            file_ops_tool,
            code_exec_tool,
            pdf_generate_tool,
            image_generate_tool,
            generation_registry,
            dispatcher_registry,
            sub_agent_dispatcher,
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
            "generate_image" => Some(Capability::LlmCall), // Image generation uses LLM-like API
            "generate_video" => Some(Capability::LlmCall), // Video generation uses LLM-like API
            "generate_audio" => Some(Capability::LlmCall), // Audio generation uses LLM-like API
            // Meta tools for smart tool discovery - no special capability required
            "list_tools" | "get_tool_schema" => None,
            // Delegate tool - no special capability required (sub-agents handle their own capabilities)
            "delegate" => None,
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

    /// Execute the image generation tool
    async fn execute_image_generate(&self, arguments: Value) -> Result<Value> {
        let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
            AetherError::tool("Image generation not available: no generation registry configured")
        })?;

        let args: crate::rig_tools::ImageGenerateArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid generate_image arguments: {}", e))
            })?;

        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Image generation failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the video generation tool
    async fn execute_video_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AetherError::tool("Video generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AetherError::tool("Invalid generate_video arguments: expected object")
        })?;

        let prompt = obj.get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AetherError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AetherError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Video) {
                    return Err(AetherError::tool(format!(
                        "Provider '{}' does not support video generation", pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Video)
                    .ok_or_else(|| AetherError::tool("No video generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing video generation");

        // Create request and generate
        let request = GenerationRequest::video(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AetherError::tool(format!("Video generation failed: {}", e))
        })?;

        // Build result
        let result = serde_json::json!({
            "provider": name,
            "prompt": prompt,
            "data": match &output.data {
                crate::generation::GenerationData::Url(url) => serde_json::json!({"type": "url", "value": url}),
                crate::generation::GenerationData::LocalPath(path) => serde_json::json!({"type": "file", "value": path}),
                crate::generation::GenerationData::Bytes(bytes) => serde_json::json!({"type": "bytes", "size": bytes.len()}),
            },
            "model": output.metadata.model,
            "duration_ms": output.metadata.duration.map(|d| d.as_millis()),
        });

        Ok(result)
    }

    /// Execute the audio generation tool
    async fn execute_audio_generate(&self, arguments: Value) -> Result<Value> {
        use crate::generation::{GenerationRequest, GenerationType};

        let registry = self.generation_registry.as_ref().ok_or_else(|| {
            AetherError::tool("Audio generation not available: no generation registry configured")
        })?;

        // Parse arguments
        let obj = arguments.as_object().ok_or_else(|| {
            AetherError::tool("Invalid generate_audio arguments: expected object")
        })?;

        let prompt = obj.get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::tool("Missing required parameter: prompt"))?;

        let provider_name = obj.get("provider").and_then(|v| v.as_str());

        // Get provider from registry
        let (name, provider) = {
            let reg = registry.read().map_err(|e| {
                AetherError::tool(format!("Failed to acquire registry lock: {}", e))
            })?;

            if let Some(pname) = provider_name {
                let p = reg.get(pname).ok_or_else(|| {
                    AetherError::tool(format!("Provider '{}' not found", pname))
                })?;
                if !p.supports(GenerationType::Audio) {
                    return Err(AetherError::tool(format!(
                        "Provider '{}' does not support audio generation", pname
                    )));
                }
                (pname.to_string(), p)
            } else {
                reg.first_for_type(GenerationType::Audio)
                    .ok_or_else(|| AetherError::tool("No audio generation provider available"))?
            }
        };

        info!(provider = %name, prompt = %prompt, "Executing audio generation");

        // Create request and generate
        let request = GenerationRequest::audio(prompt);
        let output = provider.generate(request).await.map_err(|e| {
            AetherError::tool(format!("Audio generation failed: {}", e))
        })?;

        // Build result
        let result = serde_json::json!({
            "provider": name,
            "prompt": prompt,
            "data": match &output.data {
                crate::generation::GenerationData::Url(url) => serde_json::json!({"type": "url", "value": url}),
                crate::generation::GenerationData::LocalPath(path) => serde_json::json!({"type": "file", "value": path}),
                crate::generation::GenerationData::Bytes(bytes) => serde_json::json!({"type": "bytes", "size": bytes.len()}),
            },
            "model": output.metadata.model,
            "duration_ms": output.metadata.duration.map(|d| d.as_millis()),
        });

        Ok(result)
    }

    /// Execute the list_tools meta tool
    async fn execute_list_tools(&self, arguments: Value) -> Result<Value> {
        let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
            AetherError::tool("list_tools not available: no dispatcher registry configured")
        })?;

        let args: ListToolsArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid list_tools arguments: {}", e))
        })?;

        // Create a temporary ListToolsTool and execute
        let tool = ListToolsTool::new(Arc::clone(registry));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("list_tools failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the get_tool_schema meta tool
    async fn execute_get_tool_schema(&self, arguments: Value) -> Result<Value> {
        let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
            AetherError::tool("get_tool_schema not available: no dispatcher registry configured")
        })?;

        let args: GetToolSchemaArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid get_tool_schema arguments: {}", e))
        })?;

        // Create a temporary GetToolSchemaTool and execute
        let tool = GetToolSchemaTool::new(Arc::clone(registry));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("get_tool_schema failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the delegate tool for sub-agent delegation
    async fn execute_delegate(&self, arguments: Value) -> Result<Value> {
        let dispatcher = self.sub_agent_dispatcher.as_ref().ok_or_else(|| {
            AetherError::tool("delegate not available: no sub_agent_dispatcher configured")
        })?;

        let args: DelegateArgs = serde_json::from_value(arguments).map_err(|e| {
            AetherError::tool(format!("Invalid delegate arguments: {}", e))
        })?;

        // Create a temporary DelegateTool and execute
        let tool = DelegateTool::new(Arc::clone(dispatcher));
        let result = tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("delegate failed: {}", e))
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
            "generate_image" => Box::pin(async move { self.execute_image_generate(arguments).await }),
            "generate_video" => Box::pin(async move { self.execute_video_generate(arguments).await }),
            "generate_audio" => Box::pin(async move { self.execute_audio_generate(arguments).await }),
            // Meta tools for smart tool discovery
            "list_tools" => Box::pin(async move { self.execute_list_tools(arguments).await }),
            "get_tool_schema" => Box::pin(async move { self.execute_get_tool_schema(arguments).await }),
            // Delegate tool for sub-agent delegation
            "delegate" => Box::pin(async move { self.execute_delegate(arguments).await }),
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

    #[test]
    fn test_meta_tools_not_registered_without_dispatcher_registry() {
        // Without dispatcher registry, meta tools should not be registered
        let registry = BuiltinToolRegistry::new();

        assert!(registry.get_tool("list_tools").is_none());
        assert!(registry.get_tool("get_tool_schema").is_none());
    }

    #[test]
    fn test_meta_tools_registered_with_dispatcher_registry() {
        // With dispatcher registry, meta tools should be registered
        let dispatcher_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let config = BuiltinToolConfig {
            dispatcher_registry: Some(dispatcher_registry),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert!(registry.get_tool("list_tools").is_some());
        assert!(registry.get_tool("get_tool_schema").is_some());
    }

    #[test]
    fn test_meta_tools_no_special_capability() {
        // Meta tools should not require any special capability
        let dispatcher_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let config = BuiltinToolConfig {
            dispatcher_registry: Some(dispatcher_registry),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert_eq!(
            registry.required_capability("list_tools", &serde_json::json!({})),
            None
        );
        assert_eq!(
            registry.required_capability("get_tool_schema", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn test_delegate_tool_not_registered_without_dispatcher() {
        // Without sub_agent_dispatcher, delegate tool should not be registered
        let registry = BuiltinToolRegistry::new();

        assert!(registry.get_tool("delegate").is_none());
    }

    #[test]
    fn test_delegate_tool_registered_with_dispatcher() {
        // With sub_agent_dispatcher, delegate tool should be registered
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert!(registry.get_tool("delegate").is_some());
        let delegate = registry.get_tool("delegate").unwrap();
        assert_eq!(delegate.name, "delegate");
        assert_eq!(delegate.id, "builtin:delegate");
    }

    #[test]
    fn test_delegate_tool_no_special_capability() {
        // Delegate tool should not require any special capability
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert_eq!(
            registry.required_capability("delegate", &serde_json::json!({})),
            None
        );
    }

    #[tokio::test]
    async fn test_delegate_tool_execution() {
        // With sub_agent_dispatcher, delegate tool should execute
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        // Execute delegate tool
        let result = registry.execute_tool(
            "delegate",
            serde_json::json!({
                "prompt": "List available MCP tools",
                "agent": "mcp"
            })
        ).await;

        // Should succeed (even with no tools available, it returns info about available servers)
        assert!(result.is_ok());
    }
}

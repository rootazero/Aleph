//! Core registry implementation for builtin tools

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::agents::sub_agents::SubAgentDispatcher;
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AetherError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::{BashExecTool, CodeExecTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool, SearchTool, WebFetchTool, YouTubeTool};
use crate::rig_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::rig_tools::skill_reader::{ReadSkillTool, ListSkillsTool as SkillListTool};
use crate::three_layer::{Capability, CapabilityGate};
use crate::tools::AetherTool;
use tokio::sync::RwLock;

use super::{BuiltinToolConfig, ToolRegistry};

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
/// Integrates with CapabilityGate for security enforcement.
pub struct BuiltinToolRegistry {
    /// Search tool instance
    pub(crate) search_tool: SearchTool,
    /// Web fetch tool instance
    pub(crate) web_fetch_tool: WebFetchTool,
    /// YouTube tool instance
    pub(crate) youtube_tool: YouTubeTool,
    /// File operations tool instance
    pub(crate) file_ops_tool: FileOpsTool,
    /// Bash execution tool instance (wraps CodeExecTool for shell commands)
    pub(crate) bash_tool: BashExecTool,
    /// Code execution tool instance
    pub(crate) code_exec_tool: CodeExecTool,
    /// PDF generation tool instance
    pub(crate) pdf_generate_tool: PdfGenerateTool,
    /// Image generation tool instance (optional - requires generation registry)
    pub(crate) image_generate_tool: Option<ImageGenerateTool>,
    /// Read skill tool instance (for Progressive Disclosure pattern)
    pub(crate) read_skill_tool: ReadSkillTool,
    /// List skills tool instance
    pub(crate) list_skills_tool: SkillListTool,
    /// Generation provider registry for video/audio generation
    pub(crate) generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub(crate) dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    pub(crate) sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
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
        let bash_tool = BashExecTool::new();
        let code_exec_tool = CodeExecTool::new();
        let pdf_generate_tool = PdfGenerateTool::new();

        // Skill reading tools (Progressive Disclosure pattern)
        let read_skill_tool = ReadSkillTool::default();
        let list_skills_tool = SkillListTool::default();

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
            "bash".to_string(),
            UnifiedTool::new(
                "builtin:bash",
                "bash",
                "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
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

        // Skill reading tools (Progressive Disclosure pattern)
        tools.insert(
            "read_skill".to_string(),
            UnifiedTool::new(
                "builtin:read_skill",
                "read_skill",
                ReadSkillTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "list_skills".to_string(),
            UnifiedTool::new(
                "builtin:list_skills",
                "list_skills",
                SkillListTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        info!("Registered skill reading tools (read_skill, list_skills) in BuiltinToolRegistry");

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
            bash_tool,
            code_exec_tool,
            pdf_generate_tool,
            image_generate_tool,
            read_skill_tool,
            list_skills_tool,
            generation_registry,
            dispatcher_registry,
            sub_agent_dispatcher,
            tools,
            capability_gate,
        }
    }

    /// Get the required capability for a tool
    pub(crate) fn required_capability(&self, tool_name: &str, arguments: &Value) -> Option<Capability> {
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
            "bash" => Some(Capability::ShellExec), // Bash execution requires shell capability
            "code_exec" => Some(Capability::ShellExec), // Code execution requires shell capability
            "pdf_generate" => Some(Capability::FileWrite), // PDF generation writes files
            "generate_image" => Some(Capability::LlmCall), // Image generation uses LLM-like API
            "generate_video" => Some(Capability::LlmCall), // Video generation uses LLM-like API
            "generate_audio" => Some(Capability::LlmCall), // Audio generation uses LLM-like API
            // Meta tools for smart tool discovery - no special capability required
            "list_tools" | "get_tool_schema" => None,
            // Delegate tool - no special capability required (sub-agents handle their own capabilities)
            "delegate" => None,
            // Skill reading tools - no special capability required (just reading skill files)
            "read_skill" | "list_skills" => None,
            _ => None,
        }
    }

    /// Check if an operation is permitted by the capability gate
    pub(crate) fn check_capability(&self, tool_name: &str, arguments: &Value) -> Result<()> {
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

        // Use AetherTool::call_json directly for migrated tools
        // This simplifies the code by avoiding intermediate execute_* methods
        match tool_name {
            // Core tools - use call_json directly via AetherTool trait
            "search" => Box::pin(async move { self.search_tool.call_json(arguments).await }),
            "web_fetch" => Box::pin(async move { self.web_fetch_tool.call_json(arguments).await }),
            "youtube" => Box::pin(async move { self.youtube_tool.call_json(arguments).await }),
            "file_ops" => Box::pin(async move { self.file_ops_tool.call_json(arguments).await }),
            "bash" => Box::pin(async move { self.bash_tool.call_json(arguments).await }),
            "code_exec" => Box::pin(async move { self.code_exec_tool.call_json(arguments).await }),
            "pdf_generate" => Box::pin(async move { self.pdf_generate_tool.call_json(arguments).await }),

            // Generation tools - image uses AetherTool, video/audio use legacy execute_* methods
            "generate_image" => Box::pin(async move {
                let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
                    AetherError::tool("Image generation not available: no generation registry configured")
                })?;
                tool.call_json(arguments).await
            }),
            "generate_video" => Box::pin(async move { self.execute_video_generate(arguments).await }),
            "generate_audio" => Box::pin(async move { self.execute_audio_generate(arguments).await }),

            // Meta tools for smart tool discovery - use call_json
            "list_tools" => Box::pin(async move {
                let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
                    AetherError::tool("list_tools not available: no dispatcher registry configured")
                })?;
                let tool = ListToolsTool::new(Arc::clone(registry));
                tool.call_json(arguments).await
            }),
            "get_tool_schema" => Box::pin(async move {
                let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
                    AetherError::tool("get_tool_schema not available: no dispatcher registry configured")
                })?;
                let tool = GetToolSchemaTool::new(Arc::clone(registry));
                tool.call_json(arguments).await
            }),

            // Delegate tool for sub-agent delegation (uses AetherTool)
            "delegate" => Box::pin(async move { self.execute_delegate(arguments).await }),

            // Skill reading tools - use call_json
            "read_skill" => Box::pin(async move { self.read_skill_tool.call_json(arguments).await }),
            "list_skills" => Box::pin(async move { self.list_skills_tool.call_json(arguments).await }),

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

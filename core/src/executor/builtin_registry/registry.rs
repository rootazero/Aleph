//! Core registry implementation for builtin tools

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::agents::sub_agents::SubAgentDispatcher;
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AlephError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::builtin_tools::{BashExecTool, CodeExecTool, DesktopTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool, PimTool, SearchTool, WebFetchTool, YouTubeTool};
use crate::builtin_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::builtin_tools::skill_reader::{ReadSkillTool, ListSkillsTool as SkillListTool};
#[cfg(feature = "gateway")]
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
#[cfg(feature = "gateway")]
use crate::gateway::context::GatewayContext;
// TODO: Capability system will be reimplemented following OpenClaw's sandbox/tool-policy pattern
// See: /Volumes/TBU4/Workspace/openclaw/src/agents/sandbox/
use crate::tools::AlephTool;
use tokio::sync::RwLock;

use super::{BuiltinToolConfig, ToolRegistry};

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
///
/// TODO: Security enforcement will be reimplemented following OpenClaw's sandbox/tool-policy pattern.
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
    /// Desktop bridge tool instance
    pub(crate) desktop_tool: DesktopTool,
    /// PIM (Personal Information Management) tool instance
    pub(crate) pim_tool: PimTool,
    /// Generation provider registry for video/audio generation
    pub(crate) generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub(crate) dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    pub(crate) sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    #[cfg(feature = "gateway")]
    pub(crate) gateway_context: Option<Arc<GatewayContext>>,
    /// Tool metadata for lookup
    tools: HashMap<String, UnifiedTool>,
}

impl BuiltinToolRegistry {
    /// Create a new registry with default configuration
    pub fn new() -> Self {
        Self::with_config(BuiltinToolConfig::default())
    }

    /// Create a new registry with custom configuration
    ///
    /// Aleph is designed as a powerful AI Agent that needs to perform complex
    /// multi-step tasks including file operations and code execution.
    ///
    /// # Safety Notes
    /// - Dangerous commands are still blocked by CommandChecker (rm -rf /, sudo, etc.)
    /// - File operations are sandboxed by PathPermissionChecker
    /// - TODO: Tool policy will be reimplemented following OpenClaw's sandbox pattern
    pub fn with_config(config: BuiltinToolConfig) -> Self {
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

        // Desktop bridge tool (macOS App required at runtime; gracefully unavailable otherwise)
        let desktop_tool = DesktopTool::new();

        // PIM tool (Calendar, Reminders, Notes, Contacts via Desktop Bridge)
        let pim_tool = PimTool::new();

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

        tools.insert(
            "desktop".to_string(),
            UnifiedTool::new(
                "builtin:desktop",
                "desktop",
                DesktopTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "pim".to_string(),
            UnifiedTool::new(
                "builtin:pim",
                "pim",
                PimTool::DESCRIPTION,
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

        // Add sessions tools (if gateway_context is provided)
        #[cfg(feature = "gateway")]
        let gateway_context = config.gateway_context.clone();
        #[cfg(feature = "gateway")]
        if gateway_context.is_some() {
            tools.insert(
                "sessions_list".to_string(),
                UnifiedTool::new(
                    "builtin:sessions_list",
                    "sessions_list",
                    SessionsListTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );

            tools.insert(
                "sessions_send".to_string(),
                UnifiedTool::new(
                    "builtin:sessions_send",
                    "sessions_send",
                    SessionsSendTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );

            info!("Registered sessions tools (sessions_list, sessions_send) in BuiltinToolRegistry");
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
            desktop_tool,
            pim_tool,
            generation_registry,
            dispatcher_registry,
            sub_agent_dispatcher,
            #[cfg(feature = "gateway")]
            gateway_context,
            tools,
        }
    }

    /// Check if an operation is permitted
    ///
    /// TODO: Implement tool policy following OpenClaw's sandbox/tool-policy pattern.
    /// Currently all operations are permitted; safety is enforced by:
    /// - CommandChecker (blocks dangerous shell commands)
    /// - PathPermissionChecker (sandboxes file operations)
    #[allow(unused_variables)]
    pub(crate) fn check_capability(&self, tool_name: &str, arguments: &Value) -> Result<()> {
        // TODO: Implement OpenClaw-style tool policy
        // See: /Volumes/TBU4/Workspace/openclaw/src/agents/pi-tools.policy.ts
        Ok(())
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

        // Use AlephTool::call_json directly for migrated tools
        // This simplifies the code by avoiding intermediate execute_* methods
        match tool_name {
            // Core tools - use call_json directly via AlephTool trait
            "search" => Box::pin(async move { self.search_tool.call_json(arguments).await }),
            "web_fetch" => Box::pin(async move { self.web_fetch_tool.call_json(arguments).await }),
            "youtube" => Box::pin(async move { self.youtube_tool.call_json(arguments).await }),
            "file_ops" => Box::pin(async move { self.file_ops_tool.call_json(arguments).await }),
            "bash" => Box::pin(async move { self.bash_tool.call_json(arguments).await }),
            "code_exec" => Box::pin(async move { self.code_exec_tool.call_json(arguments).await }),
            "pdf_generate" => Box::pin(async move { self.pdf_generate_tool.call_json(arguments).await }),

            // Generation tools - image uses AlephTool, video/audio use legacy execute_* methods
            "generate_image" => Box::pin(async move {
                let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("Image generation not available: no generation registry configured")
                })?;
                tool.call_json(arguments).await
            }),
            "generate_video" => Box::pin(async move { self.execute_video_generate(arguments).await }),
            "generate_audio" => Box::pin(async move { self.execute_audio_generate(arguments).await }),

            // Meta tools for smart tool discovery - use call_json
            "list_tools" => Box::pin(async move {
                let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
                    AlephError::tool("list_tools not available: no dispatcher registry configured")
                })?;
                let tool = ListToolsTool::new(Arc::clone(registry));
                tool.call_json(arguments).await
            }),
            "get_tool_schema" => Box::pin(async move {
                let registry = self.dispatcher_registry.as_ref().ok_or_else(|| {
                    AlephError::tool("get_tool_schema not available: no dispatcher registry configured")
                })?;
                let tool = GetToolSchemaTool::new(Arc::clone(registry));
                tool.call_json(arguments).await
            }),

            // Delegate tool for sub-agent delegation (uses AlephTool)
            "delegate" => Box::pin(async move { self.execute_delegate(arguments).await }),

            // Skill reading tools - use call_json
            "read_skill" => Box::pin(async move { self.read_skill_tool.call_json(arguments).await }),
            "list_skills" => Box::pin(async move { self.list_skills_tool.call_json(arguments).await }),
            "desktop" => Box::pin(async move { self.desktop_tool.call_json(arguments).await }),
            "pim" => Box::pin(async move { self.pim_tool.call_json(arguments).await }),

            // Sessions tools for cross-session communication (requires gateway feature)
            #[cfg(feature = "gateway")]
            "sessions_list" => Box::pin(async move {
                let context = self.gateway_context.as_ref().ok_or_else(|| {
                    AlephError::tool("sessions_list not available: no gateway context configured")
                })?;
                // Use "main" as default caller_agent_id; in practice, this would come from
                // the agent executing the tool via higher-level context
                let tool = SessionsListTool::new(Arc::clone(context), "main");
                tool.call_json(arguments).await
            }),
            #[cfg(feature = "gateway")]
            "sessions_send" => Box::pin(async move {
                let context = self.gateway_context.as_ref().ok_or_else(|| {
                    AlephError::tool("sessions_send not available: no gateway context configured")
                })?;
                // Note: GatewayContext doesn't implement Clone, so we dereference and clone
                // the inner context for SessionsSendTool which expects GatewayContext by value
                let tool = SessionsSendTool::with_context((**context).clone(), "main");
                tool.call_json(arguments).await
            }),

            _ => {
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move {
                    Err(AlephError::tool(format!("Unknown tool: {}", tool)))
                })
            }
        }
    }
}

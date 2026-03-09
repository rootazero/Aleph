//! Core registry implementation for builtin tools

use std::collections::HashMap;
use std::pin::Pin;
use crate::sync_primitives::Arc;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::agents::sub_agents::SubAgentDispatcher;
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AlephError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::builtin_tools::{BashExecTool, CodeExecTool, ConfigReadTool, ConfigUpdateTool, DesktopTool, FileOpsTool, ImageGenerateTool, MemoryBrowseTool, MemorySearchTool, PdfGenerateTool, PimTool, ProfileUpdateTool, ScratchpadTool, SearchTool, SoulUpdateTool, WebFetchTool};
use crate::builtin_tools::browser_tools::{
    BrowserOpenTool, BrowserClickTool, BrowserTypeTool, BrowserScreenshotTool,
    BrowserSnapshotTool, BrowserNavigateTool, BrowserTabsTool, BrowserSelectTool,
    BrowserEvaluateTool, BrowserFillFormTool, BrowserProfileTool,
};
use crate::builtin_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::builtin_tools::skill_reader::{ReadSkillTool, ListSkillsTool as SkillListTool};
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
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
    /// Soul update tool instance (identity evolution via soul_update)
    pub(crate) soul_update_tool: SoulUpdateTool,
    /// Profile update tool instance (user profile management)
    pub(crate) profile_update_tool: ProfileUpdateTool,
    /// Scratchpad tool instance (project working memory)
    pub(crate) scratchpad_tool: ScratchpadTool,
    /// Config read tool instance (optional - requires config handle)
    pub(crate) config_read_tool: Option<ConfigReadTool>,
    /// Config update tool instance (optional - requires ConfigPatcher)
    pub(crate) config_update_tool: Option<ConfigUpdateTool>,
    /// Memory search tool instance (optional - requires memory_db + embedder)
    pub(crate) memory_search_tool: Option<MemorySearchTool>,
    /// Memory browse tool instance (optional - requires memory_db)
    pub(crate) memory_browse_tool: Option<MemoryBrowseTool>,
    /// Shared workspace handle for memory tools — written by ExecutionEngine after workspace resolution
    memory_workspace_handle: Option<Arc<RwLock<String>>>,
    /// Generation provider registry for video/audio generation
    pub(crate) generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub(crate) dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    pub(crate) sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    pub(crate) gateway_context: Option<Arc<GatewayContext>>,
    /// Agent management tools (optional - requires AgentRegistry + WorkspaceManager)
    pub(crate) agent_create_tool: Option<crate::builtin_tools::agent_manage::AgentCreateTool>,
    pub(crate) agent_switch_tool: Option<crate::builtin_tools::agent_manage::AgentSwitchTool>,
    pub(crate) agent_list_tool: Option<crate::builtin_tools::agent_manage::AgentListTool>,
    pub(crate) agent_delete_tool: Option<crate::builtin_tools::agent_manage::AgentDeleteTool>,
    /// Subagent management tools (optional - requires SubAgentDispatcher + SubAgentRegistry)
    pub(crate) subagent_spawn_tool: Option<crate::builtin_tools::subagent_manage::SubagentSpawnTool>,
    pub(crate) subagent_steer_tool: Option<crate::builtin_tools::subagent_manage::SubagentSteerTool>,
    pub(crate) subagent_kill_tool: Option<crate::builtin_tools::subagent_manage::SubagentKillTool>,
    /// Browser tools (always available, share a single ProfileManager)
    pub(crate) browser_open_tool: BrowserOpenTool,
    pub(crate) browser_click_tool: BrowserClickTool,
    pub(crate) browser_type_tool: BrowserTypeTool,
    pub(crate) browser_screenshot_tool: BrowserScreenshotTool,
    pub(crate) browser_snapshot_tool: BrowserSnapshotTool,
    pub(crate) browser_navigate_tool: BrowserNavigateTool,
    pub(crate) browser_tabs_tool: BrowserTabsTool,
    pub(crate) browser_select_tool: BrowserSelectTool,
    pub(crate) browser_evaluate_tool: BrowserEvaluateTool,
    pub(crate) browser_fill_form_tool: BrowserFillFormTool,
    pub(crate) browser_profile_tool: BrowserProfileTool,
    /// Session context handle for agent management tools
    session_context_handle: Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
    /// Tool policy handle for per-agent tool access control
    tool_policy_handle: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
    /// Event bus for lifecycle event emission (held for future use; tools get their own clones)
    #[allow(dead_code)]
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
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
        let file_ops_tool = FileOpsTool::new();
        let bash_tool = BashExecTool::new();
        let code_exec_tool = CodeExecTool::new();
        let pdf_generate_tool = PdfGenerateTool::new();

        // Skill reading tools (Progressive Disclosure pattern)
        let read_skill_tool = ReadSkillTool::default();
        let list_skills_tool = SkillListTool::default();

        // Desktop bridge tool — use native in-process path,
        // with fallback to IPC bridge for unsupported actions
        let desktop_tool = {
            let native = std::sync::Arc::new(aleph_desktop::NativeDesktop::new());
            DesktopTool::new().with_native(native)
        };

        // PIM tool (Calendar, Reminders, Notes, Contacts via Desktop Bridge)
        let pim_tool = PimTool::new();

        // Soul update tool — evolves AI identity via ~/.aleph/soul.md
        let aleph_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".aleph");
        let soul_update_tool = SoulUpdateTool::new(aleph_dir.join("soul.md"));
        let profile_update_tool = ProfileUpdateTool::new(aleph_dir.join("user_profile.md"));
        let scratchpad_tool = ScratchpadTool::new();

        // Browser tools — always available, use ProfileManager from config or create default
        let browser_profile_manager = config.browser_profile_manager.clone().unwrap_or_else(|| {
            Arc::new(crate::browser::manager::ProfileManager::new(
                crate::browser::profile::BrowserSystemConfig::default(),
            ))
        });
        let browser_open_tool = BrowserOpenTool::new(Arc::clone(&browser_profile_manager));
        let browser_click_tool = BrowserClickTool::new(Arc::clone(&browser_profile_manager));
        let browser_type_tool = BrowserTypeTool::new(Arc::clone(&browser_profile_manager));
        let browser_screenshot_tool = BrowserScreenshotTool::new(Arc::clone(&browser_profile_manager));
        let browser_snapshot_tool = BrowserSnapshotTool::new(Arc::clone(&browser_profile_manager));
        let browser_navigate_tool = BrowserNavigateTool::new(Arc::clone(&browser_profile_manager));
        let browser_tabs_tool = BrowserTabsTool::new(Arc::clone(&browser_profile_manager));
        let browser_select_tool = BrowserSelectTool::new(Arc::clone(&browser_profile_manager));
        let browser_evaluate_tool = BrowserEvaluateTool::new(Arc::clone(&browser_profile_manager));
        let browser_fill_form_tool = BrowserFillFormTool::new(Arc::clone(&browser_profile_manager));
        let browser_profile_tool = BrowserProfileTool::new(browser_profile_manager);

        // Create config tools if handles are provided
        let config_read_tool = config.config.as_ref().map(|cfg| {
            info!("Creating ConfigReadTool with config handle");
            ConfigReadTool::new(Arc::clone(cfg))
        });
        let config_update_tool = config.config_patcher.as_ref().map(|patcher| {
            info!("Creating ConfigUpdateTool with ConfigPatcher");
            ConfigUpdateTool::new(Arc::clone(patcher))
        });

        // Create memory tools if backend and embedder are provided
        let (memory_search_tool, memory_browse_tool, memory_workspace_handle) =
            if let (Some(ref db), Some(ref embedder)) = (&config.memory_db, &config.embedder) {
                let search_tool = MemorySearchTool::new_with_embedder(db.clone(), Arc::clone(embedder));
                let ws_handle = search_tool.default_workspace_handle();
                let mut browse_tool = MemoryBrowseTool::new(db.clone());
                // Share the same workspace handle between search and browse tools
                browse_tool.set_workspace_handle(Arc::clone(&ws_handle));
                info!("Created memory_search and memory_browse tools");
                (Some(search_tool), Some(browse_tool), Some(ws_handle))
            } else if let Some(ref db) = config.memory_db {
                // No embedder — can still create browse tool (no embedding needed)
                let browse_tool = MemoryBrowseTool::new(db.clone());
                let ws_handle = browse_tool.default_workspace_handle();
                info!("Created memory_browse tool (no embedder for memory_search)");
                (None, Some(browse_tool), Some(ws_handle))
            } else {
                (None, None, None)
            };

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

        tools.insert(
            "soul_update".to_string(),
            UnifiedTool::new(
                "builtin:soul_update",
                "soul_update",
                SoulUpdateTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "profile_update".to_string(),
            UnifiedTool::new(
                "builtin:profile_update",
                "profile_update",
                ProfileUpdateTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "scratchpad".to_string(),
            UnifiedTool::new(
                "builtin:scratchpad",
                "scratchpad",
                ScratchpadTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        // Register browser tools metadata
        for (name, desc) in [
            ("browser_open", BrowserOpenTool::DESCRIPTION),
            ("browser_click", BrowserClickTool::DESCRIPTION),
            ("browser_type", BrowserTypeTool::DESCRIPTION),
            ("browser_screenshot", BrowserScreenshotTool::DESCRIPTION),
            ("browser_snapshot", BrowserSnapshotTool::DESCRIPTION),
            ("browser_navigate", BrowserNavigateTool::DESCRIPTION),
            ("browser_tabs", BrowserTabsTool::DESCRIPTION),
            ("browser_select", BrowserSelectTool::DESCRIPTION),
            ("browser_evaluate", BrowserEvaluateTool::DESCRIPTION),
            ("browser_fill_form", BrowserFillFormTool::DESCRIPTION),
            ("browser_profile", BrowserProfileTool::DESCRIPTION),
        ] {
            tools.insert(
                name.to_string(),
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin),
            );
        }
        info!("Registered browser tools (11 tools) in BuiltinToolRegistry");

        info!("Registered skill reading tools (read_skill, list_skills) in BuiltinToolRegistry");

        // Add memory tools if backend/embedder are available
        if memory_search_tool.is_some() {
            tools.insert(
                "memory_search".to_string(),
                UnifiedTool::new(
                    "builtin:memory_search",
                    "memory_search",
                    MemorySearchTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );
            info!("Registered memory_search tool in BuiltinToolRegistry");
        }
        if memory_browse_tool.is_some() {
            tools.insert(
                "memory_browse".to_string(),
                UnifiedTool::new(
                    "builtin:memory_browse",
                    "memory_browse",
                    MemoryBrowseTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );
            info!("Registered memory_browse tool in BuiltinToolRegistry");
        }

        // Add config tools if handles are available
        if config_read_tool.is_some() {
            tools.insert(
                "config_read".to_string(),
                UnifiedTool::new(
                    "builtin:config_read",
                    "config_read",
                    ConfigReadTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );
            info!("Registered config_read tool in BuiltinToolRegistry");
        }
        if config_update_tool.is_some() {
            tools.insert(
                "config_update".to_string(),
                UnifiedTool::new(
                    "builtin:config_update",
                    "config_update",
                    ConfigUpdateTool::DESCRIPTION,
                    ToolSource::Builtin,
                ),
            );
            info!("Registered config_update tool in BuiltinToolRegistry");
        }

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
        let gateway_context = config.gateway_context.clone();
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

        // Add subagent management tools (if SubAgentDispatcher is available)
        let (subagent_spawn_tool, subagent_steer_tool, subagent_kill_tool) =
            if let Some(ref dispatcher) = config.sub_agent_dispatcher {
                use crate::builtin_tools::subagent_manage;
                let spawn = subagent_manage::SubagentSpawnTool::new(Arc::clone(dispatcher));
                let steer = config.sub_agent_registry.as_ref().map(|reg| {
                    subagent_manage::SubagentSteerTool::new(Arc::clone(dispatcher), Arc::clone(reg))
                });
                let kill = config.sub_agent_registry.as_ref().map(|reg| {
                    subagent_manage::SubagentKillTool::new(Arc::clone(dispatcher), Arc::clone(reg))
                });

                // Register tool metadata
                tools.insert(
                    "subagent_spawn".to_string(),
                    UnifiedTool::new(
                        "builtin:subagent_spawn",
                        "subagent_spawn",
                        subagent_manage::SubagentSpawnTool::DESCRIPTION,
                        ToolSource::Builtin,
                    ),
                );
                if steer.is_some() {
                    tools.insert(
                        "subagent_steer".to_string(),
                        UnifiedTool::new(
                            "builtin:subagent_steer",
                            "subagent_steer",
                            subagent_manage::SubagentSteerTool::DESCRIPTION,
                            ToolSource::Builtin,
                        ),
                    );
                }
                if kill.is_some() {
                    tools.insert(
                        "subagent_kill".to_string(),
                        UnifiedTool::new(
                            "builtin:subagent_kill",
                            "subagent_kill",
                            subagent_manage::SubagentKillTool::DESCRIPTION,
                            ToolSource::Builtin,
                        ),
                    );
                }

                info!("Registered subagent management tools (subagent_spawn{}{})",
                    if steer.is_some() { ", subagent_steer" } else { "" },
                    if kill.is_some() { ", subagent_kill" } else { "" },
                );
                (Some(spawn), steer, kill)
            } else {
                (None, None, None)
            };

        // Add agent management tools (if AgentRegistry + WorkspaceManager are available)
        let (agent_create_tool, agent_switch_tool, agent_list_tool, agent_delete_tool, session_context_handle) =
            if let (Some(ref ar), Some(ref wm)) = (&config.agent_registry, &config.workspace_manager) {
                use crate::builtin_tools::agent_manage;
                let ctx = agent_manage::new_session_context_handle();
                let create = {
                    let tool = agent_manage::AgentCreateTool::new(
                        Arc::clone(ar), Arc::clone(wm),
                    );
                    if let Some(ref am) = config.agent_manager {
                        tool.with_agent_manager(Arc::clone(am))
                    } else {
                        tool
                    }
                };
                let switch = agent_manage::AgentSwitchTool::new(
                    Arc::clone(ar), Arc::clone(wm), config.event_bus.clone(),
                );
                let list = agent_manage::AgentListTool::new(
                    Arc::clone(ar), Arc::clone(wm),
                );
                let delete = agent_manage::AgentDeleteTool::new(
                    Arc::clone(ar), Arc::clone(wm), config.event_bus.clone(),
                );

                for (name, desc) in [
                    ("agent_create", agent_manage::AgentCreateTool::DESCRIPTION),
                    ("agent_switch", agent_manage::AgentSwitchTool::DESCRIPTION),
                    ("agent_list", agent_manage::AgentListTool::DESCRIPTION),
                    ("agent_delete", agent_manage::AgentDeleteTool::DESCRIPTION),
                ] {
                    tools.insert(
                        name.to_string(),
                        UnifiedTool::new(format!("builtin:{}", name), name, desc, ToolSource::Builtin),
                    );
                }

                info!("Registered agent management tools (agent_create, agent_switch, agent_list, agent_delete)");
                (Some(create), Some(switch), Some(list), Some(delete), Some(ctx))
            } else {
                (None, None, None, None, None)
            };

        // Initialize tool policy handle (use provided or create a default one)
        let tool_policy_handle = config.tool_policy.clone()
            .or_else(|| Some(crate::builtin_tools::agent_manage::new_tool_policy_handle()));

        Self {
            search_tool,
            web_fetch_tool,
            file_ops_tool,
            bash_tool,
            code_exec_tool,
            pdf_generate_tool,
            image_generate_tool,
            read_skill_tool,
            list_skills_tool,
            desktop_tool,
            pim_tool,
            soul_update_tool,
            profile_update_tool,
            scratchpad_tool,
            config_read_tool,
            config_update_tool,
            memory_search_tool,
            memory_browse_tool,
            memory_workspace_handle,
            generation_registry,
            dispatcher_registry,
            sub_agent_dispatcher,
            gateway_context,
            subagent_spawn_tool,
            subagent_steer_tool,
            subagent_kill_tool,
            browser_open_tool,
            browser_click_tool,
            browser_type_tool,
            browser_screenshot_tool,
            browser_snapshot_tool,
            browser_navigate_tool,
            browser_tabs_tool,
            browser_select_tool,
            browser_evaluate_tool,
            browser_fill_form_tool,
            browser_profile_tool,
            agent_create_tool,
            agent_switch_tool,
            agent_list_tool,
            agent_delete_tool,
            session_context_handle,
            tool_policy_handle,
            event_bus: config.event_bus.clone(),
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

    fn workspace_handle(&self) -> Option<Arc<RwLock<String>>> {
        self.memory_workspace_handle.clone()
    }

    fn smart_recall_config_handle(
        &self,
    ) -> Option<Arc<RwLock<Option<crate::config::types::profile::SmartRecallConfig>>>> {
        self.memory_search_tool.as_ref().map(|t| t.smart_recall_config_handle())
    }

    fn session_context_handle(
        &self,
    ) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        self.session_context_handle.clone()
    }

    fn tool_policy_handle(&self) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::ToolPolicy>>> {
        self.tool_policy_handle.clone()
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        debug!(tool = tool_name, "Executing builtin tool");

        // Enforce per-agent tool policy.
        // Uses try_read() (non-blocking) since this is a synchronous function.
        // Contention is extremely unlikely — policy is only written during agent_switch.
        if let Some(ref policy_handle) = self.tool_policy_handle {
            if let Ok(policy) = policy_handle.try_read() {
                if !policy.is_allowed(tool_name) {
                    let msg = format!(
                        "Tool '{}' is not allowed for the current agent. \
                         Use agent_list to check available tools, or switch to an agent that has access.",
                        tool_name
                    );
                    return Box::pin(async move { Err(AlephError::tool(msg)) });
                }
            }
        }

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
            "soul_update" => Box::pin(async move { self.soul_update_tool.call_json(arguments).await }),
            "profile_update" => Box::pin(async move { self.profile_update_tool.call_json(arguments).await }),
            "scratchpad" => Box::pin(async move { self.scratchpad_tool.call_json(arguments).await }),

            // Config tools - read/update Aleph configuration
            "config_read" => Box::pin(async move {
                let tool = self.config_read_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("config_read not available: no config handle configured")
                })?;
                tool.call_json(arguments).await
            }),
            "config_update" => Box::pin(async move {
                let tool = self.config_update_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("config_update not available: no ConfigPatcher configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Memory tools - search and browse personal memory
            "memory_search" => Box::pin(async move {
                let tool = self.memory_search_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_search not available: no memory backend or embedding provider configured")
                })?;
                tool.call_json(arguments).await
            }),
            "memory_browse" => Box::pin(async move {
                let tool = self.memory_browse_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_browse not available: no memory backend configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Sessions tools for cross-session communication
            "sessions_list" => Box::pin(async move {
                let context = self.gateway_context.as_ref().ok_or_else(|| {
                    AlephError::tool("sessions_list not available: no gateway context configured")
                })?;
                // Use "main" as default caller_agent_id; in practice, this would come from
                // the agent executing the tool via higher-level context
                let tool = SessionsListTool::new(Arc::clone(context), "main");
                tool.call_json(arguments).await
            }),
            "sessions_send" => Box::pin(async move {
                let context = self.gateway_context.as_ref().ok_or_else(|| {
                    AlephError::tool("sessions_send not available: no gateway context configured")
                })?;
                // Note: GatewayContext doesn't implement Clone, so we dereference and clone
                // the inner context for SessionsSendTool which expects GatewayContext by value
                let tool = SessionsSendTool::with_context((**context).clone(), "main");
                tool.call_json(arguments).await
            }),

            // Subagent management tools
            "subagent_spawn" => Box::pin(async move {
                let tool = self.subagent_spawn_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("subagent_spawn not available: no SubAgentDispatcher configured")
                })?;
                tool.call_json(arguments).await
            }),
            "subagent_steer" => Box::pin(async move {
                let tool = self.subagent_steer_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("subagent_steer not available: no SubAgentDispatcher/SubAgentRegistry configured")
                })?;
                tool.call_json(arguments).await
            }),
            "subagent_kill" => Box::pin(async move {
                let tool = self.subagent_kill_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("subagent_kill not available: no SubAgentDispatcher/SubAgentRegistry configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Browser tools
            "browser_open" => Box::pin(async move { self.browser_open_tool.call_json(arguments).await }),
            "browser_click" => Box::pin(async move { self.browser_click_tool.call_json(arguments).await }),
            "browser_type" => Box::pin(async move { self.browser_type_tool.call_json(arguments).await }),
            "browser_screenshot" => Box::pin(async move { self.browser_screenshot_tool.call_json(arguments).await }),
            "browser_snapshot" => Box::pin(async move { self.browser_snapshot_tool.call_json(arguments).await }),
            "browser_navigate" => Box::pin(async move { self.browser_navigate_tool.call_json(arguments).await }),
            "browser_tabs" => Box::pin(async move { self.browser_tabs_tool.call_json(arguments).await }),
            "browser_select" => Box::pin(async move { self.browser_select_tool.call_json(arguments).await }),
            "browser_evaluate" => Box::pin(async move { self.browser_evaluate_tool.call_json(arguments).await }),
            "browser_fill_form" => Box::pin(async move { self.browser_fill_form_tool.call_json(arguments).await }),
            "browser_profile" => Box::pin(async move { self.browser_profile_tool.call_json(arguments).await }),

            // Agent management tools — snapshot session context into arguments
            // to avoid race conditions from concurrent reads of the shared handle.
            "agent_create" | "agent_switch" | "agent_list" | "agent_delete" => {
                // Snapshot session context into tool arguments before async execution
                let arguments = {
                    let mut args = arguments;
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert("__channel".into(), serde_json::Value::String(ctx.channel.clone()));
                                obj.insert("__peer_id".into(), serde_json::Value::String(ctx.peer_id.clone()));
                            }
                        }
                    }
                    args
                };

                match tool_name {
                    "agent_create" => Box::pin(async move {
                        let tool = self.agent_create_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_create not available: no AgentRegistry/WorkspaceManager configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_switch" => Box::pin(async move {
                        let tool = self.agent_switch_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_switch not available: no AgentRegistry/WorkspaceManager configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_list" => Box::pin(async move {
                        let tool = self.agent_list_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_list not available: no AgentRegistry/WorkspaceManager configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_delete" => Box::pin(async move {
                        let tool = self.agent_delete_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_delete not available: no AgentRegistry/WorkspaceManager configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    _ => unreachable!(),
                }
            }

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

//! Builder / constructor for BuiltinToolRegistry
//!
//! Extracted from registry.rs to keep file sizes manageable.
//! Contains the `with_config()` constructor that wires up all tool instances
//! and registers their metadata.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

use tracing::info;

use crate::builtin_tools::{BashExecTool, CodeExecTool, ConfigReadTool, ConfigUpdateTool, DesktopTool, FileOpsTool, ImageGenerateTool, MemoryBrowseTool, MemorySearchTool, PdfGenerateTool, PimTool, ProfileUpdateTool, ScratchpadTool, SearchTool, SoulUpdateTool, WebFetchTool};
use crate::builtin_tools::browser_tools::{
    BrowserOpenTool, BrowserClickTool, BrowserTypeTool, BrowserScreenshotTool,
    BrowserSnapshotTool, BrowserNavigateTool, BrowserTabsTool, BrowserSelectTool,
    BrowserEvaluateTool, BrowserFillFormTool, BrowserProfileTool,
};
use crate::builtin_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::builtin_tools::skill_reader::{ReadSkillTool, ListSkillsTool as SkillListTool};
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;

use super::{BuiltinToolConfig, BuiltinToolRegistry};

impl BuiltinToolRegistry {
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
        let search_tool = SearchTool::with_api_key(config.tavily_api_key.clone());
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

        // Register always-available tool metadata
        Self::register_core_tools(&mut tools);

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

        // Register optional tool metadata
        Self::register_optional_tools(
            &mut tools,
            &memory_search_tool,
            &memory_browse_tool,
            &config_read_tool,
            &config_update_tool,
            &image_generate_tool,
            &config,
        );

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

                // Register agent tools WITH their parameter schemas so LLMs
                // know which arguments to pass (e.g. agent_id for agent_switch).
                {
                    use crate::tools::AlephTool;
                    let tool_defs = [
                        create.definition(),
                        switch.definition(),
                        list.definition(),
                        delete.definition(),
                    ];
                    for td in &tool_defs {
                        let mut ut = UnifiedTool::new(
                            format!("builtin:{}", td.name),
                            &td.name,
                            &td.description,
                            ToolSource::Builtin,
                        );
                        ut = ut.with_parameters_schema(td.parameters.clone());
                        tools.insert(td.name.clone(), ut);
                    }
                }

                info!("Registered agent management tools (agent_create, agent_switch, agent_list, agent_delete)");
                (Some(create), Some(switch), Some(list), Some(delete), Some(ctx))
            } else {
                (None, None, None, None, None)
            };

        // Add ACP delegate tools (if AcpHarnessManager is provided)
        let (claude_code_tool, codex_tool, gemini_cli_tool, acp_switch_tool) =
            if let Some(ref manager) = config.acp_manager {
                use crate::builtin_tools::acp_tools::{ClaudeCodeTool, CodexTool, GeminiCliTool, AcpSwitchTool};
                info!("Creating ACP delegate tools");

                let cc = if manager.has_harness("claude-code") {
                    tools.insert("claude_code".to_string(), UnifiedTool::new(
                        "builtin:claude_code", "claude_code",
                        ClaudeCodeTool::DESCRIPTION, ToolSource::Builtin,
                    ));
                    Some(ClaudeCodeTool::new(Arc::clone(manager)))
                } else { None };

                let cx = if manager.has_harness("codex") {
                    tools.insert("codex".to_string(), UnifiedTool::new(
                        "builtin:codex", "codex",
                        CodexTool::DESCRIPTION, ToolSource::Builtin,
                    ));
                    Some(CodexTool::new(Arc::clone(manager)))
                } else { None };

                let gm = if manager.has_harness("gemini") {
                    tools.insert("gemini_cli".to_string(), UnifiedTool::new(
                        "builtin:gemini_cli", "gemini_cli",
                        GeminiCliTool::DESCRIPTION, ToolSource::Builtin,
                    ));
                    Some(GeminiCliTool::new(Arc::clone(manager)))
                } else { None };

                // acp_switch is always available when manager exists
                tools.insert("acp_switch".to_string(), UnifiedTool::new(
                    "builtin:acp_switch", "acp_switch",
                    AcpSwitchTool::DESCRIPTION, ToolSource::Builtin,
                ));
                let sw = Some(AcpSwitchTool::new(Arc::clone(manager)));

                info!("Registered ACP tools (claude_code={}, codex={}, gemini_cli={}, acp_switch=true)",
                    cc.is_some(), cx.is_some(), gm.is_some());
                (cc, cx, gm, sw)
            } else {
                (None, None, None, None)
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
            generation_registry: config.generation_registry.clone(),
            dispatcher_registry: config.dispatcher_registry.clone(),
            sub_agent_dispatcher: config.sub_agent_dispatcher.clone(),
            gateway_context: config.gateway_context.clone(),
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
            extension_manager: config.extension_manager.clone(),
            claude_code_tool,
            codex_tool,
            gemini_cli_tool,
            acp_switch_tool,
            tools,
        }
    }

    /// Register always-available core tool metadata
    fn register_core_tools(tools: &mut HashMap<String, UnifiedTool>) {
        for (name, desc) in [
            ("search", SearchTool::DESCRIPTION),
            ("web_fetch", "Fetch and read content from a URL"),
            ("file_ops", "File system operations - list, read, write, move, copy, delete, etc."),
            ("bash", "Execute bash/shell commands (convenience wrapper for code_exec with shell)"),
            ("code_exec", CodeExecTool::DESCRIPTION),
            ("pdf_generate", PdfGenerateTool::DESCRIPTION),
            ("read_skill", ReadSkillTool::DESCRIPTION),
            ("list_skills", SkillListTool::DESCRIPTION),
            ("desktop", DesktopTool::DESCRIPTION),
            ("pim", PimTool::DESCRIPTION),
            ("soul_update", SoulUpdateTool::DESCRIPTION),
            ("profile_update", ProfileUpdateTool::DESCRIPTION),
            ("scratchpad", ScratchpadTool::DESCRIPTION),
        ] {
            tools.insert(
                name.to_string(),
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin),
            );
        }
    }

    /// Register metadata for optional tools (only when their dependencies are available)
    fn register_optional_tools(
        tools: &mut HashMap<String, UnifiedTool>,
        memory_search_tool: &Option<MemorySearchTool>,
        memory_browse_tool: &Option<MemoryBrowseTool>,
        config_read_tool: &Option<ConfigReadTool>,
        config_update_tool: &Option<ConfigUpdateTool>,
        image_generate_tool: &Option<ImageGenerateTool>,
        config: &BuiltinToolConfig,
    ) {
        // Memory tools
        if memory_search_tool.is_some() {
            tools.insert(
                "memory_search".to_string(),
                UnifiedTool::new(
                    "builtin:memory_search", "memory_search",
                    MemorySearchTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered memory_search tool in BuiltinToolRegistry");
        }
        if memory_browse_tool.is_some() {
            tools.insert(
                "memory_browse".to_string(),
                UnifiedTool::new(
                    "builtin:memory_browse", "memory_browse",
                    MemoryBrowseTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered memory_browse tool in BuiltinToolRegistry");
        }

        // Config tools
        if config_read_tool.is_some() {
            tools.insert(
                "config_read".to_string(),
                UnifiedTool::new(
                    "builtin:config_read", "config_read",
                    ConfigReadTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered config_read tool in BuiltinToolRegistry");
        }
        if config_update_tool.is_some() {
            tools.insert(
                "config_update".to_string(),
                UnifiedTool::new(
                    "builtin:config_update", "config_update",
                    ConfigUpdateTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered config_update tool in BuiltinToolRegistry");
        }

        // Generation tools
        let generation_registry = config.generation_registry.clone();
        if let Some(ref registry) = generation_registry {
            if image_generate_tool.is_some() {
                tools.insert(
                    "generate_image".to_string(),
                    UnifiedTool::new(
                        "builtin:generate_image", "generate_image",
                        ImageGenerateTool::DESCRIPTION, ToolSource::Builtin,
                    ),
                );
                info!("Registered generate_image tool in BuiltinToolRegistry");
            }

            if let Ok(reg) = registry.read() {
                use crate::generation::GenerationType;

                if reg.first_for_type(GenerationType::Video).is_some() {
                    tools.insert(
                        "generate_video".to_string(),
                        UnifiedTool::new(
                            "builtin:generate_video", "generate_video",
                            "Generate videos from text descriptions", ToolSource::Builtin,
                        ),
                    );
                    info!("Registered generate_video tool in BuiltinToolRegistry");
                }

                if reg.first_for_type(GenerationType::Audio).is_some() {
                    tools.insert(
                        "generate_audio".to_string(),
                        UnifiedTool::new(
                            "builtin:generate_audio", "generate_audio",
                            "Generate audio/music from text descriptions", ToolSource::Builtin,
                        ),
                    );
                    info!("Registered generate_audio tool in BuiltinToolRegistry");
                }
            }
        }

        // Meta tools for smart tool discovery
        if config.dispatcher_registry.is_some() {
            tools.insert(
                "list_tools".to_string(),
                UnifiedTool::new(
                    "builtin:list_tools", "list_tools",
                    ListToolsTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            tools.insert(
                "get_tool_schema".to_string(),
                UnifiedTool::new(
                    "builtin:get_tool_schema", "get_tool_schema",
                    GetToolSchemaTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered meta tools (list_tools, get_tool_schema) in BuiltinToolRegistry");
        }

        // Delegate tool for sub-agent delegation
        if config.sub_agent_dispatcher.is_some() {
            tools.insert(
                "delegate".to_string(),
                UnifiedTool::new(
                    "builtin:delegate", "delegate",
                    "Delegate a task to a specialized sub-agent for tool discovery (MCP tools or skill workflows)",
                    ToolSource::Builtin,
                ),
            );
            info!("Registered delegate tool in BuiltinToolRegistry");
        }

        // Sessions tools
        if config.gateway_context.is_some() {
            tools.insert(
                "sessions_list".to_string(),
                UnifiedTool::new(
                    "builtin:sessions_list", "sessions_list",
                    SessionsListTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            tools.insert(
                "sessions_send".to_string(),
                UnifiedTool::new(
                    "builtin:sessions_send", "sessions_send",
                    SessionsSendTool::DESCRIPTION, ToolSource::Builtin,
                ),
            );
            info!("Registered sessions tools (sessions_list, sessions_send) in BuiltinToolRegistry");
        }
    }
}

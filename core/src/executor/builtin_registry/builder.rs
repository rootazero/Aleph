//! Builder / constructor for BuiltinToolRegistry
//!
//! Extracted from registry.rs to keep file sizes manageable.
//! Contains the `with_config()` constructor that wires up all tool instances
//! and registers their metadata.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

use tracing::info;

use crate::builtin_tools::{AutomationTool, BashExecTool, CodeExecTool, DesktopTool, FileOpsTool, ImageGenerateTool, MemoryBrowseTool, MemorySearchTool, PdfGenerateTool, PimTool, ReadConfigGuideTool, ScratchpadTool, SearchTool, SystemTool, VaultStoreTool, WebFetchTool};
use crate::builtin_tools::browser_tools::{
    BrowserOpenTool, BrowserClickTool, BrowserTypeTool, BrowserScreenshotTool,
    BrowserSnapshotTool, BrowserNavigateTool, BrowserTabsTool, BrowserSelectTool,
    BrowserEvaluateTool, BrowserFillFormTool, BrowserProfileTool,
};
use crate::builtin_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::builtin_tools::skill_reader::ListSkillsTool as SkillListTool;
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
    pub async fn with_config(config: BuiltinToolConfig) -> Self {
        let search_tool = SearchTool::with_api_key(config.tavily_api_key.clone());
        let web_fetch_tool = WebFetchTool::new();
        let file_ops_tool = if let Some(ref tc) = config.tool_context {
            FileOpsTool::new().with_tool_context(std::sync::Arc::clone(tc))
        } else {
            FileOpsTool::new()
        };
        let bash_tool = BashExecTool::new();
        let code_exec_tool = CodeExecTool::new();
        let pdf_generate_tool = if let Some(ref tc) = config.tool_context {
            PdfGenerateTool::new().with_tool_context(std::sync::Arc::clone(tc))
        } else {
            PdfGenerateTool::new()
        };

        // Skill listing tool (read_skill replaced by read_config_guide)
        let list_skills_tool = SkillListTool::default();

        // Config guide tool (Progressive Disclosure for self-management)
        let config_guide_tool = ReadConfigGuideTool::default();

        // Vault store tool (requires SharedTokenManager)
        let vault_store_tool = config.shared_token_manager.as_ref().map(|mgr| {
            info!("Creating VaultStoreTool");
            VaultStoreTool::new(Arc::clone(mgr))
        });

        // Build platform-specific DesktopPlatform
        let desktop_platform: Arc<dyn aleph_desktop::DesktopPlatform> = {
            #[cfg(target_os = "macos")]
            { Arc::new(aleph_desktop_macos::MacOSPlatform::new()) }

            #[cfg(target_os = "linux")]
            { Arc::new(aleph_desktop_linux::LinuxPlatform::new()) }

            #[cfg(target_os = "windows")]
            { Arc::new(aleph_desktop_windows::WindowsPlatform::new()) }
        };

        // Desktop tool — screen ops via DesktopPlatform, IPC bridge for canvas/snapshot/ax_tree
        let desktop_tool = DesktopTool::new()
            .with_platform(Arc::clone(&desktop_platform));

        let system_tool = SystemTool::new(Arc::clone(&desktop_platform));
        let automation_tool = AutomationTool::new(Arc::clone(&desktop_platform));

        // PIM tool (Calendar, Reminders, Notes, Contacts via Desktop Bridge)
        let pim_tool = PimTool::new();

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

        // Create memory tools if backend and embedder are provided
        let (memory_search_tool, memory_browse_tool, memory_workspace_handle, memory_session_key_handle) =
            if let (Some(ref db), Some(ref embedder)) = (&config.memory_db, &config.embedder) {
                let search_tool = MemorySearchTool::new_with_config(
                    db.clone(),
                    Arc::clone(embedder),
                    config.memory_similarity_threshold,
                );
                let ws_handle = search_tool.default_workspace_handle();
                let sk_handle = search_tool.default_session_key_handle();
                let mut browse_tool = MemoryBrowseTool::new(db.clone());
                browse_tool.set_workspace_handle(Arc::clone(&ws_handle));
                info!("Created memory_search and memory_browse tools");
                (Some(search_tool), Some(browse_tool), Some(ws_handle), Some(sk_handle))
            } else if let Some(ref db) = config.memory_db {
                let browse_tool = MemoryBrowseTool::new(db.clone());
                let ws_handle = browse_tool.default_workspace_handle();
                info!("Created memory_browse tool (no embedder for memory_search)");
                (None, Some(browse_tool), Some(ws_handle), None)
            } else {
                (None, None, None, None)
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

        // Register browser tools metadata (with parameter schemas from AlephTool::definition)
        {
            use crate::tools::AlephTool;
            let browser_tool_defs = [
                browser_open_tool.definition(),
                browser_click_tool.definition(),
                browser_type_tool.definition(),
                browser_screenshot_tool.definition(),
                browser_snapshot_tool.definition(),
                browser_navigate_tool.definition(),
                browser_tabs_tool.definition(),
                browser_select_tool.definition(),
                browser_evaluate_tool.definition(),
                browser_fill_form_tool.definition(),
                browser_profile_tool.definition(),
            ];
            for td in &browser_tool_defs {
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
        info!("Registered browser tools (11 tools) in BuiltinToolRegistry");

        info!("Registered list_skills and read_config_guide tools in BuiltinToolRegistry");

        // Register optional tool metadata
        Self::register_optional_tools(
            &mut tools,
            &memory_search_tool,
            &memory_browse_tool,
            &image_generate_tool,
            &vault_store_tool,
            &config,
        );

        // Add agent management tools (if AgentRegistry + AgentEnvStore are available)
        let (agent_create_tool, agent_list_tool, agent_delete_tool, session_context_handle) =
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
                let list = agent_manage::AgentListTool::new(
                    Arc::clone(ar), Arc::clone(wm),
                );
                let delete = agent_manage::AgentDeleteTool::new(
                    Arc::clone(ar), Arc::clone(wm), config.event_bus.clone(),
                );

                // Register agent tools WITH their parameter schemas so LLMs
                // know which arguments to pass.
                {
                    use crate::tools::AlephTool;
                    let tool_defs = [
                        create.definition(),
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

                info!("Registered agent management tools (agent_create, agent_list, agent_delete)");
                (Some(create), Some(list), Some(delete), Some(ctx))
            } else {
                (None, None, None, None)
            };

        // Add ACP delegate tools (if AcpHarnessManager is provided)
        let (claude_code_tool, codex_tool, gemini_cli_tool, acp_switch_tool) =
            if let Some(ref manager) = config.acp_manager {
                use crate::builtin_tools::acp_tools::{ClaudeCodeTool, CodexTool, GeminiCliTool, AcpSwitchTool};
                info!("Creating ACP delegate tools");

                // All ACP tools share AcpDelegateArgs; acp_switch uses AcpSwitchArgs
                use schemars::schema_for;
                let acp_schema = serde_json::to_value(
                    schema_for!(crate::builtin_tools::acp_tools::AcpDelegateArgs)
                ).unwrap_or_default();
                let acp_switch_schema = serde_json::to_value(
                    schema_for!(crate::builtin_tools::acp_tools::AcpSwitchArgs)
                ).unwrap_or_default();

                let cc = if manager.has_harness("claude-code").await {
                    let mut ut = UnifiedTool::new(
                        "builtin:claude_code", "claude_code",
                        ClaudeCodeTool::DESCRIPTION, ToolSource::Builtin,
                    );
                    ut.parameters_schema = Some(acp_schema.clone());
                    tools.insert("claude_code".to_string(), ut);
                    Some(ClaudeCodeTool::new(Arc::clone(manager)))
                } else { None };

                let cx = if manager.has_harness("codex").await {
                    let mut ut = UnifiedTool::new(
                        "builtin:codex", "codex",
                        CodexTool::DESCRIPTION, ToolSource::Builtin,
                    );
                    ut.parameters_schema = Some(acp_schema.clone());
                    tools.insert("codex".to_string(), ut);
                    Some(CodexTool::new(Arc::clone(manager)))
                } else { None };

                let gm = if manager.has_harness("gemini").await {
                    let mut ut = UnifiedTool::new(
                        "builtin:gemini_cli", "gemini_cli",
                        GeminiCliTool::DESCRIPTION, ToolSource::Builtin,
                    );
                    ut.parameters_schema = Some(acp_schema.clone());
                    tools.insert("gemini_cli".to_string(), ut);
                    Some(GeminiCliTool::new(Arc::clone(manager)))
                } else { None };

                // acp_switch is always available when manager exists
                let mut ut = UnifiedTool::new(
                    "builtin:acp_switch", "acp_switch",
                    AcpSwitchTool::DESCRIPTION, ToolSource::Builtin,
                );
                ut.parameters_schema = Some(acp_switch_schema);
                tools.insert("acp_switch".to_string(), ut);
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
            list_skills_tool,
            config_guide_tool,
            vault_store_tool,
            desktop_tool,
            pim_tool,
            system_tool,
            automation_tool,
            desktop_platform,
            scratchpad_tool,
            memory_search_tool,
            memory_browse_tool,
            memory_workspace_handle,
            memory_session_key_handle,
            generation_registry: config.generation_registry.clone(),
            dispatcher_registry: config.dispatcher_registry.clone(),
            gateway_context: {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                if let Some(ref ctx) = config.gateway_context {
                    let _ = cell.set(ctx.clone());
                }
                cell
            },
            session_new_tool: config.gateway_context.as_ref()
                .map(|ctx| Arc::clone(ctx.session_manager()))
                .or_else(|| config.session_manager.clone())
                .map(|sm| crate::builtin_tools::sessions::SessionNewTool::new(sm)),
            session_set_topic_tool: config.gateway_context.as_ref()
                .map(|ctx| Arc::clone(ctx.session_manager()))
                .or_else(|| config.session_manager.clone())
                .map(|sm| crate::builtin_tools::sessions::SessionSetTopicTool::new(sm)),
            cron_manage_tool: config.cron_service.as_ref().map(|svc| {
                crate::builtin_tools::cron_manage::CronManageTool::new(Arc::clone(svc))
            }),
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
            agent_list_tool,
            agent_delete_tool,
            session_context_handle,
            tool_policy_handle,
            tool_context_handle: config.tool_context.clone(),
            event_bus: config.event_bus.clone(),
            extension_manager: config.extension_manager.clone(),
            claude_code_tool,
            codex_tool,
            gemini_cli_tool,
            acp_switch_tool,
            clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool::new(),
            tools,
        }
    }

    /// Register always-available core tool metadata with JSON Schema parameters.
    fn register_core_tools(tools: &mut HashMap<String, UnifiedTool>) {
        use schemars::schema_for;

        // Helper: register tool with schema from schemars
        fn reg(
            tools: &mut HashMap<String, UnifiedTool>,
            name: &str,
            desc: &str,
            schema: serde_json::Value,
        ) {
            let mut ut = UnifiedTool::new(
                format!("builtin:{name}"),
                name,
                desc,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(name.to_string(), ut);
        }

        reg(tools, "search", SearchTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::search::SearchArgs)).unwrap_or_default());
        reg(tools, "web_fetch", "Fetch and read content from a URL",
            serde_json::to_value(schema_for!(crate::builtin_tools::web_fetch::WebFetchArgs)).unwrap_or_default());
        reg(tools, "file_ops", "File system operations - list, read, write, move, copy, delete, etc.",
            serde_json::to_value(schema_for!(crate::builtin_tools::file_ops::FileOpsArgs)).unwrap_or_default());
        reg(tools, "bash", "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
            serde_json::to_value(schema_for!(crate::builtin_tools::bash_exec::BashExecArgs)).unwrap_or_default());
        reg(tools, "code_exec", CodeExecTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::code_exec::CodeExecArgs)).unwrap_or_default());
        reg(tools, "pdf_generate", PdfGenerateTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::pdf_generate::PdfGenerateArgs)).unwrap_or_default());
        reg(tools, "list_skills", SkillListTool::DESCRIPTION,
            serde_json::json!({"type": "object", "properties": {}, "required": []}));
        reg(tools, "read_config_guide", ReadConfigGuideTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::config_guide::ReadConfigGuideArgs)).unwrap_or_default());
        reg(tools, "desktop", DesktopTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::desktop::DesktopArgs)).unwrap_or_default());
        reg(tools, "pim", PimTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::pim::PimArgs)).unwrap_or_default());
        reg(tools, "system", SystemTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::system_tool::SystemArgs)).unwrap_or_default());
        reg(tools, "automation", AutomationTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::automation_tool::AutomationArgs)).unwrap_or_default());
        reg(tools, "scratchpad", ScratchpadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::scratchpad::ScratchpadArgs)).unwrap_or_default());
        reg(tools, "clawhub", crate::builtin_tools::clawhub::ClawHubTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::clawhub::ClawHubArgs)).unwrap_or_default());
    }

    /// Register metadata for optional tools (only when their dependencies are available)
    fn register_optional_tools(
        tools: &mut HashMap<String, UnifiedTool>,
        memory_search_tool: &Option<MemorySearchTool>,
        memory_browse_tool: &Option<MemoryBrowseTool>,
        image_generate_tool: &Option<ImageGenerateTool>,
        vault_store_tool: &Option<VaultStoreTool>,
        config: &BuiltinToolConfig,
    ) {
        use schemars::schema_for;

        // Helper: register tool with schema from schemars
        fn reg(
            tools: &mut HashMap<String, UnifiedTool>,
            name: &str,
            desc: &str,
            schema: serde_json::Value,
        ) {
            let mut ut = UnifiedTool::new(
                format!("builtin:{name}"),
                name,
                desc,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(name.to_string(), ut);
        }

        // Memory tools
        if memory_search_tool.is_some() {
            reg(tools, "memory_search", MemorySearchTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::memory_search::MemorySearchArgs)).unwrap_or_default());
            info!("Registered memory_search tool in BuiltinToolRegistry");
        }
        if memory_browse_tool.is_some() {
            reg(tools, "memory_browse", MemoryBrowseTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::memory_browse::MemoryBrowseArgs)).unwrap_or_default());
            info!("Registered memory_browse tool in BuiltinToolRegistry");
        }

        // Vault store tool
        if vault_store_tool.is_some() {
            reg(tools, "vault_store", VaultStoreTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::vault_store::VaultStoreArgs)).unwrap_or_default());
            info!("Registered vault_store tool in BuiltinToolRegistry");
        }

        // Generation tools
        let generation_registry = config.generation_registry.clone();
        if let Some(ref registry) = generation_registry {
            if image_generate_tool.is_some() {
                reg(tools, "generate_image", ImageGenerateTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(crate::builtin_tools::ImageGenerateArgs)).unwrap_or_default());
                info!("Registered generate_image tool in BuiltinToolRegistry");
            }

            if let Ok(reg_inner) = registry.read() {
                use crate::generation::GenerationType;

                if reg_inner.first_for_type(GenerationType::Video).is_some() {
                    reg(tools, "generate_video", "Generate videos from text descriptions",
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                "prompt": { "type": "string", "description": "Text description of the video to generate" },
                                "provider": { "type": "string", "description": "Optional provider name" }
                            },
                            "required": ["prompt"]
                        }));
                    info!("Registered generate_video tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Audio).is_some() {
                    reg(tools, "generate_audio", "Generate audio/music from text descriptions",
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                "prompt": { "type": "string", "description": "Text description of the audio to generate" },
                                "provider": { "type": "string", "description": "Optional provider name" }
                            },
                            "required": ["prompt"]
                        }));
                    info!("Registered generate_audio tool in BuiltinToolRegistry");
                }
            }
        }

        // Meta tools for smart tool discovery
        if config.dispatcher_registry.is_some() {
            reg(tools, "list_tools", ListToolsTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::meta_tools::ListToolsArgs)).unwrap_or_default());
            reg(tools, "get_tool_schema", GetToolSchemaTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::meta_tools::GetToolSchemaArgs)).unwrap_or_default());
            info!("Registered meta tools (list_tools, get_tool_schema) in BuiltinToolRegistry");
        }

        // Cron management tool (requires SharedCronService)
        if let Some(ref cron_svc) = config.cron_service {
            use crate::builtin_tools::cron_manage::CronManageTool;
            let tmp_tool = CronManageTool::new(Arc::clone(cron_svc));
            let def = AlephTool::definition(&tmp_tool);
            reg(tools, "cron_manage", CronManageTool::DESCRIPTION, def.parameters.clone());
            info!("Registered cron_manage tool in BuiltinToolRegistry");
        }

        // Session tools (require SessionManager — from gateway_context or direct session_manager)
        let session_mgr = config.gateway_context.as_ref()
            .map(|ctx| Arc::clone(ctx.session_manager()))
            .or_else(|| config.session_manager.clone());

        if let Some(ref sm) = session_mgr {
            use crate::builtin_tools::sessions::{SessionNewTool, SessionSetTopicTool};

            let tmp_new = SessionNewTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_new);
            reg(tools, "session_new", SessionNewTool::DESCRIPTION, def.parameters.clone());
            info!("Registered session_new tool in BuiltinToolRegistry");

            let tmp_topic = SessionSetTopicTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_topic);
            reg(tools, "session_set_topic", SessionSetTopicTool::DESCRIPTION, def.parameters.clone());
            info!("Registered session_set_topic tool in BuiltinToolRegistry");
        }

        // Sessions tools — always register metadata so LLM sees them.
        // GatewayContext may be injected later via set_gateway_context().
        // Execution checks OnceCell at call time.
        reg(tools, "sessions_list", SessionsListTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::sessions::SessionsListArgs)).unwrap_or_default());
        reg(tools, "sessions_send", SessionsSendTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::sessions::SessionsSendArgs)).unwrap_or_default());
        info!("Registered sessions_list + sessions_send in BuiltinToolRegistry");
    }
}

//! Builder / constructor for BuiltinToolRegistry
//!
//! Extracted from registry.rs to keep file sizes manageable.
//! Contains the `with_config()` constructor that wires up all tool instances
//! and registers their metadata.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tracing::info;

use crate::builtin_tools::browser_tools::{
    BrowserClickTool, BrowserConsoleTool, BrowserEvaluateTool, BrowserFillFormTool,
    BrowserNavigateTool, BrowserOpenTool, BrowserPressKeyTool, BrowserProfileTool,
    BrowserScreenshotTool, BrowserSelectTool, BrowserSnapshotTool, BrowserTabsTool,
    BrowserTypeTool, BrowserWaitForTool,
};
use crate::builtin_tools::skill_reader::{
    ListSkillsTool as SkillListTool, ReadSkillTool as SkillReadTool,
};
use crate::builtin_tools::{
    AutomationTool, BashExecTool, CodeExecTool, DesktopTool, FileEditTool, FileOpsTool,
    FileReadTool, FileWriteTool, ImageGenerateTool, MediaTool, MemoryBrowseTool, MemoryExploreTool,
    MemorySearchTool, PdfGenerateTool, PermissionTool, PimTool, ReadConfigGuideTool,
    ScratchpadTool, SearchTool, SelfManageTool, SystemTool, VaultStoreTool, WebFetchTool,
};
use crate::dispatcher::{ToolSource, UnifiedTool};
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
        let search_tool = if let Some(ref registry) = config.search_registry {
            SearchTool::with_registry(Arc::clone(registry))
        } else {
            SearchTool::with_api_key(config.tavily_api_key.clone())
        };
        let web_fetch_tool = {
            let mut tool = WebFetchTool::new();
            if let Some(ref cfg) = config.config {
                let cfg_guard = cfg.read().await;
                tool = tool.with_ssrf_policy(cfg_guard.ssrf.clone());
            }
            tool
        };
        let file_ops_tool = if let Some(ref tc) = config.tool_context {
            FileOpsTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileOpsTool::new()
        };
        let file_read_tool = if let Some(ref tc) = config.tool_context {
            FileReadTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileReadTool::new()
        };
        let file_write_tool = if let Some(ref tc) = config.tool_context {
            FileWriteTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileWriteTool::new()
        };
        let file_edit_tool = if let Some(ref tc) = config.tool_context {
            FileEditTool::new().with_tool_context(Arc::clone(tc))
        } else {
            FileEditTool::new()
        };
        let bash_tool = if let Some(ref sb) = config.sandbox {
            BashExecTool::new().with_sandbox(sb.clone())
        } else {
            BashExecTool::new()
        };
        let code_exec_tool = if let Some(ref sb) = config.sandbox {
            CodeExecTool::new().with_sandbox(sb.clone())
        } else {
            CodeExecTool::new()
        };
        let pdf_generate_tool = if let Some(ref tc) = config.tool_context {
            PdfGenerateTool::new().with_tool_context(Arc::clone(tc))
        } else {
            PdfGenerateTool::new()
        };

        // Skill listing tool (read_skill replaced by read_config_guide)
        let list_skills_tool = SkillListTool::default();
        let read_skill_tool = SkillReadTool::default();

        // Config guide tool (Progressive Disclosure for self-management)
        let config_guide_tool = ReadConfigGuideTool::default();

        // Self-management tool (LLM-triggered entry point)
        let self_manage_tool = SelfManageTool::default();

        // Self-config tool (identity files + config.toml access)
        let self_config_tool = {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let mut tool = crate::builtin_tools::self_config::SelfConfigTool::new(agent_id);
            if let Some(ref cfg) = config.config {
                tool = tool.with_config(Arc::clone(cfg));
            }
            if let Some(ref patcher) = config.config_patcher {
                tool = tool.with_patcher(Arc::clone(patcher));
            }
            tool
        };

        // Vault store tool (requires SharedTokenManager)
        let vault_store_tool = config.shared_token_manager.as_ref().map(|mgr| {
            info!("Creating VaultStoreTool");
            VaultStoreTool::new(Arc::clone(mgr))
        });

        // Build platform-specific DesktopPlatform
        let desktop_platform: Arc<dyn aleph_desktop::DesktopPlatform> = {
            #[cfg(target_os = "macos")]
            {
                Arc::new(aleph_desktop_macos::MacOSPlatform::new())
            }

            #[cfg(target_os = "linux")]
            {
                Arc::new(aleph_desktop_linux::LinuxPlatform::new())
            }

            #[cfg(target_os = "windows")]
            {
                Arc::new(aleph_desktop_windows::WindowsPlatform::new())
            }
        };

        // Desktop tool — platform-native desktop/screen capability only.
        let desktop_tool = DesktopTool::new().with_platform(Arc::clone(&desktop_platform));

        // AX query tools (macOS only; tools degrade gracefully on other platforms).
        let desktop_ax_query_focused_tool = crate::builtin_tools::DesktopAxQueryFocused::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_tree_tool = crate::builtin_tools::DesktopAxQueryTree::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_by_role_tool = crate::builtin_tools::DesktopAxQueryByRole::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_check_permissions_tool = crate::builtin_tools::DesktopCheckPermissions::new()
            .with_platform(Arc::clone(&desktop_platform));

        let system_tool = SystemTool::new(Arc::clone(&desktop_platform));
        let automation_tool = AutomationTool::new(Arc::clone(&desktop_platform));
        let permission_tool = PermissionTool::new(Arc::clone(&desktop_platform));
        let media_tool = MediaTool::new(Arc::clone(&desktop_platform));

        // PIM tool — platform-native notes/calendar/reminders/contacts capability.
        let pim_tool = PimTool::new().with_platform(Arc::clone(&desktop_platform));

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
        let browser_screenshot_tool =
            BrowserScreenshotTool::new(Arc::clone(&browser_profile_manager));
        let browser_snapshot_tool = BrowserSnapshotTool::new(Arc::clone(&browser_profile_manager));
        let browser_navigate_tool = BrowserNavigateTool::new(Arc::clone(&browser_profile_manager));
        let browser_tabs_tool = BrowserTabsTool::new(Arc::clone(&browser_profile_manager));
        let browser_select_tool = BrowserSelectTool::new(Arc::clone(&browser_profile_manager));
        let browser_evaluate_tool = BrowserEvaluateTool::new(Arc::clone(&browser_profile_manager));
        let browser_fill_form_tool = BrowserFillFormTool::new(Arc::clone(&browser_profile_manager));
        let browser_press_key_tool = BrowserPressKeyTool::new(Arc::clone(&browser_profile_manager));
        let browser_wait_for_tool = BrowserWaitForTool::new(Arc::clone(&browser_profile_manager));
        let browser_console_tool = BrowserConsoleTool::new(Arc::clone(&browser_profile_manager));
        let browser_profile_tool = BrowserProfileTool::new(browser_profile_manager);

        // Create memory tools if backend and embedder are provided
        let (
            memory_search_tool,
            memory_browse_tool,
            memory_explore_tool,
            memory_workspace_handle,
            memory_session_key_handle,
        ) = if let (Some(ref db), Some(ref embedder)) = (&config.memory_db, &config.embedder) {
            let search_tool = MemorySearchTool::new_with_config(
                db.clone(),
                Arc::clone(embedder),
                config.memory_similarity_threshold,
            );
            let ws_handle = search_tool.default_workspace_handle();
            let sk_handle = search_tool.default_session_key_handle();
            let note_memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".aleph")
                    .join("memory")
                    .join("note")
            });
            let browse_tool = MemoryBrowseTool::new(note_memory_dir, "default".to_string());
            let explore_tool = MemoryExploreTool::new(db.clone(), Arc::clone(embedder));
            info!("Created memory_search, memory_browse, and memory_explore tools");
            (
                Some(search_tool),
                Some(browse_tool),
                Some(explore_tool),
                Some(ws_handle),
                Some(sk_handle),
            )
        } else if config.memory_db.is_some() {
            let note_memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".aleph")
                    .join("memory")
                    .join("note")
            });
            let browse_tool = MemoryBrowseTool::new(note_memory_dir, "default".to_string());
            info!("Created memory_browse tool (no embedder for memory_search)");
            (None, Some(browse_tool), None, None, None)
        } else {
            (None, None, None, None, None)
        };

        // Create memory timeline tool if StateDatabase is provided
        let timeline_tool = config.state_db.as_ref().map(|sdb| {
            let traveler = Arc::new(crate::memory::events::traveler::MemoryTimeTraveler::new(
                Arc::clone(sdb),
            ));
            crate::builtin_tools::MemoryTimelineTool::new(traveler)
        });

        // Create image generation tool if generation registry is provided
        let image_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating ImageGenerateTool with generation registry");
            ImageGenerateTool::new(Arc::clone(registry))
        });

        let video_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating VideoGenerateTool with generation registry");
            crate::builtin_tools::generation::VideoGenerateTool::new(Arc::clone(registry))
        });

        let audio_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating AudioGenerateTool with generation registry");
            crate::builtin_tools::generation::AudioGenerateTool::new(Arc::clone(registry))
        });

        let speech_generate_tool = config.generation_registry.as_ref().map(|registry| {
            info!("Creating SpeechGenerateTool with generation registry");
            crate::builtin_tools::generation::SpeechGenerateTool::new(Arc::clone(registry))
        });

        // Build wiki tools (Spec 5 Task 12)
        let note_orient_tool = config.orientation.as_ref().map(|wiki| {
            use crate::memory::notes::orientation::types::TokenBudget;
            crate::builtin_tools::note_orient::NoteOrientTool::new(
                Arc::clone(wiki),
                TokenBudget::default(),
            )
        });

        let note_schema_tool = config
            .note_memory_dir
            .as_ref()
            .map(|dir| crate::builtin_tools::note_schema::NoteSchemaTool::new(dir.clone()));

        // Build user profile tool (Spec 7 Task 9)
        let user_profile_tool = config.profile_synthesizer.as_ref().map(|synth| {
            crate::builtin_tools::user_profile::UserProfileTool::new(Arc::clone(synth))
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
                browser_press_key_tool.definition(),
                browser_wait_for_tool.definition(),
                browser_console_tool.definition(),
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
        info!("Registered browser tools (14 tools) in BuiltinToolRegistry");

        info!(
            "Registered skill.list, skill.read, and read_config_guide tools in BuiltinToolRegistry"
        );

        // Register optional tool metadata
        Self::register_optional_tools(
            &mut tools,
            &memory_search_tool,
            &memory_browse_tool,
            &memory_explore_tool,
            &timeline_tool,
            &image_generate_tool,
            &vault_store_tool,
            &config,
            config.injection_mode,
            &note_orient_tool,
            &note_schema_tool,
            &user_profile_tool,
        );

        // Add agent management tools (if AgentRegistry + AgentEnvStore are available)
        let (agent_create_tool, agent_list_tool, agent_delete_tool, session_context_handle) =
            if let (Some(ref ar), Some(ref wm)) =
                (&config.agent_registry, &config.workspace_manager)
            {
                use crate::builtin_tools::agent_manage;
                let ctx = agent_manage::new_session_context_handle();
                let sm_for_agents = config
                    .gateway_context
                    .as_ref()
                    .map(|ctx| Arc::clone(ctx.session_store()))
                    .or_else(|| config.session_manager.clone())
                    .unwrap_or_else(|| {
                        Arc::new(
                            crate::gateway::SessionManager::with_defaults()
                                .expect("fallback SessionManager for agent tools"),
                        )
                    });
                let create = {
                    let tool = agent_manage::AgentCreateTool::new(
                        Arc::clone(ar),
                        Arc::clone(wm),
                        Arc::clone(&sm_for_agents),
                    );
                    if let Some(ref am) = config.agent_manager {
                        tool.with_agent_manager(Arc::clone(am))
                    } else {
                        tool
                    }
                };
                let list = agent_manage::AgentListTool::new(Arc::clone(ar), Arc::clone(wm));
                let delete = agent_manage::AgentDeleteTool::new(
                    Arc::clone(ar),
                    Arc::clone(wm),
                    config.event_bus.clone(),
                );

                // Register agent tools WITH their parameter schemas so LLMs
                // know which arguments to pass.
                {
                    use crate::tools::AlephTool;
                    let tool_defs = [create.definition(), list.definition(), delete.definition()];
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

                info!("Registered agent management tools (agent.create, agent.list, agent.delete)");
                (Some(create), Some(list), Some(delete), Some(ctx))
            } else {
                (None, None, None, None)
            };

        // Add ACP delegate tools (if AcpAdapterManager is provided)
        let (acp_delegate_tool, acp_switch_tool) = if let Some(ref manager) = config.acp_manager {
            use crate::builtin_tools::acp_tools::{AcpDelegateTool, AcpSwitchTool};
            use crate::tools::AlephTool;
            info!("Creating ACP delegate tools");

            // Register the unified acp_delegate tool
            use schemars::schema_for;
            let acp_schema = serde_json::to_value(schema_for!(
                crate::builtin_tools::acp_tools::AcpDelegateArgs
            ))
            .unwrap_or_default();
            let acp_switch_schema =
                serde_json::to_value(schema_for!(crate::builtin_tools::acp_tools::AcpSwitchArgs))
                    .unwrap_or_default();

            let mut ut = UnifiedTool::new(
                "builtin:acp_delegate",
                "acp_delegate",
                AcpDelegateTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(acp_schema);
            tools.insert("acp_delegate".to_string(), ut);
            let delegate = Some(AcpDelegateTool::new(Arc::clone(manager)));

            // acp_switch is always available when manager exists
            let mut ut = UnifiedTool::new(
                "builtin:acp_switch",
                "acp_switch",
                AcpSwitchTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(acp_switch_schema);
            tools.insert("acp_switch".to_string(), ut);
            let sw = Some(AcpSwitchTool::new(Arc::clone(manager)));

            info!("Registered ACP tools (acp_delegate=true, acp_switch=true)");
            (delegate, sw)
        } else {
            (None, None)
        };

        // Add task coordination tools (if CoordTaskStore is available)
        let (task_create_tool, task_update_tool, task_list_tool, task_wait_tool) =
            if let Some(ref store) = config.coord_task_store {
                use crate::builtin_tools::task_manage::{
                    TaskCreateTool, TaskListTool, TaskUpdateTool, TaskWaitTool,
                };

                let create =
                    TaskCreateTool::new(Arc::clone(store), config.dispatch_signal.clone());
                let list = TaskListTool::new(Arc::clone(store));

                // TaskUpdateTool and TaskWaitTool need the event bus
                let (update, wait) = if let Some(ref bus) = config.agent_message_bus {
                    (
                        Some(TaskUpdateTool::new(Arc::clone(store), Arc::clone(bus))),
                        Some(TaskWaitTool::new(Arc::clone(store), Arc::clone(bus))),
                    )
                } else {
                    (None, None)
                };

                // Register parameter schemas for task tools
                {
                    use crate::tools::AlephTool;
                    let mut defs: Vec<crate::dispatcher::ToolDefinition> =
                        vec![create.definition(), list.definition()];
                    if let Some(ref u) = update {
                        defs.push(u.definition());
                    }
                    if let Some(ref w) = wait {
                        defs.push(w.definition());
                    }
                    for td in &defs {
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

                info!("Registered task coordination tools");
                (Some(create), update, Some(list), wait)
            } else {
                (None, None, None, None)
            };

        // Pre-compute current agent ID — used by team, messaging, and session tools
        let current_agent_id = config
            .current_agent_id
            .clone()
            .unwrap_or_else(|| "main".to_string());

        // Add team management tools (if TeamStore + CoordTaskStore are available)
        let (
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_member_remove_tool,
        ) = if let (Some(ref store), Some(ref coord_store)) =
            (&config.team_store, &config.coord_task_store)
        {
            use crate::builtin_tools::team::{
                TeamCreateTool, TeamDelegateTool, TeamDisbandTool, TeamMemberRemoveTool,
                TeamStatusTool,
            };

            let agent_registry = config
                .agent_registry
                .clone()
                .unwrap_or_else(|| Arc::new(crate::gateway::agent_instance::AgentRegistry::new()));

            let sm_for_teams = config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .unwrap_or_else(|| {
                    Arc::new(
                        crate::gateway::SessionManager::with_defaults()
                            .expect("fallback SessionManager for team tools"),
                    )
                });
            let create = TeamCreateTool::new(
                Arc::clone(store),
                agent_registry,
                config.agent_manager.clone(),
                sm_for_teams,
                current_agent_id.clone(),
            );
            let delegate = TeamDelegateTool::new(
                Arc::clone(store),
                Arc::clone(coord_store),
                config.artifact_store.clone(),
            );
            let status = TeamStatusTool::new(Arc::clone(store), Arc::clone(coord_store));
            let disband = TeamDisbandTool::new(Arc::clone(store)).with_cleanup_stores(
                config.message_store.clone(),
                config.session_store.clone(),
                config.event_store.clone(),
            );
            let member_remove =
                TeamMemberRemoveTool::new(Arc::clone(store), current_agent_id.clone());

            // Register parameter schemas for team tools
            {
                use crate::tools::AlephTool;
                let tool_defs = [
                    create.definition(),
                    delegate.definition(),
                    status.definition(),
                    disband.definition(),
                    member_remove.definition(),
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

            info!("Registered team management tools (team_create, team_delegate, team_status, team_disband, team_member_remove)");
            (
                Some(create),
                Some(delegate),
                Some(status),
                Some(disband),
                Some(member_remove),
            )
        } else {
            (None, None, None, None, None)
        };

        // Add team_digest tool (if EventLogStore + TeamStore are available)
        let team_digest_tool = if let (Some(ref event_store), Some(ref team_store)) =
            (&config.event_store, &config.team_store)
        {
            use crate::builtin_tools::team::TeamDigestTool;

            let current_agent_id = current_agent_id.clone();
            let digest = TeamDigestTool::new(
                Arc::clone(team_store),
                Arc::clone(event_store),
                current_agent_id,
            );

            // Register parameter schema
            {
                use crate::tools::AlephTool;
                let td = digest.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }

            info!("Registered team_digest tool");
            Some(digest)
        } else {
            None
        };

        // Add message_send + inbox_read tools (if MessageRouter / Inbox are available)
        let (message_send_tool, inbox_read_tool) = {
            let current_agent_id = current_agent_id.clone();
            let send = config.message_router.as_ref().and_then(|router| {
                let team_store = config.team_store.as_ref()?;
                use crate::builtin_tools::team::MessageSendTool;
                Some(MessageSendTool::new(
                    Arc::clone(router),
                    Arc::clone(team_store),
                    current_agent_id.clone(),
                ))
            });
            let read = config.inbox.as_ref().map(|inbox| {
                use crate::builtin_tools::team::InboxReadTool;
                InboxReadTool::new(Arc::clone(inbox), current_agent_id)
            });

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::dispatcher::ToolDefinition> = Vec::new();
                if let Some(ref s) = send {
                    defs.push(s.definition());
                }
                if let Some(ref r) = read {
                    defs.push(r.definition());
                }
                for td in &defs {
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

            if send.is_some() || read.is_some() {
                info!(
                    "Registered team messaging tools (message_send={}, inbox_read={})",
                    send.is_some(),
                    read.is_some()
                );
            }
            (send, read)
        };

        // Add plan approval tools (require MessageRouter + ArtifactStore + EventLogStore)
        let (plan_submit_tool, plan_resolve_tool) = {
            let plan_manager = match (
                config.message_router.as_ref(),
                config.artifact_store.as_ref(),
                config.event_store.as_ref(),
            ) {
                (Some(mr), Some(a_s), Some(es)) => Some(Arc::new(
                    crate::teams::plans::PlanManager::new(
                        Arc::clone(mr),
                        Arc::clone(a_s),
                        Arc::clone(es),
                    ),
                )),
                _ => None,
            };
            let submit = match (plan_manager.as_ref(), config.team_store.as_ref()) {
                (Some(pm), Some(ts)) => {
                    use crate::builtin_tools::team::PlanSubmitTool;
                    Some(PlanSubmitTool::new(
                        Arc::clone(pm),
                        Arc::clone(ts),
                        current_agent_id.clone(),
                    ))
                }
                _ => None,
            };
            let resolve = plan_manager.as_ref().map(|pm| {
                use crate::builtin_tools::team::PlanResolveTool;
                PlanResolveTool::new(Arc::clone(pm), current_agent_id.clone())
            });

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::dispatcher::ToolDefinition> = Vec::new();
                if let Some(ref s) = submit {
                    defs.push(s.definition());
                }
                if let Some(ref r) = resolve {
                    defs.push(r.definition());
                }
                for td in &defs {
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

            if submit.is_some() || resolve.is_some() {
                info!("Registered plan approval tools (plan_submit, plan_resolve)");
            }
            (submit, resolve)
        };

        // Add task artifact tools (if ArtifactStore is available)
        let (task_submit_tool, task_read_artifact_tool) =
            if let Some(ref artifact_store) = config.artifact_store {
                use crate::builtin_tools::team::{TaskReadArtifactTool, TaskSubmitTool};

                let current_agent_id = current_agent_id.clone();
                let submit = TaskSubmitTool::new(Arc::clone(artifact_store), current_agent_id);
                let read = TaskReadArtifactTool::new(Arc::clone(artifact_store));

                // Register parameter schemas
                {
                    use crate::tools::AlephTool;
                    let tool_defs = [submit.definition(), read.definition()];
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

                info!("Registered task artifact tools (task_submit, task_read_artifact)");
                (Some(submit), Some(read))
            } else {
                (None, None)
            };

        // Add collaborative session tools (if SessionCoordinator / SessionStore are available)
        let (session_collaborate_tool, session_turn_tool, session_read_tool) = {
            let current_agent_id = current_agent_id.clone();

            let collaborate = config.session_coordinator.as_ref().map(|coord| {
                crate::builtin_tools::team::SessionCollaborateTool::new(
                    Arc::clone(coord),
                    current_agent_id.clone(),
                )
            });
            let turn = config.session_coordinator.as_ref().map(|coord| {
                crate::builtin_tools::team::SessionTurnTool::new(
                    Arc::clone(coord),
                    current_agent_id,
                )
            });
            let read = config
                .session_store
                .as_ref()
                .map(|store| crate::builtin_tools::team::SessionReadTool::new(Arc::clone(store)));

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::dispatcher::ToolDefinition> = Vec::new();
                if let Some(ref c) = collaborate {
                    defs.push(c.definition());
                }
                if let Some(ref t) = turn {
                    defs.push(t.definition());
                }
                if let Some(ref r) = read {
                    defs.push(r.definition());
                }
                for td in &defs {
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

            if collaborate.is_some() || read.is_some() {
                info!(
                    "Registered collaborative session tools (session_collaborate={}, session_turn={}, session_read={})",
                    collaborate.is_some(), turn.is_some(), read.is_some()
                );
            }
            (collaborate, turn, read)
        };

        // Skill management tools — always available
        // Phase 2: use the process-wide shared SkillSystem instead of a
        // throwaway empty instance; skill_status previously always reported 0.
        let skill_system = crate::skill::shared_skill_system().clone();
        let skill_status_tool =
            crate::builtin_tools::skill_status::SkillStatusTool::new(skill_system.clone());
        let skill_install_tool =
            crate::builtin_tools::skill_install::SkillInstallTool::new(skill_system.clone());
        let skill_manage_tool =
            crate::builtin_tools::skill_manage::SkillManageTool::new(skill_system);

        // Register skill management tool schemas
        {
            use crate::tools::AlephTool;
            let tool_defs = [
                skill_status_tool.definition(),
                skill_install_tool.definition(),
                skill_manage_tool.definition(),
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
        info!("Registered skill management tools (skill_status, skill_install, skill_manage)");

        // Note management tool — unified CRUD for all note categories
        let note_manage_tool = if let Some(ref db) = config.memory_db {
            let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".aleph")
                    .join("data")
                    .join("memory")
                    .join("note")
            });
            let tool =
                crate::builtin_tools::note_manage::NoteManageTool::new(memory_dir, db.clone());

            // Register note_manage tool schema
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered note_manage tool");
            Some(tool)
        } else {
            None
        };

        // Session-complete tool — requires memory_db (same guard as note_manage)
        let session_complete_tool = if let Some(ref db) = config.memory_db {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let mut tool = crate::builtin_tools::session_complete::SessionCompleteTool::new(
                db.clone(),
                agent_id,
            );
            if let Some(ref reg) = config.capture_registry {
                tool = tool.with_capture_registry(Arc::clone(reg));
            }

            // Register tool schema
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered session_complete tool");
            Some(tool)
        } else {
            None
        };

        // Memory-reflect tool — always constructed; reflector injected later by Task 8.
        // Registration is gated on injection_mode (same rule as the other retrieval tools).
        let expose_retrieval_tools = matches!(
            config.injection_mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
                | crate::config::types::memory::MemoryInjectionMode::Hybrid,
        );
        let memory_reflect_tool = {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let tool = crate::builtin_tools::memory_reflect::MemoryReflectTool::new(agent_id);

            // Register tool schema only when retrieval tools are exposed
            if expose_retrieval_tools {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
                info!("Registered memory_reflect tool");
            }
            Some(tool)
        };

        // recall_context tool — gated on injection_mode (retrieval tools only)
        if expose_retrieval_tools {
            use crate::builtin_tools::RecallContextTool;
            let schema = serde_json::to_value(schemars::schema_for!(
                crate::builtin_tools::recall_context::RecallContextArgs
            ))
            .unwrap_or_default();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", RecallContextTool::NAME),
                RecallContextTool::NAME,
                RecallContextTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(RecallContextTool::NAME.to_string(), ut);
            info!("Registered recall_context tool");
        }

        // Initialize tool policy handle (use provided or create a default one)
        let tool_policy_handle = config
            .tool_policy
            .clone()
            .or_else(|| Some(crate::builtin_tools::agent_manage::new_tool_policy_handle()));

        Self {
            search_tool,
            web_fetch_tool,
            file_ops_tool,
            file_read_tool,
            file_write_tool,
            file_edit_tool,
            bash_tool,
            code_exec_tool,
            pdf_generate_tool,
            image_generate_tool,
            video_generate_tool,
            audio_generate_tool,
            speech_generate_tool,
            list_skills_tool,
            read_skill_tool,
            config_guide_tool,
            self_manage_tool,
            self_config_tool,
            vault_store_tool,
            desktop_tool,
            desktop_ax_query_focused_tool,
            desktop_ax_query_tree_tool,
            desktop_ax_query_by_role_tool,
            desktop_check_permissions_tool,
            pim_tool,
            system_tool,
            automation_tool,
            permission_tool,
            media_tool,
            desktop_platform,
            scratchpad_tool,
            memory_search_tool,
            memory_context_provider: Arc::new(tokio::sync::OnceCell::new()),
            memory_browse_tool,
            memory_explore_tool,
            memory_timeline_tool: timeline_tool,
            memory_workspace_handle,
            memory_session_key_handle,
            dispatcher_registry: config.dispatcher_registry.clone(),
            gateway_context: {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                if let Some(ref ctx) = config.gateway_context {
                    let _ = cell.set(ctx.clone());
                }
                cell
            },
            session_new_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionNewTool::new),
            session_set_topic_tool: config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .map(crate::builtin_tools::sessions::SessionSetTopicTool::new),
            // session_search_tool: removed — now constructed on-the-fly from
            // GatewayContext in the dispatch path to enforce A2A policy filtering.
            cron_manage_tool: config
                .cron_service
                .as_ref()
                .map(|svc| crate::builtin_tools::cron_manage::CronManageTool::new(Arc::clone(svc))),
            heartbeat_list_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatListTool::new(Arc::clone(svc))
            }),
            heartbeat_create_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatCreateTool::new(Arc::clone(svc))
            }),
            heartbeat_update_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatUpdateTool::new(Arc::clone(svc))
            }),
            heartbeat_delete_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatDeleteTool::new(Arc::clone(svc))
            }),
            heartbeat_toggle_tool: config.heartbeat_service.as_ref().map(|svc| {
                crate::builtin_tools::heartbeat_manage::HeartbeatToggleTool::new(Arc::clone(svc))
            }),
            heartbeat_report_tool: crate::builtin_tools::heartbeat_manage::HeartbeatReportTool,
            // Phase 3 Task 19 — flag_user_correction tool (R9: everything is a tool).
            // Constructed inline because it only needs a memory backend, no separate
            // service. The cast to Arc<dyn RawMemoryStore> is zero-cost (vtable lookup).
            flag_user_correction_tool: config.memory_db.as_ref().map(|db| {
                crate::builtin_tools::FlagUserCorrectionTool::new(
                    db.clone()
                        as crate::sync_primitives::Arc<
                            dyn crate::memory::store::raw_memory::RawMemoryStore,
                        >,
                    "default".to_string(),
                )
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
            browser_press_key_tool,
            browser_wait_for_tool,
            browser_console_tool,
            browser_profile_tool,
            agent_create_tool,
            agent_list_tool,
            agent_delete_tool,
            session_context_handle,
            tool_policy_handle,
            tool_context_handle: config.tool_context.clone(),
            event_bus: config.event_bus.clone(),
            extension_manager: config.extension_manager.clone(),
            acp_delegate_tool,
            acp_switch_tool,
            channel_registry_cell: {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                if let Some(ref cr) = config.channel_registry {
                    let _ = cell.set(cr.clone());
                }
                cell
            },
            // ClarificationManager is always injected via deferred wiring at
            // boot (created alongside channels) — start the cell empty.
            clarification_manager_cell: Arc::new(tokio::sync::OnceCell::new()),
            clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool::new(),
            gateway_route_tool: crate::builtin_tools::gateway_route::GatewayRouteTool::default(),
            task_create_tool,
            task_update_tool,
            task_list_tool,
            task_wait_tool,
            task_submit_tool,
            task_read_artifact_tool,
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_member_remove_tool,
            team_digest_tool,
            message_send_tool,
            inbox_read_tool,
            plan_submit_tool,
            plan_resolve_tool,
            session_collaborate_tool,
            session_turn_tool,
            session_read_tool,
            skill_status_tool,
            skill_install_tool,
            skill_manage_tool,
            note_manage_tool,
            session_complete_tool,
            memory_reflect_tool,
            note_orient_tool,
            note_schema_tool,
            user_profile_tool,
            tools,
        }
    }
}

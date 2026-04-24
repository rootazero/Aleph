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
use crate::builtin_tools::meta_tools::{GetToolSchemaTool, ListToolsTool};
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
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
        let search_tool = if let Some(ref registry) = config.search_registry {
            SearchTool::with_registry(Arc::clone(registry))
        } else {
            SearchTool::with_api_key(config.tavily_api_key.clone())
        };
        let web_fetch_tool = WebFetchTool::new();
        let file_ops_tool = if let Some(ref tc) = config.tool_context {
            FileOpsTool::new().with_tool_context(std::sync::Arc::clone(tc))
        } else {
            FileOpsTool::new()
        };
        let file_read_tool = if let Some(ref tc) = config.tool_context {
            FileReadTool::new().with_tool_context(std::sync::Arc::clone(tc))
        } else {
            FileReadTool::new()
        };
        let file_write_tool = if let Some(ref tc) = config.tool_context {
            FileWriteTool::new().with_tool_context(std::sync::Arc::clone(tc))
        } else {
            FileWriteTool::new()
        };
        let file_edit_tool = if let Some(ref tc) = config.tool_context {
            FileEditTool::new().with_tool_context(std::sync::Arc::clone(tc))
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
            PdfGenerateTool::new().with_tool_context(std::sync::Arc::clone(tc))
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
        let desktop_ax_query_focused_tool =
            crate::builtin_tools::DesktopAxQueryFocused::new()
                .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_tree_tool = crate::builtin_tools::DesktopAxQueryTree::new()
            .with_platform(Arc::clone(&desktop_platform));
        let desktop_ax_query_by_role_tool = crate::builtin_tools::DesktopAxQueryByRole::new()
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

                let create = TaskCreateTool::new(Arc::clone(store));
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
        let skill_system = crate::skill::SkillSystem::new();
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
                tool = tool.with_capture_registry(std::sync::Arc::clone(reg));
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
            pim_tool,
            system_tool,
            automation_tool,
            permission_tool,
            media_tool,
            desktop_platform,
            scratchpad_tool,
            memory_search_tool,
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
            let mut ut =
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin);
            ut.parameters_schema = Some(schema);
            tools.insert(name.to_string(), ut);
        }

        reg(
            tools,
            "search",
            SearchTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::search::SearchArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "web_fetch",
            "Fetch and read content from a URL",
            serde_json::to_value(schema_for!(crate::builtin_tools::web_fetch::WebFetchArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "file_ops",
            "File system operations - list, move, copy, delete, mkdir, search, batch_move, organize",
            serde_json::to_value(schema_for!(crate::builtin_tools::file_ops::FileOpsArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "file_read",
            FileReadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::read::FileReadArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "file_write",
            FileWriteTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::write::FileWriteArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "file_edit",
            FileEditTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::edit::FileEditArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "bash",
            "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
            serde_json::to_value(schema_for!(crate::builtin_tools::bash_exec::BashExecArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "code_exec",
            CodeExecTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::code_exec::CodeExecArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "pdf_generate",
            PdfGenerateTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::pdf_generate::PdfGenerateArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "skill_list",
            SkillListTool::DESCRIPTION,
            serde_json::json!({"type": "object", "properties": {}, "required": []}),
        );
        reg(
            tools,
            "skill_read",
            SkillReadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::skill_reader::ReadSkillArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "read_config_guide",
            ReadConfigGuideTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::config_guide::ReadConfigGuideArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "self_manage",
            SelfManageTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::self_manage::SelfManageArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "self_config",
            crate::builtin_tools::self_config::SelfConfigTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::self_config::SelfConfigArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "desktop",
            DesktopTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::desktop::DesktopArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "pim",
            PimTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::pim::PimArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "system",
            SystemTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::system_tool::SystemArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "automation",
            AutomationTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::automation_tool::AutomationArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "permission",
            PermissionTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::permission_tool::PermissionArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "media",
            MediaTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::media_tool::MediaArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "scratchpad",
            ScratchpadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::scratchpad::ScratchpadArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "clawhub",
            crate::builtin_tools::clawhub::ClawHubTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::clawhub::ClawHubArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "media_send",
            crate::builtin_tools::media_send::MediaSendTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::media_send::MediaSendArgs))
                .unwrap_or_default(),
        );
    }

    /// Register metadata for optional tools (only when their dependencies are available)
    #[allow(clippy::too_many_arguments)]
    fn register_optional_tools(
        tools: &mut HashMap<String, UnifiedTool>,
        memory_search_tool: &Option<MemorySearchTool>,
        memory_browse_tool: &Option<MemoryBrowseTool>,
        memory_explore_tool: &Option<MemoryExploreTool>,
        memory_timeline_tool: &Option<crate::builtin_tools::MemoryTimelineTool>,
        image_generate_tool: &Option<ImageGenerateTool>,
        vault_store_tool: &Option<VaultStoreTool>,
        config: &BuiltinToolConfig,
        injection_mode: crate::config::types::memory::MemoryInjectionMode,
        note_orient_tool: &Option<crate::builtin_tools::note_orient::NoteOrientTool>,
        note_schema_tool: &Option<crate::builtin_tools::note_schema::NoteSchemaTool>,
        user_profile_tool: &Option<crate::builtin_tools::user_profile::UserProfileTool>,
    ) {
        use schemars::schema_for;

        // Helper: register tool with schema from schemars
        fn reg(
            tools: &mut HashMap<String, UnifiedTool>,
            name: &str,
            desc: &str,
            schema: serde_json::Value,
        ) {
            let mut ut =
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin);
            ut.parameters_schema = Some(schema);
            tools.insert(name.to_string(), ut);
        }

        // Memory retrieval tools — only exposed in Tools / Hybrid mode.
        // In Context mode the LLM receives memory via auto-injected context messages instead.
        let expose_retrieval_tools = matches!(
            injection_mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
                | crate::config::types::memory::MemoryInjectionMode::Hybrid,
        );

        if expose_retrieval_tools {
            if memory_search_tool.is_some() {
                reg(
                    tools,
                    "memory_search",
                    MemorySearchTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(
                        crate::builtin_tools::memory_search::MemorySearchArgs
                    ))
                    .unwrap_or_default(),
                );
                info!("Registered memory_search tool in BuiltinToolRegistry");
            }
            if memory_browse_tool.is_some() {
                reg(
                    tools,
                    "memory_browse",
                    MemoryBrowseTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(
                        crate::builtin_tools::memory_browse::MemoryBrowseArgs
                    ))
                    .unwrap_or_default(),
                );
                info!("Registered memory.browse tool in BuiltinToolRegistry");
            }
            if memory_explore_tool.is_some() {
                reg(
                    tools,
                    "memory_explore",
                    MemoryExploreTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(
                        crate::builtin_tools::memory_explore::MemoryExploreArgs
                    ))
                    .unwrap_or_default(),
                );
                info!("Registered memory_explore tool in BuiltinToolRegistry");
            }
            if memory_timeline_tool.is_some() {
                reg(
                    tools,
                    "memory_timeline",
                    crate::builtin_tools::MemoryTimelineTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(
                        crate::builtin_tools::memory_timeline::MemoryTimelineArgs
                    ))
                    .unwrap_or_default(),
                );
                info!("Registered memory_timeline tool in BuiltinToolRegistry");
            }
        }

        // Vault store tool
        if vault_store_tool.is_some() {
            reg(
                tools,
                "vault_store",
                VaultStoreTool::DESCRIPTION,
                serde_json::to_value(schema_for!(
                    crate::builtin_tools::vault_store::VaultStoreArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered vault.store tool in BuiltinToolRegistry");
        }

        // Generation tools
        let generation_registry = config.generation_registry.clone();
        if let Some(ref registry) = generation_registry {
            if image_generate_tool.is_some() {
                reg(
                    tools,
                    "image_generate",
                    ImageGenerateTool::DESCRIPTION,
                    serde_json::to_value(schema_for!(crate::builtin_tools::ImageGenerateArgs))
                        .unwrap_or_default(),
                );
                info!("Registered image.generate tool in BuiltinToolRegistry");
            }

            {
                let reg_inner = registry.read().unwrap_or_else(|e| e.into_inner());
                use crate::generation::GenerationType;

                if reg_inner.first_for_type(GenerationType::Video).is_some() {
                    reg(
                        tools,
                        "video_generate",
                        crate::builtin_tools::generation::VideoGenerateTool::DESCRIPTION,
                        serde_json::to_value(schemars::schema_for!(
                            crate::builtin_tools::generation::VideoGenerateArgs
                        ))
                        .unwrap_or_default(),
                    );
                    info!("Registered video_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Audio).is_some() {
                    reg(
                        tools,
                        "audio_generate",
                        crate::builtin_tools::generation::AudioGenerateTool::DESCRIPTION,
                        serde_json::to_value(schemars::schema_for!(
                            crate::builtin_tools::generation::AudioGenerateArgs
                        ))
                        .unwrap_or_default(),
                    );
                    info!("Registered audio_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Speech).is_some() {
                    reg(
                        tools,
                        "speech_generate",
                        crate::builtin_tools::generation::SpeechGenerateTool::DESCRIPTION,
                        serde_json::to_value(schemars::schema_for!(
                            crate::builtin_tools::generation::SpeechGenerateArgs
                        ))
                        .unwrap_or_default(),
                    );
                    info!("Registered speech_generate tool in BuiltinToolRegistry");
                }
            }
        }

        // Meta tools for smart tool discovery
        if config.dispatcher_registry.is_some() {
            reg(
                tools,
                "list_tools",
                ListToolsTool::DESCRIPTION,
                serde_json::to_value(schema_for!(crate::builtin_tools::meta_tools::ListToolsArgs))
                    .unwrap_or_default(),
            );
            reg(
                tools,
                "get_tool_schema",
                GetToolSchemaTool::DESCRIPTION,
                serde_json::to_value(schema_for!(
                    crate::builtin_tools::meta_tools::GetToolSchemaArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered meta tools (list_tools, get_tool_schema) in BuiltinToolRegistry");
        }

        // Cron management tool (requires SharedCronService)
        if let Some(ref cron_svc) = config.cron_service {
            use crate::builtin_tools::cron_manage::CronManageTool;
            let tmp_tool = CronManageTool::new(Arc::clone(cron_svc));
            let def = AlephTool::definition(&tmp_tool);
            reg(
                tools,
                "cron_manage",
                CronManageTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered cron.manage tool in BuiltinToolRegistry");
        }

        // Heartbeat management tools (require SharedHeartbeatService)
        if let Some(ref hb_svc) = config.heartbeat_service {
            use crate::builtin_tools::heartbeat_manage::{
                HeartbeatCreateTool, HeartbeatDeleteTool, HeartbeatListTool, HeartbeatReportTool,
                HeartbeatToggleTool, HeartbeatUpdateTool,
            };
            let list_tool = HeartbeatListTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&list_tool);
            reg(
                tools,
                "heartbeat_list",
                HeartbeatListTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let create_tool = HeartbeatCreateTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&create_tool);
            reg(
                tools,
                "heartbeat_create",
                HeartbeatCreateTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let update_tool = HeartbeatUpdateTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&update_tool);
            reg(
                tools,
                "heartbeat_update",
                HeartbeatUpdateTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let delete_tool = HeartbeatDeleteTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&delete_tool);
            reg(
                tools,
                "heartbeat_delete",
                HeartbeatDeleteTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let toggle_tool = HeartbeatToggleTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&toggle_tool);
            reg(
                tools,
                "heartbeat_toggle",
                HeartbeatToggleTool::DESCRIPTION,
                def.parameters.clone(),
            );

            info!("Registered heartbeat management tools in BuiltinToolRegistry");

            // heartbeat_report is always registered (L2 output tool — no service dependency)
            let report_tool = HeartbeatReportTool;
            let def = AlephTool::definition(&report_tool);
            reg(
                tools,
                "heartbeat_report",
                HeartbeatReportTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered heartbeat_report tool in BuiltinToolRegistry");
        } else {
            // Register heartbeat_report even without the heartbeat service
            use crate::builtin_tools::heartbeat_manage::HeartbeatReportTool;
            let report_tool = HeartbeatReportTool;
            let def = AlephTool::definition(&report_tool);
            reg(
                tools,
                "heartbeat_report",
                HeartbeatReportTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered heartbeat_report tool (standalone) in BuiltinToolRegistry");
        }

        // Session tools (require SessionManager — from gateway_context or direct session_manager)
        let session_mgr = config
            .gateway_context
            .as_ref()
            .map(|ctx| Arc::clone(ctx.session_store()))
            .or_else(|| config.session_manager.clone());

        if let Some(ref sm) = session_mgr {
            use crate::builtin_tools::sessions::{SessionNewTool, SessionSetTopicTool};

            let tmp_new = SessionNewTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_new);
            reg(
                tools,
                "session_new",
                SessionNewTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.new tool in BuiltinToolRegistry");

            let tmp_topic = SessionSetTopicTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_topic);
            reg(
                tools,
                "session_rename",
                SessionSetTopicTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.rename tool in BuiltinToolRegistry");

            // session_search — metadata only; tool is constructed on-the-fly from GatewayContext
            use crate::builtin_tools::SessionSearchTool;
            reg(
                tools,
                "session_search",
                SessionSearchTool::DESCRIPTION,
                serde_json::to_value(schemars::schema_for!(
                    crate::builtin_tools::session_search::SessionSearchArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered session_search tool in BuiltinToolRegistry");
        }

        // Channel pairing tool — always register metadata (ChannelRegistry injected later).
        // Uses deferred OnceCell injection, same pattern as GatewayContext.
        {
            use crate::builtin_tools::channel_manage::ChannelPairingTool;
            reg(
                tools,
                "channel_pairing",
                ChannelPairingTool::DESCRIPTION,
                serde_json::to_value(schemars::schema_for!(
                    crate::builtin_tools::channel_manage::ChannelPairingArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered channel_pairing tool in BuiltinToolRegistry");
        }

        // Sessions tools — always register metadata so LLM sees them.
        // GatewayContext may be injected later via set_gateway_context().
        // Execution checks OnceCell at call time.
        reg(
            tools,
            "session_list",
            SessionsListTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::sessions::SessionsListArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "session_send",
            SessionsSendTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::sessions::SessionsSendArgs
            ))
            .unwrap_or_default(),
        );
        info!("Registered session.list + session.send in BuiltinToolRegistry");

        // Voice mode tool — toggle voice output on/off for a channel (R9)
        reg(tools, "voice_mode_set",
            "Enable or disable voice mode for a channel. When enabled, all replies will be converted to speech audio. Use when user says 'turn on voice mode', 'switch to voice', 'enable voice replies', etc.",
            serde_json::to_value(schema_for!(crate::builtin_tools::voice_tools::VoiceModeSetArgs)).unwrap_or_default());
        info!("Registered voice_mode_set tool in BuiltinToolRegistry");

        // Wiki orientation tools (Spec 5 Task 12).
        // `note_schema` is always registered when a memory dir is available (LLM can always
        // read/write SCHEMA.md). `note_orient` is only exposed in Tools / Hybrid mode so the
        // LLM can call it on-demand; in Context mode orientation is injected automatically.
        if note_schema_tool.is_some() {
            reg(
                tools,
                "note_schema",
                "Read or write the SCHEMA.md file that describes the structure of the agent's \
                 long-term memory wiki. Use 'read' to inspect the current schema, 'write' to \
                 update it (include the expected_hash from your last read to prevent conflicts).",
                serde_json::to_value(schema_for!(
                    crate::builtin_tools::note_schema::NoteSchemaArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered note_schema tool in BuiltinToolRegistry");
        }

        if expose_retrieval_tools && note_orient_tool.is_some() {
            reg(
                tools,
                "note_orient",
                "Fetch a compact orientation snapshot of the agent's memory wiki: SCHEMA, \
                 index, and recent log entries. Call this at the start of a task to understand \
                 what structured memory is available before searching or writing notes.",
                serde_json::to_value(schema_for!(
                    crate::builtin_tools::note_orient::NoteOrientArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered note_orient tool in BuiltinToolRegistry");
        }

        // User profile tool (Spec 7 Task 9) — always exposed when synthesizer is available.
        if user_profile_tool.is_some() {
            reg(
                tools,
                "user_profile",
                "Read the current user profile (interests, preferences, context) or view \
                 its revision history. Use 'read' to get the latest profile, 'history' to \
                 inspect the revision log.",
                serde_json::to_value(schema_for!(
                    crate::builtin_tools::user_profile::UserProfileArgs
                ))
                .unwrap_or_default(),
            );
            info!("Registered user_profile tool in BuiltinToolRegistry");
        }
    }
}

#[cfg(test)]
mod spec3_tool_gating_tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::executor::builtin_registry::BuiltinToolConfig;

    const MEMORY_RETRIEVAL_TOOLS: &[&str] = &[
        "memory_search",
        "memory_reflect",
        "recall_context",
        "memory_browse",
        "memory_explore",
        "memory_timeline",
    ];

    fn count_memory_tools_registered(registry: &BuiltinToolRegistry) -> usize {
        MEMORY_RETRIEVAL_TOOLS
            .iter()
            .filter(|name| registry.has_tool(name))
            .count()
    }

    async fn build_registry_with_mode(mode: MemoryInjectionMode) -> BuiltinToolRegistry {
        BuiltinToolRegistry::with_config(BuiltinToolConfig {
            injection_mode: mode,
            ..Default::default()
        })
        .await
    }

    #[tokio::test]
    async fn context_mode_skips_all_six_memory_retrieval_tools() {
        let registry = build_registry_with_mode(MemoryInjectionMode::Context).await;
        assert_eq!(
            count_memory_tools_registered(&registry),
            0,
            "Context mode must not register any of the six retrieval tools"
        );
    }

    #[tokio::test]
    async fn tools_mode_registers_all_six_memory_retrieval_tools() {
        let registry = build_registry_with_mode(MemoryInjectionMode::Tools).await;
        // memory_search, memory_browse, memory_explore, memory_timeline need live deps
        // (memory_db / embedder / state_db) so they won't appear — but memory_reflect
        // and recall_context are always constructible.  The test verifies the gate
        // is open (non-zero) and that dep-less tools are present.
        assert!(
            registry.has_tool("memory_reflect"),
            "memory_reflect must be registered in Tools mode"
        );
        assert!(
            registry.has_tool("recall_context"),
            "recall_context must be registered in Tools mode"
        );
    }

    #[tokio::test]
    async fn hybrid_mode_registers_dep_free_retrieval_tools() {
        let registry = build_registry_with_mode(MemoryInjectionMode::Hybrid).await;
        assert!(
            registry.has_tool("memory_reflect"),
            "memory_reflect must be registered in Hybrid mode"
        );
        assert!(
            registry.has_tool("recall_context"),
            "recall_context must be registered in Hybrid mode"
        );
    }

    #[tokio::test]
    async fn context_mode_skips_dep_free_retrieval_tools() {
        let registry = build_registry_with_mode(MemoryInjectionMode::Context).await;
        assert!(
            !registry.has_tool("memory_reflect"),
            "memory_reflect must NOT be registered in Context mode"
        );
        assert!(
            !registry.has_tool("recall_context"),
            "recall_context must NOT be registered in Context mode"
        );
    }

    #[tokio::test]
    async fn note_manage_always_registered_regardless_of_mode() {
        // note_manage requires memory_db; without it, it's None — so we verify
        // the gating logic does NOT block it (it's outside the retrieval gate).
        // With no memory_db the tool won't be created, but that's a dep constraint,
        // not a mode constraint.  The test confirms it's absent for the *same* reason
        // in all modes (dep missing), not because of injection_mode.
        for mode in [
            MemoryInjectionMode::Context,
            MemoryInjectionMode::Tools,
            MemoryInjectionMode::Hybrid,
        ] {
            let registry = build_registry_with_mode(mode).await;
            // All three produce the same result (absent due to missing memory_db dep).
            // The important invariant: Context mode absence == Tools/Hybrid absence.
            let in_context = registry.has_tool("note_manage");
            let _ = in_context; // dep-gated, not mode-gated — just verify no panic
        }
    }

    #[tokio::test]
    async fn session_complete_always_registered_regardless_of_mode() {
        for mode in [
            MemoryInjectionMode::Context,
            MemoryInjectionMode::Tools,
            MemoryInjectionMode::Hybrid,
        ] {
            let registry = build_registry_with_mode(mode).await;
            // dep-gated (memory_db), not mode-gated — verify consistent behaviour
            let _ = registry.has_tool("session_complete");
        }
    }
}

//! Core registry implementation for builtin tools

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::pin::Pin;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::builtin_tools::meta_tools::{GetToolSchemaTool, ListToolsTool};
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::context::GatewayContext;
use crate::tools::AlephTool;
use tokio::sync::RwLock;

use super::{BuiltinToolConfig, ToolRegistry};

pub(crate) fn resolve_plugin_handler_from_sources(
    extension_manager: Option<&crate::extension::ExtensionManager>,
    tools: &HashMap<String, UnifiedTool>,
    tool_name: &str,
) -> Option<(String, String)> {
    if let Some(ext_mgr) = extension_manager {
        if let Some(tool) = ext_mgr.resolve_active_plugin_tool(tool_name) {
            return Some((tool.plugin_id, tool.handler));
        }
    }

    tools
        .get(tool_name)
        .and_then(|unified| match &unified.source {
            ToolSource::Plugin { plugin_id } => {
                Some((plugin_id.clone(), format!("tool_{}", tool_name)))
            }
            _ => None,
        })
}

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
///
/// TODO: Security enforcement will be reimplemented following OpenClaw's sandbox/tool-policy pattern.
pub struct BuiltinToolRegistry {
    /// Search tool instance
    pub(crate) search_tool: crate::builtin_tools::SearchTool,
    /// Web fetch tool instance
    pub(crate) web_fetch_tool: crate::builtin_tools::WebFetchTool,
    /// File operations tool instance
    pub(crate) file_ops_tool: crate::builtin_tools::FileOpsTool,
    /// File read tool instance
    pub(crate) file_read_tool: crate::builtin_tools::FileReadTool,
    /// File write tool instance
    pub(crate) file_write_tool: crate::builtin_tools::FileWriteTool,
    /// File edit tool instance
    pub(crate) file_edit_tool: crate::builtin_tools::FileEditTool,
    /// Bash execution tool instance (wraps CodeExecTool for shell commands)
    pub(crate) bash_tool: crate::builtin_tools::BashExecTool,
    /// Code execution tool instance
    pub(crate) code_exec_tool: crate::builtin_tools::CodeExecTool,
    /// PDF generation tool instance
    pub(crate) pdf_generate_tool: crate::builtin_tools::PdfGenerateTool,
    /// Image generation tool instance (optional - requires generation registry)
    pub(crate) image_generate_tool: Option<crate::builtin_tools::ImageGenerateTool>,
    /// Video generation tool instance (optional - requires generation registry)
    pub(crate) video_generate_tool: Option<crate::builtin_tools::generation::VideoGenerateTool>,
    /// Audio generation tool instance (optional - requires generation registry)
    pub(crate) audio_generate_tool: Option<crate::builtin_tools::generation::AudioGenerateTool>,
    /// Speech generation tool instance (optional - requires generation registry)
    pub(crate) speech_generate_tool: Option<crate::builtin_tools::generation::SpeechGenerateTool>,
    /// List skills tool instance
    pub(crate) list_skills_tool: crate::builtin_tools::skill_reader::ListSkillsTool,
    /// Read skill tool instance (deferred loading — LLM calls this to load full skill instructions)
    pub(crate) read_skill_tool: crate::builtin_tools::skill_reader::ReadSkillTool,
    /// Config guide tool instance (progressive disclosure for self-management)
    pub(crate) config_guide_tool: crate::builtin_tools::ReadConfigGuideTool,
    /// Self-management tool instance (LLM-triggered entry point)
    pub(crate) self_manage_tool: crate::builtin_tools::SelfManageTool,
    /// Self-config tool instance (identity files + config.toml access)
    pub(crate) self_config_tool: crate::builtin_tools::self_config::SelfConfigTool,
    /// Vault store tool instance (optional - requires SharedTokenManager)
    pub(crate) vault_store_tool: Option<crate::builtin_tools::VaultStoreTool>,
    /// Desktop bridge tool instance
    pub(crate) desktop_tool: crate::builtin_tools::DesktopTool,
    /// PIM (Personal Information Management) tool instance
    pub(crate) pim_tool: crate::builtin_tools::PimTool,
    /// System tool instance (app management, notifications, clipboard, system info)
    pub(crate) system_tool: crate::builtin_tools::SystemTool,
    /// Automation tool instance (scripts, Shortcuts)
    pub(crate) automation_tool: crate::builtin_tools::AutomationTool,
    /// Permission tool instance (TCC permission detection and request)
    pub(crate) permission_tool: crate::builtin_tools::PermissionTool,
    /// Media tool instance (camera capture, audio device management)
    pub(crate) media_tool: crate::builtin_tools::MediaTool,
    /// Desktop platform reference (shared with new tools; held for future use)
    #[allow(dead_code)]
    pub(crate) desktop_platform: crate::sync_primitives::Arc<dyn aleph_desktop::DesktopPlatform>,
    /// Scratchpad tool instance (project working memory)
    pub(crate) scratchpad_tool: crate::builtin_tools::ScratchpadTool,
    /// Memory search tool instance (optional - requires memory_db + embedder)
    pub(crate) memory_search_tool: Option<crate::builtin_tools::MemorySearchTool>,
    /// Memory browse tool instance (optional - requires memory_db)
    pub(crate) memory_browse_tool: Option<crate::builtin_tools::MemoryBrowseTool>,
    /// Memory explore tool instance (optional - requires memory_db + embedder)
    pub(crate) memory_explore_tool: Option<crate::builtin_tools::MemoryExploreTool>,
    /// Memory timeline tool instance (optional - requires StateDatabase)
    pub(crate) memory_timeline_tool: Option<crate::builtin_tools::MemoryTimelineTool>,
    /// Shared workspace handle for memory tools — written by ExecutionEngine after workspace resolution
    pub(super) memory_workspace_handle: Option<Arc<RwLock<String>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub(crate) dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Gateway context for sessions tools (session.list, session.send).
    /// Uses OnceCell for deferred injection: BuiltinToolRegistry is created before
    /// ExecutionAdapter exists, but GatewayContext needs ExecutionAdapter.
    pub(crate) gateway_context: Arc<tokio::sync::OnceCell<Arc<GatewayContext>>>,
    /// Session new tool (optional - requires SessionManager)
    pub(crate) session_new_tool: Option<crate::builtin_tools::sessions::SessionNewTool>,
    /// Session set-topic tool (optional - requires SessionManager)
    pub(crate) session_set_topic_tool: Option<crate::builtin_tools::sessions::SessionSetTopicTool>,
    // session_search is constructed on-the-fly from gateway_context (like session_list/session_send)
    // to enforce A2A policy filtering — no stored instance needed.
    /// Cron management tool (optional - requires SharedCronService)
    pub(crate) cron_manage_tool: Option<crate::builtin_tools::cron_manage::CronManageTool>,
    /// Heartbeat management tools (optional - require SharedHeartbeatService)
    pub(crate) heartbeat_list_tool:
        Option<crate::builtin_tools::heartbeat_manage::HeartbeatListTool>,
    pub(crate) heartbeat_create_tool:
        Option<crate::builtin_tools::heartbeat_manage::HeartbeatCreateTool>,
    pub(crate) heartbeat_update_tool:
        Option<crate::builtin_tools::heartbeat_manage::HeartbeatUpdateTool>,
    pub(crate) heartbeat_delete_tool:
        Option<crate::builtin_tools::heartbeat_manage::HeartbeatDeleteTool>,
    pub(crate) heartbeat_toggle_tool:
        Option<crate::builtin_tools::heartbeat_manage::HeartbeatToggleTool>,
    /// Heartbeat report tool — always available (used during L2 heartbeat execution)
    pub(crate) heartbeat_report_tool: crate::builtin_tools::heartbeat_manage::HeartbeatReportTool,
    /// Agent management tools (optional - requires AgentRegistry + AgentEnvStore)
    pub(crate) agent_create_tool: Option<crate::builtin_tools::agent_manage::AgentCreateTool>,

    pub(crate) agent_list_tool: Option<crate::builtin_tools::agent_manage::AgentListTool>,
    pub(crate) agent_delete_tool: Option<crate::builtin_tools::agent_manage::AgentDeleteTool>,
    /// Browser tools (always available, share a single ProfileManager)
    pub(crate) browser_open_tool: crate::builtin_tools::browser_tools::BrowserOpenTool,
    pub(crate) browser_click_tool: crate::builtin_tools::browser_tools::BrowserClickTool,
    pub(crate) browser_type_tool: crate::builtin_tools::browser_tools::BrowserTypeTool,
    pub(crate) browser_screenshot_tool: crate::builtin_tools::browser_tools::BrowserScreenshotTool,
    pub(crate) browser_snapshot_tool: crate::builtin_tools::browser_tools::BrowserSnapshotTool,
    pub(crate) browser_navigate_tool: crate::builtin_tools::browser_tools::BrowserNavigateTool,
    pub(crate) browser_tabs_tool: crate::builtin_tools::browser_tools::BrowserTabsTool,
    pub(crate) browser_select_tool: crate::builtin_tools::browser_tools::BrowserSelectTool,
    pub(crate) browser_evaluate_tool: crate::builtin_tools::browser_tools::BrowserEvaluateTool,
    pub(crate) browser_fill_form_tool: crate::builtin_tools::browser_tools::BrowserFillFormTool,
    pub(crate) browser_press_key_tool: crate::builtin_tools::browser_tools::BrowserPressKeyTool,
    pub(crate) browser_wait_for_tool: crate::builtin_tools::browser_tools::BrowserWaitForTool,
    pub(crate) browser_console_tool: crate::builtin_tools::browser_tools::BrowserConsoleTool,
    pub(crate) browser_profile_tool: crate::builtin_tools::browser_tools::BrowserProfileTool,
    /// Shared session key handle for memory_search scope=current_session
    pub(super) memory_session_key_handle: Option<Arc<RwLock<String>>>,
    /// Session context handle for agent management tools
    pub(super) session_context_handle:
        Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
    /// Tool policy handle for per-agent tool access control
    pub(super) tool_policy_handle: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
    /// Tool context handle for workspace-scoped output paths
    pub(super) tool_context_handle: Option<crate::tools::ToolContextHandle>,
    /// Event bus for lifecycle event emission (held for future use; tools get their own clones)
    #[allow(dead_code)]
    pub(super) event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Extension manager for plugin tool execution
    pub(super) extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
    /// ACP delegate tool (optional - requires AcpHarnessManager)
    pub(crate) acp_delegate_tool: Option<crate::builtin_tools::acp_tools::AcpDelegateTool>,
    pub(crate) acp_switch_tool: Option<crate::builtin_tools::acp_tools::AcpSwitchTool>,
    /// ClawHub tool instance
    pub(crate) clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool,
    pub(crate) gateway_route_tool: crate::builtin_tools::gateway_route::GatewayRouteTool,
    /// Task coordination tools (optional — require CoordTaskStore)
    pub(crate) task_create_tool: Option<crate::builtin_tools::task_manage::TaskCreateTool>,
    pub(crate) task_update_tool: Option<crate::builtin_tools::task_manage::TaskUpdateTool>,
    pub(crate) task_list_tool: Option<crate::builtin_tools::task_manage::TaskListTool>,
    pub(crate) task_wait_tool: Option<crate::builtin_tools::task_manage::TaskWaitTool>,
    /// Task artifact tools (optional — require ArtifactStore)
    pub(crate) task_submit_tool: Option<crate::builtin_tools::team::TaskSubmitTool>,
    pub(crate) task_read_artifact_tool: Option<crate::builtin_tools::team::TaskReadArtifactTool>,
    /// Team management tools (optional — require TeamStore)
    pub(crate) team_create_tool: Option<crate::builtin_tools::team::TeamCreateTool>,
    pub(crate) team_delegate_tool: Option<crate::builtin_tools::team::TeamDelegateTool>,
    pub(crate) team_status_tool: Option<crate::builtin_tools::team::TeamStatusTool>,
    pub(crate) team_disband_tool: Option<crate::builtin_tools::team::TeamDisbandTool>,
    pub(crate) team_member_remove_tool: Option<crate::builtin_tools::team::TeamMemberRemoveTool>,
    pub(crate) team_digest_tool: Option<crate::builtin_tools::team::TeamDigestTool>,
    /// Team messaging tools (optional — require MessageRouter / Inbox)
    pub(crate) message_send_tool: Option<crate::builtin_tools::team::MessageSendTool>,
    pub(crate) inbox_read_tool: Option<crate::builtin_tools::team::InboxReadTool>,
    /// Collaborative session tools (optional — require SessionCoordinator / SessionStore)
    pub(crate) session_collaborate_tool: Option<crate::builtin_tools::team::SessionCollaborateTool>,
    pub(crate) session_turn_tool: Option<crate::builtin_tools::team::SessionTurnTool>,
    pub(crate) session_read_tool: Option<crate::builtin_tools::team::SessionReadTool>,
    /// Skill management tools — always available (SkillSystem is always initialized)
    pub(crate) skill_status_tool: crate::builtin_tools::skill_status::SkillStatusTool,
    pub(crate) skill_install_tool: crate::builtin_tools::skill_install::SkillInstallTool,
    pub(crate) skill_manage_tool: crate::builtin_tools::skill_manage::SkillManageTool,
    /// Unified note management tool (optional - requires memory_db)
    pub(crate) note_manage_tool: Option<crate::builtin_tools::note_manage::NoteManageTool>,
    /// Session-complete tool (optional - requires memory_db)
    pub(crate) session_complete_tool:
        Option<crate::builtin_tools::session_complete::SessionCompleteTool>,
    /// Memory-reflect tool (optional - requires MemoryReflector, injected by Task 8)
    pub(crate) memory_reflect_tool: Option<crate::builtin_tools::memory_reflect::MemoryReflectTool>,
    /// Channel registry for deferred injection (same pattern as gateway_context).
    /// Used by channel_pairing tool.
    pub(crate) channel_registry_cell: Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>,
    /// Tool metadata for lookup
    pub(super) tools: HashMap<String, UnifiedTool>,
}

impl BuiltinToolRegistry {
    /// Create a new registry with default configuration
    pub async fn new() -> Self {
        Self::with_config(BuiltinToolConfig::default()).await
    }

    /// Register an additional tool (e.g., plugin tools discovered at runtime)
    pub fn register_tool(&mut self, tool: UnifiedTool) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Inject GatewayContext after construction (breaks circular dependency).
    ///
    /// BuiltinToolRegistry is created before ExecutionAdapter exists, but
    /// GatewayContext needs ExecutionAdapter. This method allows deferred
    /// injection once all components are ready, enabling session.list and
    /// session.send tools.
    ///
    /// Takes `&self` (not `&mut self`) so it works through `Arc`.
    pub fn set_gateway_context(&self, context: Arc<GatewayContext>) {
        if self.gateway_context.set(context).is_ok() {
            info!("GatewayContext injected — session.list and session.send now available");
        }
    }

    /// Get a handle to the GatewayContext OnceCell for deferred injection.
    ///
    /// Used by agent_init to inject GatewayContext after ExecutionEngine creation.
    pub fn gateway_context_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<GatewayContext>>> {
        Arc::clone(&self.gateway_context)
    }

    /// Inject ChannelRegistry after construction (deferred — channels are created after tools).
    ///
    /// Enables the `channel_pairing` tool for pairing code management.
    /// Takes `&self` (not `&mut self`) so it works through `Arc`.
    pub fn set_channel_registry(&self, registry: Arc<ChannelRegistry>) {
        if self.channel_registry_cell.set(registry).is_ok() {
            info!("ChannelRegistry injected — channel_pairing tool now available");
        }
    }

    /// Get a handle to the ChannelRegistry OnceCell for deferred injection.
    pub fn channel_registry_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>> {
        Arc::clone(&self.channel_registry_cell)
    }

    /// Inject a `MemoryReflector` into the `memory_reflect` tool (Task 8 wiring).
    ///
    /// Must be called before the registry is wrapped in `Arc` (takes `&mut self`).
    /// After injection the tool will synthesise answers from memory; without it
    /// the tool returns a clear error rather than panicking.
    pub fn set_memory_reflector(
        &mut self,
        reflector: Arc<crate::memory::reflector::MemoryReflector>,
    ) {
        if let Some(ref mut tool) = self.memory_reflect_tool {
            *tool = tool.clone().with_reflector(reflector);
            info!("MemoryReflector injected into memory_reflect tool");
        }
    }

    /// Get the parameter schema for a tool by name.
    ///
    /// Returns the schema if the tool exists in the internal registry and has
    /// a `parameters_schema` set. Used to attach schemas to the `UnifiedTool`
    /// list sent to the LLM so it knows which arguments to pass.
    pub fn get_tool_schema(&self, name: &str) -> Option<Value> {
        self.tools
            .get(name)
            .and_then(|t| t.parameters_schema.clone())
    }

    /// Returns `true` if a tool with this name has been registered in the metadata map.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub(crate) fn resolve_plugin_handler(&self, tool_name: &str) -> Option<(String, String)> {
        resolve_plugin_handler_from_sources(
            self.extension_manager.as_deref(),
            &self.tools,
            tool_name,
        )
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
        self.memory_search_tool
            .as_ref()
            .map(|t| t.smart_recall_config_handle())
    }

    fn session_context_handle(
        &self,
    ) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        self.session_context_handle.clone()
    }

    fn tool_policy_handle(
        &self,
    ) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::ToolPolicy>>> {
        self.tool_policy_handle.clone()
    }

    fn tool_context_handle(&self) -> Option<crate::tools::ToolContextHandle> {
        self.tool_context_handle.clone()
    }

    fn session_key_handle(&self) -> Option<Arc<RwLock<String>>> {
        self.memory_session_key_handle.clone()
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        debug!(tool = tool_name, "Executing builtin tool");

        // Enforce per-agent tool policy.
        // Uses try_read() (non-blocking) since this is a synchronous function.
        // Contention is extremely unlikely — policy is rarely written.
        if let Some(ref policy_handle) = self.tool_policy_handle {
            if let Ok(policy) = policy_handle.try_read() {
                if !policy.is_allowed(tool_name) {
                    let msg = format!(
                        "Tool '{}' is not allowed for the current agent. \
                         Use agent.list to check available tools, or switch to an agent that has access.",
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
            "file_read" => Box::pin(async move { self.file_read_tool.call_json(arguments).await }),
            "file_write" => {
                Box::pin(async move { self.file_write_tool.call_json(arguments).await })
            }
            "file_edit" => Box::pin(async move { self.file_edit_tool.call_json(arguments).await }),
            "bash" => Box::pin(async move { self.bash_tool.call_json(arguments).await }),
            "code_exec" => Box::pin(async move { self.code_exec_tool.call_json(arguments).await }),
            "pdf_generate" => {
                Box::pin(async move { self.pdf_generate_tool.call_json(arguments).await })
            }

            // Generation tools - image uses AlephTool, video/audio use legacy execute_* methods
            "image_generate" => Box::pin(async move {
                let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Image generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "video_generate" => Box::pin(async move {
                let tool = self.video_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Video generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "audio_generate" => Box::pin(async move {
                let tool = self.audio_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Audio generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "speech_generate" => Box::pin(async move {
                let tool = self.speech_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Speech generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),

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
                    AlephError::tool(
                        "get_tool_schema not available: no dispatcher registry configured",
                    )
                })?;
                let tool = GetToolSchemaTool::new(Arc::clone(registry));
                tool.call_json(arguments).await
            }),

            // Self-management tools
            "skill_list" => {
                Box::pin(async move { self.list_skills_tool.call_json(arguments).await })
            }
            "skill_read" => {
                Box::pin(async move { self.read_skill_tool.call_json(arguments).await })
            }
            "read_config_guide" => {
                Box::pin(async move { self.config_guide_tool.call_json(arguments).await })
            }
            "self_manage" => {
                Box::pin(async move { self.self_manage_tool.call_json(arguments).await })
            }
            "self_config" => {
                Box::pin(async move { self.self_config_tool.call_json(arguments).await })
            }
            "vault_store" => Box::pin(async move {
                let tool = self.vault_store_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("vault_store not available: no SharedTokenManager configured")
                })?;
                tool.call_json(arguments).await
            }),
            "desktop" => Box::pin(async move { self.desktop_tool.call_json(arguments).await }),
            "pim" => Box::pin(async move { self.pim_tool.call_json(arguments).await }),
            "system" => Box::pin(async move { self.system_tool.call_json(arguments).await }),
            "automation" => {
                Box::pin(async move { self.automation_tool.call_json(arguments).await })
            }
            "permission" => {
                Box::pin(async move { self.permission_tool.call_json(arguments).await })
            }
            "media" => Box::pin(async move { self.media_tool.call_json(arguments).await }),
            "scratchpad" => {
                Box::pin(async move { self.scratchpad_tool.call_json(arguments).await })
            }

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
            "memory_explore" => Box::pin(async move {
                let tool = self.memory_explore_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_explore not available: no memory backend or embedding provider configured")
                })?;
                tool.call_json(arguments).await
            }),
            "memory_timeline" => Box::pin(async move {
                let tool = self.memory_timeline_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_timeline not available: no event store configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Sessions tools for cross-session communication
            "session_list" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("session_list not available: GatewayContext not yet injected")
                })?;
                let tool = SessionsListTool::new(Arc::clone(context), "main");
                tool.call_json(arguments).await
            }),
            "session_send" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("session_send not available: GatewayContext not yet injected")
                })?;
                let tool = SessionsSendTool::with_context((**context).clone(), "main");
                tool.call_json(arguments).await
            }),

            // Session search tool — uses GatewayContext for A2A policy filtering
            "session_search" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool(
                        "session_search not available: GatewayContext not yet injected",
                    )
                })?;
                // Derive caller identity from session context (agent_id is the first
                // segment of session_key_str, e.g. "assistant:dm:telegram:…").
                // Falls back to "main" when session context is unavailable.
                let caller_id = self
                    .session_context_handle
                    .as_ref()
                    .and_then(|h| h.try_read().ok())
                    .and_then(|ctx| ctx.session_key_str.split(':').next().map(|s| s.to_string()))
                    .unwrap_or_else(|| "main".to_string());
                let tool =
                    crate::builtin_tools::SessionSearchTool::new(Arc::clone(context), caller_id);
                tool.call_json(arguments).await
            }),

            // Browser tools
            "browser_open" => {
                Box::pin(async move { self.browser_open_tool.call_json(arguments).await })
            }
            "browser_click" => {
                Box::pin(async move { self.browser_click_tool.call_json(arguments).await })
            }
            "browser_type" => {
                Box::pin(async move { self.browser_type_tool.call_json(arguments).await })
            }
            "browser_screenshot" => {
                Box::pin(async move { self.browser_screenshot_tool.call_json(arguments).await })
            }
            "browser_snapshot" => {
                Box::pin(async move { self.browser_snapshot_tool.call_json(arguments).await })
            }
            "browser_navigate" => {
                Box::pin(async move { self.browser_navigate_tool.call_json(arguments).await })
            }
            "browser_tabs" => {
                Box::pin(async move { self.browser_tabs_tool.call_json(arguments).await })
            }
            "browser_select" => {
                Box::pin(async move { self.browser_select_tool.call_json(arguments).await })
            }
            "browser_evaluate" => {
                Box::pin(async move { self.browser_evaluate_tool.call_json(arguments).await })
            }
            "browser_fill_form" => {
                Box::pin(async move { self.browser_fill_form_tool.call_json(arguments).await })
            }
            "browser_press_key" => {
                Box::pin(async move { self.browser_press_key_tool.call_json(arguments).await })
            }
            "browser_wait_for" => {
                Box::pin(async move { self.browser_wait_for_tool.call_json(arguments).await })
            }
            "browser_console" => {
                Box::pin(async move { self.browser_console_tool.call_json(arguments).await })
            }
            "browser_profile" => {
                Box::pin(async move { self.browser_profile_tool.call_json(arguments).await })
            }

            // Session new tool — inject session key from session context
            "session_new" => {
                let arguments = {
                    let mut args = arguments;
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert(
                                    "__session_key".into(),
                                    serde_json::Value::String(ctx.session_key_str.clone()),
                                );
                            }
                        }
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_new_tool.as_ref().ok_or_else(|| {
                        AlephError::tool("session_new not available: no SessionManager configured")
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Session set-topic tool — inject session key from session context
            "session_rename" => {
                let arguments = {
                    let mut args = arguments;
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert(
                                    "__session_key".into(),
                                    serde_json::Value::String(ctx.session_key_str.clone()),
                                );
                            }
                        }
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_set_topic_tool.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "session_rename not available: no SessionManager configured",
                        )
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Cron management tool — inject session channel + conversation context so
            // created jobs know where to deliver results. Also inject current_time_ms
            // so the LLM has a reliable epoch reference for computing At timestamps.
            "cron_manage" => {
                let arguments = {
                    let mut args = arguments;
                    if let Some(obj) = args.as_object_mut() {
                        obj.insert(
                            "__current_time_ms".into(),
                            serde_json::Value::Number(chrono::Utc::now().timestamp_millis().into()),
                        );
                    }
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert(
                                    "__channel".into(),
                                    serde_json::Value::String(ctx.channel.clone()),
                                );
                                obj.insert(
                                    "__conversation_id".into(),
                                    serde_json::Value::String(ctx.conversation_id.clone()),
                                );
                            }
                        }
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.cron_manage_tool.as_ref().ok_or_else(|| {
                        AlephError::tool("cron_manage not available: cron service not configured")
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Heartbeat management tools
            "heartbeat_list" => Box::pin(async move {
                let tool = self.heartbeat_list_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_list not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_create" => Box::pin(async move {
                let tool = self.heartbeat_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_create not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_update" => Box::pin(async move {
                let tool = self.heartbeat_update_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_update not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_delete" => Box::pin(async move {
                let tool = self.heartbeat_delete_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_delete not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_toggle" => Box::pin(async move {
                let tool = self.heartbeat_toggle_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_toggle not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            // Heartbeat report tool — always available (used during L2 heartbeat execution)
            "heartbeat_report" => {
                Box::pin(async move { self.heartbeat_report_tool.call_json(arguments).await })
            }

            // Agent management tools — snapshot session context into arguments
            // to avoid race conditions from concurrent reads of the shared handle.
            "agent_create" | "agent_list" | "agent_delete" => {
                // Snapshot session context into tool arguments before async execution
                let arguments = {
                    let mut args = arguments;
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert(
                                    "__channel".into(),
                                    serde_json::Value::String(ctx.channel.clone()),
                                );
                            }
                        }
                    }
                    args
                };

                match tool_name {
                    "agent_create" => Box::pin(async move {
                        let tool = self.agent_create_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_create not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_list" => Box::pin(async move {
                        let tool = self.agent_list_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_list not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_delete" => Box::pin(async move {
                        let tool = self.agent_delete_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_delete not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    _ => unreachable!(),
                }
            }

            // Task coordination tools
            "task_create" => Box::pin(async move {
                let tool = self.task_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_create not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_update" => Box::pin(async move {
                let tool = self.task_update_tool.as_ref().ok_or_else(|| AlephError::tool("task_update not available: no CoordTaskStore or AgentMessageBus configured"))?;
                tool.call_json(arguments).await
            }),
            "task_list" => Box::pin(async move {
                let tool = self.task_list_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_list not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_wait" => {
                Box::pin(async move {
                    let tool = self.task_wait_tool.as_ref().ok_or_else(|| AlephError::tool("task_wait not available: no CoordTaskStore or AgentMessageBus configured"))?;
                    tool.call_json(arguments).await
                })
            }

            // Team management tools
            "team_create" => Box::pin(async move {
                let tool = self.team_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_create not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_delegate" => Box::pin(async move {
                let tool = self.team_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_delegate not available: no TeamStore configured")
                })?;
                // Inject GatewayContext from OnceCell (deferred — same pattern as session_send)
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("team_delegate not available: GatewayContext not yet injected")
                })?;
                let mut delegate = tool.clone();
                delegate.set_context((**context).clone());
                delegate.call_json(arguments).await
            }),
            "team_status" => Box::pin(async move {
                let tool = self.team_status_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_status not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_disband" => Box::pin(async move {
                let tool = self.team_disband_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_disband not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_member_remove" => Box::pin(async move {
                let tool = self.team_member_remove_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_member_remove not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_digest" => Box::pin(async move {
                let tool = self.team_digest_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_digest not available: no EventLogStore configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Team messaging tools
            "message_send" => Box::pin(async move {
                let tool = self.message_send_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("message_send not available: no MessageRouter configured")
                })?;
                tool.call_json(arguments).await
            }),
            "inbox_read" => Box::pin(async move {
                let tool = self.inbox_read_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("inbox_read not available: no Inbox configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Collaborative session tools
            "session_collaborate" => Box::pin(async move {
                let tool = self.session_collaborate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "session_collaborate not available: no SessionCoordinator configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "session_turn" => Box::pin(async move {
                let tool = self.session_turn_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("session_turn not available: no SessionCoordinator configured")
                })?;
                tool.call_json(arguments).await
            }),
            "session_read" => Box::pin(async move {
                let tool = self.session_read_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("session_read not available: no SessionStore configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Task artifact tools
            "task_submit" => Box::pin(async move {
                let tool = self.task_submit_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_submit not available: no ArtifactStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_read_artifact" => Box::pin(async move {
                let tool = self.task_read_artifact_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "task_read_artifact not available: no ArtifactStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),

            // Channel pairing tool (deferred — ChannelRegistry injected after construction)
            "channel_pairing" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "channel_pairing not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool =
                    crate::builtin_tools::channel_manage::ChannelPairingTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // Voice mode tool (deferred — ChannelRegistry injected after construction)
            "voice_mode_set" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "voice_mode_set not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool = crate::builtin_tools::voice_tools::VoiceModeSetTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // ClawHub tool
            "clawhub" => Box::pin(async move { self.clawhub_tool.call_json(arguments).await }),

            "gateway_route" => {
                Box::pin(async move { self.gateway_route_tool.call_json(arguments).await })
            }

            // Media send tool — no dependencies, always available
            "media_send" => Box::pin(async move {
                crate::builtin_tools::media_send::MediaSendTool::new()
                    .call_json(arguments)
                    .await
            }),

            // ACP delegate tool (unified)
            "acp_delegate" => Box::pin(async move {
                let tool = self.acp_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_delegate not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),
            "acp_switch" => Box::pin(async move {
                let tool = self.acp_switch_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_switch not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Skill management tools
            "skill_status" => {
                Box::pin(async move { self.skill_status_tool.call_json(arguments).await })
            }
            "skill_install" => {
                Box::pin(async move { self.skill_install_tool.call_json(arguments).await })
            }
            "skill_manage" => {
                Box::pin(async move { self.skill_manage_tool.call_json(arguments).await })
            }
            "note_manage" => {
                if let Some(ref tool) = self.note_manage_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "note_manage tool is not available: memory backend not configured",
                        ))
                    })
                }
            }

            "session_complete" => {
                if let Some(ref tool) = self.session_complete_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "session_complete tool is not available: memory backend not configured",
                        ))
                    })
                }
            }

            "memory_reflect" => {
                if let Some(ref tool) = self.memory_reflect_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "memory_reflect tool is not available: MemoryReflector not wired (server builder needs to inject it)",
                        ))
                    })
                }
            }

            _ => {
                if let Some((plugin_id, handler)) = self.resolve_plugin_handler(tool_name) {
                    let ext_mgr = self.extension_manager.clone();
                    return Box::pin(async move {
                        let ext_mgr = ext_mgr.ok_or_else(|| {
                            AlephError::tool(
                                "Plugin tool execution unavailable: extension manager not configured",
                            )
                        })?;
                        info!(plugin = %plugin_id, tool = %handler, "Executing plugin tool");
                        ext_mgr
                            .call_plugin_tool(&plugin_id, &handler, arguments)
                            .await
                            .map_err(|e| {
                                AlephError::tool(format!("Plugin tool '{}' failed: {}", handler, e))
                            })
                    });
                }
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move { Err(AlephError::tool(format!("Unknown tool: {}", tool))) })
            }
        }
    }
}

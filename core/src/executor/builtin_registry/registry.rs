//! Core registry implementation for builtin tools

use std::collections::HashMap;
use std::pin::Pin;
use crate::sync_primitives::Arc;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::builtin_tools::meta_tools::{ListToolsTool, GetToolSchemaTool};
use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool};
use crate::error::{AlephError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::gateway::context::GatewayContext;
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
    pub(crate) search_tool: crate::builtin_tools::SearchTool,
    /// Web fetch tool instance
    pub(crate) web_fetch_tool: crate::builtin_tools::WebFetchTool,
    /// File operations tool instance
    pub(crate) file_ops_tool: crate::builtin_tools::FileOpsTool,
    /// Bash execution tool instance (wraps CodeExecTool for shell commands)
    pub(crate) bash_tool: crate::builtin_tools::BashExecTool,
    /// Code execution tool instance
    pub(crate) code_exec_tool: crate::builtin_tools::CodeExecTool,
    /// PDF generation tool instance
    pub(crate) pdf_generate_tool: crate::builtin_tools::PdfGenerateTool,
    /// Image generation tool instance (optional - requires generation registry)
    pub(crate) image_generate_tool: Option<crate::builtin_tools::ImageGenerateTool>,
    /// Read skill tool instance (for Progressive Disclosure pattern)
    pub(crate) read_skill_tool: crate::builtin_tools::skill_reader::ReadSkillTool,
    /// List skills tool instance
    pub(crate) list_skills_tool: crate::builtin_tools::skill_reader::ListSkillsTool,
    /// Desktop bridge tool instance
    pub(crate) desktop_tool: crate::builtin_tools::DesktopTool,
    /// PIM (Personal Information Management) tool instance
    pub(crate) pim_tool: crate::builtin_tools::PimTool,
    /// Soul update tool instance (identity evolution via soul_update)
    pub(crate) soul_update_tool: crate::builtin_tools::SoulUpdateTool,
    /// Profile update tool instance (user profile management)
    pub(crate) profile_update_tool: crate::builtin_tools::ProfileUpdateTool,
    /// Scratchpad tool instance (project working memory)
    pub(crate) scratchpad_tool: crate::builtin_tools::ScratchpadTool,
    /// Config read tool instance (optional - requires config handle)
    pub(crate) config_read_tool: Option<crate::builtin_tools::ConfigReadTool>,
    /// Config update tool instance (optional - requires ConfigPatcher)
    pub(crate) config_update_tool: Option<crate::builtin_tools::ConfigUpdateTool>,
    /// Memory search tool instance (optional - requires memory_db + embedder)
    pub(crate) memory_search_tool: Option<crate::builtin_tools::MemorySearchTool>,
    /// Memory browse tool instance (optional - requires memory_db)
    pub(crate) memory_browse_tool: Option<crate::builtin_tools::MemoryBrowseTool>,
    /// Shared workspace handle for memory tools — written by ExecutionEngine after workspace resolution
    pub(super) memory_workspace_handle: Option<Arc<RwLock<String>>>,
    /// Generation provider registry for video/audio generation
    pub(crate) generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub(crate) dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    pub(crate) gateway_context: Option<Arc<GatewayContext>>,
    /// Session new tool (optional - requires SessionManager)
    pub(crate) session_new_tool: Option<crate::builtin_tools::sessions::SessionNewTool>,
    /// Cron management tool (optional - requires SharedCronService)
    pub(crate) cron_manage_tool: Option<crate::builtin_tools::cron_manage::CronManageTool>,
    /// Agent management tools (optional - requires AgentRegistry + WorkspaceManager)
    pub(crate) agent_create_tool: Option<crate::builtin_tools::agent_manage::AgentCreateTool>,
    pub(crate) agent_switch_tool: Option<crate::builtin_tools::agent_manage::AgentSwitchTool>,
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
    pub(crate) browser_profile_tool: crate::builtin_tools::browser_tools::BrowserProfileTool,
    /// Session context handle for agent management tools
    pub(super) session_context_handle: Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
    /// Tool policy handle for per-agent tool access control
    pub(super) tool_policy_handle: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
    /// Tool context handle for workspace-scoped output paths
    pub(super) tool_context_handle: Option<crate::tools::ToolContextHandle>,
    /// Event bus for lifecycle event emission (held for future use; tools get their own clones)
    #[allow(dead_code)]
    pub(super) event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Extension manager for plugin tool execution
    pub(super) extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
    /// ACP delegate tools (optional - requires AcpHarnessManager)
    pub(crate) claude_code_tool: Option<crate::builtin_tools::acp_tools::ClaudeCodeTool>,
    pub(crate) codex_tool: Option<crate::builtin_tools::acp_tools::CodexTool>,
    pub(crate) gemini_cli_tool: Option<crate::builtin_tools::acp_tools::GeminiCliTool>,
    pub(crate) acp_switch_tool: Option<crate::builtin_tools::acp_tools::AcpSwitchTool>,
    /// ClawHub tool instance
    pub(crate) clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool,
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

    fn tool_context_handle(&self) -> Option<crate::tools::ToolContextHandle> {
        self.tool_context_handle.clone()
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

            // Session new tool — inject session key from session context
            "session_new" => {
                let arguments = {
                    let mut args = arguments;
                    if let Some(ref h) = self.session_context_handle {
                        if let Ok(ctx) = h.try_read() {
                            if let Some(obj) = args.as_object_mut() {
                                obj.insert("__session_key".into(), serde_json::Value::String(ctx.session_key_str.clone()));
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

            // Cron management tool
            "cron_manage" => Box::pin(async move {
                let tool = self.cron_manage_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("cron_manage not available: cron service not configured")
                })?;
                tool.call_json(arguments).await
            }),

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

            // ClawHub tool
            "clawhub" => Box::pin(async move { self.clawhub_tool.call_json(arguments).await }),

            // ACP delegate tools
            "claude_code" => Box::pin(async move {
                let tool = self.claude_code_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("claude_code not available: ACP not configured or claude-code harness not found")
                })?;
                tool.call_json(arguments).await
            }),
            "codex" => Box::pin(async move {
                let tool = self.codex_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("codex not available: ACP not configured or codex harness not found")
                })?;
                tool.call_json(arguments).await
            }),
            "gemini_cli" => Box::pin(async move {
                let tool = self.gemini_cli_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("gemini_cli not available: ACP not configured or gemini harness not found")
                })?;
                tool.call_json(arguments).await
            }),
            "acp_switch" => Box::pin(async move {
                let tool = self.acp_switch_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_switch not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),

            _ => {
                // Check if this is a plugin tool
                if let Some(unified) = self.tools.get(tool_name) {
                    if let ToolSource::Plugin { ref plugin_id } = unified.source {
                        let plugin_id = plugin_id.clone();
                        // plugin-host.js registers tool handlers as "tool_{name}"
                        let handler = format!("tool_{}", tool_name);
                        let ext_mgr = self.extension_manager.clone();
                        return Box::pin(async move {
                            let ext_mgr = ext_mgr.ok_or_else(|| {
                                AlephError::tool("Plugin tool execution unavailable: extension manager not configured")
                            })?;
                            info!(plugin = %plugin_id, tool = %handler, "Executing plugin tool");
                            ext_mgr.call_plugin_tool(&plugin_id, &handler, arguments)
                                .await
                                .map_err(|e| AlephError::tool(format!("Plugin tool '{}' failed: {}", handler, e)))
                        });
                    }
                }
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move {
                    Err(AlephError::tool(format!("Unknown tool: {}", tool)))
                })
            }
        }
    }
}

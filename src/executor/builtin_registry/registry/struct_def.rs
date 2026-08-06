//! The `BuiltinToolRegistry` struct definition (field declarations only).
#![allow(unused_imports)]

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::pin::Pin;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::context::GatewayContext;
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;
use tokio::sync::RwLock;

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
///
/// Security enforcement is layered, not centralised in this registry:
/// - `GuardrailRegistry` (input / output / tool-call) covers content checks.
/// - `WorkspaceSandbox` covers OS-level isolation.
/// - `ApprovalGate` covers HITL escalation.
///
/// See docs/reference/SANDBOX.md and docs/reference/SECURITY.md.
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
    /// V4A multi-file structured patch tool instance
    pub(crate) apply_patch_tool: crate::builtin_tools::ApplyPatchTool,
    /// Bash execution tool instance (wraps `CodeExecTool` for shell commands)
    pub(crate) bash_tool: crate::builtin_tools::BashExecTool,
    /// Code execution tool instance
    pub(crate) code_exec_tool: crate::builtin_tools::CodeExecTool,
    pub(crate) code_check_tool: crate::builtin_tools::CodeCheckTool,
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
    /// Config guide tool instance (progressive disclosure for self-management)
    pub(crate) config_guide_tool: crate::builtin_tools::ReadConfigGuideTool,
    /// Ctx-search tool instance (BM25 retrieval over offloaded tool output)
    pub(crate) ctx_search_tool: crate::builtin_tools::CtxSearchTool,
    /// Recall-events tool instance (BM25 retrieval over this session's event log)
    pub(crate) recall_events_tool: crate::builtin_tools::RecallEventsTool,
    /// Self-management tool instance (LLM-triggered entry point)
    pub(crate) self_manage_tool: crate::builtin_tools::SelfManageTool,
    /// Hooks-manage tool instance (runtime hook inventory + global hooks.json CRUD)
    pub(crate) hooks_manage_tool: crate::builtin_tools::HooksManageTool,
    /// Self-config tool instance (identity files + config.toml access)
    pub(crate) self_config_tool: crate::builtin_tools::self_config::SelfConfigTool,
    /// Moa-manage tool instance (session MoA activation + preset CRUD)
    pub(crate) moa_manage_tool: crate::builtin_tools::moa_manage::MoaManageTool,
    /// List-models tool instance (LLM-facing model discovery: capability + cost)
    pub(crate) list_models_tool: crate::builtin_tools::list_models::ListModelsTool,
    /// Doctor tool instance (self-diagnosis; carries live config + vault
    /// handles so the engine can probe provider connectivity at runtime)
    pub(crate) doctor_tool: crate::builtin_tools::DoctorTool,
    /// Vault store tool instance (optional - requires `SharedTokenManager`)
    pub(crate) vault_store_tool: Option<crate::builtin_tools::VaultStoreTool>,
    /// Desktop bridge tool instance
    pub(crate) desktop_tool: crate::builtin_tools::DesktopTool,
    /// Accessibility query tools (macOS-backed; graceful no-op on other platforms).
    pub(crate) desktop_ax_query_focused_tool: crate::builtin_tools::DesktopAxQueryFocused,
    pub(crate) desktop_ax_query_tree_tool: crate::builtin_tools::DesktopAxQueryTree,
    pub(crate) desktop_ax_query_by_role_tool: crate::builtin_tools::DesktopAxQueryByRole,
    /// Accessibility snapshot tool — flat indexed interactable-element list.
    pub(crate) desktop_ax_snapshot_tool: crate::builtin_tools::DesktopAxSnapshot,
    /// Visual set-of-marks tool — annotated screenshot with numbered elements.
    pub(crate) desktop_som_tool: crate::builtin_tools::DesktopSom,
    /// Visual grounding tool — natural-language target → on-screen coordinates (AX + OCR).
    pub(crate) desktop_gui_locate_tool: crate::builtin_tools::DesktopGuiLocate,
    /// Permission check tool (macOS-backed; graceful no-op on other platforms).
    pub(crate) desktop_check_permissions_tool: crate::builtin_tools::DesktopCheckPermissions,
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
    /// Scratchpad tool instance (project working memory)
    pub(crate) scratchpad_tool: crate::builtin_tools::ScratchpadTool,
    /// Standing-goal tool instance (persistent objective, R8).
    pub(crate) goal_tool: crate::builtin_tools::GoalTool,
    pub(crate) loop_graph_tool: crate::builtin_tools::LoopGraphTool,
    /// Loop tool instance (in-session timer loop, R8). In-memory only.
    pub(crate) loop_tool: crate::builtin_tools::LoopTool,
    /// Strategy tool instance (persistent planner output, R8).
    pub(crate) strategy_tool: crate::builtin_tools::StrategyTool,
    /// Memory search tool instance (optional - requires `memory_db` + embedder)
    pub(crate) memory_search_tool: Option<crate::builtin_tools::MemorySearchTool>,
    /// Memory context provider — used by the `remember` tool to resolve the
    /// per-agent `CuratedMemoryStore`. Uses `OnceCell` for deferred injection:
    /// the registry is wrapped in `Arc` before the MCP is constructed.
    pub(crate) memory_context_provider:
        Arc<tokio::sync::OnceCell<Arc<crate::thinker::MemoryContextProvider>>>,
    /// Cluster node registry, injected at startup via `set_node_registry`; `node_invoke` uses it for addressing.
    pub(crate) node_registry: Arc<tokio::sync::OnceCell<Arc<crate::cluster::NodeRegistry>>>,
    /// Security store holding the `role=node` device records, injected at
    /// startup via `set_node_security_store`; `node_manage` needs it to make an
    /// enroll idempotent and a deregister sticky.
    pub(crate) node_security_store:
        Arc<tokio::sync::OnceCell<Arc<crate::gateway::security::SecurityStore>>>,
    /// Memory browse tool instance (optional - requires `memory_db`)
    pub(crate) memory_browse_tool: Option<crate::builtin_tools::MemoryBrowseTool>,
    /// Memory explore tool instance (optional - requires `memory_db` + embedder)
    pub(crate) memory_explore_tool: Option<crate::builtin_tools::MemoryExploreTool>,
    /// Memory timeline tool instance (optional - requires `StateDatabase`)
    pub(crate) memory_timeline_tool: Option<crate::builtin_tools::MemoryTimelineTool>,
    /// Phase 3 self-evolution path α — records user-correction signals into
    /// `raw_memory` under <aleph://correction/{id>}. Optional because it requires
    /// a memory backend (Arc<dyn RawMemoryStore>).
    pub(crate) flag_user_correction_tool: Option<crate::builtin_tools::FlagUserCorrectionTool>,
    /// Shared workspace handle for memory tools — written by `ExecutionEngine` after workspace resolution
    pub(crate) memory_workspace_handle: Option<Arc<RwLock<String>>>,
    /// Gateway context for sessions tools (session.list, session.send).
    /// Uses `OnceCell` for deferred injection: `BuiltinToolRegistry` is created before
    /// `ExecutionAdapter` exists, but `GatewayContext` needs `ExecutionAdapter`.
    pub(crate) gateway_context: Arc<tokio::sync::OnceCell<Arc<GatewayContext>>>,
    /// Session new tool (optional - requires `SessionManager`)
    pub(crate) session_new_tool: Option<crate::builtin_tools::sessions::SessionNewTool>,
    /// Session compact tool (optional - requires `SessionManager`)
    pub(crate) session_compact_tool: Option<crate::builtin_tools::sessions::SessionCompactTool>,
    /// Session set-topic tool (optional - requires `SessionManager`)
    pub(crate) session_set_topic_tool: Option<crate::builtin_tools::sessions::SessionSetTopicTool>,
    pub(crate) session_set_mode_tool: Option<crate::builtin_tools::sessions::SessionSetModeTool>,
    // session_search is constructed on-the-fly from gateway_context (like session_list/session_send)
    // to enforce A2A policy filtering — no stored instance needed.
    /// Cron management tool (optional - requires `SharedCronService`)
    pub(crate) cron_manage_tool: Option<crate::builtin_tools::cron_manage::CronManageTool>,
    /// Heartbeat management tools (optional - require `SharedHeartbeatService`)
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
    /// Agent management tools (optional - requires `AgentRegistry` + `AgentEnvStore`)
    pub(crate) agent_create_tool: Option<crate::builtin_tools::agent_manage::AgentCreateTool>,

    pub(crate) agent_list_tool: Option<crate::builtin_tools::agent_manage::AgentListTool>,
    pub(crate) agent_delete_tool: Option<crate::builtin_tools::agent_manage::AgentDeleteTool>,
    pub(crate) agent_switch_tool: Option<crate::builtin_tools::agent_manage::AgentSwitchTool>,
    /// `agent_unbind` — companion to `agent_switch`. Clears the explicit
    /// channel→agent binding (returns to default routing on next message).
    pub(crate) agent_unbind_tool: Option<crate::builtin_tools::agent_manage::AgentUnbindTool>,
    /// `agent_update` — patch an existing agent's editable fields.
    pub(crate) agent_update_tool: Option<crate::builtin_tools::agent_manage::AgentUpdateTool>,
    /// `agent_info` — always available (read-only, depends only on the agent
    /// definition catalog, which is built unconditionally).
    pub(crate) agent_info_tool: crate::builtin_tools::agent_manage::AgentInfoTool,
    /// Browser tools (always available, share a single `ProfileManager`)
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
    pub(crate) browser_batch_tool: crate::builtin_tools::browser_tools::BrowserBatchTool,
    pub(crate) browser_console_tool: crate::builtin_tools::browser_tools::BrowserConsoleTool,
    pub(crate) browser_hover_tool: crate::builtin_tools::browser_tools::BrowserHoverTool,
    pub(crate) browser_scroll_tool: crate::builtin_tools::browser_tools::BrowserScrollTool,
    pub(crate) browser_pdf_tool: crate::builtin_tools::browser_tools::BrowserPdfTool,
    pub(crate) browser_network_tool: crate::builtin_tools::browser_tools::BrowserNetworkTool,
    pub(crate) browser_dialog_tool: crate::builtin_tools::browser_tools::BrowserDialogTool,
    pub(crate) browser_drag_tool: crate::builtin_tools::browser_tools::BrowserDragTool,
    pub(crate) browser_upload_tool: crate::builtin_tools::browser_tools::BrowserUploadTool,
    pub(crate) browser_resize_tool: crate::builtin_tools::browser_tools::BrowserResizeTool,
    pub(crate) browser_emulate_tool: crate::builtin_tools::browser_tools::BrowserEmulateTool,
    pub(crate) browser_cookies_tool: crate::builtin_tools::browser_tools::BrowserCookiesTool,
    pub(crate) browser_session_tool: crate::builtin_tools::browser_tools::BrowserSessionTool,
    pub(crate) browser_profile_tool: crate::builtin_tools::browser_tools::BrowserProfileTool,
    /// Shared session key handle for `memory_search` `scope=current_session`
    pub(crate) memory_session_key_handle: Option<Arc<RwLock<String>>>,
    /// Session context handle for agent management tools
    pub(crate) session_context_handle:
        Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
    /// Extension manager for plugin tool execution
    pub(crate) extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
    /// ACP delegate tool (optional - requires `AcpAdapterManager`)
    pub(crate) acp_delegate_tool: Option<crate::builtin_tools::acp_tools::AcpDelegateTool>,
    pub(crate) acp_switch_tool: Option<crate::builtin_tools::acp_tools::AcpSwitchTool>,
    pub(crate) acp_session_control_tool:
        Option<crate::builtin_tools::acp_tools::AcpSessionControlTool>,
    /// A2A outbound tools (optional - require the A2A subsystem enabled)
    pub(crate) a2a_delegate_tool: Option<crate::builtin_tools::a2a_tools::A2ADelegateTool>,
    pub(crate) a2a_agents_tool: Option<crate::builtin_tools::a2a_tools::A2AAgentsTool>,
    pub(crate) gateway_route_tool: crate::builtin_tools::gateway_route::GatewayRouteTool,
    /// Task coordination tools (optional — require `CoordTaskStore`)
    pub(crate) task_create_tool: Option<crate::builtin_tools::task_manage::TaskCreateTool>,
    pub(crate) task_update_tool: Option<crate::builtin_tools::task_manage::TaskUpdateTool>,
    pub(crate) task_list_tool: Option<crate::builtin_tools::task_manage::TaskListTool>,
    pub(crate) task_wait_tool: Option<crate::builtin_tools::task_manage::TaskWaitTool>,
    /// Per-task handoff comments (optional — requires `CoordTaskStore`).
    pub(crate) task_comment_tool: Option<crate::builtin_tools::team::TaskCommentTool>,
    /// Task artifact tools (optional — require `ArtifactStore`)
    pub(crate) task_submit_tool: Option<crate::builtin_tools::team::TaskSubmitTool>,
    pub(crate) task_read_artifact_tool: Option<crate::builtin_tools::team::TaskReadArtifactTool>,
    /// Leader task acceptance/verification (strategy round 2 — group chat).
    /// Optional because it requires both a `CoordTaskStore` and a `TeamStore`.
    pub(crate) task_review_tool: Option<crate::builtin_tools::team::TaskReviewTool>,
    /// Team management tools (optional — require `TeamStore`)
    pub(crate) team_create_tool: Option<crate::builtin_tools::team::TeamCreateTool>,
    pub(crate) team_delegate_tool: Option<crate::builtin_tools::team::TeamDelegateTool>,
    pub(crate) team_status_tool: Option<crate::builtin_tools::team::TeamStatusTool>,
    pub(crate) team_disband_tool: Option<crate::builtin_tools::team::TeamDisbandTool>,
    pub(crate) team_set_protocol_tool: Option<crate::builtin_tools::team::TeamSetProtocolTool>,
    pub(crate) team_member_add_tool: Option<crate::builtin_tools::team::TeamMemberAddTool>,
    pub(crate) team_member_remove_tool: Option<crate::builtin_tools::team::TeamMemberRemoveTool>,
    pub(crate) team_digest_tool: Option<crate::builtin_tools::team::TeamDigestTool>,
    /// One-shot team instantiation from a TOML blueprint (optional — requires
    /// `TeamStore` + `CoordTaskStore` + `AgentRegistry` + `SessionStore`).
    pub(crate) team_from_template_tool: Option<crate::builtin_tools::team::TeamFromTemplateTool>,
    /// Unified team-snapshot tool (optional — requires `TeamStore` +
    /// `CoordTaskStore` + `SqliteSnapshotStore`; the snapshot store is constructed
    /// alongside `coord_task_store` in the boot path so they share a connection).
    pub(crate) team_snapshot_tool: Option<crate::builtin_tools::team::TeamSnapshotTool>,
    /// Per-team token usage aggregation (optional — requires `TeamStore` +
    /// `StateDatabase`; both are populated alongside the other team-coord
    /// stores in the boot path).
    pub(crate) team_usage_tool: Option<crate::builtin_tools::team::TeamUsageTool>,
    /// ACP-backed team member management (optional — requires `TeamStore`).
    /// Lets agents attach external coding CLIs (Claude Code, Codex, ...) as
    /// first-class team members via `team_acp_member`.
    pub(crate) team_acp_member_tool: Option<crate::builtin_tools::team::TeamAcpMemberTool>,
    /// Workflow ↔ JSON Canvas bridge (optional — requires `CoordTaskStore`).
    pub(crate) team_workflow_canvas_tool:
        Option<crate::builtin_tools::team::TeamWorkflowCanvasTool>,
    /// Step-level workflow review (Phase C — openteams parity). Optional
    /// because it requires a `CoordTaskStore`.
    pub(crate) workflow_step_review_tool:
        Option<crate::builtin_tools::team::WorkflowStepReviewTool>,
    /// Workflow-template tool (save/list/describe/delete/run). Optional —
    /// `run` requires a `CoordTaskStore` to materialise steps into the DAG.
    pub(crate) workflow_tool: Option<crate::builtin_tools::workflow_tool::WorkflowTool>,
    /// Admin-context task control (R3 — `ClawTeam` parity). Pause/resume/
    /// retry/skip without going through reviewer flow. Optional —
    /// requires a `CoordTaskStore`.
    pub(crate) team_task_control_tool: Option<crate::builtin_tools::team::TeamTaskControlTool>,
    /// Exit-journal tool (R3 — `ClawTeam` parity). The executing agent
    /// calls this on task wrap-up to leave a structured summary that
    /// feeds the unified trace + replay UI. Optional — requires a
    /// `CoordTaskStore`.
    pub(crate) task_exit_journal_tool: Option<crate::builtin_tools::team::TaskExitJournalTool>,
    /// Team messaging tools (optional — require `MessageRouter` / Inbox)
    pub(crate) message_send_tool: Option<crate::builtin_tools::team::MessageSendTool>,
    pub(crate) inbox_read_tool: Option<crate::builtin_tools::team::InboxReadTool>,
    /// Plan approval tools (optional — require `MessageRouter` + `ArtifactStore` + `EventLogStore`)
    pub(crate) plan_submit_tool: Option<crate::builtin_tools::team::PlanSubmitTool>,
    pub(crate) plan_resolve_tool: Option<crate::builtin_tools::team::PlanResolveTool>,
    /// Worker lifecycle tools (optional — require `MessageRouter` + `TeamStore`).
    /// Three-tool triad: worker reports idle, worker requests shutdown,
    /// leader resolves the request. Pairs `MessageType::Idle` /
    /// `ShutdownRequest` / `ShutdownApproved` / `ShutdownRejected` with
    /// auto-resolved leader recipient, mirroring `ClawTeam`'s
    /// `lifecycle idle / request-shutdown / approve-shutdown` commands.
    pub(crate) lifecycle_idle_tool: Option<crate::builtin_tools::team::LifecycleIdleTool>,
    pub(crate) lifecycle_request_shutdown_tool:
        Option<crate::builtin_tools::team::LifecycleRequestShutdownTool>,
    pub(crate) lifecycle_resolve_shutdown_tool:
        Option<crate::builtin_tools::team::LifecycleResolveShutdownTool>,
    /// Collaborative session tools (optional — require `SessionCoordinator` / `SessionStore`)
    pub(crate) session_collaborate_tool: Option<crate::builtin_tools::team::SessionCollaborateTool>,
    pub(crate) session_turn_tool: Option<crate::builtin_tools::team::SessionTurnTool>,
    pub(crate) session_read_tool: Option<crate::builtin_tools::team::SessionReadTool>,
    /// Google Meet tool — always available; holds an optional out-of-core
    /// transport bridge and reports "not configured" when absent.
    pub(crate) google_meet_tool: crate::builtin_tools::google_meet::GoogleMeetTool,
    /// Skill management tools — always available (`SkillSystem` is always initialized)
    pub(crate) skill_status_tool: crate::builtin_tools::skill_status::SkillStatusTool,
    pub(crate) skill_install_tool: crate::builtin_tools::skill_install::SkillInstallTool,
    pub(crate) skill_manage_tool: crate::builtin_tools::skill_manage::SkillManageTool,
    /// Unified note management tool (optional - requires `memory_db`)
    pub(crate) note_manage_tool: Option<crate::builtin_tools::note_manage::NoteManageTool>,
    /// Session-complete tool (optional - requires `memory_db`)
    pub(crate) session_complete_tool:
        Option<crate::builtin_tools::session_complete::SessionCompleteTool>,
    /// Memory-reflect tool (optional - requires `MemoryReflector`, injected by Task 8)
    pub(crate) memory_reflect_tool: Option<crate::builtin_tools::memory_reflect::MemoryReflectTool>,
    /// Channel registry for deferred injection (same pattern as `gateway_context`).
    /// Used by `channel_pairing` tool.
    pub(crate) channel_registry_cell: Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>,
    /// Clarification manager for deferred injection (same pattern as
    /// `channel_registry_cell`). Used by the `ask_user` tool.
    pub(crate) clarification_manager_cell:
        Arc<tokio::sync::OnceCell<Arc<crate::clarification::ClarificationManager>>>,
    /// Wiki orient tool (Spec 5 Task 12) — optional, requires wiki handle.
    pub(crate) note_orient_tool: Option<crate::builtin_tools::note_orient::NoteOrientTool>,
    /// Note schema tool (Spec 5 Task 12) — always Some when `note_memory_dir` is set.
    pub(crate) note_schema_tool: Option<crate::builtin_tools::note_schema::NoteSchemaTool>,
    /// User profile tool (Spec 7 Task 9) — optional, requires `ProfileSynthesizer`.
    pub(crate) user_profile_tool: Option<crate::builtin_tools::user_profile::UserProfileTool>,
    /// Hub catalog search (optional - requires CatalogCache). Supplies the
    /// `entry_id` that resolve-spec / install-run take.
    pub(crate) hub_catalog_search_tool: Option<crate::builtin_tools::hub::HubCatalogSearchTool>,
    /// Store catalog-sync tool (optional - requires CatalogCache + marketplace configs)
    pub(crate) hub_catalog_sync_tool: Option<crate::builtin_tools::hub::HubCatalogSyncTool>,
    /// Store resolve-spec tool (optional - requires CatalogCache + marketplace configs)
    pub(crate) hub_resolve_spec_tool: Option<crate::builtin_tools::hub::HubResolveSpecTool>,
    /// Store install-run tool (optional - requires CatalogCache + marketplace
    /// configs + vault; live MCP handle optional).
    pub(crate) hub_install_run_tool: Option<crate::builtin_tools::hub::HubInstallRunTool>,
    /// Store install-verify tool (optional - live MCP handle optional for plugin-only verification).
    pub(crate) hub_install_verify_tool: Option<crate::builtin_tools::hub::HubInstallVerifyTool>,
    /// Store fetch-docs tool (scaffold - HTTP-only, no CatalogCache dep)
    pub(crate) hub_fetch_docs_tool: crate::builtin_tools::hub::HubFetchDocsTool,
    /// Live Config handle for the `config_audit` tool (security-posture audit).
    /// Built per-call from this handle, mirroring `create_tool_boxed`.
    pub(crate) config: Option<Arc<RwLock<crate::config::Config>>>,
    /// Media pipeline for `media_understand` / `audio_transcribe` /
    /// `document_extract`. Built per-call; `None` → tools report "not configured".
    pub(crate) media_pipeline: Option<Arc<crate::media::MediaPipeline>>,
    /// Mirror of `MemoryConfig.project_scoped` — needed at dispatch to derive
    /// the same (optionally project-scoped) agent id the compaction pipeline
    /// writes session raw chunks under.
    pub(crate) memory_project_scoped: bool,
    /// Memory backend for the `recall_context` tool (pre-compression recovery).
    /// Built per-call with the active session id from `session_context_handle`.
    pub(crate) recall_context_db: Option<crate::memory::store::MemoryBackend>,
    /// Memory backend for the `memory_trace` tool (evidence-chain walk).
    pub(crate) memory_trace_db: Option<crate::memory::store::MemoryBackend>,
    /// Tool metadata for lookup
    pub(crate) tools: HashMap<String, UnifiedTool>,
}

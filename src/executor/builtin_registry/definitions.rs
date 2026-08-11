//! Builtin tool definitions — static catalog base.
//!
//! # Architecture
//!
//! `BUILTIN_TOOL_DEFINITIONS` is the static, unconditional subset of the
//! builtin tool surface: the names/descriptions that exist regardless of
//! runtime configuration. It seeds the slash-command catalog and the base of
//! the LLM tool list, and `AlephToolServer` sources tool construction from it
//! via `create_tool_boxed()`.
//!
//! It is deliberately NOT the complete tool surface. Conditionally-registered
//! tools (generation tools gated on a provider, team tools gated on a coord
//! store, ACP tools, meta discovery tools, LLM-only tools like
//! `scratchpad`/`goal`) live only in `BuiltinToolRegistry`'s runtime metadata
//! map, populated by the builder. The LLM tool list is therefore completed
//! from `BuiltinToolRegistry::unified_tools()` at agent init — adding a tool
//! here is only needed when it should also have a command surface and be
//! advertised even before its dependencies are configured.
//!
//! # Invariant: `description` is never a literal here
//!
//! Every entry's `description` must reference the named tool's own
//! `DESCRIPTION` const. Writing the text inline instead does not "duplicate"
//! it — it **replaces** it: `agent_init` builds the model's tool list from this
//! catalog and then appends only names the catalog did not already claim
//! (`filter(|t| !existing.contains(&t.name))`), so a literal here silently
//! shadows both the tool's `AlephTool::DESCRIPTION` and whatever the registry
//! constructor registers under that name. The failure is invisible from every
//! direction: the tool compiles, its const has tests, the catalog has tests,
//! and the model simply never receives a word of it.
//!
//! That was not hypothetical. Until 2026-08-04, 143 of 155 entries were
//! literals; `no_sentence_is_stated_twice` recorded that the memory writers'
//! `AFTER A SUCCESSFUL WRITE` contract "ships zero times" for exactly this
//! reason, and the 2026-07-26 prompt-prune round moved ~750 tokens out of a
//! prompt layer and into tool descriptions that were not connected — deleting
//! the text while believing it had been relocated.
//!
//! Guarded by `no_catalog_entry_inlines_its_description`. A tool with no
//! `DESCRIPTION` const gets one (see `NoteOrientTool`), rather than an entry
//! here growing a literal.
//!
//! # Usage
//!
//! - `BUILTIN_TOOL_DEFINITIONS` - List of all tool definitions
//! - `create_tool_boxed()` - Create boxed tool instance for `AlephToolServer`
//! - `get_builtin_tool_names()` - Get list of all tool names
//! - `is_builtin_tool()` - Check if a name is a builtin tool

use crate::sync_primitives::Arc;

use crate::builtin_tools::note_manage::NoteManageTool;
use crate::builtin_tools::skill_reader::ListSkillsTool as SkillListTool;
use crate::builtin_tools::{
    ApplyPatchTool, BashExecTool, CodeCheckTool, CodeExecTool, ConfigAuditTool, CtxSearchTool,
    DesktopAxQueryByRole, DesktopAxQueryFocused, DesktopAxQueryTree, DesktopAxSnapshot,
    DesktopCheckPermissions, DesktopGuiLocate, DesktopSom, DesktopTool, DoctorTool, FileEditTool,
    FileOpsTool, FileReadTool, FileWriteTool, FlagUserCorrectionTool, ImageGenerateTool,
    PdfGenerateTool, ReadConfigGuideTool, RecallEventsTool, RememberTool, SearchTool,
    SelectModelTool, SelfManageTool, VaultStoreTool, WebFetchTool,
};
use crate::tools::AlephToolDyn;

use super::BuiltinToolConfig;

/// Definition of a builtin tool
///
/// This struct describes how to create and identify a builtin tool.
#[derive(Clone)]
pub struct BuiltinToolDefinition {
    /// Tool name (e.g., "search", "bash", "`file_ops`")
    pub name: &'static str,
    /// Tool description for AI prompts
    pub description: &'static str,
    /// Whether this tool requires special configuration
    pub requires_config: bool,
}

/// All builtin tools in the system - Single Source of Truth
///
/// This is the authoritative list of all builtin tools.
/// Both `BuiltinToolRegistry` and `AlephToolServer` use this list.
pub const BUILTIN_TOOL_DEFINITIONS: &[BuiltinToolDefinition] = &[
    BuiltinToolDefinition {
        name: "search",
        description: <crate::builtin_tools::search::SearchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false, // Optional API key
    },
    BuiltinToolDefinition {
        name: "web_fetch",
        description: <crate::builtin_tools::web_fetch::WebFetchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_ops",
        description: <FileOpsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_read",
        description: <FileReadTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_write",
        description: <FileWriteTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_edit",
        description: <FileEditTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "apply_patch",
        description: <ApplyPatchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "bash",
        description: <crate::builtin_tools::bash_exec::BashExecTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "code_exec",
        description: <crate::builtin_tools::code_exec::CodeExecTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "code_check",
        description: <crate::builtin_tools::code_check::CodeCheckTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "ctx_search",
        description: <crate::builtin_tools::ctx_search::CtxSearchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "recall_events",
        description: <crate::builtin_tools::recall_events::RecallEventsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "pdf_generate",
        description: <crate::builtin_tools::pdf_generate::PdfGenerateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "image_generate",
        description: <crate::builtin_tools::ImageGenerateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires generation registry
    },
    BuiltinToolDefinition {
        name: "skill_list",
        description: <crate::builtin_tools::ListSkillsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_read",
        description: <crate::builtin_tools::ReadSkillTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "gateway_route",
        description: <crate::builtin_tools::gateway_route::GatewayRouteTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop",
        description: <crate::builtin_tools::desktop::DesktopTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_focused",
        description: <crate::builtin_tools::DesktopAxQueryFocused as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_tree",
        description: <crate::builtin_tools::DesktopAxQueryTree as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_by_role",
        description: <crate::builtin_tools::DesktopAxQueryByRole as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_snapshot",
        description: <crate::builtin_tools::DesktopAxSnapshot as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_som",
        description: <crate::builtin_tools::DesktopSom as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_gui_locate",
        description: <crate::builtin_tools::DesktopGuiLocate as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_check_permissions",
        description: <crate::builtin_tools::DesktopCheckPermissions as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "read_config_guide",
        description: <crate::builtin_tools::config_guide::ReadConfigGuideTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "config_audit",
        description: <crate::builtin_tools::config_audit::ConfigAuditTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires the live Config handle
    },
    BuiltinToolDefinition {
        name: "doctor",
        description: <crate::builtin_tools::doctor::DoctorTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "select_model",
        description: <crate::builtin_tools::select_model::SelectModelTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "list_models",
        description: <crate::builtin_tools::list_models::ListModelsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Reads injected config + vault for provider/credential state
    },
    BuiltinToolDefinition {
        name: "self_manage",
        description: <crate::builtin_tools::self_manage::SelfManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "hooks_manage",
        description: <crate::builtin_tools::hooks_manage::HooksManageTool as crate::tools::AlephTool>::DESCRIPTION,
        // Reads the process-global extension manager, not injected config.
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "self_config",
        description: <crate::builtin_tools::self_config::SelfConfigTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires per-agent agent_id (injected at construction)
    },
    BuiltinToolDefinition {
        name: "moa",
        description: <crate::builtin_tools::moa_manage::MoaManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // needs injected config + patcher handles
    },
    BuiltinToolDefinition {
        name: "vault_store",
        description: <crate::builtin_tools::vault_store::VaultStoreTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedTokenManager
    },
    BuiltinToolDefinition {
        name: "memory_search",
        description: <crate::builtin_tools::memory_search::MemorySearchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_browse",
        description: <crate::builtin_tools::memory_browse::MemoryBrowseTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires memory_db
    },
    BuiltinToolDefinition {
        name: "memory_explore",
        description: <crate::builtin_tools::memory_explore::MemoryExploreTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_timeline",
        description: <crate::builtin_tools::memory_timeline::MemoryTimelineTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires StateDatabase
    },
    BuiltinToolDefinition {
        name: "remember",
        // Point at the const, do not restate it. A literal here SHADOWS the
        // tool's own `DESCRIPTION`: `agent_init` builds the LLM tool list from
        // this catalog and then only APPENDS registry tools whose name is not
        // already present, so whatever is written here is the only thing the
        // model ever reads. The D4 acknowledgment contract, the batch/budget
        // semantics and the `memory_trace` pointer all live in the const and
        // shipped nowhere while this line was prose.
        description: <RememberTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires MemoryContextProvider (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_list",
        description: <crate::builtin_tools::node_list::NodeListTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_invoke",
        description: <crate::builtin_tools::node_invoke::NodeInvokeTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_invoke_many",
        description: <crate::builtin_tools::node_invoke_many::NodeInvokeManyTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_manage",
        description: <crate::builtin_tools::node_manage::NodeManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires NodeRegistry + SecurityStore (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "agent_identity",
        // Points at the const, and must keep doing so. A literal here SHADOWS
        // `AgentIdentityTool::DESCRIPTION` outright — `agent_init` only appends
        // registry tools whose name the catalog does not already carry — and the
        // literal that used to sit here did not so much as mention `export`,
        // so the whole pin-the-root-fingerprint contract shipped to nobody.
        // Pinned by `agent_identity::tests::the_catalog_ships_the_tools_own_description`.
        description: <crate::builtin_tools::agent_identity::AgentIdentityTool
            as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "node_file",
        description: <crate::builtin_tools::node_file::NodeFileTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    // Memory lifecycle & knowledge-wiki tools — require a memory backend / wiki /
    // profile synthesizer; created dynamically in BuiltinToolRegistry::with_config().
    BuiltinToolDefinition {
        name: "memory_reflect",
        description: <crate::builtin_tools::memory_reflect::MemoryReflectTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "recall_context",
        description: crate::builtin_tools::recall_context::RecallContextTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "memory_trace",
        // The const, not a hand-copied duplicate of it — same reason as the
        // file tools above: this is the model-facing list, so a literal here
        // silently drifts from the guidance the tool documents about itself.
        description: crate::builtin_tools::memory_trace::MemoryTraceTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "note_graph_query",
        description: crate::builtin_tools::note_graph_query::NoteGraphQueryTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "governance_metrics",
        // Points at the const, not a paraphrase of it: a literal here SHADOWS
        // `GovernanceMetricsTool::DESCRIPTION` (agent_init only appends
        // registry tools whose name this catalog does not already carry), and
        // the sentence being shadowed is the one that decides whether an audit
        // verdict is right — "synthesis_sum is 0 on every consolidate run by
        // design". The audit template was carrying a second copy of it purely
        // because this destination was dark.
        description: <crate::builtin_tools::governance_metrics::GovernanceMetricsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "tool_usage",
        // Const, not a literal: a literal here would SHADOW
        // `ToolUsageTool::DESCRIPTION` (agent_init only appends registry tools
        // whose name this catalog does not already carry), and the sentence
        // being shadowed is the one that stops the model reading a `—` count as
        // "unused" and proposing to delete a hooks-only plugin.
        description: <crate::builtin_tools::tool_usage::ToolUsageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "note_orient",
        description: crate::builtin_tools::note_orient::NoteOrientTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "note_schema",
        description: crate::builtin_tools::note_schema::NoteSchemaTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "user_profile",
        description: crate::builtin_tools::user_profile::UserProfileTool::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_complete",
        description: <crate::builtin_tools::session_complete::SessionCompleteTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "flag_user_correction",
        // See the `remember` entry: a literal here shadows the const, and this
        // tool's D4 acknowledgment contract lives only in the const.
        description: <FlagUserCorrectionTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_list",
        description: <crate::builtin_tools::sessions::list_tool::SessionsListTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_send",
        description: <crate::builtin_tools::sessions::send_tool::SessionsSendTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_new",
        description: <crate::builtin_tools::sessions::new_tool::SessionNewTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_compact",
        description: <crate::builtin_tools::sessions::compact_tool::SessionCompactTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_rename",
        description: <crate::builtin_tools::sessions::set_topic_tool::SessionSetTopicTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_set_mode",
        description: <crate::builtin_tools::sessions::set_mode_tool::SessionSetModeTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_search",
        description: <crate::builtin_tools::session_search::SessionSearchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SessionManager
    },
    BuiltinToolDefinition {
        name: "cron_manage",
        description: <crate::builtin_tools::cron_manage::CronManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedCronService
    },
    // Heartbeat management tools — require SharedHeartbeatService
    BuiltinToolDefinition {
        name: "heartbeat_list",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatListTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_create",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatCreateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_update",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatUpdateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_delete",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatDeleteTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_toggle",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatToggleTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires SharedHeartbeatService
    },
    // Heartbeat report tool — always available, used during L2 heartbeat execution
    BuiltinToolDefinition {
        name: "heartbeat_report",
        description: <crate::builtin_tools::heartbeat_manage::HeartbeatReportTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "agent_create",
        description: <crate::builtin_tools::agent_manage::create::AgentCreateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires agent_registry + workspace_manager
    },

    BuiltinToolDefinition {
        name: "agent_list",
        description: <crate::builtin_tools::agent_manage::list::AgentListTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires agent_registry
    },
    BuiltinToolDefinition {
        name: "agent_delete",
        description: <crate::builtin_tools::agent_manage::delete::AgentDeleteTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_switch",
        description: <crate::builtin_tools::agent_manage::switch::AgentSwitchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_unbind",
        description: <crate::builtin_tools::agent_manage::unbind::AgentUnbindTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires workspace_manager (and registry for symmetry)
    },
    BuiltinToolDefinition {
        name: "agent_update",
        description: <crate::builtin_tools::agent_manage::update::AgentUpdateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires agent_registry; TOML manager optional
    },
    BuiltinToolDefinition {
        name: "agent_info",
        description: <crate::builtin_tools::agent_manage::info::AgentInfoTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false, // Always available — builds its own agent-definition catalog
    },
    // The conversational face of `workspace.*` (R8). Shares every verdict with
    // the RPC handlers via `gateway::agent_env::ops`.
    BuiltinToolDefinition {
        name: "workspace_manage",
        description: <crate::builtin_tools::workspace_manage::WorkspaceManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires the gateway's AgentEnvStore (the one with the event bus)
    },
    // Browser tools — always available, share a ProfileManager
    BuiltinToolDefinition {
        name: "browser_open",
        description: <crate::builtin_tools::browser_tools::open::BrowserOpenTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_click",
        description: <crate::builtin_tools::browser_tools::click::BrowserClickTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_type",
        description: <crate::builtin_tools::browser_tools::type_text::BrowserTypeTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_screenshot",
        description: <crate::builtin_tools::browser_tools::screenshot::BrowserScreenshotTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_snapshot",
        description: <crate::builtin_tools::browser_tools::snapshot::BrowserSnapshotTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_navigate",
        description: <crate::builtin_tools::browser_tools::navigate::BrowserNavigateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_tabs",
        description: <crate::builtin_tools::browser_tools::tabs::BrowserTabsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_select",
        description: <crate::builtin_tools::browser_tools::select::BrowserSelectTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_evaluate",
        description: <crate::builtin_tools::browser_tools::evaluate::BrowserEvaluateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_fill_form",
        description: <crate::builtin_tools::browser_tools::fill_form::BrowserFillFormTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_press_key",
        description: <crate::builtin_tools::browser_tools::press_key::BrowserPressKeyTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_wait_for",
        description: <crate::builtin_tools::browser_tools::wait_for::BrowserWaitForTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_batch",
        description: <crate::builtin_tools::browser_tools::batch::BrowserBatchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_console",
        description: <crate::builtin_tools::browser_tools::console::BrowserConsoleTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_hover",
        description: <crate::builtin_tools::browser_tools::hover::BrowserHoverTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_scroll",
        description: <crate::builtin_tools::browser_tools::scroll::BrowserScrollTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_pdf",
        description: <crate::builtin_tools::browser_tools::pdf::BrowserPdfTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_network",
        description: <crate::builtin_tools::browser_tools::network::BrowserNetworkTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_dialog",
        description: <crate::builtin_tools::browser_tools::dialog::BrowserDialogTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_drag",
        description: <crate::builtin_tools::browser_tools::drag::BrowserDragTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_upload",
        description: <crate::builtin_tools::browser_tools::upload::BrowserUploadTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_resize",
        description: <crate::builtin_tools::browser_tools::resize::BrowserResizeTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_emulate",
        description: <crate::builtin_tools::browser_tools::emulate::BrowserEmulateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_cookies",
        description: <crate::builtin_tools::browser_tools::cookies::BrowserCookiesTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_session",
        description: <crate::builtin_tools::browser_tools::session::BrowserSessionTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_profile",
        description: <crate::builtin_tools::browser_tools::profile_tool::BrowserProfileTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    // Media tools — require MediaPipeline
    BuiltinToolDefinition {
        name: "media_understand",
        description: <crate::builtin_tools::media_tools::understand::MediaUnderstandTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "audio_transcribe",
        description: <crate::builtin_tools::media_tools::transcribe::AudioTranscribeTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "document_extract",
        description: <crate::builtin_tools::media_tools::extract::DocumentExtractTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires media_pipeline
    },
    // Aleph Hub tools — require CatalogCache
    BuiltinToolDefinition {
        name: "hub_catalog_search",
        description: <crate::builtin_tools::hub::catalog_search::HubCatalogSearchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_catalog_sync",
        description: <crate::builtin_tools::hub::catalog_sync::HubCatalogSyncTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_resolve_spec",
        description: <crate::builtin_tools::hub::resolve_spec::HubResolveSpecTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_install_run",
        description: <crate::builtin_tools::hub::install_run::HubInstallRunTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires CatalogCache + marketplace configs + vault
    },
    BuiltinToolDefinition {
        name: "hub_install_verify",
        description: <crate::builtin_tools::hub::install_verify::HubInstallVerifyTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires live McpManagerHandle for MCP verification
    },
    BuiltinToolDefinition {
        name: "hub_fetch_docs",
        description: <crate::builtin_tools::hub::fetch_docs::HubFetchDocsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false, // No CatalogCache needed; HTTP-only
    },
    // Team management tools — require TeamStore
    BuiltinToolDefinition {
        name: "team_create",
        description: <crate::builtin_tools::TeamCreateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_delegate",
        description: <crate::builtin_tools::TeamDelegateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_status",
        description: <crate::builtin_tools::TeamStatusTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_disband",
        description: <crate::builtin_tools::TeamDisbandTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_set_protocol",
        description: <crate::builtin_tools::TeamSetProtocolTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_member_add",
        description: <crate::builtin_tools::TeamMemberAddTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_member_remove",
        description: <crate::builtin_tools::TeamMemberRemoveTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_digest",
        description: <crate::builtin_tools::TeamDigestTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_from_template",
        description: <crate::builtin_tools::team::from_template::TeamFromTemplateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_snapshot",
        description: <crate::builtin_tools::team::snapshot::TeamSnapshotTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_usage",
        description: <crate::builtin_tools::team::usage::TeamUsageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_workflow_canvas",
        description: <crate::builtin_tools::team::workflow_canvas::TeamWorkflowCanvasTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Team messaging tools — require MessageRouter / Inbox
    BuiltinToolDefinition {
        name: "message_send",
        description: <crate::builtin_tools::team::message_send::MessageSendTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "plan_submit",
        description: <crate::builtin_tools::team::plan_submit::PlanSubmitTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "plan_resolve",
        description: <crate::builtin_tools::team::plan_resolve::PlanResolveTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "inbox_read",
        description: <crate::builtin_tools::team::inbox_read::InboxReadTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Worker lifecycle tools — require MessageRouter + TeamStore
    BuiltinToolDefinition {
        name: "lifecycle_idle",
        description: <crate::builtin_tools::team::lifecycle_idle::LifecycleIdleTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "lifecycle_request_shutdown",
        description: <crate::builtin_tools::team::lifecycle_request_shutdown::LifecycleRequestShutdownTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "lifecycle_resolve_shutdown",
        description: <crate::builtin_tools::team::lifecycle_resolve_shutdown::LifecycleResolveShutdownTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Task coordination tools — require CoordTaskStore
    BuiltinToolDefinition {
        name: "task_create",
        description: <crate::builtin_tools::task_manage::create::TaskCreateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_update",
        description: <crate::builtin_tools::task_manage::update::TaskUpdateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_list",
        description: <crate::builtin_tools::task_manage::list::TaskListTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_wait",
        description: <crate::builtin_tools::task_manage::wait::TaskWaitTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_comment",
        description: <crate::builtin_tools::team::task_comment::TaskCommentTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_acp_member",
        description: <crate::builtin_tools::team::acp_member::TeamAcpMemberTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "workflow_step_review",
        description: <crate::builtin_tools::team::workflow_step::WorkflowStepReviewTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "workflow",
        // Point at the constant, do not restate it. A hand-written literal here
        // SHADOWS the tool's own `DESCRIPTION` (agent_init builds the model's
        // tool table from this catalog first and then only appends names the
        // catalog lacks), and this one enumerated five of the fifteen actions —
        // so cancel / pause / resume / status / export / import / the proposal
        // family were never advertised to the model at all.
        description: <crate::builtin_tools::workflow_tool::WorkflowTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Task artifact tools — require ArtifactStore
    BuiltinToolDefinition {
        name: "task_submit",
        description: <crate::builtin_tools::team::task_submit::TaskSubmitTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_read_artifact",
        description: <crate::builtin_tools::team::task_read_artifact::TaskReadArtifactTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_review",
        description: <crate::builtin_tools::team::task_review::TaskReviewTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Collaborative session tools — require SessionCoordinator / SessionStore
    BuiltinToolDefinition {
        name: "session_collaborate",
        description: <crate::builtin_tools::team::session_collaborate::SessionCollaborateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_turn",
        description: <crate::builtin_tools::team::session_turn::SessionTurnTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_read",
        description: <crate::builtin_tools::team::session_read::SessionReadTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // Channel management tools — require ChannelRegistry
    BuiltinToolDefinition {
        name: "channel_pairing",
        description: <crate::builtin_tools::channel_manage::ChannelPairingTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true, // Requires ChannelRegistry (deferred injection)
    },
    // Google Meet — thin contract over an out-of-core transport bridge.
    // Always available; reports "bridge not configured" when no bridge is set.
    BuiltinToolDefinition {
        name: "google_meet",
        description: <crate::builtin_tools::google_meet::GoogleMeetTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false, // bridge optional; tool degrades gracefully
    },
    // Media send tool — no dependencies, just passes URLs through to ReplyEmitter
    BuiltinToolDefinition {
        name: "media_send",
        description: <crate::builtin_tools::media_send::MediaSendTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    // Deliverable publisher — needs only the artifact store, which resolves
    // from the data directory at first use.
    BuiltinToolDefinition {
        name: "artifact_publish",
        description: <crate::builtin_tools::artifact_publish::ArtifactPublishTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    // Human-in-the-loop clarification tool — requires ChannelRegistry +
    // ClarificationManager (deferred injection).
    BuiltinToolDefinition {
        name: "ask_user",
        description: <crate::builtin_tools::ask_user::AskUserTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // voice_mode_set is a LLM tool only — NOT a slash command.
    // Use /voice on|off instead. Excluded from BUILTIN_TOOL_DEFINITIONS
    // to avoid appearing in command lists. Because the loop tool list is
    // sourced from BUILTIN_TOOL_DEFINITIONS, the agent-init builder appends
    // it (and `scratchpad`) to the LLM tool list from the registry map so it
    // stays callable — see agent_init/mod.rs "LLM-only builtin tools".

    // Skill management tools — LLM-callable tools for querying and configuring skills
    BuiltinToolDefinition {
        name: "skill_status",
        description: <crate::builtin_tools::skill_status::SkillStatusTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_install",
        description: <crate::builtin_tools::skill_install::SkillInstallTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_manage",
        description: <crate::builtin_tools::skill_manage::SkillManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "note_manage",
        // See the `remember` entry: a literal here shadows the const, and this
        // tool's D4 acknowledgment contract and `destination` receipt pointer
        // live only in the const.
        description: <NoteManageTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // ACP delegate tool — unified delegation to any external CLI agent.
    // Requires AcpAdapterManager; execution returns clear error if harness unavailable.
    BuiltinToolDefinition {
        name: "acp_delegate",
        description: <crate::builtin_tools::acp_tools::AcpDelegateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "acp_switch",
        description: <crate::builtin_tools::acp_tools::AcpSwitchTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    // A2A outbound tools — delegate to / manage remote Agent-to-Agent agents.
    // Require the A2A subsystem ([a2a] enabled); execution returns a clear error otherwise.
    BuiltinToolDefinition {
        name: "a2a_delegate",
        description: <crate::builtin_tools::a2a_tools::A2ADelegateTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "a2a_agents",
        description: <crate::builtin_tools::a2a_tools::A2AAgentsTool as crate::tools::AlephTool>::DESCRIPTION,
        requires_config: true,
    },
];

/// Create a boxed tool instance by name
///
/// This function is used by `AlephToolServer` to create tool instances
/// for tool management and hot-reload capabilities.
///
/// # Arguments
/// * `name` - Tool name (must match `BUILTIN_TOOL_DEFINITIONS`)
/// * `config` - Optional configuration for tools that need it
///
/// # Returns
/// * `Some(tool)` - Boxed tool instance if the tool exists
/// * `None` - If the tool name is unknown or requires missing config
#[must_use]
pub fn create_tool_boxed(
    name: &str,
    config: Option<&BuiltinToolConfig>,
) -> Option<Box<dyn AlephToolDyn>> {
    match name {
        "search" => {
            let tool = if let Some(cfg) = config {
                SearchTool::with_api_key(cfg.tavily_api_key.clone())
            } else {
                SearchTool::new()
            };
            Some(Box::new(tool))
        }
        "web_fetch" => Some(Box::new(WebFetchTool::new())),
        "google_meet" => Some(Box::new(
            crate::builtin_tools::google_meet::GoogleMeetTool::new(
                config.and_then(|c| c.google_meet_bridge.clone()),
            ),
        )),
        "file_ops" => Some(Box::new(FileOpsTool::new())),
        "file_read" => Some(Box::new(FileReadTool::new())),
        "file_write" => Some(Box::new(FileWriteTool::new())),
        "file_edit" => Some(Box::new(FileEditTool::new())),
        "apply_patch" => Some(Box::new(ApplyPatchTool::new())),
        "bash" => Some(Box::new(BashExecTool::new())),
        "code_exec" => Some(Box::new(CodeExecTool::new())),
        "code_check" => Some(Box::new(CodeCheckTool::new())),
        "ctx_search" => Some(Box::new(CtxSearchTool::new())),
        "recall_events" => Some(Box::new(RecallEventsTool::new())),
        "pdf_generate" => Some(Box::new(PdfGenerateTool::new())),
        "image_generate" => {
            if let Some(cfg) = config {
                if let Some(ref registry) = cfg.generation_registry {
                    return Some(Box::new(ImageGenerateTool::new(Arc::clone(registry))));
                }
            }
            None // Requires generation registry
        }
        "skill_list" => Some(Box::new(SkillListTool::default())),
        "read_config_guide" => Some(Box::new(ReadConfigGuideTool::default())),
        "config_audit" => config
            .and_then(|c| c.config.as_ref())
            .map(|cfg| Box::new(ConfigAuditTool::new(Arc::clone(cfg))) as Box<dyn AlephToolDyn>),
        "doctor" => {
            let mut tool = DoctorTool::default();
            if let Some(cfg) = config {
                if let (Some(c), Some(m)) = (&cfg.config, &cfg.shared_token_manager) {
                    tool = tool.with_runtime(Arc::clone(c), Arc::clone(m));
                }
                if let Some(mcp) = &cfg.hub_mcp_handle {
                    tool = tool.with_mcp(mcp.clone());
                }
            }
            Some(Box::new(tool))
        }
        "select_model" => Some(Box::new(SelectModelTool)),
        "self_manage" => Some(Box::new(SelfManageTool::default())),
        "hooks_manage" => Some(Box::new(
            crate::builtin_tools::hooks_manage::HooksManageTool::new(),
        )),
        "desktop" => Some(Box::new(DesktopTool::new())),
        "desktop_ax_query_focused" => Some(Box::new(DesktopAxQueryFocused::new())),
        "desktop_ax_query_tree" => Some(Box::new(DesktopAxQueryTree::new())),
        "desktop_ax_query_by_role" => Some(Box::new(DesktopAxQueryByRole::new())),
        "desktop_ax_snapshot" => Some(Box::new(DesktopAxSnapshot::new())),
        "desktop_som" => Some(Box::new(DesktopSom::new())),
        "desktop_gui_locate" => Some(Box::new(DesktopGuiLocate::new())),
        "desktop_check_permissions" => Some(Box::new(DesktopCheckPermissions::new())),
        "vault_store" => config
            .and_then(|c| c.shared_token_manager.as_ref())
            .map(|mgr| Box::new(VaultStoreTool::new(Arc::clone(mgr))) as Box<dyn AlephToolDyn>),
        // Takes the gateway's own store `Arc` and nothing else — that instance
        // is the one carrying the event bus, so the mutating verbs announce
        // themselves to open Panels. A store opened here would work and stay
        // silent.
        "workspace_manage" => config
            .and_then(|c| c.workspace_manager.as_ref())
            .map(|store| {
                Box::new(
                    crate::builtin_tools::workspace_manage::WorkspaceManageTool::new(Arc::clone(
                        store,
                    )),
                ) as Box<dyn AlephToolDyn>
            }),
        // Sessions tools require gateway_context and caller_agent_id at runtime,
        // so they cannot be created via create_tool_boxed. They are created
        // dynamically in BuiltinToolRegistry::execute_tool().
        "session_list" | "session_send" => None,
        // Session new tool requires SessionManager (from gateway_context) at runtime
        "session_new" => None,
        // Session compact tool requires SessionManager (from gateway_context) at runtime
        "session_compact" => None,
        // Session set-topic tool requires SessionManager (from gateway_context) at runtime
        "session_rename" => None,
        // Session set-mode tool requires SessionManager (from gateway_context) at runtime
        "session_set_mode" => None,
        // Session search tool requires SessionManager at runtime
        "session_search" => None,
        // Remember tool requires MemoryContextProvider (per-agent CuratedMemoryStore)
        // and is built fresh per call from session context — same pattern as session_search.
        "remember" => None,
        // node_invoke requires the gateway NodeRegistry, injected at boot via
        // set_node_registry; built fresh per call — same pattern as remember.
        "node_invoke" => None,
        // node_list / node_invoke_many require the same deferred NodeRegistry.
        "node_list" | "node_invoke_many" => None,
        // node_manage additionally needs the SecurityStore (device records);
        // both are injected at boot, so it is built fresh per call too.
        "node_manage" => None,
        // node_file requires the gateway NodeRegistry, injected at boot via
        // set_node_registry; built fresh per call — same pattern as node_invoke.
        "node_file" => None,
        // agent_identity reads the process-global ledger installed at boot
        // (`identity::install_from`), the same shape as
        // `session::service::global_session_service` — so it needs no injected
        // handle and no deferred-dependency plumbing at all.
        "agent_identity" => Some(Box::new(
            crate::builtin_tools::agent_identity::AgentIdentityTool::new(),
        )),
        // Cron management tool requires SharedCronService at runtime
        "cron_manage" => None,
        // ask_user requires ChannelRegistry + ClarificationManager, injected
        // after construction — built per call in BuiltinToolRegistry.
        "ask_user" => None,
        // Heartbeat management tools require SharedHeartbeatService at runtime
        "heartbeat_list" | "heartbeat_create" | "heartbeat_update" | "heartbeat_delete"
        | "heartbeat_toggle" => None,
        // Heartbeat report tool — always available (no dependencies)
        "heartbeat_report" => Some(Box::new(
            crate::builtin_tools::heartbeat_manage::HeartbeatReportTool,
        )),
        // Agent management tools are created dynamically in
        // BuiltinToolRegistry::with_config() — agent_create/list/delete/switch
        // need agent_registry + workspace_manager; agent_info builds its own
        // catalog.
        "agent_create" | "agent_list" | "agent_delete" | "agent_switch" | "agent_unbind"
        | "agent_update" | "agent_info" => None,
        // self_config requires the per-agent agent_id, injected at construction time
        // in BuiltinToolRegistry — cannot be created standalone here.
        "self_config" => None,
        // moa requires the shared Config handle + ConfigPatcher, injected at
        // boot — constructed in the builder, same pattern as self_config.
        "moa" => None,
        // list_models needs the injected config + vault handles (provider/credential
        // state), bound at BuiltinToolRegistry construction — not standalone here.
        "list_models" => None,
        // Media tools — require MediaPipeline
        "media_understand" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::MediaUnderstandTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "audio_transcribe" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::AudioTranscribeTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "document_extract" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::DocumentExtractTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "media_send" => Some(Box::new(
            crate::builtin_tools::media_send::MediaSendTool::new(),
        )),
        "artifact_publish" => Some(Box::new(
            crate::builtin_tools::artifact_publish::ArtifactPublishTool::new(),
        )),
        // Team management tools require TeamStore at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "team_create"
        | "team_delegate"
        | "team_status"
        | "team_disband"
        | "team_set_protocol"
        | "team_member_add"
        | "team_member_remove"
        | "team_digest"
        | "team_from_template"
        | "team_snapshot"
        | "team_usage"
        | "team_acp_member"
        | "team_workflow_canvas"
        | "workflow_step_review"
        | "workflow"
        | "message_send"
        | "inbox_read"
        | "plan_submit"
        | "plan_resolve"
        | "lifecycle_idle"
        | "lifecycle_request_shutdown"
        | "lifecycle_resolve_shutdown" => None,
        // Task coordination tools require CoordTaskStore at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "task_create" | "task_update" | "task_list" | "task_wait" | "task_comment" => None,
        // Task artifact tools require ArtifactStore + current_agent_id at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "task_submit" | "task_read_artifact" => None,
        "task_review" => None,
        // Session collaboration tools require SessionCoordinator / SessionStore at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "session_collaborate" | "session_turn" | "session_read" => None,
        // Browser tools are intentionally NOT built here (same rationale as
        // goal/loop below): they require the live `ProfileManager` plus the
        // approval policy and vision bridge wired by the BuiltinToolRegistry
        // constructor. This session-less path would produce tools without the
        // approval gate — a security-relevant half-assembly — so they fall
        // through to `None`.
        "browser_open" | "browser_click" | "browser_type" | "browser_screenshot"
        | "browser_snapshot" | "browser_navigate" | "browser_tabs" | "browser_select"
        | "browser_evaluate" | "browser_fill_form" | "browser_press_key" | "browser_wait_for"
        | "browser_batch" | "browser_console" | "browser_hover" | "browser_scroll"
        | "browser_pdf" | "browser_network" | "browser_dialog" | "browser_drag"
        | "browser_upload" | "browser_resize" | "browser_emulate" | "browser_cookies"
        | "browser_session" | "browser_profile" => None,
        // Skill management tools — always available
        // Phase 2: share the process-wide initialized SkillSystem so
        // skill_status/install/manage see the same registry as the gateway.
        "skill_status" => Some(Box::new(
            crate::builtin_tools::skill_status::SkillStatusTool::new(
                crate::skill::shared_skill_system().clone(),
            ),
        )),
        "skill_install" => Some(Box::new(
            crate::builtin_tools::skill_install::SkillInstallTool::new(
                crate::skill::shared_skill_system().clone(),
            ),
        )),
        "skill_manage" => Some(Box::new(
            crate::builtin_tools::skill_manage::SkillManageTool::new(
                crate::skill::shared_skill_system().clone(),
            ),
        )),
        // `goal` and `loop` are intentionally NOT built here: both hard-require a
        // live per-session binding (`GoalTool`/`LoopTool::call` error on an empty
        // session), which this session-less fallback path cannot supply — building
        // them here yields a present-but-100%-broken tool. The live agent loop
        // wires them with a session via the BuiltinToolRegistry builder
        // constructor; an unconfigured name falls through to `_ => None` (skipped),
        // which is strictly better than surfacing an always-erroring tool.
        // Strategy tool — backed by the process-global StrategyStore
        // (init_global at boot). None before boot.
        "strategy" => crate::strategy::global().map(|store| {
            Box::new(crate::builtin_tools::StrategyTool::new(store)) as Box<dyn AlephToolDyn>
        }),
        // Memory lifecycle & knowledge-wiki tools require a memory backend / wiki /
        // profile synthesizer + per-session context — built dynamically in
        // BuiltinToolRegistry::with_config(), same as note_manage below.
        "memory_reflect"
        | "recall_context"
        | "memory_trace"
        | "governance_metrics"
        | "note_orient"
        | "note_schema"
        | "user_profile"
        | "session_complete"
        | "flag_user_correction" => None,
        // note_manage requires memory backend — cannot create standalone fallback
        "note_manage" => None,
        _ => None,
    }
}

/// Get list of all builtin tool names
///
/// This is used for initialization and display purposes.
#[must_use]
pub fn get_builtin_tool_names() -> Vec<String> {
    BUILTIN_TOOL_DEFINITIONS
        .iter()
        .map(|def| def.name.to_string())
        .collect()
}

/// Check if a tool name is a builtin tool
#[allow(dead_code)] // test-only helper
pub fn is_builtin_tool(name: &str) -> bool {
    BUILTIN_TOOL_DEFINITIONS.iter().any(|def| def.name == name)
}

/// Brings the trait consts named by [`REGISTRY_ONLY_DESCRIPTIONS`] into scope
/// exactly the way `builder/core_tools.rs` has them, so the bytes measured
/// resolve to the same const the registration passes.
#[cfg(test)]
use crate::tools::AlephTool as _;

/// Tools the registry constructor registers that this catalog does NOT
/// list — and whose descriptions reach the model exactly as a catalog
/// entry's does.
///
/// `agent_init` builds the model's tool list from `BUILTIN_TOOL_DEFINITIONS`
/// and then completes it from `BuiltinToolRegistry::unified_tools()`, so a
/// `reg(tools, "goal", GoalTool::DESCRIPTION, …)` in
/// `builder/core_tools.rs` ships its full description on every request even
/// though no entry here names it. Registry-only registration is an accepted
/// pattern (see `tool_catalog_init.rs`) — being unmeasured is not.
///
/// By direct const reference, never by literal. This is the same rule
/// `no_catalog_entry_inlines_its_description` enforces on the catalog, for
/// the same reason: a literal here would measure bytes that have stopped
/// being the bytes actually sent, and the ratchet would go on passing.
///
/// Kept honest from both sides by `every_registered_core_tool_is_accounted`:
/// a new `reg(` name that is in neither table fails by name, and an entry
/// here that no longer corresponds to a registration fails too (a stale
/// const inflates the ceiling with bytes nobody sends).
///
/// `pub(crate)` because the byte ratchet is not the only guard that was
/// blind to this surface: `thinker::prompt_contract::no_sentence_is_stated_twice`
/// scans the same shipped text for duplication and ingested only
/// `BUILTIN_TOOL_DEFINITIONS`. It reads this table rather than keeping its
/// own list — a second list is the exact failure this table exists to
/// prevent, one layer up.
#[cfg(test)]
pub(crate) const REGISTRY_ONLY_DESCRIPTIONS: &[(&str, &str)] = &[
    ("pim", crate::builtin_tools::PimTool::DESCRIPTION),
    ("system", crate::builtin_tools::SystemTool::DESCRIPTION),
    (
        "automation",
        crate::builtin_tools::AutomationTool::DESCRIPTION,
    ),
    (
        "permission",
        crate::builtin_tools::PermissionTool::DESCRIPTION,
    ),
    ("media", crate::builtin_tools::MediaTool::DESCRIPTION),
    (
        "scratchpad",
        crate::builtin_tools::ScratchpadTool::DESCRIPTION,
    ),
    ("goal", crate::builtin_tools::GoalTool::DESCRIPTION),
    ("loop", crate::builtin_tools::LoopTool::DESCRIPTION),
    (
        "loop_graph",
        crate::builtin_tools::LoopGraphTool::DESCRIPTION,
    ),
    ("strategy", crate::builtin_tools::StrategyTool::DESCRIPTION),
];

/// Tools the **per-request tool service** appends to the model's list beside
/// the registry snapshot — a third registration shape, measured by neither
/// table above until now.
///
/// `ScopedToolService::list()` builds its definitions from the
/// `LoopToolRegistry` snapshot and then *pushes* extra ones that were attached
/// to the service itself (`with_subagent_tool`). Such a tool is in neither
/// `BUILTIN_TOOL_DEFINITIONS` nor `builder/core_tools.rs`: it never reaches a
/// `reg(` site, so `every_registered_core_tool_is_accounted` cannot see it,
/// and adding it to `REGISTRY_ONLY_DESCRIPTIONS` would be a lie in the other
/// direction — that table's staleness half asserts every entry IS registered
/// in `core_tools.rs`, so the entry would fail as stale on the way in.
///
/// This is the same lesson the 2026-08-10 repointing recorded, arriving for
/// the third time in the same place: *the question is never whether the rule
/// is right, it is how many registration shapes the guard recognises.* The
/// catalog was shape one, `reg(` shape two, and this is shape three.
///
/// Entries are `(wire name, description const, schema constructor)`. By direct
/// const/fn reference, never a literal — for the reason spelled out on
/// `REGISTRY_ONLY_DESCRIPTIONS`: a literal here measures bytes that have
/// stopped being the bytes actually sent, and the ratchet goes on passing.
///
/// Unlike the other two surfaces this one carries its **schema** too, and that
/// is not symmetry for its own sake: `subagent` is in `default_core_tools()`,
/// so progressive disclosure never collapses it and the full schema ships on
/// every request the tool is attached to. See `NON_CATALOG_SCHEMA_CEILING_BYTES`
/// for what that bound does and does not cover.
///
/// Kept honest by `every_injected_tool_is_accounted`, which reads the
/// injection site in `tools/scoped/mod.rs` — at runtime an injected definition
/// and a registry one are the same struct, so the push site is the only
/// witness.
#[cfg(test)]
pub(crate) const INJECTED_TOOL_DESCRIPTIONS: &[(&str, &str, fn() -> serde_json::Value)] = &[(
    crate::agents::subagent_tool::SUBAGENT_TOOL_NAME,
    crate::agents::subagent_tool::SubagentTool::DESCRIPTION,
    crate::agents::subagent_tool::SubagentTool::schema_value,
)];

/// Tools the **MCP bridge** installs straight into the process-wide
/// `ToolHandlerRegistry` — the fourth registration shape.
///
/// `mcp/tool_bridge.rs::sync_builtins` registers these against a capability
/// gate (a connected server actually offering resources / prompts / a
/// non-stdio transport), and `run_loop` snapshots that registry into every
/// request's `LoopToolRegistry`. So they are in no catalog, reach no `reg(`
/// site, and are pushed by no tool service: all three censuses above are
/// structurally blind to them, which is exactly how six tools shipped their
/// descriptions unmeasured.
///
/// Entries are `(wire name, description const, schema constructor)`, by direct
/// const/fn reference for the reason `REGISTRY_ONLY_DESCRIPTIONS` states. The
/// name comes from `tool_bridge`'s own const rather than a literal here, so the
/// census below compares this table against the registration site by VALUE and
/// a rename cannot leave the two agreeing on a name nobody uses.
///
/// These are gated, not unconditional — an install with no MCP server pays none
/// of it. That ranks them below the other three surfaces; it does not make them
/// free, and a ceiling that omitted them would still be reporting a smaller
/// surface than the one being paid for.
#[cfg(test)]
pub(crate) const BRIDGE_TOOL_DESCRIPTIONS: &[(&str, &str, fn() -> serde_json::Value)] = &[
    (
        crate::mcp::tool_bridge::RESOURCE_TOOL,
        crate::builtin_tools::mcp_resource::McpReadResourceTool::DESCRIPTION,
        crate::builtin_tools::mcp_resource::McpReadResourceTool::schema_value,
    ),
    (
        crate::mcp::tool_bridge::RESOURCE_LIST_TOOL,
        crate::builtin_tools::mcp_resource::McpListResourcesTool::DESCRIPTION,
        crate::builtin_tools::mcp_resource::McpListResourcesTool::schema_value,
    ),
    (
        crate::mcp::tool_bridge::RESOURCE_TEMPLATE_LIST_TOOL,
        crate::builtin_tools::mcp_resource::McpListResourceTemplatesTool::DESCRIPTION,
        crate::builtin_tools::mcp_resource::McpListResourceTemplatesTool::schema_value,
    ),
    (
        crate::mcp::tool_bridge::PROMPT_TOOL,
        crate::builtin_tools::mcp_prompt::McpGetPromptTool::DESCRIPTION,
        crate::builtin_tools::mcp_prompt::McpGetPromptTool::schema_value,
    ),
    (
        crate::mcp::tool_bridge::PROMPT_LIST_TOOL,
        crate::builtin_tools::mcp_prompt::McpListPromptsTool::DESCRIPTION,
        crate::builtin_tools::mcp_prompt::McpListPromptsTool::schema_value,
    ),
    (
        crate::mcp::tool_bridge::LOGIN_TOOL,
        crate::builtin_tools::mcp_login::McpLoginTool::DESCRIPTION,
        crate::builtin_tools::mcp_login::McpLoginTool::schema_value,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool the per-request tool service injects is measured.
    ///
    /// Like `every_registered_core_tool_is_accounted`, this has to read the
    /// source: at runtime an injected `ToolDefinition` and a registry-derived
    /// one are the same struct in the same vector, so nothing observable says
    /// "these bytes are outside the ceiling". The push site is the only
    /// witness.
    ///
    /// **The shape it recognises** — and therefore the shape it does not — is
    /// `defs.push(Self::<tool>_definition(…))` in `ScopedToolService`. That is
    /// the mechanism by which a definition joins the model's list without
    /// coming from the registry snapshot. A future injected tool that arrives
    /// by some other form (`defs.extend`, a differently-named constructor) is
    /// outside this scan's field of view; a constructor that merely breaks the
    /// `<tool>_definition` naming convention fails LOUDLY here as unaccounted,
    /// which is the safe direction.
    ///
    /// `describe()` uses the same constructor and is deliberately not an
    /// anchor: a tool reachable only through `describe()` is not in the list
    /// the model is sent, so it costs nothing per request.
    #[test]
    fn every_injected_tool_is_accounted() {
        // CRLF-safe: strip carriage returns before any matching, so a Windows
        // checkout scans the same bytes a Unix one does. (The separator here
        // is not newline-anchored either — see the same note on
        // `every_registered_core_tool_is_accounted`.)
        let src = include_str!("../../tools/scoped/mod.rs").replace('\r', "");

        const OPENER: &str = "defs.push(Self::";
        let openers = src.lines().filter(|l| l.trim().starts_with(OPENER)).count();

        let mut injected: Vec<String> = Vec::new();
        let mut matched = 0usize;
        for line in src.lines().map(str::trim) {
            let Some(rest) = line.strip_prefix(OPENER) else {
                continue;
            };
            let Some(stem) = rest
                .split('(')
                .next()
                .and_then(|ctor| ctor.strip_suffix("_definition"))
            else {
                continue;
            };
            matched += 1;
            if !injected.iter().any(|n| n == stem) {
                injected.push(stem.to_string());
            }
        }

        // Non-vacuity, both halves. A scan that stopped seeing the injection
        // sites certifies nothing, and it would fail SILENTLY — the
        // unaccounted list below would simply come back empty.
        assert_eq!(
            matched, openers,
            "the source scan matched {matched} constructors for {openers} `{OPENER}` sites in \
             tools/scoped/mod.rs — it is no longer reading every injection, so the checks below \
             prove nothing"
        );
        assert!(
            !injected.is_empty(),
            "no injected tool found in tools/scoped/mod.rs — the scan is looking at the wrong \
             shape, and an empty census cannot fail"
        );

        let unaccounted: Vec<&str> = injected
            .iter()
            .map(String::as_str)
            .filter(|stem| !INJECTED_TOOL_DESCRIPTIONS.iter().any(|(n, ..)| n == stem))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "these tools are pushed onto the model's tool list by ScopedToolService but appear \
             in no measured table. Their description ships on every request that attaches them, \
             and their schema too when they are in `default_core_tools()`, so they spend \
             per-request bytes that nothing bounds. Add each to INJECTED_TOOL_DESCRIPTIONS by \
             direct const/fn reference (never a literal), then re-measure both ceilings: \
             {unaccounted:?}"
        );

        let stale: Vec<&str> = INJECTED_TOOL_DESCRIPTIONS
            .iter()
            .map(|(name, ..)| *name)
            .filter(|name| !injected.iter().any(|i| i == name))
            .collect();
        assert!(
            stale.is_empty(),
            "these entries in INJECTED_TOOL_DESCRIPTIONS are no longer pushed in \
             tools/scoped/mod.rs — they charge the ceilings for bytes that no longer ship, which \
             leaves room for real growth to slip under: {stale:?}"
        );

        // A name in two tables is counted twice, and a ceiling that double-counts
        // is that much looser than it reads — the same failure
        // `every_registered_core_tool_is_accounted` checks for on its own pair.
        let doubled: Vec<&str> = INJECTED_TOOL_DESCRIPTIONS
            .iter()
            .map(|(name, ..)| *name)
            .filter(|name| {
                BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == *name)
                    || REGISTRY_ONLY_DESCRIPTIONS.iter().any(|(n, _)| n == name)
            })
            .collect();
        assert!(
            doubled.is_empty(),
            "these tools are in INJECTED_TOOL_DESCRIPTIONS and also in one of the other two \
             measured tables, so the ratchet counts their description twice: {doubled:?}"
        );
    }

    /// No entry may spell its description out here — see the module invariant.
    ///
    /// This has to be a source-level assertion. Nothing at runtime can tell a
    /// description that came from `SomeTool::DESCRIPTION` apart from a literal
    /// that happens to hold the same bytes today, and the failure mode is
    /// precisely that they *stop* holding the same bytes while every test stays
    /// green. `include_str!` is the only witness to which one was written.
    #[test]
    fn no_catalog_entry_inlines_its_description() {
        let src = include_str!("definitions.rs");

        // Scope the scan to the catalog body so a future `description:`
        // field added OUTSIDE the catalog (e.g. a private helper struct)
        // does not make the count drift. The catalog constant's body is
        // bounded by the first `];` after the constant's start; every
        // scan site inside that body is an entry, no more.
        let start = src
            .find("pub const BUILTIN_TOOL_DEFINITIONS")
            .expect("catalog constant present");
        let end = src[start..]
            .find("];")
            .map(|i| start + i)
            .expect("catalog terminator present");
        let catalog_src = &src[start..=end];

        // Scan statements, not lines: rustfmt wraps a long path onto the line
        // after `description:`, and a line-only scan reads that as a value with
        // no const in it. Accumulate from the field name to the terminating
        // comma so both shapes are one site.
        let mut sites: Vec<String> = Vec::new();
        let mut pending: Option<String> = None;
        for line in catalog_src.lines().map(str::trim) {
            match pending.as_mut() {
                Some(acc) => {
                    acc.push(' ');
                    acc.push_str(line);
                }
                None if line.starts_with("description:") => pending = Some(line.to_string()),
                None => continue,
            }
            let done = pending.as_ref().is_some_and(|acc| acc.ends_with(','));
            if done {
                sites.push(pending.take().unwrap_or_default());
            }
        }

        // The scan must see exactly one site per entry. If an entry is ever
        // written in a shape this scan does not match, the count drifts and the
        // offender check below would be certifying a surface that no longer
        // covers the catalog — passing by not looking, which is the failure
        // this whole invariant exists to prevent.
        assert_eq!(
            sites.len(),
            BUILTIN_TOOL_DEFINITIONS.len(),
            "the source scan found {} `description:` sites for {} catalog entries — it is no \
             longer reading every entry, so the check below proves nothing",
            sites.len(),
            BUILTIN_TOOL_DEFINITIONS.len()
        );

        let offenders: Vec<&str> = sites
            .iter()
            .map(String::as_str)
            .filter(|site| !site.contains("DESCRIPTION"))
            .collect();
        assert!(
            offenders.is_empty(),
            "these entries inline their description instead of referencing the tool's own \
             `DESCRIPTION` const. A literal here does not duplicate that const — it replaces \
             it, and the model receives this text instead of anything the tool documents. If \
             the tool has no `DESCRIPTION` const, add one (see `NoteOrientTool`) and point \
             both this catalog and the registry constructor at it:\n{}",
            offenders.join("\n")
        );
    }

    /// Total description bytes the builtin tool surface puts in every request.
    ///
    /// Covers `BUILTIN_TOOL_DEFINITIONS` **plus** `REGISTRY_ONLY_DESCRIPTIONS`
    /// — the model's tool list is assembled from both, so a ceiling over only
    /// the catalog bounds a surface smaller than the one being paid for. The
    /// `CATALOG_` in the name is now narrower than what is measured; it is
    /// kept because the rounds recorded below, `doctor.rs`, and the criteria
    /// list all refer to this constant and its test by name.
    ///
    /// Measured, not computed: `cargo test catalog_description_bytes` prints the
    /// live number on failure. Raising it is a real cost — descriptions ship
    /// with the tool list, and `[tools] core` is non-empty by default while
    /// `truncate_tool_descriptions` is not, so a non-core tool's schema is
    /// collapsed but its **description is sent in full**. There is no mode in
    /// which these bytes are free.
    ///
    /// Before raising, answer the same three questions the harness budget asks:
    /// 1. Is this a runtime fact the model cannot infer (an action name, a
    ///    platform gate, a refusal condition), or is it teaching a strong model
    ///    how to think? The second belongs nowhere.
    /// 2. Would a stronger model still need it? Few-shot example blocks are the
    ///    usual answer of "no" — the schema already carries the parameters, and
    ///    examples become a cage (R9).
    /// 3. Does some other tool already say it? `no_sentence_is_stated_twice`
    ///    catches exact repeats; near-repeats it cannot see are still waste.
    ///
    /// History: 2026-08-04, the sweep that pointed all 155 entries at their
    /// tools' consts. 29,854 B -> 81,274 B, and the increase is the point: 143
    /// of the 155 entries were terse literals totalling 13,508 B that shadowed
    /// 64,866 B of documentation the tools had written and the model had never
    /// been shown. Trimmed in the same pass, under the questions above: the
    /// `desktop` and `bash` few-shot example blocks (-1,756 B), one `bash`
    /// sentence reworded from an instruction into the runtime fact behind it
    /// (-88 B), and four copies of the AX platform-support sentence down to
    /// one (-690 B).
    ///
    /// 2026-08-05, the self-config/doctor round: 81,274 -> 81,270 B. `doctor`
    /// gained four checks (disk space, duplicate daemons, SQLite integrity,
    /// loop-graph) and two arguments (`only` / `skip`) and still came in 4 B
    /// under, by cutting the prose the model can read off the tool's own
    /// output — its findings enumerate every check id — and keeping only what
    /// nothing else says: that the filters exist, and which check is the
    /// expensive one.
    /// 2026-08-06, the memory D1–D4 round-3 pass: 81,270 -> 82,462 B. The D4
    /// acknowledgment contract had only its positive half — "acknowledge after
    /// a successful write" — stated on all three ladder writers, while every
    /// one of them can settle WITHOUT writing and still return a successful
    /// tool result (over-budget hot zone, spent retry budget, a correction
    /// already on record). The model had no instruction for the case where the
    /// user asked for something durable and the system declined, so it either
    /// reported a save that never happened or went quiet. Against the three
    /// questions: (1) the refusal SHAPE is a runtime fact — which field says
    /// nothing landed, and that it is not an error — not a lesson in how to
    /// think; (2) a stronger model still cannot infer that
    /// `flag_user_correction` keys its duplicate check on severity, so
    /// escalation stays open; (3) `no_sentence_is_stated_twice` covers the
    /// catalog and passes. Paid for in part by cutting `remember`'s one-line
    /// copy of the destination ladder (-167 B): the authoritative ladder ships
    /// in `MemoryProtocolLayer` on every non-Minimal request, and a second
    /// abbreviated copy in a tool is exactly the near-repeat question 3 asks
    /// about.
    ///
    /// 2026-08-10, two events on the same day, both additive to this ceiling:
    ///
    /// First the measurement was repointed: 82,462 -> 93,358 B.
    /// **The increase is not new spending — it is bytes that were already
    /// being sent and were not being counted.** Ten tools — `pim`, `system`,
    /// `automation`, `permission`, `media`, `scratchpad`, `goal`, `loop`,
    /// `loop_graph`, `strategy` — are registered only by
    /// `builder/core_tools.rs` and have never had a catalog entry. Their
    /// descriptions have shipped to the model on every request the whole time
    /// (`agent_init` completes the tool list from the registry map), but the
    /// ceiling summed the catalog alone, so it bounded ~85% of the surface
    /// while reading as the whole of it — the guard underpinning "DESCRIPTION
    /// bytes are precious" was handing out clean bills of health on 13,389 B
    /// it could not see. Nothing was spent to make that number appear; what
    /// changed is that the next tool to grow one of those ten now has to
    /// answer the three questions above like every other tool.
    ///
    /// The catalog half went the other way in the same pass, 82,462 ->
    /// 79,969 B. -2,419 of that is `code_exec`, which restated six things
    /// `bash` documents in its own words — statelessness, `working_dir`,
    /// `timeout`/exit 124, output caps and head-tail elision, signal exit
    /// codes, the escalation/justification contract — and had already
    /// conceded the overlap in prose ("See `bash` for the exact reuse and
    /// narrowing policy"). Both tools are in `default_core_tools()` and
    /// neither is deferred in any session mode, so every one of those bytes
    /// went out twice per request; `code_exec` now carries its language table
    /// and one pointer sentence. Its `Examples:` block went with them under
    /// question 2 — the `bash` and `desktop` example blocks were cut for
    /// exactly that reason on 2026-08-04 and `code_exec` was simply missed.
    /// The remaining -74 B are unrelated same-day trims to `bash` and the
    /// file tools that happened to land in the same measurement.
    ///
    /// Then `workspace_manage` (R8, the conversational face of
    /// `workspace.*`, the same day's remote main) added its catalog entry:
    /// catalog 79,969 -> 80,549 B (+580 B — its own entry is 519 B, the rest
    /// the same round's workflow-tool description adjustment). Against the three
    /// questions: (1) what survived is runtime fact only — that a workspace id
    /// IS an agent id and names its memory partition, that the record cannot
    /// change model/tools/prompt (so the model does not promise a config
    /// change it did not make), that `create` is not `agent_create`, and that
    /// `archive` is soft and reversible with the id still taken (so it does
    /// not report a deletion that did not happen); (2) a stronger model cannot
    /// derive any of those from the name or the schema — they are facts about
    /// this deployment's data model; (3) the first draft was 778 B and
    /// everything the argument schema already carries was cut from it — that
    /// `update` is a patch, that `include_archived` widens `list`, what each
    /// field means — since the schema ships to any caller that has promoted
    /// the tool, and a description is not the place to say it a second time.
    ///
    /// Merged state after both halves of the day: 93,938 B — 80,549 B
    /// catalog + 13,389 B registry-only.
    ///
    /// Then `tool_usage` (2026-08-10, the R8 face of the extension invocation
    /// sidecar): catalog 80,549 -> 80,917 B, +368 B. Against the three
    /// questions: (1) two runtime facts survive, and only two — that a `—`
    /// count is a *different claim* from `0` (a plugin shipping only hooks has
    /// no tool-call channel to measure, so reading its blank as "unused" is how
    /// a live plugin gets deleted), and which three tools actually perform a
    /// removal, since this one deliberately performs none; (2) neither is
    /// derivable by a stronger model — the first is a property of this store's
    /// accounting, the second a fact about which surfaces own which registry;
    /// (3) the first draft was 575 B and everything the schema already ships
    /// was cut — `scope`/`idle_days` semantics live in their `#[schemars]`
    /// descriptions, and the "answer is partial" caveat was cut because the
    /// runtime `summary` string states it in the response itself, which is
    /// strictly better than paying for it on every request that merely lists
    /// the tool.
    /// 2026-08-11, the third registration shape: 94,306 -> 95,333 B. As with
    /// the first half of 2026-08-10, **the increase is not new spending** — it
    /// is bytes that were already going out and were never counted. `subagent`
    /// (1,039 B) reaches the model through neither surface this ceiling summed:
    /// it has no catalog entry and no `reg(` site, because the per-request
    /// `ScopedToolService` *pushes* it onto the list it hands the model
    /// (`with_subagent_tool`). `every_registered_core_tool_is_accounted` reads
    /// `reg(` sites, so it was structurally incapable of naming this tool, and
    /// `REGISTRY_ONLY_DESCRIPTIONS` could not host it either — that table's
    /// staleness half asserts every entry IS registered in `core_tools.rs`, so
    /// the entry would have failed on the way in. Hence a third table
    /// (`INJECTED_TOOL_DESCRIPTIONS`) and a third census
    /// (`every_injected_tool_is_accounted`).
    ///
    /// The lesson is the one the 2026-08-10 entry recorded, arriving one shape
    /// later in the same place: the question a census answers is never "is the
    /// rule right" but "how many registration shapes does it recognise". Asking
    /// it once more, in the same sitting, turned up a FOURTH — the six tools
    /// `mcp/tool_bridge.rs` installs directly into the `ToolHandlerRegistry`
    /// `run_loop` snapshots. They are measured in the same pass
    /// (`BRIDGE_TOOL_DESCRIPTIONS`, +1,944 B) rather than recorded as a known
    /// gap, because a guard that certifies three of four shapes is the failure
    /// this ledger keeps re-recording: it hands a clean bill of health to the
    /// half still unmeasured. Unlike the other three these are capability-gated
    /// — an install with no MCP server pays none of it — which ranks them
    /// lower, not free.
    ///
    /// Two book-keeping notes, so neither number is absorbed in silence:
    /// * the catalog half measured 80,905 B here, 12 B under the 80,917 B the
    ///   `tool_usage` entry above recorded. Unrelated trims landed in between;
    ///   the slack is stated rather than left as headroom that reads like it
    ///   was always there.
    /// * catalog and registry-only descriptions ship on EVERY request, while
    ///   this one ships only where the agent has `subagent` attached — so the
    ///   ceiling bounds a slight over-estimate for an agent without it. That is
    ///   the conservative direction, and the only one under which a single
    ///   number stays meaningful.
    ///
    /// Same day, the fourth shape measured: 95,333 -> 97,277 B (+1,944 B, the
    /// six MCP bridge tools). Not new spending either — see the paragraph
    /// above for why they were invisible and why they were closed in the same
    /// pass rather than logged as a gap.
    const CATALOG_DESCRIPTION_CEILING_BYTES: usize = 97_277;

    #[test]
    fn catalog_description_bytes_ratchet() {
        let catalog: usize = BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| def.description.len())
            .sum();
        let registry_only: usize = REGISTRY_ONLY_DESCRIPTIONS
            .iter()
            .map(|(_, desc)| desc.len())
            .sum();
        let injected: usize = INJECTED_TOOL_DESCRIPTIONS
            .iter()
            .map(|(_, desc, _)| desc.len())
            .sum();
        let bridge: usize = BRIDGE_TOOL_DESCRIPTIONS
            .iter()
            .map(|(_, desc, _)| desc.len())
            .sum();
        let total = catalog + registry_only + injected + bridge;

        let mut largest: Vec<(&str, usize)> = BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| (def.name, def.description.len()))
            .chain(
                REGISTRY_ONLY_DESCRIPTIONS
                    .iter()
                    .map(|(name, desc)| (*name, desc.len())),
            )
            .chain(
                INJECTED_TOOL_DESCRIPTIONS
                    .iter()
                    .map(|(name, desc, _)| (*name, desc.len())),
            )
            .chain(
                BRIDGE_TOOL_DESCRIPTIONS
                    .iter()
                    .map(|(name, desc, _)| (*name, desc.len())),
            )
            .collect();
        largest.sort_by_key(|(_, len)| std::cmp::Reverse(*len));
        largest.truncate(5);

        assert!(
            total <= CATALOG_DESCRIPTION_CEILING_BYTES,
            "builtin tool descriptions total {total} B ({catalog} B catalog + {registry_only} B \
             registry-only + {injected} B injected + {bridge} B bridge), over the ceiling of \
             {CATALOG_DESCRIPTION_CEILING_BYTES} B. These \
             bytes ship in every request that lists these tools. Largest: {largest:?}. Answer \
             the three questions documented on CATALOG_DESCRIPTION_CEILING_BYTES before \
             raising it."
        );

        // A ceiling over an empty measurement is not a ceiling. If either half
        // ever stops carrying descriptions, this guard must fail loudly rather
        // than certify the remainder as the whole surface — that is exactly how
        // it spent its life bounding the catalog alone.
        assert!(
            catalog > 10_000,
            "catalog descriptions measured only {catalog} B — the guard is no longer reading \
             the descriptions it exists to bound"
        );
        assert!(
            registry_only > 10_000,
            "registry-only descriptions measured only {registry_only} B — the guard has lost \
             sight of the tools that reach the model without a catalog entry"
        );
        // The floor here is only `> 0`, and deliberately so: this half is one
        // tool, so any number that reads like a size would be a magic constant
        // guessing at `subagent`'s length. What it has to catch is what the
        // other two floors catch — the half going silent, which is how a
        // census stops bounding a surface while still passing.
        assert!(
            injected > 0,
            "injected descriptions measured 0 B — the guard has lost sight of the tools the \
             per-request tool service pushes onto the model's list"
        );
        assert!(
            bridge > 0,
            "bridge descriptions measured 0 B — the guard has lost sight of the tools the MCP \
             bridge installs directly into the registry run_loop snapshots"
        );
    }

    /// Schema bytes of the per-request injected surface.
    ///
    /// **Scope, stated plainly so this is not read as more than it is:** this
    /// bounds the schemas of the two NON-CATALOG shapes —
    /// `INJECTED_TOOL_DESCRIPTIONS` and `BRIDGE_TOOL_DESCRIPTIONS` — and
    /// nothing else. Catalog and registry-only tool schemas are NOT measured
    /// here or anywhere: `BuiltinToolDefinition` carries no schema field, so
    /// reaching them means constructing every tool. That is a real remaining
    /// gap, not a claim this constant quietly covers, and it is why the name
    /// says `NON_CATALOG_` instead of `SCHEMA_`.
    ///
    /// Why these two surfaces earn a schema bound when the catalog has none:
    /// they are reachable without an instance (a `fn() -> Value` per entry),
    /// and their tools are core — `subagent` and five of the six bridge tools
    /// are in `default_core_tools()`, so progressive disclosure never collapses
    /// them, and `truncate_tool_descriptions` does not apply to schemas at all.
    /// These bytes go out in full. Both are also surfaces with no catalog entry
    /// to review, so schema growth here is exactly the growth nobody sees.
    ///
    /// Measured, not computed: the failure prints the live number.
    ///
    /// History: 2026-08-11, first measurement — 8,834 B: `subagent` 6,584 B
    /// plus 2,250 B across the six MCP bridge tools (301–418 B each). The round
    /// that added `subagent`'s per-call `context` / `fork_turns` arguments grew
    /// that schema by ~0.6 KB and no guard in the repository could observe it;
    /// that is what put the third registration shape on the map at all, and
    /// asking the same question once more turned up the fourth.
    ///
    /// For scale, `subagent`'s schema alone outweighs every tool DESCRIPTION in
    /// the repository except `desktop`'s. Nothing is trimmed here — this round
    /// bought the ability to see these bytes; cutting argument prose is a
    /// separate judgement against the three questions, not a measurement
    /// change.
    const NON_CATALOG_SCHEMA_CEILING_BYTES: usize = 8_834;

    #[test]
    fn non_catalog_tool_schema_bytes_ratchet() {
        let mut sizes: Vec<(&str, usize)> = INJECTED_TOOL_DESCRIPTIONS
            .iter()
            .chain(BRIDGE_TOOL_DESCRIPTIONS.iter())
            .map(|(name, _, schema)| (*name, schema().to_string().len()))
            .collect();
        let total: usize = sizes.iter().map(|(_, len)| len).sum();
        sizes.sort_by_key(|(_, len)| std::cmp::Reverse(*len));

        assert!(
            total <= NON_CATALOG_SCHEMA_CEILING_BYTES,
            "non-catalog tool schemas total {total} B, over the ceiling of \
             {NON_CATALOG_SCHEMA_CEILING_BYTES} B. Per tool: {sizes:?}. These ship uncollapsed on \
             every request that attaches the tool — an argument's `description` here costs the \
             same as one in the tool's own DESCRIPTION. Answer the three questions documented \
             on CATALOG_DESCRIPTION_CEILING_BYTES before raising it."
        );

        // A ceiling over an empty measurement is not a ceiling — the same
        // non-vacuity the description ratchet learned to assert. `to_string()`
        // on a `Value::Null` is 4 bytes, so a schema constructor that lost its
        // body would still register; the floor is what catches it.
        assert!(
            total > 100,
            "non-catalog tool schemas measured only {total} B — the guard is no longer reading \
             the \
             schemas it exists to bound"
        );
    }

    /// Schema bytes carried by the registry map — the catalog half and the
    /// registry-only half at once.
    ///
    /// One number covers both because the catalog has no schema of its own.
    /// `agent_init` builds the model's tool list from `BUILTIN_TOOL_DEFINITIONS`
    /// for name and description, then attaches parameters from
    /// `tool_registry.get_tool_schema(def.name)` — a lookup of
    /// `UnifiedTool.parameters_schema` — and completes the tail from that same
    /// map. So "the catalog entry's schema" IS a registry-map entry, and a
    /// catalog tool absent from the map ships with no parameters at all.
    ///
    /// **What this bounds, precisely.** The map as built with NOTHING wired:
    /// `register_core_tools` plus `register_optional_tools` with every
    /// dependency `None` and a default config. That is the unconditional
    /// subset — the schemas that go out in every deployment regardless of what
    /// is configured — and it is deterministic, which a ceiling has to be.
    ///
    /// **What it does not bound**, stated here rather than left to be
    /// discovered: tools the constructor registers only when their dependency
    /// is live (memory, generation, cron/heartbeat, teams, desktop) carry
    /// schemas this number does not contain. They are enumerated by
    /// `tools_without_an_unconditional_schema_are_pinned` rather than waved at,
    /// so the residue is a list someone can shorten, not a vague caveat.
    ///
    /// Note the asymmetry with descriptions: `truncate_tool_descriptions`
    /// defaults to false so every description ships in full, while a non-core
    /// tool's schema IS collapsed by progressive disclosure. This ceiling is
    /// therefore an upper bound — what a `[tools] core = ["*"]` install pays,
    /// and what any install pays for its core set.
    ///
    /// History: 2026-08-11, first measurement — 90,215 B across 50 tools.
    /// Until this round no schema in the repository was measured at all; the
    /// `subagent` round exposed the gap, and the catalog and registry halves
    /// turned out to be reachable after all — not through the catalog table,
    /// but through the same registration functions production calls.
    ///
    /// The number is the finding. 90,215 B of schema sits beside 97,277 B of
    /// description: the argument schemas are very nearly a SECOND copy of the
    /// tool surface, and they were the unmeasured half the whole time. Largest
    /// on first measurement: `desktop` 17,014 B — one tool, more than a sixth
    /// of the total, and about 1.5x its own (already largest) description —
    /// then `loop_graph` 6,368, `goal` 4,985, `scratchpad` 3,891, `self_config`
    /// 3,553. Nothing is trimmed in this round; measuring and cutting are
    /// separate acts, and cutting an argument description is a judgement about
    /// what the model can still call correctly without it.
    const REGISTRY_SCHEMA_CEILING_BYTES: usize = 90_215;

    /// The tool map with nothing wired — the deterministic half of what the
    /// constructor builds.
    ///
    /// Calls the same two registration functions the constructor does, in the
    /// same order, so the schemas measured are byte-for-byte the ones
    /// `get_tool_schema` would hand `agent_init`. Re-deriving them from
    /// `schemars` here instead would measure a second opinion about what ships.
    fn unconditional_registry_map(
    ) -> std::collections::HashMap<String, crate::tool_metadata::UnifiedTool> {
        let mut tools = std::collections::HashMap::new();
        crate::executor::builtin_registry::BuiltinToolRegistry::register_core_tools(&mut tools);
        crate::executor::builtin_registry::BuiltinToolRegistry::register_optional_tools(
            &mut tools,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &crate::executor::builtin_registry::BuiltinToolConfig::default(),
            crate::config::types::memory::MemoryInjectionMode::default(),
            &None,
            &None,
            &None,
        );
        tools
    }

    #[test]
    fn registry_schema_bytes_ratchet() {
        let map = unconditional_registry_map();
        let mut sizes: Vec<(&str, usize)> = map
            .iter()
            .filter_map(|(name, tool)| {
                tool.parameters_schema
                    .as_ref()
                    .map(|schema| (name.as_str(), schema.to_string().len()))
            })
            .collect();
        let total: usize = sizes.iter().map(|(_, len)| len).sum();
        sizes.sort_by_key(|(name, len)| (std::cmp::Reverse(*len), *name));
        let largest: Vec<(&str, usize)> = sizes.iter().take(5).copied().collect();

        assert!(
            total <= REGISTRY_SCHEMA_CEILING_BYTES,
            "registry tool schemas total {total} B across {} tools, over the ceiling of \
             {REGISTRY_SCHEMA_CEILING_BYTES} B. Largest: {largest:?}. A `#[schemars(description \
             = ...)]` on an argument costs exactly what a sentence in the tool's DESCRIPTION \
             costs, and unlike a description it is easy to add without noticing. Answer the \
             three questions documented on CATALOG_DESCRIPTION_CEILING_BYTES before raising it.",
            sizes.len()
        );

        // A ceiling over an empty measurement is not a ceiling — the same
        // non-vacuity every other ratchet in this module asserts. If the two
        // registration calls above ever stop populating the map, this must fail
        // loudly rather than certify zero bytes as a clean bill of health.
        assert!(
            total > 10_000,
            "registry schemas measured only {total} B across {} tools — the guard is no longer \
             reading the map it exists to bound",
            sizes.len()
        );
    }

    /// The tools whose schema this module cannot reach without wiring a
    /// dependency, pinned so the residue cannot grow in silence.
    ///
    /// A guard that measured the reachable schemas and said nothing about the
    /// rest would be handing out the same clean bill of health that let three
    /// registration shapes ship unmeasured. This one names them.
    ///
    /// A ratchet, not an equality: shrinking the list is good and needs no
    /// ceremony, growing it means a new tool ships a schema nothing bounds and
    /// should cost a deliberate edit here. Lower the number when you improve
    /// it, or the slack left behind is room for the next one to slip back in.
    ///
    /// 2026-08-11: 125. Most are conditionally registered (memory, browser,
    /// desktop AX, teams/tasks, hub, cron/heartbeat, generation), so in a wired
    /// deployment they DO carry schemas — the residue is a statement about what
    /// a deterministic test can reach, not about what production sends. That is
    /// exactly why it is written down instead of implied.
    #[test]
    fn tools_without_an_unconditional_schema_are_pinned() {
        let map = unconditional_registry_map();
        let mut missing: Vec<&str> = BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| def.name)
            .chain(REGISTRY_ONLY_DESCRIPTIONS.iter().map(|(name, _)| *name))
            .filter(|name| {
                !map.get(*name)
                    .is_some_and(|tool| tool.parameters_schema.is_some())
            })
            .collect();
        missing.sort_unstable();
        missing.dedup();

        assert!(
            missing.len() <= 125,
            "{} tools have no schema in the unconditionally-built registry map, up from the 125 \
             recorded here, so `registry_schema_bytes_ratchet` does not bound them. Either a \
             tool ships with no parameters at all (free, and fine), or it registers only once a \
             dependency is live and its schema is unmeasured (not fine, just not cheap to fix). \
             Raise this pin deliberately: {missing:?}",
            missing.len()
        );
    }

    /// Every tool the MCP bridge installs is measured.
    ///
    /// The fourth shape's witness is `mcp/tool_bridge.rs`'s own name consts:
    /// registering a bridge tool means declaring one, and `sync_builtins` keys
    /// every install off it. Compared by VALUE, not by identifier — the table
    /// references those same consts, so a rename that touched only one side
    /// would otherwise leave both agreeing on a name the wire never sees.
    ///
    /// Fails BY NAME in both directions, like its three siblings: a seventh
    /// bridge tool is named as unaccounted, and an entry whose const has gone
    /// away is named as stale (it would keep charging the ceilings for bytes
    /// nobody sends).
    #[test]
    fn every_bridge_tool_is_accounted() {
        // CRLF-safe, and the separator is not line-anchored — see the same note
        // on `every_registered_core_tool_is_accounted`.
        let src = include_str!("../../mcp/tool_bridge.rs").replace('\r', "");

        const MARKER: &str = "_TOOL: &str = ";
        let declarations = src
            .lines()
            .filter(|l| l.trim_start().starts_with("pub(crate) const") && l.contains(MARKER))
            .count();

        let registered: Vec<&str> = src
            .lines()
            .filter(|l| l.trim_start().starts_with("pub(crate) const"))
            .filter_map(|l| l.split_once(MARKER))
            .filter_map(|(_, rest)| rest.trim().strip_prefix('"'))
            .filter_map(|rest| rest.split('"').next())
            .collect();

        // Non-vacuity in both directions. A const whose value the scan cannot
        // read (say it is spelled `= SomeTool::NAME` one day) must fail here
        // rather than drop quietly out of the census — an unreadable
        // registration is not an absent one.
        assert_eq!(
            registered.len(),
            declarations,
            "read {} names from {declarations} `{MARKER}` declarations in mcp/tool_bridge.rs — \
             a declaration this scan cannot parse is a tool it cannot hold to account, so the \
             checks below prove nothing. Found: {registered:?}",
            registered.len()
        );
        assert!(
            !registered.is_empty(),
            "no bridge tool names found in mcp/tool_bridge.rs — the scan is looking at the \
             wrong shape, and an empty census cannot fail"
        );

        let unaccounted: Vec<&str> = registered
            .iter()
            .copied()
            .filter(|name| !BRIDGE_TOOL_DESCRIPTIONS.iter().any(|(n, ..)| n == name))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "these tools are installed into the MCP tool registry by tool_bridge.rs but appear \
             in no measured table. `run_loop` snapshots that registry into every request, so \
             their description — and their schema, since five of the six are in \
             `default_core_tools()` and never collapse — ship unbounded whenever their \
             capability gate is open. Add each to BRIDGE_TOOL_DESCRIPTIONS by direct const/fn \
             reference (never a literal), then re-measure both ceilings: {unaccounted:?}"
        );

        let stale: Vec<&str> = BRIDGE_TOOL_DESCRIPTIONS
            .iter()
            .map(|(name, ..)| *name)
            .filter(|name| !registered.contains(name))
            .collect();
        assert!(
            stale.is_empty(),
            "these entries in BRIDGE_TOOL_DESCRIPTIONS no longer correspond to a name const in \
             tool_bridge.rs — they charge the ceilings for bytes that no longer ship, which \
             leaves room for real growth to slip under: {stale:?}"
        );

        let doubled: Vec<&str> = BRIDGE_TOOL_DESCRIPTIONS
            .iter()
            .map(|(name, ..)| *name)
            .filter(|name| {
                BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == *name)
                    || REGISTRY_ONLY_DESCRIPTIONS.iter().any(|(n, _)| n == name)
                    || INJECTED_TOOL_DESCRIPTIONS.iter().any(|(n, ..)| n == name)
            })
            .collect();
        assert!(
            doubled.is_empty(),
            "these tools are in BRIDGE_TOOL_DESCRIPTIONS and also in another measured table, \
             so the ratchet counts them twice and both ceilings are that much looser than they \
             read: {doubled:?}"
        );
    }

    /// Every tool the core registry registers is measured by the ratchet.
    ///
    /// This has to read the source. At runtime a registry-only tool and a
    /// catalogued one are indistinguishable — both are just entries in the
    /// `unified_tools()` map — so nothing observable says "these bytes are
    /// outside the ceiling". The registration site is the only witness.
    ///
    /// Fails BY NAME in both directions: an eleventh registry-only tool is
    /// named as unaccounted, and an accounted entry whose registration has
    /// gone away is named as stale (it would otherwise keep charging the
    /// ceiling for bytes nobody sends). A name in BOTH tables is also a
    /// failure — the ratchet would count it twice.
    #[test]
    fn every_registered_core_tool_is_accounted() {
        // CRLF-safe: strip carriage returns before any matching, so a Windows
        // checkout scans the same bytes a Unix one does.
        let src = include_str!("builder/core_tools.rs").replace('\r', "");

        // The registrations are `reg(` on its own line, then `tools,`, then
        // the name literal. Take the first string literal after each opener.
        let mut registered: Vec<String> = Vec::new();
        let mut awaiting_name = false;
        for line in src.lines().map(str::trim) {
            if line == "reg(" {
                awaiting_name = true;
                continue;
            }
            if awaiting_name {
                if let Some(rest) = line.strip_prefix('"') {
                    if let Some(name) = rest.split('"').next() {
                        registered.push(name.to_string());
                        awaiting_name = false;
                    }
                }
            }
        }

        // Non-vacuity: every opener must have yielded a name. If rustfmt ever
        // collapses a `reg(...)` onto one line the scan silently stops seeing
        // it, and a census that cannot see a registration certifies nothing.
        let openers = src.lines().filter(|l| l.trim() == "reg(").count();
        assert_eq!(
            registered.len(),
            openers,
            "the source scan matched {} names for {} `reg(` sites in core_tools.rs — it is no \
             longer reading every registration, so the checks below prove nothing",
            registered.len(),
            openers
        );
        assert!(
            registered.len() > 20,
            "only {} registrations found in core_tools.rs — the scan is looking at the wrong \
             shape",
            registered.len()
        );

        let catalogued = |name: &str| BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == name);
        let accounted = |name: &str| REGISTRY_ONLY_DESCRIPTIONS.iter().any(|(n, _)| *n == name);

        let unaccounted: Vec<&str> = registered
            .iter()
            .map(String::as_str)
            .filter(|name| !catalogued(name) && !accounted(name))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "these tools are registered in core_tools.rs but appear in neither \
             BUILTIN_TOOL_DEFINITIONS nor REGISTRY_ONLY_DESCRIPTIONS. Their descriptions still \
             reach the model — agent_init completes the tool list from the registry map — so \
             they are spending per-request bytes that nothing measures. Add each to \
             REGISTRY_ONLY_DESCRIPTIONS by direct const reference (never a literal), then \
             re-measure the ceiling: {unaccounted:?}"
        );

        let stale: Vec<&str> = REGISTRY_ONLY_DESCRIPTIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !registered.iter().any(|r| r == name))
            .collect();
        assert!(
            stale.is_empty(),
            "these entries in REGISTRY_ONLY_DESCRIPTIONS are no longer registered in \
             core_tools.rs — they charge the ceiling for bytes that no longer ship, which \
             leaves room for real growth to slip under it: {stale:?}"
        );

        let doubled: Vec<&str> = REGISTRY_ONLY_DESCRIPTIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| catalogued(name))
            .collect();
        assert!(
            doubled.is_empty(),
            "these tools are in BOTH BUILTIN_TOOL_DEFINITIONS and REGISTRY_ONLY_DESCRIPTIONS, \
             so the ratchet counts their description twice and the ceiling is that much \
             looser than it reads: {doubled:?}"
        );
    }

    #[test]
    fn strategy_tool_is_listed_in_a_group() {
        // The `strategy` builtin must be discoverable via the category groups
        // (same surface as goal/loop), or the LLM never sees it.
        let listed = crate::executor::builtin_registry::groups::TOOL_CATEGORIES
            .iter()
            .any(|cat| cat.tools.contains(&"strategy"));
        assert!(listed, "strategy tool must appear in a tool category group");
    }

    #[test]
    fn test_all_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify core tools
        assert!(names.contains(&"search".to_string()));
        assert!(names.contains(&"web_fetch".to_string()));
        assert!(names.contains(&"file_ops".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"code_exec".to_string()));
        assert!(names.contains(&"pdf_generate".to_string()));
        assert!(names.contains(&"image_generate".to_string()));
        assert!(names.contains(&"skill_list".to_string()));
        assert!(names.contains(&"read_config_guide".to_string()));
        assert!(names.contains(&"vault_store".to_string()));

        // Verify browser tools
        assert!(names.contains(&"browser_open".to_string()));
        assert!(names.contains(&"browser_click".to_string()));
        assert!(names.contains(&"browser_type".to_string()));
        assert!(names.contains(&"browser_screenshot".to_string()));
        assert!(names.contains(&"browser_snapshot".to_string()));
        assert!(names.contains(&"browser_navigate".to_string()));
        assert!(names.contains(&"browser_tabs".to_string()));
        assert!(names.contains(&"browser_select".to_string()));
        assert!(names.contains(&"browser_evaluate".to_string()));
        assert!(names.contains(&"browser_fill_form".to_string()));
        assert!(names.contains(&"browser_press_key".to_string()));
        assert!(names.contains(&"browser_wait_for".to_string()));
        assert!(names.contains(&"browser_batch".to_string()));
        assert!(names.contains(&"browser_console".to_string()));
        assert!(names.contains(&"browser_hover".to_string()));
        assert!(names.contains(&"browser_scroll".to_string()));
        assert!(names.contains(&"browser_pdf".to_string()));
        assert!(names.contains(&"browser_network".to_string()));
        assert!(names.contains(&"browser_dialog".to_string()));
        assert!(names.contains(&"browser_profile".to_string()));
    }

    #[test]
    fn test_sessions_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify sessions tools are defined when gateway feature is enabled
        assert!(names.contains(&"session_list".to_string()));
        assert!(names.contains(&"session_send".to_string()));
    }

    #[test]
    fn test_sessions_tools_require_config() {
        // Sessions tools require gateway_context (dynamic creation)
        assert!(create_tool_boxed("session_list", None).is_none());
        assert!(create_tool_boxed("session_send", None).is_none());
    }

    #[test]
    fn test_is_builtin_tool() {
        assert!(is_builtin_tool("bash"));
        assert!(is_builtin_tool("code_exec"));
        assert!(is_builtin_tool("file_ops"));
        assert!(!is_builtin_tool("unknown_tool"));
        assert!(!is_builtin_tool("mcp:filesystem"));
    }

    #[test]
    fn test_is_builtin_tool_sessions() {
        assert!(is_builtin_tool("session_list"));
        assert!(is_builtin_tool("session_send"));
    }

    #[test]
    fn test_create_tool_boxed() {
        // Test creating basic tools without config
        assert!(create_tool_boxed("bash", None).is_some());
        assert!(create_tool_boxed("code_exec", None).is_some());
        assert!(create_tool_boxed("file_ops", None).is_some());

        // Test unknown tool
        assert!(create_tool_boxed("unknown", None).is_none());

        // Test tool requiring config (should return None without config)
        assert!(create_tool_boxed("image_generate", None).is_none());

        // Test browser tools (require the live wiring from the registry
        // constructor — see the browser_* arm above — so None here)
        assert!(create_tool_boxed("browser_open", None).is_none());
        assert!(create_tool_boxed("browser_click", None).is_none());
        assert!(create_tool_boxed("browser_type", None).is_none());
        assert!(create_tool_boxed("browser_screenshot", None).is_none());
        assert!(create_tool_boxed("browser_snapshot", None).is_none());
        assert!(create_tool_boxed("browser_navigate", None).is_none());
        assert!(create_tool_boxed("browser_tabs", None).is_none());
        assert!(create_tool_boxed("browser_select", None).is_none());
        assert!(create_tool_boxed("browser_evaluate", None).is_none());
        assert!(create_tool_boxed("browser_fill_form", None).is_none());
        assert!(create_tool_boxed("browser_press_key", None).is_none());
        assert!(create_tool_boxed("browser_wait_for", None).is_none());
        assert!(create_tool_boxed("browser_batch", None).is_none());
        assert!(create_tool_boxed("browser_console", None).is_none());
        assert!(create_tool_boxed("browser_profile", None).is_none());
    }

    #[test]
    fn test_tool_definitions_consistency() {
        // Verify all definitions have non-empty names and descriptions
        for def in BUILTIN_TOOL_DEFINITIONS {
            assert!(!def.name.is_empty(), "Tool name cannot be empty");
            assert!(
                !def.description.is_empty(),
                "Tool description cannot be empty"
            );
        }
    }
}

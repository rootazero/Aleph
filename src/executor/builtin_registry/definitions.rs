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
        description: "Search the internet using Tavily API",
        requires_config: false, // Optional API key
    },
    BuiltinToolDefinition {
        name: "web_fetch",
        description: "Fetch and read content from a URL",
        requires_config: false,
    },
    // File-tool descriptions are the canonical `AlephTool::DESCRIPTION` consts —
    // the same rich usage guidance the tools document themselves. This is the
    // LLM-facing list (agent_init maps `BUILTIN_TOOL_DEFINITIONS` straight into
    // the model's tool list), so referencing the consts both delivers that
    // guidance to the model (R9 — intelligence lives in the prompt) and keeps a
    // single source of truth instead of a terse literal that silently drifts.
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
        description: "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "code_exec",
        description: "Execute code in various programming languages (Python, JavaScript, Shell)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "code_check",
        description: "Run the project's type-checker/linter (auto-detected: cargo/tsc/go/ruff) and return structured diagnostics",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "ctx_search",
        description: "BM25-search large tool outputs that were offloaded out of the context window; retrieve only the relevant sections instead of re-reading whole files",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "recall_events",
        description: "BM25-search this session's own event timeline (tool calls, results, errors, messages) that compaction dropped from context; restore continuity by retrieving only the relevant past events",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "pdf_generate",
        description: "Generate PDF documents from text/Markdown",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "image_generate",
        description: "Generate images from text prompts",
        requires_config: true, // Requires generation registry
    },
    BuiltinToolDefinition {
        name: "skill_list",
        description: "List all installed skills",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_read",
        description: "Read the full instructions of an installed skill. Call this before executing any skill-based task.",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "gateway_route",
        description: "Query Aleph's routing engine to determine which agent and session a message would be routed to. Returns the target agent, session key, and how the match was made — a deterministic channel→agent lookup, not intent classification.",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop",
        description: "Control the desktop via platform-native capabilities: screenshots, OCR, keyboard/mouse, app launch, windows, and screen recording",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_focused",
        description: "Return the UI element currently holding keyboard focus via the OS accessibility API (macOS)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_tree",
        description: "Return the AX element tree for a process (frontmost if pid omitted); bounded by max_depth (default 6)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_query_by_role",
        description: "Collect all AX elements whose role matches `role` (e.g. \"AXButton\") in a process",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_ax_snapshot",
        description: "Snapshot an app's interactable UI as a flat indexed element list with pre-computed click centers (set-of-marks GUI targeting)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_som",
        description: "Capture the screen with every clickable element outlined and numbered (visual set-of-marks); returns the annotated image plus an indexed element list with ready-to-click centers",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_gui_locate",
        description: "Resolve a human-readable on-screen target (e.g. \"Send\", \"Login button\") into clickable pixel coordinates via AX tree fuzzy match + OCR fallback",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop_check_permissions",
        description: "Check macOS TCC permission status for the kinds Aleph needs (accessibility, input monitoring, screen recording, camera, microphone)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "read_config_guide",
        description: "Get Aleph configuration manual for self-management operations",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "config_audit",
        description: "Audit the live security posture (SSRF, sandbox, shell safety, PII filtering) and return structured findings — read-only",
        requires_config: true, // Requires the live Config handle
    },
    BuiltinToolDefinition {
        name: "doctor",
        description: "Self-diagnose runtime health (data dir, instance lock, config parse, hook consent, live provider connectivity) with structured findings; fix=true applies safe deterministic repairs — read-only by default; also use it to verify your repairs",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "select_model",
        description: "Switch the LLM model for the rest of this conversation (larger context, vision, reasoning, or cheaper chat); applies from the next turn",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "list_models",
        description: "Discover switchable LLM models with their context window, vision/tool/reasoning support, and price per million tokens — pair with select_model to choose on capability/cost grounds",
        requires_config: true, // Reads injected config + vault for provider/credential state
    },
    BuiltinToolDefinition {
        name: "self_manage",
        description: "Enter self-management mode when user wants to configure, modify, or fix Aleph",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "hooks_manage",
        description: "Inspect and edit event hooks; action='list' reports why a hook cannot fire (dead matcher, observer-only event, consent still pending)",
        // Reads the process-global extension manager, not injected config.
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "self_config",
        description: "Read/write Aleph identity files and config.toml with validation and natural-language preview; route_status surfaces live provider health (circuit breakers, cooldowns, load)",
        requires_config: true, // Requires per-agent agent_id (injected at construction)
    },
    BuiltinToolDefinition {
        name: "moa",
        description: "Mixture-of-Agents advisory mode: parallel advisor models consult on the live conversation and feed private guidance to the acting aggregator; manage per-session activation and presets",
        requires_config: true, // needs injected config + patcher handles
    },
    BuiltinToolDefinition {
        name: "vault_store",
        description: "Manage encrypted secret vault (store/delete/list API keys)",
        requires_config: true, // Requires SharedTokenManager
    },
    BuiltinToolDefinition {
        name: "memory_search",
        description: "Search personal memory for relevant facts and conversation history with workspace-scoped retrieval",
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_browse",
        description: "Browse personal memory via hierarchical VFS navigation (ls, read, glob on aleph:// paths)",
        requires_config: true, // Requires memory_db
    },
    BuiltinToolDefinition {
        name: "memory_explore",
        description: "Explore related knowledge by following semantic connections from a starting query across multiple hops",
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_timeline",
        description: "View the complete lifecycle of a memory fact — creation, modification, decay, invalidation timeline",
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
        description: "List the online cluster nodes (remote execution arms): id, name, declared commands, tags, connected-at. Optionally filter by tags (AND match) to preview which nodes a node_invoke_many fan-out would hit.",
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_invoke",
        description: "Run a command on a connected cluster node (a remote execution arm). Address the node by name or id; the command must be one the node declares (e.g. \"bash\"), and `args` is that command's JSON payload passed through verbatim.",
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_invoke_many",
        description: "Fan a command out concurrently to every online cluster node carrying ALL the given tags (empty tags = all online nodes). Per-node results are aggregated; one node's failure doesn't stop the others.",
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    BuiltinToolDefinition {
        name: "node_manage",
        description: "Change cluster membership: enroll a node slot by name (idempotent, returns its node_id and the command to run on that machine), or deregister a node so it is evicted now and refused if it reconnects. Fleet management by conversation; `node_list` reads the fleet, this writes it.",
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
        description: "Transfer a file between the center and a connected cluster node by path (push/pull). Bytes move host-to-host over the cluster channel and never enter the conversation; 8 MB cap; the node must declare file.read/file.write.",
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
    // Memory lifecycle & knowledge-wiki tools — require a memory backend / wiki /
    // profile synthesizer; created dynamically in BuiltinToolRegistry::with_config().
    BuiltinToolDefinition {
        name: "memory_reflect",
        description: "Synthesise a distilled answer from long-term memory with cited note paths (vs memory_search's raw hits)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "recall_context",
        description: "Retrieve pre-compression conversation details — specific code, error messages, or decisions from earlier in the conversation",
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
        description: "Interrogate the long-term memory knowledge graph (read-only): `schema` introspection (categories, edge relation-types, totals), N-hop `neighbors`, `community` members, and top `related` peers.",
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
        name: "note_orient",
        description: "Fetch a compact orientation snapshot of the memory wiki: SCHEMA, index, and recent log entries",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "note_schema",
        description: "Read or write SCHEMA.md, the file describing the structure of the agent's long-term memory wiki",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "user_profile",
        description: "Read the current user profile (interests, preferences, context) or view its revision history",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_complete",
        description: "Signal that a self-contained task has completed, triggering a memory retrospective for future similar tasks",
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
        description: "List sessions accessible to this agent for cross-session communication",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_send",
        description: "Send messages to other sessions (same or different agent)",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_new",
        description: "Start a new conversation session, closing the current one",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_compact",
        description: "Compact the current conversation: summarize the older turns, keep the recent ones",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_rename",
        description: "Rename the current session's topic/title",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_set_mode",
        description: "Switch this session's usage mode (chat / work / code)",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_search",
        description: "Search past conversation transcripts across all sessions using full-text search",
        requires_config: true, // Requires SessionManager
    },
    BuiltinToolDefinition {
        name: "cron_manage",
        description: "Manage scheduled tasks — create, list, delete, enable/disable cron jobs",
        requires_config: true, // Requires SharedCronService
    },
    // Heartbeat management tools — require SharedHeartbeatService
    BuiltinToolDefinition {
        name: "heartbeat_list",
        description: "List all heartbeat monitoring tasks",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_create",
        description: "Create a new heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_update",
        description: "Update an existing heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_delete",
        description: "Delete a heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_toggle",
        description: "Enable or disable a heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    // Heartbeat report tool — always available, used during L2 heartbeat execution
    BuiltinToolDefinition {
        name: "heartbeat_report",
        description: "Report results of a heartbeat monitoring analysis (silent or notify user)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "agent_create",
        description: "Create a new agent with an isolated workspace and register it for use",
        requires_config: true, // Requires agent_registry + workspace_manager
    },

    BuiltinToolDefinition {
        name: "agent_list",
        description: "List all registered agents and show which is active for the current session",
        requires_config: true, // Requires agent_registry
    },
    BuiltinToolDefinition {
        name: "agent_delete",
        description: "Delete an agent and archive its workspace (cannot delete 'main')",
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_switch",
        description: "Switch the active agent bound to the current channel to another existing agent",
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_info",
        description: "Get detailed capabilities and configuration of a registered agent (allowed/denied tools, iteration limits, context mode, usage hints)",
        requires_config: false, // Always available — builds its own agent-definition catalog
    },
    // Browser tools — always available, share a ProfileManager
    BuiltinToolDefinition {
        name: "browser_open",
        description: "Open URL in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_click",
        description: "Click or double-click element in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_type",
        description: "Type text in browser element",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_screenshot",
        description: "Capture browser screenshot",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_snapshot",
        description: "Get browser ARIA accessibility tree",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_navigate",
        description: "Navigate browser back/forward/refresh",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_tabs",
        description: "List, switch, or close browser tabs",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_select",
        description: "Select dropdown option in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_evaluate",
        description: "Execute JavaScript in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_fill_form",
        description: "Fill multiple form fields in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_press_key",
        description: "Press a keyboard key in the browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_wait_for",
        description: "Wait for specific text to appear on the page",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_console",
        description: "Read browser console messages for debugging",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_hover",
        description: "Hover the pointer over an element in the browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_scroll",
        description: "Scroll the browser viewport up/down/left/right",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_pdf",
        description: "Print the current browser page to a PDF file",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_network",
        description: "Read the browser network request log for debugging",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_dialog",
        description: "Respond to a native browser dialog (alert/confirm/prompt)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_drag",
        description: "Drag one element onto another in the browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_upload",
        description: "Attach local files to a file input in the browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_resize",
        description: "Resize the browser viewport",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_emulate",
        description: "Emulate color scheme, geolocation, network/CPU throttling, HTTP headers, or user-agent on the active tab",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_cookies",
        description: "List, get, set, delete, or clear cookies in the managed browser session",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_session",
        description: "Save or restore a browser login session (cookies + localStorage) by name",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_profile",
        description: "List and manage browser profiles",
        requires_config: false,
    },
    // Media tools — require MediaPipeline
    BuiltinToolDefinition {
        name: "media_understand",
        description: "Understand media content (images, audio, video, documents) with auto-detection and multi-provider fallback",
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "audio_transcribe",
        description: "Transcribe audio files to text with language detection",
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "document_extract",
        description: "Extract text and structured data from documents",
        requires_config: true, // Requires media_pipeline
    },
    // Aleph Hub tools — require CatalogCache
    BuiltinToolDefinition {
        name: "hub_catalog_search",
        description: "Search or browse the Aleph Hub catalog of installable extensions; returns the entry_id that hub_resolve_spec and hub_install_run require, plus installed / update-available / config / consent state per hit.",
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_catalog_sync",
        description: "Refresh the local cache from the published Aleph Hub catalog. Keeps the last-good cache on failure.",
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_resolve_spec",
        description: "Resolve the install spec for a catalog entry by its id from the local catalog cache.",
        requires_config: true, // Requires CatalogCache
    },
    BuiltinToolDefinition {
        name: "hub_install_run",
        description: "Install a catalog entry by id (trust-gated). Clean specs install directly; ack-required specs bounce to the user for consent via the Extensions UI; OCI is rejected.",
        requires_config: true, // Requires CatalogCache + marketplace configs + vault
    },
    BuiltinToolDefinition {
        name: "hub_install_verify",
        description: "Verify that a just-installed extension is healthy. For MCP servers: checks the server is running and exposes ≥1 tool. For plugins and skills: checks the artifact is present on disk.",
        requires_config: true, // Requires live McpManagerHandle for MCP verification
    },
    BuiltinToolDefinition {
        name: "hub_fetch_docs",
        description: "Fetch a text document (README / manifest) over HTTP with SSRF protection and a 64 KiB cap, and scan it for prompt-injection patterns before returning it.",
        requires_config: false, // No CatalogCache needed; HTTP-only
    },
    // Team management tools — require TeamStore
    BuiltinToolDefinition {
        name: "team_create",
        description: "Create a new team with the calling agent as leader, enrolling existing or inline-created members",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_delegate",
        description: "Delegate a task to a team member, execute it, and return the result",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_status",
        description: "Query the current state of a team, including members and task history",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_disband",
        description: "Mark a team as disbanded (preserved for history, cannot be undone)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_set_protocol",
        description: "Set or clear a team's operating protocol (role definitions, hand-off rules, quality standards) injected into every member's launch context. Pass an empty protocol to clear.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_member_add",
        description: "Add a member to an existing team (leader only). Accepts native agent IDs OR ACP harness refs like 'acp:claude-code' or 'acp:codex/backend' to bring external CLI agents (Claude Code / Codex / Gemini CLI / …) into the shared team session.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_member_remove",
        description: "Remove a member from a team (leader only, cannot remove self)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_digest",
        description: "Generate a summary of recent team activity for the specified time period",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_from_template",
        description: "Materialize a team from a TOML blueprint in one shot — leader + workers + initial task DAG. Use `teams.list_templates` RPC to discover available templates.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_snapshot",
        description: "Manage team snapshots — create / list / get / restore (dry-run by default) / delete. Restore is conservative: InProgress tasks are never clobbered.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_usage",
        description: "Aggregate LLM provider token usage for a team over an optional time window. Returns the team total plus a per-agent breakdown. Cost is not computed — tokens are factual, cost is a rate-card concern.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_workflow_canvas",
        description: "Convert between a team's CoordTask DAG and an Obsidian-compatible JSON Canvas document. action='export' renders the team's tasks as a canvas; action='import' materializes a canvas into coord-tasks (dry_run=true previews without writing).",
        requires_config: true,
    },
    // Team messaging tools — require MessageRouter / Inbox
    BuiltinToolDefinition {
        name: "message_send",
        description: "Send a message to team members with to/cc routing",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "plan_submit",
        description: "Submit a plan for team-leader approval before starting significant work",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "plan_resolve",
        description: "Approve or reject a plan submitted via plan_submit (team leader only)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "inbox_read",
        description: "Read inbox messages or a full thread. Use mode='inbox' (default) to read your messages, mode='thread' with thread_id to read a conversation thread.",
        requires_config: true,
    },
    // Worker lifecycle tools — require MessageRouter + TeamStore
    BuiltinToolDefinition {
        name: "lifecycle_idle",
        description: "Report that this worker is idle and awaiting work. Sends an `idle` message to the team leader.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "lifecycle_request_shutdown",
        description: "Request the team leader's permission to terminate this worker. Pair with lifecycle_resolve_shutdown.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "lifecycle_resolve_shutdown",
        description: "Approve or reject a shutdown request from a worker. Sets decision to 'approve' or 'reject'.",
        requires_config: true,
    },
    // Task coordination tools — require CoordTaskStore
    BuiltinToolDefinition {
        name: "task_create",
        description: "Create a coordination task with optional dependencies and team assignment",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_update",
        description: "Update a coordination task's status, owner, result, or metadata",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_list",
        description: "List coordination tasks with optional filtering by team, status, or owner",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_wait",
        description: "Wait for specific tasks or all team tasks to complete",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_comment",
        description: "Append a free-text handoff note to a coordination task — survives retries and is visible in the kanban drawer",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_acp_member",
        description: "Attach / detach / list external coding CLI sessions (Claude Code, Codex, Gemini CLI) as ACP-backed team members",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "workflow_step_review",
        description: "Approve / reject / retry / skip a single workflow step. Lead-agent step control between dependent tasks (openteams parity).",
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
        description: "Submit a structured artifact as task output",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_read_artifact",
        description: "Read artifacts submitted for a task",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_review",
        description: "Leader accepts/rejects a member's submitted task (approve→completed, reject→in_progress)",
        requires_config: true,
    },
    // Collaborative session tools — require SessionCoordinator / SessionStore
    BuiltinToolDefinition {
        name: "session_collaborate",
        description: "Start a collaborative session between team members for real-time multi-turn discussion",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_turn",
        description: "Respond in a collaborative session or propose its conclusion",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_read",
        description: "Read a collaborative session's transcript, status, and outcome",
        requires_config: true,
    },
    // Channel management tools — require ChannelRegistry
    BuiltinToolDefinition {
        name: "channel_pairing",
        description: "Manage channel pairing codes — generate new codes or list active ones for Telegram/other channels",
        requires_config: true, // Requires ChannelRegistry (deferred injection)
    },
    // Google Meet — thin contract over an out-of-core transport bridge.
    // Always available; reports "bridge not configured" when no bridge is set.
    BuiltinToolDefinition {
        name: "google_meet",
        description: "Join, create, leave, speak into, or query a Google Meet call via the configured transport bridge",
        requires_config: false, // bridge optional; tool degrades gracefully
    },
    // Media send tool — no dependencies, just passes URLs through to ReplyEmitter
    BuiltinToolDefinition {
        name: "media_send",
        description: "Send media files (images, videos, audio) directly to the user in the chat",
        requires_config: false,
    },
    // Deliverable publisher — needs only the artifact store, which resolves
    // from the data directory at first use.
    BuiltinToolDefinition {
        name: "artifact_publish",
        description: "Publish the finished work product (report, analysis, plan) as a standalone document that opens in the user's browser",
        requires_config: false,
    },
    // Human-in-the-loop clarification tool — requires ChannelRegistry +
    // ClarificationManager (deferred injection).
    BuiltinToolDefinition {
        name: "ask_user",
        description: "Ask the user a clarifying question and wait for their reply before continuing — use instead of guessing when the task is ambiguous or a required detail is missing",
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
        description: "Query skill system status — list all skills with readiness, missing deps, and install options",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_install",
        description: "Install missing dependencies for a skill",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_manage",
        description: "Configure, author, and curate skills — enable/disable, change scope, create/edit/patch skills, write supporting files, pin/archive/delete",
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
        description: "Delegate a task to an external CLI agent via ACP. Use 'claude-code', 'codex', or 'gemini' as the harness parameter, or any custom harness registered via acp.create.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "acp_switch",
        description: "Switch to direct conversation with an external CLI agent (Claude Code, Codex, or Gemini), or switch back to Aleph.",
        requires_config: true,
    },
    // A2A outbound tools — delegate to / manage remote Agent-to-Agent agents.
    // Require the A2A subsystem ([a2a] enabled); execution returns a clear error otherwise.
    BuiltinToolDefinition {
        name: "a2a_delegate",
        description: "Delegate a task to a remote agent over the A2A (Agent-to-Agent) protocol.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "a2a_agents",
        description: "List, add, or remove the remote A2A agents Aleph can delegate to.",
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
        "agent_create" | "agent_list" | "agent_delete" | "agent_switch" | "agent_info" => None,
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
        | "browser_console" | "browser_hover" | "browser_scroll" | "browser_pdf"
        | "browser_network" | "browser_dialog" | "browser_drag" | "browser_upload"
        | "browser_resize" | "browser_emulate" | "browser_cookies" | "browser_session"
        | "browser_profile" => None,
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

#[cfg(test)]
mod tests {
    use super::*;

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

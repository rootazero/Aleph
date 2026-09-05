//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via `SearXNG`
//! - [`WebFetchTool`] - Web page fetching
//! - [`FileOpsTool`] - File system operations (list, read, write, move, copy, delete, mkdir, search)
//! - [`CodeExecTool`] - Code execution (Python, JavaScript, Shell)
//! - [`PdfGenerateTool`] - PDF generation from text/Markdown
//! - [`ImageGenerateTool`] - Image generation from text prompts
//! - [`SpeechGenerateTool`] - Text-to-speech generation
//!
//! # Tool Progress Notifications
//!
//! `notify_tool_start` / `notify_tool_result` / `notify_tool_streaming_chunk`
//! are kept as no-op stubs so the ~85 callers in this module still compile,
//! but no live consumer of any callback machinery exists in this tree — the
//! earlier "stream progress to UI" design now flows through the gateway
//! event bus instead. Replace these stubs with direct `event_bus.emit(...)`
//! calls if a real consumer ever reappears.

pub mod a2a_tools;
pub mod acp_tools;
pub mod acting_agent;
pub mod agent_identity;
pub mod agent_manage;
pub mod artifact_publish;
pub mod ask_user;
pub mod automation_tool;
pub mod bash_exec;
// `src/browser/` lives at the crate root (see `lib.rs`); it is NOT a
// `builtin_tools` sub-module. Earlier revisions of this file declared
// `pub mod browser;` here, and the old "deleted; Task 13 recreates with
// text-first design" note was stale — the module has since been recreated
// at the crate root and is consumed by `builtin_tools/browser_tools/*`,
// `gateway/handlers/browser_config.rs`, `tools/probes/browser.rs`,
// `diagnostics/checks/browser_runtime.rs`, and the executor's tool
// registry. Re-declaring it here would just be a duplicate declaration.
pub mod browser_tools;
pub mod canvas;
pub mod channel_directory;
pub mod channel_manage;
pub mod channel_message;
pub mod channel_outbox;
pub mod code_check;
pub mod code_exec;
pub mod command_canonicalize;
pub mod command_ledger;
pub mod config_audit;
pub mod config_guide;
pub mod crawl4ai;
pub mod cron_manage;
pub mod ctx_search;
pub mod desktop;
pub mod doctor;
pub mod error;
pub mod file_ops;
pub mod file_search;
pub mod flag_user_correction;
pub mod gateway_route;
pub mod generation;
pub mod goal;
pub mod google_meet;
pub mod governance_metrics;
pub mod heartbeat_manage;
pub mod hooks_manage;
pub mod hub;
pub mod list_models;
pub mod loop_graph_manage;
pub mod loop_manage;
pub mod mcp_login;
pub mod mcp_prompt;
pub mod mcp_resource;
pub mod media_send;
pub mod media_tool;
pub mod media_tools;
pub mod memory_browse;
pub mod memory_explore;
pub mod memory_reflect;
pub mod memory_search;
pub mod memory_timeline;
pub mod memory_trace;
pub mod meta_tools;
pub mod moa_manage;
pub mod node_file;
pub mod node_invoke;
pub mod node_invoke_many;
pub mod node_list;
pub mod node_manage;
pub mod note_graph_query;
pub mod note_manage;
pub mod note_orient;
pub mod note_schema;
pub mod partial_output;
pub mod pdf_generate;
pub mod permission_tool;
pub mod pim;
pub mod plugin_manage;
pub mod process_completion;
pub mod process_journal;
pub mod process_registry;
pub mod project_manage;
pub mod recall_context;
pub mod recall_events;
pub mod remember;
pub mod scratchpad;
pub mod scratchpad_registry;
pub mod search;
pub mod select_model;
pub mod self_config;
pub mod self_manage;
pub mod session_complete;
pub mod session_search;
pub mod sessions;
pub mod skill_install;
pub mod skill_manage;
pub mod skill_reader;
pub mod skill_status;
pub mod strategy_manage;
pub mod system_tool;
pub mod task_manage;
pub mod team;
pub mod terminal;
pub mod tool_usage;
pub mod user_profile;
pub mod vault_store;
pub mod voice_tools;
pub mod web_fetch;
pub mod workflow_tool;
pub mod workspace_manage;

pub use a2a_tools::{
    new_a2a_tool_handle, A2AAgentsArgs, A2AAgentsOutput, A2AAgentsTool, A2ADelegateArgs,
    A2ADelegateOutput, A2ADelegateTool, A2AToolDeps, A2AToolHandle,
};
pub use acp_tools::{
    AcpDelegateArgs, AcpDelegateOutput, AcpDelegateTool, AcpSwitchArgs, AcpSwitchOutput,
    AcpSwitchTool,
};
pub use agent_identity::{AgentIdentityArgs, AgentIdentityTool};
pub use agent_manage::{
    generate_agent_id_from_name, validate_agent_id, AgentCreateArgs, AgentCreateOutput,
    AgentCreateTool, AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool, AgentInfoArgs,
    AgentInfoOutput, AgentInfoTool, AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool,
    AgentManageContext, AgentManageError, AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool,
    AgentUnbindArgs, AgentUnbindOutput, AgentUnbindTool, AgentUpdateArgs, AgentUpdateOutput,
    AgentUpdateTool,
};
pub use automation_tool::{AutomationArgs, AutomationOutput, AutomationTool};
pub use bash_exec::{BashExecArgs, BashExecTool};
pub use browser_tools::{
    BrowserClickArgs, BrowserClickOutput, BrowserClickTool, BrowserConsoleArgs,
    BrowserConsoleOutput, BrowserConsoleTool, BrowserCookiesArgs, BrowserCookiesOutput,
    BrowserCookiesTool, BrowserDialogArgs, BrowserDialogOutput, BrowserDialogTool, BrowserDragArgs,
    BrowserDragOutput, BrowserDragTool, BrowserEmulateArgs, BrowserEmulateOutput,
    BrowserEmulateTool, BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool,
    BrowserExecArgs, BrowserExecOutput, BrowserExecTool, BrowserFillFormArgs,
    BrowserFillFormOutput, BrowserFillFormTool, BrowserHoverArgs, BrowserHoverOutput,
    BrowserHoverTool, BrowserNavigateArgs, BrowserNavigateOutput, BrowserNavigateTool,
    BrowserNetworkArgs, BrowserNetworkOutput, BrowserNetworkTool, BrowserOpenArgs,
    BrowserOpenOutput, BrowserOpenTool, BrowserPdfArgs, BrowserPdfOutput, BrowserPdfTool,
    BrowserPressKeyArgs, BrowserPressKeyOutput, BrowserPressKeyTool, BrowserProfileArgs,
    BrowserProfileOutput, BrowserProfileTool, BrowserResizeArgs, BrowserResizeOutput,
    BrowserResizeTool, BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool,
    BrowserScrollArgs, BrowserScrollOutput, BrowserScrollTool, BrowserSelectArgs,
    BrowserSelectOutput, BrowserSelectTool, BrowserSessionArgs, BrowserSessionOutput,
    BrowserSessionTool, BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool,
    BrowserTabsArgs, BrowserTabsOutput, BrowserTabsTool, BrowserTypeArgs, BrowserTypeOutput,
    BrowserTypeTool, BrowserUploadArgs, BrowserUploadOutput, BrowserUploadTool, BrowserWaitForArgs,
    BrowserWaitForOutput, BrowserWaitForTool,
};
pub use canvas::{CanvasTool, CanvasToolAction, CanvasToolArgs};
pub use channel_directory::{
    ChannelDirectoryArgs, ChannelDirectoryEntry, ChannelDirectoryOutput, ChannelDirectoryTool,
};
pub use channel_manage::{
    ChannelPairingArgs, ChannelPairingOutput, ChannelPairingTool, PairingAction,
};
pub use channel_message::{
    ChannelMessageAction, ChannelMessageArgs, ChannelMessageOutput, ChannelMessageTool,
};
pub use channel_outbox::{
    ChannelOutboxArgs, ChannelOutboxOutput, ChannelOutboxTool, DeadLetterEntry, OutboxAction,
};
pub use code_check::{CodeCheckArgs, CodeCheckOutput, CodeCheckTool};
pub use code_exec::{CodeExecArgs, CodeExecTool};
pub use config_audit::{ConfigAuditArgs, ConfigAuditOutput, ConfigAuditTool};
pub use config_guide::{
    GuideTopic, ReadConfigGuideArgs, ReadConfigGuideOutput, ReadConfigGuideTool,
};
pub use cron_manage::{CronAction, CronManageArgs, CronManageOutput, CronManageTool};
pub use ctx_search::{CtxSearchArgs, CtxSearchOutput, CtxSearchTool};
pub use desktop::{
    DesktopArgs, DesktopAxQueryByRole, DesktopAxQueryByRoleArgs, DesktopAxQueryFocused,
    DesktopAxQueryFocusedArgs, DesktopAxQueryTree, DesktopAxQueryTreeArgs, DesktopAxSnapshot,
    DesktopAxSnapshotArgs, DesktopCheckPermissions, DesktopCheckPermissionsArgs, DesktopGuiLocate,
    DesktopGuiLocateArgs, DesktopOutput, DesktopSom, DesktopSomArgs, DesktopTool,
};
pub use doctor::{DoctorArgs, DoctorOutput, DoctorTool};
pub use error::ToolError;
pub use file_ops::{
    ApplyPatchArgs, ApplyPatchOutput, ApplyPatchTool, FileEditTool, FileOpsArgs, FileOpsTool,
    FileReadTool, FileWriteTool,
};
pub use file_search::{FindArgs, FindOutput, FindTool, GrepArgs, GrepOutput, GrepTool};
pub use flag_user_correction::{
    FlagUserCorrectionArgs, FlagUserCorrectionOutput, FlagUserCorrectionTool,
};
pub use gateway_route::{GatewayRouteArgs, GatewayRouteOutput, GatewayRouteTool};
pub use generation::{
    AudioGenerateArgs, AudioGenerateOutput, AudioGenerateTool, ImageGenerateArgs,
    ImageGenerateTool, SpeechGenerateArgs, SpeechGenerateTool, VideoGenerateArgs,
    VideoGenerateOutput, VideoGenerateTool,
};
pub use goal::{GoalAction, GoalArgs, GoalOutput, GoalTool};
pub use google_meet::{
    GoogleMeetAction, GoogleMeetArgs, GoogleMeetBridge, GoogleMeetMode, GoogleMeetOutput,
    GoogleMeetTool, GoogleMeetTransport,
};
pub use heartbeat_manage::{
    HeartbeatCreateArgs, HeartbeatCreateOutput, HeartbeatCreateTool, HeartbeatDeleteArgs,
    HeartbeatDeleteOutput, HeartbeatDeleteTool, HeartbeatListArgs, HeartbeatListOutput,
    HeartbeatListTool, HeartbeatReportAction, HeartbeatReportArgs, HeartbeatReportOutput,
    HeartbeatReportTool, HeartbeatToggleArgs, HeartbeatToggleOutput, HeartbeatToggleTool,
    HeartbeatUpdateArgs, HeartbeatUpdateOutput, HeartbeatUpdateTool,
};
pub use hooks_manage::{HooksAction, HooksManageArgs, HooksManageOutput, HooksManageTool};
pub use hub::*;
pub use list_models::{ListModelsArgs, ListModelsOutput, ListModelsTool};
pub use loop_graph_manage::{LoopGraphAction, LoopGraphArgs, LoopGraphOutput, LoopGraphTool};
pub use loop_manage::{LoopAction, LoopArgs, LoopOutput, LoopTool};
pub use mcp_login::{McpLoginArgs, McpLoginOutput, McpLoginTool};
pub use mcp_prompt::{
    McpGetPromptArgs, McpGetPromptOutput, McpGetPromptTool, McpListPromptsArgs,
    McpListPromptsOutput, McpListPromptsTool, McpPromptEntry, PromptOutputMessage,
};
pub use mcp_resource::{
    McpListResourcesArgs, McpListResourcesOutput, McpListResourcesTool, McpReadResourceArgs,
    McpReadResourceOutput, McpReadResourceTool, McpResourceEntry,
};
pub use media_tool::{MediaArgs, MediaOutput, MediaTool};
pub use media_tools::{
    AudioTranscribeArgs, AudioTranscribeOutput, AudioTranscribeTool, DocumentExtractArgs,
    DocumentExtractOutput, DocumentExtractTool, MediaUnderstandArgs, MediaUnderstandOutput,
    MediaUnderstandTool,
};
pub use memory_browse::{MemoryBrowseArgs, MemoryBrowseOutput, MemoryBrowseTool};
pub use memory_explore::{MemoryExploreArgs, MemoryExploreOutput, MemoryExploreTool};
pub use memory_reflect::{MemoryReflectArgs, MemoryReflectResult, MemoryReflectTool};
pub use memory_search::{MemorySearchArgs, MemorySearchOutput, MemorySearchTool, PathCluster};
pub use memory_timeline::{MemoryTimelineArgs, MemoryTimelineOutput, MemoryTimelineTool};
pub use moa_manage::{MoaManageArgs, MoaManageOutput, MoaManageTool};
pub use node_file::{NodeFileArgs, NodeFileTool};
pub use node_invoke::{NodeInvokeArgs, NodeInvokeTool};
pub use node_invoke_many::{NodeInvokeManyArgs, NodeInvokeManyTool};
pub use node_list::{NodeListArgs, NodeListTool};
pub use node_manage::{NodeManageArgs, NodeManageTool};
pub use pdf_generate::{
    ContentFormat, PageSize, PdfGenerateArgs, PdfGenerateOutput, PdfGenerateTool, RenderEngine,
};
pub use permission_tool::{PermissionArgs, PermissionOutput, PermissionTool};
pub use pim::{PimArgs, PimOutput, PimTool};
pub use plugin_manage::{PluginAction, PluginManageArgs, PluginManageOutput, PluginManageTool};
pub use recall_context::{RecallContextArgs, RecallContextResult, RecallContextTool};
pub use recall_events::{RecallEventsArgs, RecallEventsOutput, RecallEventsTool};
pub use remember::{RememberArgs, RememberOutput, RememberTool};
pub use scratchpad::{ScratchpadAction, ScratchpadArgs, ScratchpadOutput, ScratchpadTool};
pub use search::{SearchArgs, SearchTool};
pub use select_model::{SelectModelArgs, SelectModelOutput, SelectModelTool};
pub use self_manage::{SelfManageArgs, SelfManageOutput, SelfManageTool};
pub use session_complete::{SessionCompleteArgs, SessionCompleteResult, SessionCompleteTool};
pub use session_search::{SessionSearchArgs, SessionSearchOutput, SessionSearchTool};
pub use sessions::*;
pub use skill_reader::{
    ListSkillsArgs, ListSkillsOutput, ListSkillsTool, ReadSkillArgs, ReadSkillOutput,
    ReadSkillTool, SkillSummary,
};
pub use strategy_manage::{StrategyAction, StrategyArgs, StrategyOutput, StrategyTool};
pub use system_tool::{SystemArgs, SystemOutput, SystemTool};
pub use task_manage::*;
pub use team::*;
pub use terminal::{TerminalAction, TerminalArgs, TerminalOutput, TerminalTool};
pub use vault_store::{VaultAction, VaultStoreArgs, VaultStoreOutput, VaultStoreTool};
pub use voice_tools::{
    LocalVoiceArgs, LocalVoiceOutput, LocalVoiceTool, VoiceModeSetArgs, VoiceModeSetOutput,
    VoiceModeSetTool,
};
pub use web_fetch::{ExtractMode, Extractor, WebFetchArgs, WebFetchResult, WebFetchTool};
pub use workspace_manage::{WorkspaceManageArgs, WorkspaceManageTool};

// ============================================================================
// Tool Progress Notifications (no-op stubs)
// ============================================================================
//
// The original progress-callback machinery had no live consumer anywhere in
// the tree — every `notify_tool_*` site was a no-op fire and forget. Tool
// progress now flows through the gateway event bus
// (`notify_tool_result` → `GatewayEventEmitter`), which is already wired
// from `executor/builtin_registry/registry/tool_registry_impl.rs`. The stubs
// below are kept so the ~85 existing `notify_tool_*` callers stay compile-
// clean without churn; rip them out when the call sites are updated to
// publish via the event bus directly.

/// Notify that a tool has started execution (legacy no-op).
pub fn notify_tool_start(_tool_name: &str, _args_summary: &str) {}

/// Notify that a tool has completed execution (legacy no-op).
pub fn notify_tool_result(_tool_name: &str, _result_summary: &str, _success: bool) {}

/// Notify that a tool has emitted a streaming chunk (legacy no-op).
pub fn notify_tool_streaming_chunk(_tool_name: &str, _chunk: &str) {}

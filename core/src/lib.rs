// Aether Core Library
//
//! Aether is a system-level AI middleware that acts as an invisible "ether"
//! connecting user intent with AI models through a frictionless, native interface.
//!
//! # Architecture
//!
//! The core library runs as a standalone daemon (`aether-gateway`) that exposes
//! a WebSocket JSON-RPC interface. Native clients (Swift on macOS, React on Web)
//! communicate with this gateway to access AI processing, tool execution,
//! and memory management functionality.
//!
//! ```text
//! ┌─────────────────┐      ┌─────────────────┐
//! │  macOS App      │      │  aether-gateway │
//! │  (Swift)        │─────▶│  (Rust Daemon)  │
//! │                 │  WS  │  ws://127.0.0.1 │
//! └─────────────────┘      └─────────────────┘
//! ```
//!
//! # Gateway Interface
//!
//! The primary interface is the WebSocket Gateway with JSON-RPC 2.0 protocol:
//!
//! - **agent.run**: Execute AI agent with tool calling
//! - **session.***: Session management (history, compaction)
//! - **config.***: Configuration management (hot-reload)
//! - **memory.***: Memory operations (search, store)
//!
//! # Features
//!
//! - ✅ Unified AI processing with multiple providers (OpenAI, Claude, Gemini, Ollama)
//! - ✅ Tool execution (search, MCP, skills)
//! - ✅ Memory management with vector search
//! - ✅ Configuration hot-reload
//! - ✅ Multi-turn conversation support
//! - ✅ Vision/OCR capabilities
//! - ✅ WebSocket streaming for real-time updates

#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]

// Module declarations
// NOTE: clipboard module retained for ImageData/ImageFormat types (used by AI providers)
// Clipboard operations are handled by Swift ClipboardManager
pub mod agent_loop; // NEW: Agent Loop - observe-think-act-feedback cycle
pub mod agents; // Unified agent system: rig-core agent + sub-agent delegation
pub mod capability;
pub mod checkpoint; // NEW: File snapshots and rollback (Claude Code style)
pub mod clarification; // NEW: Phantom Flow interaction types
mod clipboard;
pub mod command; // NEW: Command completion system
pub mod components; // NEW: Core event handler components for agentic loop
pub mod compressor; // NEW: Context compression for Agent Loop
mod config;
pub mod conversation; // NEW: Multi-turn conversation support
mod core;
pub mod dispatcher; // Core dispatch center: model routing, tool registry, task orchestration
mod error;
pub mod event; // NEW: Event-driven architecture for agentic loop
mod event_handler;
pub mod executor; // Unified executor for multi-step task execution
// FFI module removed - using Gateway WebSocket architecture
pub mod generation;
pub mod initialization {
    //! Unified initialization module
    //!
    //! Re-exports from the new init_unified module for backward compatibility.
    //! The actual implementation is in init_unified/.
    pub use crate::init_unified::*;
}
mod init_unified;
pub mod intent; // NEW: Smart intent detection for conversation flow
pub mod logging;
pub mod mcp; // NEW: MCP (Model Context Protocol) capability
pub mod memory;
pub mod markdown; // Markdown parsing utilities (fence detection for streaming)
pub mod discovery; // NEW: Multi-level component discovery system (Claude Code compatible)
pub mod extension; // NEW: Extension system (Claude Code compatible)
pub mod metrics;
pub mod three_layer; // Three-layer control architecture (Orchestrator/Skill-DAG/Tools)
pub mod payload; // Structured context protocol with capability support
pub mod prompt; // NEW: Unified prompt management (executor/conversational)
pub mod providers;
pub mod builtin_tools; // Built-in tools for AetherTool system
pub mod runtimes; // NEW: Runtime manager for external tools (uv, fnm, yt-dlp)
pub mod search; // NEW: Search capability with multiple provider support
pub mod services; // NEW: Shared foundation services (FileOps, GitOps, SystemInfo)
pub mod skills; // NEW: Claude Agent Skills support
pub mod suggestion; // NEW: AI response suggestion parsing
pub mod thinker; // NEW: LLM decision-making layer for Agent Loop
mod title_generator; // Title generation for conversation topics
pub mod tool_output; // NEW: Tool output truncation and cleanup (OpenCode style)
pub mod tools; // NEW: Unified tool system (replacing rig-core Tool trait)
pub mod thinking; // Thinking levels and streaming block processing
pub mod exec; // Command execution security
pub mod routing; // Channel-aware routing and session key system
// uniffi_core removed - using Gateway WebSocket architecture
pub mod utils; // NEW: Capability executor for enriching payloads
pub mod cli; // CLI utilities for Gateway commands
pub mod video; // NEW: Video transcript extraction (YouTube)
pub mod vision; // NEW: Vision capability (screen OCR, image understanding) // NEW: Media generation providers (image, video, audio, speech)

// NEW: WebSocket Gateway for Moltbot-style architecture
#[cfg(feature = "gateway")]
pub mod gateway;

// NEW: Cron job scheduling
#[cfg(feature = "cron")]
pub mod cron;

// NEW: Browser control via CDP
#[cfg(feature = "browser")]
pub mod browser;

// NEW: Permission and Question systems (OpenCode compatible)
pub mod permission; // Rule-based permission system
pub mod question; // Structured user interaction

// NEW: Wizard system for guided configuration flows
pub mod wizard;

// NEW: PTY-based process supervisor for Claude Code control
pub mod supervisor;

// Integration tests module
#[cfg(test)]
mod tests;

// Re-export public types
// NOTE: ImageData/ImageFormat still exported for AI provider image encoding
pub use crate::clipboard::{ImageData, ImageFormat};
pub use crate::command::{CommandExecutionResult, CommandNode, CommandRegistry, CommandType};
pub use crate::config::{
    AiRetrievalPolicy,
    BehaviorConfig,
    CompressionPolicy,
    Config,
    ContextRuleConfig,
    DispatcherConfigToml,
    // Experimental feature flags
    ExperimentalPolicy,
    FullConfig,
    GeneralConfig,
    // Generation config types
    GenerationConfig,
    GenerationDefaults,
    GenerationProviderConfig,
    IntentDetectionConfig,
    IntentDetectionPolicy,
    KeywordPolicy,
    KeywordRuleConfig,
    MemoryConfig,
    MemoryPolicies,
    MetricsPolicy,
    PIIConfig,
    // Mechanism-policy separation types
    PoliciesConfig,
    PolicyKeywordRule,
    PolicyWeightedKeyword,
    ProviderConfig,
    ProviderConfigEntry,
    RetryPolicy,
    RoutingRuleConfig,
    SearchBackendConfig,
    SearchBackendEntry,
    SearchConfig,
    SearchConfigInternal,
    ShortcutsConfig,
    SkillsConfig,
    SmartFlowConfig,
    SmartMatchingConfig,
    SuggestionParsingConfig,
    TestConnectionResult,
    TextFormatPolicy,
    ToolSafetyPolicy,
    VideoConfig,
    WebFetchPolicy,
};
// Internal types from core module
pub use crate::core::{
    AppMemoryInfo, CapturedContext, CompressionStats, MediaAttachment,
    MemoryEntryFFI as MemoryEntry,
};
pub use crate::error::{AetherError, AetherException, Result};
pub use crate::memory::context::CompressionResult;
// Event handler types
pub use crate::event_handler::{
    ErrorType, McpServerErrorFFI, McpStartupReportFFI, ProcessingState,
};
// Skills management exports (moved from old initialization module)
pub use crate::skills::{
    initialize_builtin_skills, initialize_builtin_skills_ffi, list_installed_skills,
};
pub use crate::utils::paths::{get_skills_dir, get_skills_dir_string};
// Initialization exports (new unified module)
pub use crate::initialization::{
    InitError, InitPhase, InitProgressHandler, InitializationCoordinator, InitializationResult,
};
// NOTE: Skill modification functions are in AetherCore to ensure automatic tool registry refresh.
// Use AetherCore.delete_skill(), AetherCore.install_skill(), etc.
pub use crate::clarification::{
    ClarificationOption, ClarificationRequest, ClarificationResult, ClarificationResultType,
    ClarificationType, QuestionGroup,
};
pub use crate::conversation::{ConversationManager, ConversationSession, ConversationTurn};
pub use crate::dispatcher::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationAction, ConfirmationConfig,
    ConfirmationDecision, ConfirmationState, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult, PendingConfirmation, PendingConfirmationInfo,
    PendingConfirmationStore, ToolCategory, ToolConfirmation, ToolDefinition, ToolRegistry,
    ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
    UserConfirmationDecision,
    // DAG execution callback types
    DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan, UserDecision as DagUserDecision,
    NoOpExecutionCallback, ExecutionCallback,
};
pub use crate::intent::{
    // Core classifier types
    AgentModePrompt,
    // Aggregator types
    AggregatedIntent,
    AggregatorConfig,
    AiIntentDetector,
    AiIntentResult,
    AmbiguousTaskFFI,
    // Context types
    AppContext,
    // Cache types
    CacheConfig,
    CacheMetrics,
    CachedIntent,
    // Calibrator types
    CalibratedSignal,
    CalibrationHistory,
    CalibratorConfig,
    ConfidenceCalibrator,
    ConflictResolution,
    ConflictResolutionFFI,
    ConversationContext,
    DefaultsResolver,
    ExecutableTask,
    ExecutableTaskFFI,
    ExecutionIntent,
    ExecutionIntentTypeFFI,
    InputFeatures,
    IntentAction,
    IntentAggregator,
    IntentCache,
    IntentClassifier,
    IntentSignal,
    MatchingContext,
    MatchingContextBuilder,
    MissingParameter,
    OrganizeMethod,
    OrganizeMethodFFI,
    ParameterSource,
    ParameterSourceFFI,
    PendingParam,
    PresetRegistry,
    // Rollback types
    RollbackCapable,
    RollbackConfig,
    RollbackEntry,
    RollbackManager,
    RollbackResult,
    RoutingLayer,
    ScenarioPreset,
    TaskCategory,
    TaskCategoryFFI,
    TaskParameters,
    TaskParametersFFI,
    TimeContext,
    // New unified execution decider
    ContextSignals,
    DeciderConfig,
    DecisionMetadata,
    DecisionResult,
    ExecutionIntentDecider,
    ExecutionMode,
    IntentLayer,
    ToolInvocation,
    // New simplified router for Agent Loop
    DirectMode,
    DirectRouteInfo,
    IntentRouter,
    RouteResult,
    ThinkingContext,
};
// Backward compatibility: DecisionLayer is deprecated, use IntentLayer instead
#[allow(deprecated)]
pub use crate::intent::DecisionLayer;
pub use crate::logging::{create_pii_scrubbing_layer, LogLevel, PiiScrubbingLayer};
pub use crate::mcp::{
    McpEnvVar, McpServerConfig, McpServerPermissions, McpServerStatus, McpServerStatusInfo,
    McpServerType, McpServiceInfo, McpSettingsConfig, McpToolInfo,
};
pub use crate::memory::database::MemoryStats;
pub use crate::metrics::StageTimer;
pub use crate::providers::AiProvider;
pub use crate::search::{ProviderTestResult, SearchProviderTestConfig};
pub use crate::skills::{Skill, SkillInfo, SkillsInstaller, SkillsRegistry};
// Extension system exports (Claude Code compatible plugin system)
pub use crate::extension::{
    // Core types
    AetherConfig, AgentMode as ExtAgentMode, ComponentLoader, ComponentRegistry, ConfigManager,
    ExtensionAgent, ExtensionCommand, ExtensionConfig, ExtensionError, ExtensionManager,
    ExtensionPlugin, ExtensionResult, ExtensionSkill, HookAction, HookConfig,
    HookEvent, LoadSummary, McpServerConfig as ExtMcpServerConfig,
    PermissionAction, PermissionRule, PluginInfo, SkillFrontmatter, SkillType,
    SyncExtensionManager,
    // Hook execution
    hooks::{HookContext, HookExecutor, HookResult},
    // Config types
    config::{AgentConfigOverride, McpConfig, OAuthConfig, ProviderOverride},
    // Utility functions
    build_skill_instructions, default_plugins_dir, is_valid_plugin_dir,
};
pub use crate::suggestion::{ParsedSuggestions, SuggestionOption, SuggestionParser};
pub use crate::utils::pii;
pub use crate::vision::{
    CaptureMode, VisionConfig, VisionRequest, VisionResult, VisionService, VisionTask,
};
// Dispatcher FFI exports removed - types moved to gateway/handlers/

// Generation exports (media generation providers)
// Note: providers module is accessible via aethecore::generation::providers
pub use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationParams,
    GenerationProgress, GenerationProvider, GenerationProviderRegistry, GenerationRequest,
    GenerationType, MockGenerationProvider,
};


// Executor exports
pub use crate::executor::{
    ExecutionContext, ExecutionResult, ExecutorError, SingleStepConfig, SingleStepExecutor,
    TaskExecutionResult, ToolCallRecord as ExecutorToolCallRecord,
    ToolRegistry as ExecutorToolRegistry,
};

// Exec security exports (command execution approval)
pub use crate::exec::{
    // Config
    AgentExecConfig, AllowlistEntry, ExecApprovalsFile, ExecAsk, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
    // Analysis
    CommandAnalysis, CommandResolution, CommandSegment,
    // Parser
    analyze_shell_command, tokenize_segment,
    // Allowlist
    match_allowlist,
    // Decision
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
    // Socket
    ApprovalDecisionType, ApprovalRequestPayload, SegmentInfo, SocketMessage,
    // Manager
    ExecApprovalManager, ExecApprovalRecord, PendingApproval,
    // Storage
    ConfigWithHash, ExecApprovalsStorage, StorageError,
};

// Prompt exports (unified prompt management)
pub use crate::prompt::{
    ConversationalPrompt, ExecutorPrompt, PromptBuilder, PromptConfig, PromptTemplate, TemplateVar,
    ToolInfo as PromptToolInfo,
};

// Event system exports (event-driven agentic loop)
pub use crate::event::{
    AetherEvent,
    // Event payload types
    AiResponse,
    CompactionInfo,
    ErrorKind,
    EventBus,
    EventBusConfig,
    EventContext,
    EventHandler,
    EventHandlerRegistry,
    EventSubscriber,
    EventType,
    HandlerError,
    InputContext,
    InputEvent,
    LoopState,
    PlanRequest,
    PlanStep,
    SessionDiff,
    SessionInfo,
    StepStatus,
    StopReason,
    SubAgentRequest,
    SubAgentResult,
    TaskPlan,
    TimestampedEvent,
    TokenUsage,
    ToolCallError,
    ToolCallRequest,
    ToolCallResult,
    ToolCallRetry,
    ToolCallStarted,
    UserQuestion,
    UserResponse,
};

// Component exports (event handler implementations for agentic loop)
pub use crate::components::{
    AiResponsePart,
    Complexity,
    ComponentContext,
    Decision,
    // Session types
    ExecutionSession,
    // Core components
    IntentAnalyzer,
    LoopConfig,
    LoopController,
    ModelLimit,
    PlanPart,
    ReasoningPart,
    RecorderError,
    SessionCompactor,
    SessionPart,
    SessionRecord,
    SessionRecorder,
    SessionStatus,
    SubAgentHandler,
    SubAgentPart,
    SummaryPart,
    TaskPlanner,
    TokenTracker,
    ToolCallPart,
    ToolCallRecord,
    ToolCallStatus,
    ToolExecutor,
    UserInputPart,
};
// Note: ToolRetryPolicy from components is available as components::ToolRetryPolicy
// This is distinct from config::RetryPolicy (network retry policy)

// Agent Loop exports (new unified agent architecture)
pub use crate::agent_loop::{
    ActionExecutor, AgentLoop, CollectingCallback, CompressedHistory, CompressorTrait,
    GuardViolation, LoggingCallback, LoopCallback, LoopConfig as AgentLoopConfig, LoopGuard,
    LoopResult, LoopState as AgentLoopState, LoopStep, NoOpLoopCallback, Observation, RequestContext,
    StepSummary, ThinkerTrait, Thinking,
    // Decision types
    Action, ActionResult, Decision as AgentDecision, LlmAction, LlmResponse,
    // Config types
    CompressionConfig, ModelRoutingConfig,
};

// Thinker exports (LLM decision-making layer)
pub use crate::thinker::{
    DecisionParser, Message, MessageRole as ThinkerMessageRole, ModelId,
    PromptBuilder as ThinkerPromptBuilder, PromptConfig as ThinkerPromptConfig, ProviderRegistry,
    RoutingCondition, RoutingRule, SingleProviderRegistry, Thinker, ThinkerConfig,
    ThinkerModelSelector, ToolFilter, ToolFilterConfig,
};
// Deprecated alias for backward compatibility
#[allow(deprecated)]
pub use crate::thinker::ModelRouter;

// Compressor exports (context compression)
pub use crate::compressor::{
    CompressionPrompt, ContextCompressor, KeyInfo, KeyInfoExtractor, NoOpCompressor,
    RuleBasedStrategy,
};

// Permission system exports (OpenCode compatible)
// Note: PermissionRule is not exported here to avoid conflict with extension::PermissionRule
// Access via permission::PermissionRule if needed
pub use crate::permission::{
    PermissionConfig, PermissionConfigMap, PermissionError, PermissionEvaluator,
    PermissionManager, PermissionManagerConfig, PermissionMapping, Ruleset,
};

// Question system exports (structured user interaction)
pub use crate::question::{QuestionError, QuestionManager, QuestionManagerConfig};

// Wizard system exports (guided configuration flows)
pub use crate::wizard::{
    CliPrompter, ProgressHandle, RpcPrompter, WizardPrompter,
    WizardFlow, WizardSession, WizardSessionError,
    StepExecutor, StepType, WizardNextResult, WizardOption, WizardStatus, WizardStep,
    // Flows
    OnboardingFlow, OnboardingData, ProviderSetupFlow, QuickSetupFlow,
};

// Supervisor exports (PTY-based process control)
pub use crate::supervisor::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};

// Tool system exports (unified tool traits replacing rig-core)
pub use crate::tools::{AetherTool, AetherToolDyn, AetherToolServer, AetherToolServerHandle};

// Session tools exports (A2A communication)
pub use crate::tools::sessions::{
    // Policy
    AgentToAgentPolicy, A2ARule, RuleMatcher,
    // Visibility
    SessionToolsVisibility, VisibilityContext,
    // Types
    SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus,
    // Registry
    SubagentRegistry, SubagentRun,
    // Tool params/results
    SessionsListParams, SessionsListResult,
    SessionsSendParams, SessionsSendResult,
    SessionsSpawnParams, SessionsSpawnResult,
    build_subagent_system_prompt,
};

// Agent system exports (unified: agent loop + sub-agent delegation)
pub use crate::agents::{
    // Sub-agent delegation
    builtin_agents,
    // Agent configuration (re-exported from agents::rig)
    AgentConfig,
    AgentDef,
    AgentMode,
    AgentRegistry,
    BuiltinToolConfig,
    ChatMessage,
    ConversationHistory,
    MessageRole,
    RigAgentConfig,
    TaskTool,
    TaskToolError,
    TaskToolResult,
    // Tool server utilities
    create_builtin_tool_server,
    create_builtin_tools_list,
};
// Note: ToolCallInfo/ToolCallResult from agents::rig::types are internal;
// use crate::event::ToolCallResult for event system

// Core interface moved to Gateway WebSocket architecture
// See gateway/handlers/ for RPC method implementations

// Test-only exports
#[cfg(test)]
pub use crate::event_handler::MockEventHandler;

/// Initialize the tracing subscriber for logging
///
/// This function should be called once at application startup.
/// It configures structured logging with environment-based filtering,
/// daily log file rotation, and automatic PII scrubbing.
///
/// # Log Files
///
/// - Location: `~/.aether/logs/`
/// - Format: `aether-YYYY-MM-DD.log`
/// - Rotation: Daily
/// - Privacy: All PII automatically scrubbed
///
/// # Environment Variables
///
/// - `RUST_LOG`: Controls log level (e.g., "debug", "info", "aether=debug")
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::init_logging;
///
/// init_logging();
/// ```
pub fn init_logging() {
    // Use file-based logging with PII scrubbing
    if let Err(e) = crate::logging::init_file_logging() {
        eprintln!("Warning: Failed to initialize file logging: {}", e);
        eprintln!("Falling back to console-only logging");

        // Fallback to console-only logging
        use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        // Use try_init to avoid panic if already initialized
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true))
            .try_init();
    }
}

// UniFFI and C ABI removed - using Gateway WebSocket architecture
// See core/src/gateway/ for the new control plane implementation

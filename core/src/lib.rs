// Aether Core Library
//
//! Aether is a system-level AI middleware that acts as an invisible "ether"
//! connecting user intent with AI models through a frictionless, native interface.
//!
//! # Architecture
//!
//! The core library is built as a headless service (cdylib/staticlib) that exposes
//! a clean FFI boundary via UniFFI. Native clients (Swift on macOS, C# on Windows,
//! GTK on Linux) communicate with this core to access AI processing, tool execution,
//! and memory management functionality.
//!
//! # V2 Interface (Primary)
//!
//! The primary interface is `AetherV2Core`, which uses the rig-core framework for
//! AI provider integration and tool execution.
//!
//! - **AetherV2Core**: Main entry point for all AI and tool operations
//! - **AetherV2EventHandler**: Callback interface for Rust → Client communication
//! - **ProcessingState**: State machine for UI feedback
//!
//! # Usage Example
//!
//! ```rust,ignore
//! use aethecore::UniffiAetherCore;
//!
//! // Initialize core with event handler
//! let handler = MyEventHandler::new();
//! let core = UniffiAetherCore::new("/path/to/config.toml", handler).unwrap();
//!
//! // Process user input (uses RigAgentManager internally)
//! let result = core.process("Hello, world!".to_string(), None);
//!
//! // List available tools
//! let tools = core.list_tools();
//! ```
//!
//! # Features
//!
//! - ✅ Unified AI processing with multiple providers (OpenAI, Claude, Gemini, Ollama)
//! - ✅ Tool execution (search, MCP, skills)
//! - ✅ Memory management with vector search
//! - ✅ Configuration hot-reload
//! - ✅ Multi-turn conversation support
//! - ✅ Vision/OCR capabilities

// Allow clippy lints for UniFFI generated code
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]
#![allow(unpredictable_function_pointer_comparisons)]

// Module declarations
// NOTE: clipboard module retained for ImageData/ImageFormat types (used by AI providers)
// Clipboard operations are handled by Swift ClipboardManager
pub mod agents; // Unified agent system: rig-core agent + sub-agent delegation
pub mod capability;
pub mod clarification; // NEW: Phantom Flow interaction types
mod clipboard;
pub mod command; // NEW: Command completion system
pub mod components; // NEW: Core event handler components for agentic loop
mod config;
pub mod conversation; // NEW: Multi-turn conversation support
mod core;
pub mod cowork_ffi; // Cowork FFI bindings
pub mod dispatcher; // Core dispatch center: model routing, tool registry, task orchestration
mod error;
pub mod event; // NEW: Event-driven architecture for agentic loop
mod event_handler;
pub mod executor; // NEW: Unified executor for 2-layer architecture
pub mod ffi; // FFI module - split AetherCore implementation
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
pub mod metrics;
pub mod orchestrator; // NEW: Unified request orchestrator (Phase 1 + Phase 2 coordination)
pub mod payload; // Structured context protocol with capability support
pub mod planner; // NEW: Unified planner for 2-layer architecture
pub mod prompt; // NEW: Unified prompt management (executor/conversational)
pub mod providers;
pub mod rig_tools; // NEW: Rig-compatible tool wrapper
pub mod runtimes; // NEW: Runtime manager for external tools (uv, fnm, yt-dlp)
pub mod search; // NEW: Search capability with multiple provider support
pub mod services; // NEW: Shared foundation services (FileOps, GitOps, SystemInfo)
pub mod skills; // NEW: Claude Agent Skills support
pub mod suggestion; // NEW: AI response suggestion parsing
mod title_generator; // Title generation for conversation topics
pub mod uniffi_core; // UniFFI core bindings - re-exports from ffi module
pub mod utils; // NEW: Capability executor for enriching payloads
pub mod video; // NEW: Video transcript extraction (YouTube)
pub mod vision; // NEW: Vision capability (screen OCR, image understanding) // NEW: Media generation providers (image, video, audio, speech)

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
    TriggerConfig,
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
// Event handler types (legacy V1 trait removed, use AetherEventHandler from uniffi_core)
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
    ClarificationType,
};
pub use crate::conversation::{ConversationManager, ConversationSession, ConversationTurn};
pub use crate::dispatcher::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationAction, ConfirmationConfig,
    ConfirmationDecision, ConfirmationState, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult, PendingConfirmation, PendingConfirmationInfo,
    PendingConfirmationStore, ToolCategory, ToolConfirmation, ToolDefinition, ToolRegistry,
    ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
    UserConfirmationDecision,
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
    DecisionLayer,
    DecisionMetadata,
    DecisionResult,
    ExecutionIntentDecider,
    ExecutionMode,
    ToolInvocation,
};
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
pub use crate::suggestion::{ParsedSuggestions, SuggestionOption, SuggestionParser};
pub use crate::utils::pii;
pub use crate::vision::{
    CaptureMode, VisionConfig, VisionRequest, VisionResult, VisionService, VisionTask,
};
// Cowork FFI exports (task orchestration)
pub use crate::cowork_ffi::{
    // Budget management types (Model Router P1)
    BudgetEnforcementFFI,
    BudgetLimitStatusFFI,
    BudgetPeriodFFI,
    BudgetScopeFFI,
    BudgetStatusFFI,
    // P2: Semantic Cache types
    CacheHitTypeFFI,
    CacheStatsFFI,
    // Model Router types
    CapabilityMappingFFI,
    // Base Cowork types
    CodeExecConfigFFI,
    // P2: Prompt Analysis types
    ContextSizeFFI,
    CoworkConfigFFI,
    CoworkExecutionState,
    CoworkExecutionSummaryFFI,
    CoworkProgressEventFFI,
    CoworkProgressEventType,
    CoworkProgressHandler,
    CoworkTaskDependencyFFI,
    CoworkTaskFFI,
    CoworkTaskGraphFFI,
    CoworkTaskStatusState,
    CoworkTaskTypeCategory,
    DomainFFI,
    FfiProgressSubscriber,
    FileOpsConfigFFI,
    // Health monitoring types
    HealthStatisticsFFI,
    LanguageFFI,
    // Model profile types
    ModelCapabilityFFI,
    ModelCostStrategyFFI,
    ModelCostTierFFI,
    ModelHealthStatusFFI,
    ModelHealthSummaryFFI,
    ModelLatencyTierFFI,
    ModelProfileFFI,
    ModelRoutingRulesFFI,
    PromptFeaturesFFI,
    ReasoningLevelFFI,
    StageResultFFI,
    TaskTypeMappingFFI,
};

// Generation exports (media generation providers)
// Note: providers module is accessible via aethecore::generation::providers
pub use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationParams,
    GenerationProgress, GenerationProvider, GenerationProviderRegistry, GenerationRequest,
    GenerationType, MockGenerationProvider,
};

// Planner exports (unified 2-layer architecture)
pub use crate::planner::{
    ExecutionPlan, PlannedTask, PlannerConfig, PlannerError, ToolInfo, UnifiedPlanner,
};

// Prompt exports (unified prompt management)
pub use crate::prompt::{
    ConversationalPrompt, ExecutorPrompt, PromptBuilder, PromptConfig, PromptTemplate, TemplateVar,
    ToolInfo as PromptToolInfo,
};

// Executor exports (unified 2-layer architecture)
pub use crate::executor::{
    ExecutionContext, ExecutionResult, ExecutorConfig, ExecutorError, TaskExecutionResult,
    ToolCallRecord as ExecutorToolCallRecord, UnifiedExecutor,
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
// Note: RetryPolicy from components is available as components::RetryPolicy
// to avoid conflict with config::RetryPolicy (network retry policy)

// Agent system exports (unified: rig-core agent + sub-agent delegation)
pub use crate::agents::{
    // Sub-agent delegation
    builtin_agents,
    // Rig-core AI agent (re-exported from agents::rig)
    AgentConfig,
    AgentDef,
    AgentMode,
    AgentRegistry,
    AgentResponse,
    BuiltinToolConfig,
    ChatMessage,
    ConversationHistory,
    MessageRole,
    RigAgentConfig,
    RigAgentManager,
    TaskTool,
    TaskToolError,
    TaskToolResult,
};
// Note: ToolCallInfo/ToolCallResult from agents::rig::types are internal;
// use crate::event::ToolCallResult for event system

// Core interface exports (rig-core based architecture)
pub use crate::uniffi_core::{
    init_core,
    AetherCore,
    AetherEventHandler,
    AetherFfiError,
    // Generation FFI types
    GenerationDataFFI,
    GenerationDataTypeFFI,
    GenerationMetadataFFI,
    GenerationOutputFFI,
    GenerationParamsFFI,
    GenerationProgressFFI,
    GenerationProviderConfigFFI,
    GenerationProviderInfoFFI,
    GenerationTypeFFI,
    MemoryItem,
    ProcessOptions,
    // Runtime FFI types
    RuntimeInfo,
    RuntimeUpdateInfo,
    SessionSummary,
    ToolInfoFFI,
};

// Initialization FFI types (unified) - UniFFI only
#[cfg(feature = "uniffi")]
pub use crate::uniffi_core::{
    needs_first_time_init, run_initialization, InitProgressHandlerFFI, InitResultFFI,
};

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
/// - Location: `~/.config/aether/logs/`
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

        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let filter =
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(true))
                .init();
        });
    }
}

// Include UniFFI scaffolding (macOS only)
// This generates all the FFI glue code at compile time
#[cfg(feature = "uniffi")]
uniffi::include_scaffolding!("aether");

// C ABI exports for Windows (csbindgen)
#[cfg(feature = "cabi")]
pub mod ffi_cabi;

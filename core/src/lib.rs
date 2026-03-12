// Aleph Core Library
//
//! Aleph is a system-level AI middleware that acts as an invisible "ether"
//! connecting user intent with AI models through a frictionless, native interface.
//!
//! # Architecture
//!
//! The core library runs as a standalone daemon (`aleph-gateway`) that exposes
//! a WebSocket JSON-RPC interface. Native clients (Swift on macOS, React on Web)
//! communicate with this gateway to access AI processing, tool execution,
//! and memory management functionality.
//!
//! ```text
//! ┌─────────────────┐      ┌─────────────────┐
//! │  macOS App      │      │  aleph-gateway │
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

#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]

// =============================================================================
// Module Declarations
// =============================================================================

pub mod agent_loop;
pub mod agents;
pub mod arena;
pub mod approval;
pub mod browser;
pub mod builtin_tools;
pub mod capability;
pub mod clarification;
mod clipboard;
pub mod cli;
pub mod command;
pub mod components;
pub mod compressor;
mod config;
pub mod conversation;
mod core;
pub mod desktop;
pub mod discovery;
pub mod dispatcher;
pub mod domain;
pub mod engine;
mod error;
pub mod event;
mod event_handler;
pub mod exec;
pub mod executor;
pub mod extension;
pub mod generation;
mod init_unified;
pub mod intent;
pub mod logging;
pub mod markdown;
pub mod media;
pub mod mcp;
pub mod memory;
pub mod metrics;
pub mod payload;
pub mod permission;
pub mod pii;
pub mod prompt;
pub mod providers;
pub mod question;
pub mod routing;
pub mod runtimes;
pub mod search;
pub mod skills;
pub mod skill;
pub mod suggestion;
pub mod supervisor;
pub mod thinker;
pub mod tool_output;
pub mod tools;
pub mod utils;
pub mod vision;
pub mod wizard;
pub mod spec_driven;
pub mod resilience;
pub mod resilient;
pub mod daemon;
pub mod scheduler;
pub mod secrets;
pub(crate) mod sync_primitives;

/// Unified initialization module (re-export for backward compatibility)
pub mod initialization {
    pub use crate::init_unified::*;
}

pub mod a2a;
pub mod gateway;
pub mod group_chat;
pub mod cron;

#[cfg(test)]
mod tests;

// =============================================================================
// Core API Exports
// =============================================================================

// Error types (always needed)
pub use crate::error::{AlephError, AlephException, Result};

// Configuration (main entry points and commonly used types)
pub use crate::config::{
    Config, FullConfig, ChannelInstanceConfig, ProviderConfig, RoutingRuleConfig,
    MemoryConfig, BehaviorConfig, ShortcutsConfig, GeneralConfig, SmartFlowConfig,
    backup::ConfigBackup,
    patcher::ConfigPatcher,
    agent_resolver::{AgentDefinitionResolver, ResolvedAgent},
    agent_manager::AgentManager,
    policies::CompressionPolicy,
};

// Initialization
pub use crate::initialization::{
    InitError, InitPhase, InitProgressHandler, InitializationCoordinator, InitializationResult,
};

// Logging
pub use crate::logging::{create_pii_scrubbing_layer, LogLevel, PiiScrubbingLayer};

// =============================================================================
// Agent System Exports
// =============================================================================

// Agent Loop (agent loop types)
pub use crate::agent_loop::{
    LoopCallback, LoopConfig as AgentLoopConfig, LoopRunResult,
    AgentLoop,
};

// Thinker (LLM layer - provider registry)
pub use crate::thinker::{ProviderRegistry, SingleProviderRegistry, SwappableProviderRegistry};

// =============================================================================
// Tool System Exports
// =============================================================================

// Unified tool traits
pub use crate::tools::{AlephTool, AlephToolDyn, AlephToolServer, AlephToolServerHandle};

// Dispatcher (tool registry)
pub use crate::dispatcher::{
    DispatcherConfig, ToolCategory, ToolDefinition, ToolRegistry, ToolResult,
    ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// Tool Index (Tool-as-Resource)
pub use crate::dispatcher::tool_index::{
    // Config
    ToolRetrievalConfig,
    // Inference
    InferredPurpose, SemanticPurposeInferrer,
    // Coordinator
    ToolIndexCoordinator, ToolMeta,
    // Retrieval
    HydrationLevel, HydratedTool, ToolRetrieval,
};

// =============================================================================
// Extension System Exports
// =============================================================================

pub use crate::extension::{
    ContentLoader, ExtensionConfig, ExtensionError,
    ExtensionManager, ExtensionResult, LoadSummary, PluginInfo, SyncExtensionManager,
};

// =============================================================================
// Skills & MCP Exports
// =============================================================================

pub use crate::skills::{
    initialize_builtin_skills, list_installed_skills,
    Skill, SkillInfo, SkillsInstaller, SkillsRegistry,
};

pub use crate::mcp::{
    McpServerConfig, McpServerStatus, McpServerStatusInfo, McpServerType, McpToolInfo,
};

// =============================================================================
// Exec Security Exports
// =============================================================================

pub use crate::exec::{
    ApprovalDecision, ApprovalRequest, ExecContext,
    ExecApprovalManager, PendingApproval, SecurityKernel,
    analyze_shell_command, decide_exec_approval, match_allowlist,
};

// =============================================================================
// Supervisor Exports
// =============================================================================

pub use crate::supervisor::{
    ClaudeSupervisor, PtySize, SupervisorConfig, SupervisorError, SupervisorEvent,
};

// =============================================================================
// Wizard Exports
// =============================================================================

pub use crate::wizard::{
    WizardFlow, WizardPrompter, WizardSession, WizardSessionError, WizardStatus,
};

// =============================================================================
// Spec-Driven Development Exports
// =============================================================================

pub use crate::spec_driven::{
    AssertionType, EvaluationResult, LlmJudge, NoOpWorkflowCallback, Spec, SpecDrivenWorkflow,
    SpecMetadata, SpecTarget, SpecWriter, TestCase, TestResult, TestType, TestWriter,
    WorkflowCallback, WorkflowConfig, WorkflowResult,
};

// =============================================================================
// Resilient Task Exports
// =============================================================================

pub use crate::resilient::{
    classify_error, execute_resilient, DegradationReason, DegradationStrategy, ErrorClass, FnTask,
    PodcastResult, PodcastTask, ResilienceConfig, ResilientCronJob, ResilientExecutor,
    ResilientTask, TaskContext, TaskOutcome,
};

// =============================================================================
// Daemon Subsystem Exports (Phase 3+4: Proactive AI)
// =============================================================================

pub use crate::daemon::{
    DaemonCli, DaemonCommand, DaemonConfig, DaemonEventBus, DaemonStatus,
};

// WorldModel (Phase 3)
pub use crate::daemon::worldmodel::{
    ActivityType, CoreState, EnhancedContext, WorldModel, WorldModelConfig,
};

// Dispatcher (Phase 4) - Note: Using ProactiveDispatcher* to avoid conflict with tool system
pub use crate::daemon::dispatcher::{
    ActionExecutor, ActionType, Dispatcher as ProactiveDispatcher,
    DispatcherConfig as ProactiveDispatcherConfig, DispatcherMode,
    NotificationPriority, Policy, PolicyEngine, ProposedAction, RiskLevel,
};

// Events
pub use crate::daemon::events::{
    DaemonEvent, DerivedEvent, FsEventType, PressureLevel, PressureType, ProcessEventType,
    RawEvent, SystemEvent, SystemStateType, TimeTrigger,
};

// =============================================================================
// Memory & Search Exports
// =============================================================================

pub use crate::resilience::database::MemoryStats;
pub use crate::search::{ProviderTestResult, SearchProviderTestConfig};

// =============================================================================
// Vision & Generation Exports
// =============================================================================

pub use crate::generation::{GenerationProvider, GenerationProviderRegistry, GenerationType};

// Media Pipeline Exports
pub use crate::media::{
    AudioFormat, DocFormat, MediaChunk, MediaError, MediaImageFormat, MediaInput, MediaOutput,
    MediaPipeline, MediaPolicy, MediaProvider, MediaType, VideoFormat,
};

// =============================================================================
// Conversation Exports
// =============================================================================

pub use crate::conversation::{ConversationManager, ConversationSession, ConversationTurn};

// =============================================================================
// Group Chat Exports
// =============================================================================

pub use crate::group_chat::{
    GroupChatExecutor,
    GroupChatOrchestrator, GroupChatSession, PersonaRegistry,
    GroupChatCommandParser, GroupChatRenderer,
    GroupChatError, GroupChatMessage, GroupChatRequest, GroupChatStatus,
    Persona, PersonaSource, Speaker, RenderedContent, ContentFormat,
    CoordinatorPlan, RespondentPlan,
};

// =============================================================================
// Provider Exports
// =============================================================================

pub use crate::providers::AiProvider;

// =============================================================================
// Utility Exports
// =============================================================================

pub use crate::clipboard::{ImageData, ImageFormat};
pub use crate::utils::paths::{get_skills_dir, get_skills_dir_string};
pub use crate::metrics::StageTimer;

// Event handler types (for backward compatibility)
pub use crate::event_handler::{ErrorType, McpServerError, McpStartupReport, ProcessingState};

// Core types (for backward compatibility)
pub use crate::core::{CapturedContext, CompressionStats, MediaAttachment, MemoryEntry};

// =============================================================================
// Initialization Function
// =============================================================================

/// Initialize the tracing subscriber for logging
///
/// This function should be called once at application startup.
/// It configures structured logging with environment-based filtering,
/// daily log file rotation, and automatic PII scrubbing.
pub fn init_logging() {
    if let Err(e) = crate::logging::init_file_logging() {
        eprintln!("Warning: Failed to initialize file logging: {}", e);
        eprintln!("Falling back to console-only logging");

        use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true))
            .try_init();
    }
}

// =============================================================================
// Test Exports
// =============================================================================

#[cfg(test)]
pub use crate::event_handler::MockEventHandler;

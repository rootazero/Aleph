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
pub mod approval;
pub(crate) mod arena;
pub mod browser;
pub mod builtin_tools;
pub mod bundled;
pub mod capability;
pub mod clarification;
mod clipboard;
pub mod command;
pub mod components;
pub mod compressor;
mod config;
pub mod context;
pub mod conversation;
mod core;
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
pub mod mcp;
pub mod media;
pub mod memory;
pub mod metrics;
pub mod payload;
pub mod permission;
pub mod pii;
pub mod prompt;
pub mod providers;
pub mod routing;
pub mod runtimes;
pub mod search;
pub mod skill;

pub(crate) mod supervisor;
pub mod thinker;
pub(crate) mod tool_output;
pub mod tools;
pub mod utils;
pub mod vision;
pub mod wizard;

pub mod daemon;
pub mod resilience;
pub mod resilient;
pub mod scheduler;
pub mod secrets;
pub mod security;
pub(crate) mod sync_primitives;

/// Unified initialization module (re-export for backward compatibility)
pub mod initialization {
    pub use crate::init_unified::*;
}

pub mod a2a;
pub mod acp;
pub mod clawhub;
pub mod gateway;
pub mod group_chat;
pub mod tasks;
pub mod teams;

#[cfg(test)]
mod tests;

// =============================================================================
// Core API Exports
// =============================================================================

// Error types (always needed)
pub use crate::error::{AlephError, AlephException, Result};

// Configuration (main entry points and commonly used types)
pub use crate::config::{
    agent_manager::AgentManager,
    agent_resolver::{AgentDefinitionResolver, ResolvedAgent},
    backup::ConfigBackup,
    guides::deploy_guides,
    patcher::ConfigPatcher,
    policies::CompressionPolicy,
    types::acp::{AcpConfig, AcpHarnessEntry, HarnessModeSerde, OutputFormatSerde},
    types::generation::GenerationConfig,
    BehaviorConfig, ChannelInstanceConfig, Config, EmbeddingProviderConfig, FullConfig,
    GeneralConfig, GenerationProviderConfig, MemoryConfig, PluginMarketplaceEntry, ProviderConfig,
    RoutingRuleConfig, SmartFlowConfig,
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
    AgentLoop, LoopCallback, LoopConfig as AgentLoopConfig, LoopRunResult,
};

// Thinker (LLM layer - provider registry)
pub use crate::thinker::{
    MultiProviderRegistry, ProviderRegistry, SingleProviderRegistry, SwappableProviderRegistry,
};

// =============================================================================
// Tool System Exports
// =============================================================================

// Unified tool traits
pub use crate::tools::{AlephTool, AlephToolDyn, AlephToolServer, AlephToolServerHandle};

// Dispatcher (tool registry)
pub use crate::dispatcher::{
    ToolCategory, ToolDefinition, ToolRegistry, ToolResult, ToolSafetyLevel, ToolSource,
    ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// Tool Index (Tool-as-Resource)
pub use crate::dispatcher::tool_index::{
    HydratedTool,
    // Retrieval
    HydrationLevel,
    // Inference
    InferredPurpose,
    SemanticPurposeInferrer,
    // Coordinator
    ToolIndexCoordinator,
    ToolMeta,
    ToolRetrieval,
    // Config
    ToolRetrievalConfig,
};

// =============================================================================
// Extension System Exports
// =============================================================================

pub use crate::extension::{
    ExtensionConfig, ExtensionError, ExtensionManager, ExtensionResult, LoadSummary, PluginInfo,
    SyncExtensionManager,
};

// =============================================================================
// Skills & MCP Exports
// =============================================================================

pub use crate::skill::SkillInfo;

pub use crate::skill::{
    InstallExecutor, InstallResult, SkillConfigUpdate, SkillStatusEntry, SkillStatusFilter,
    SkillSystem, SkillsConfig,
};

pub use crate::mcp::{
    McpServerConfig, McpServerStatus, McpServerStatusInfo, McpServerType, McpToolInfo,
};

// =============================================================================
// Exec Security Exports
// =============================================================================

pub use crate::exec::{
    analyze_shell_command, decide_exec_approval, match_allowlist, ApprovalDecision,
    ApprovalRequest, ExecApprovalManager, ExecContext, PendingApproval, SecurityKernel,
};

// =============================================================================
// Supervisor Exports
// =============================================================================

// =============================================================================
// Wizard Exports
// =============================================================================

pub use crate::wizard::{
    WizardFlow, WizardPrompter, WizardSession, WizardSessionError, WizardStatus,
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

pub use crate::daemon::{DaemonCli, DaemonCommand, DaemonConfig, DaemonEventBus, DaemonStatus};

// WorldModel (Phase 3)
pub use crate::daemon::worldmodel::{
    ActivityType, CoreState, EnhancedContext, WorldModel, WorldModelConfig,
};

// Dispatcher (Phase 4) - Note: Using ProactiveDispatcher* to avoid conflict with tool system
pub use crate::daemon::dispatcher::{
    ActionExecutor, ActionType, Dispatcher as ProactiveDispatcher,
    DispatcherConfig as ProactiveDispatcherConfig, DispatcherMode, NotificationPriority, Policy,
    PolicyEngine, ProposedAction, RiskLevel,
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

pub use crate::generation::{
    GenerationProvider, GenerationProviderRegistry, GenerationType, VoiceInfo,
};

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
// Provider Exports
// =============================================================================

pub use crate::providers::AiProvider;

// =============================================================================
// Utility Exports
// =============================================================================

pub use crate::clipboard::{ImageData, ImageFormat};
pub use crate::metrics::StageTimer;
pub use crate::utils::paths::{get_skills_dir, get_skills_dir_string};

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

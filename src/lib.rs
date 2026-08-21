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
// Consciously-accepted clippy lints. These flag patterns that are either
// idiomatic for this codebase or whose "fix" is a risky/large refactor that
// would change runtime behavior or churn hundreds of call sites — neither of
// which earns its keep:
//   * module_inception      — `mod foo { mod foo }` is our deliberate domain
//                             layout; renaming would churn every import.
//   * too_many_arguments    — DI-heavy boot/builder functions; bundling args
//                             into a struct is pure churn with no clarity win.
//   * type_complexity       — a handful of trait-object/future signatures that
//                             read clearer inline than behind an alias.
//   * await_holding_lock    — short-lived `std::sync` guards held across an
//                             await on the multi-threaded tokio runtime; each
//                             site was reviewed as non-contending. Revisit if
//                             a lock ever moves onto a hot contended path.
#![allow(
    clippy::module_inception,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::await_holding_lock
)]

// =============================================================================
// Module Declarations
// =============================================================================

pub mod agents;
pub mod approval;
pub mod artifacts;
pub mod browser;
pub mod builtin_tools;
pub mod bundled;
pub mod canvas;
pub mod clarification;
pub mod cli;
pub mod cluster;

pub mod command;
mod config;
pub mod context;
pub mod diagnostics;
pub mod discovery;
pub mod domain;
mod error;
pub mod event;
mod event_handler;
pub mod exec;
pub mod executor;
pub mod export;
pub mod extension;
pub mod fetch;
pub mod generation;
pub mod goal;
pub mod guardrails;
pub mod hub;
pub mod identity;
pub mod json_canvas_io;
pub mod logging;
pub mod loop_graph;
pub mod looping;
pub mod markdown;
pub mod mcp;
pub mod media;
pub mod memory;
pub mod metrics;
pub mod orchestrator;
pub mod pii;
pub mod pricing;
pub mod projects;
pub mod providers;
pub mod routing;
pub mod runtimes;
pub mod sandbox;
pub mod scope;
pub mod search;
pub mod session;
pub mod skill;
pub mod spend;
pub mod strategy;
pub mod tool_metadata;

pub mod harness;
pub mod thinker;
pub(crate) mod tool_output;
pub mod tools;
pub mod utils;
pub mod verification;
pub mod vision;
pub mod wizard;
pub mod workflow;

pub mod resilience;
pub mod secrets;
pub mod security;
pub mod sync_primitives;

pub mod a2a;
pub mod acp;
pub mod gateway;
pub mod group_chat;
pub mod tasks;
pub mod teams;

// =============================================================================
// Core API Exports
// =============================================================================

// Error types (always needed)
pub use crate::error::{AlephError, AlephException, Result};

// Configuration (main entry points and commonly used types)
pub use crate::config::{
    agent_manager::AgentManager,
    agent_resolver::{agents_root_for, workspace_root_for, AgentDefinitionResolver, ResolvedAgent},
    backup::ConfigBackup,
    guides::deploy_guides,
    patcher::ConfigPatcher,
    policies::CompressionPolicy,
    types::acp::{AcpAdapterEntry, AcpConfig, AdapterModeSerde, OutputFormatSerde},
    types::generation::GenerationConfig,
    types::memory::{DreamingConfig, MemoryDecayPolicy},
    types::phase6_wiring::{
        ContextBudgetToml, FallbackProviderToml, GuardrailsToml, ModelThresholdToml, StabilityToml,
        StrategyToml,
    },
    types::privacy::{PiiAction, PlatformPiiPolicy, PrivacyConfig},
    types::resume::ResumeConfig,
    types::security::ShellSecurityConfig,
    types::stop_hooks::StopHookConfig,
    types::voice_local::LOCAL_PROVIDER_TYPE,
    AssemblerConfig, BehaviorConfig, ChannelInstanceConfig, Config, EmbeddingProviderConfig,
    GeneralConfig, GenerationProviderConfig, MemoryConfig, MemoryInjectionMode,
    PluginMarketplaceEntry, ProviderConfig, RoutingRuleConfig,
};

// Logging
pub use crate::logging::LogLevel;

// =============================================================================
// Agent System Exports
// =============================================================================

// Thinker (LLM layer - provider registry)
pub use crate::thinker::{
    MultiProviderRegistry, ProviderRegistry, SingleProviderRegistry, SwappableProviderRegistry,
};

// =============================================================================
// Tool System Exports
// =============================================================================

// Unified tool traits
pub use crate::tools::{AlephTool, AlephToolDyn, AlephToolServer};

// Tool Metadata (registry)
pub use crate::tool_metadata::{
    ToolCatalog, ToolCategory, ToolDefinition, ToolSafetyLevel, ToolSource, ToolSourceType,
    UnifiedTool,
};

// =============================================================================
// Extension System Exports
// =============================================================================

pub use crate::extension::{
    ExtensionConfig, ExtensionError, ExtensionManager, ExtensionResult, LoadSummary, PluginInfo,
};

// =============================================================================
// Skills & MCP Exports
// =============================================================================

pub use crate::skill::SkillInfo;

pub use crate::skill::{
    InstallExecutor, InstallResult, SkillConfigUpdate, SkillStatusEntry, SkillStatusFilter,
    SkillSystem, SkillsConfig,
};

// =============================================================================
// Exec Security Exports
// =============================================================================

pub use crate::exec::{
    analyze_shell_command, ApprovalRequest, ExecApprovalManager, PendingApproval, SecurityKernel,
};

// =============================================================================
// Memory & Search Exports
// =============================================================================

// =============================================================================
// Vision & Generation Exports
// =============================================================================

pub use crate::generation::{
    GenerationProvider, GenerationProviderRegistry, GenerationType, VoiceInfo,
};

// Media Pipeline Exports
pub use crate::media::{
    AudioFormat, DocFormat, MediaError, MediaImageFormat, MediaInput, MediaOutput, MediaPipeline,
    MediaPolicy, MediaProvider, MediaType, VideoFormat,
};

// =============================================================================
// Provider Exports
// =============================================================================

pub use crate::providers::AiProvider;

// =============================================================================
// Utility Exports
// =============================================================================

pub use crate::metrics::StageTimer;
pub use crate::utils::paths::get_skills_dir;

// Event handler types (for backward compatibility)
pub use crate::event_handler::{ErrorType, McpServerError, McpStartupReport, ProcessingState};

// Aether Core Library
//
//! Aether is a system-level AI middleware that acts as an invisible "ether"
//! connecting user intent with AI models through a frictionless, native interface.
//!
//! # Architecture
//!
//! The core library is built as a headless service (cdylib/staticlib) that exposes
//! a clean FFI boundary via UniFFI. Native clients (Swift on macOS, C# on Windows,
//! GTK on Linux) communicate with this core to access hotkey detection, clipboard
//! management, and AI routing functionality.
//!
//! # Core Components
//!
//! - **AetherCore**: Main entry point that orchestrates all subsystems
//! - **HotkeyListener**: Global hotkey detection (Cmd+~ on macOS)
//! - **ClipboardManager**: Clipboard read/write operations
//! - **AetherEventHandler**: Callback trait for Rust → Client communication
//! - **ProcessingState**: State machine for UI feedback
//!
//! # Usage Example
//!
//! ```rust,no_run
//! use aethecore::*;
//!
//! // Client implements AetherEventHandler trait
//! struct MyHandler;
//! impl AetherEventHandler for MyHandler {
//!     fn on_state_changed(&self, state: ProcessingState) {
//!         println!("State: {:?}", state);
//!     }
//!     fn on_hotkey_detected(&self, content: String) {
//!         println!("Hotkey! Clipboard: {}", content);
//!     }
//!     fn on_error(&self, message: String) {
//!         eprintln!("Error: {}", message);
//!     }
//! }
//!
//! // Create core with handler (Box required for UniFFI)
//! let handler = Box::new(MyHandler);
//! let core = AetherCore::new(handler).unwrap();
//!
//! // Start listening for Cmd+~
//! core.start_listening().unwrap();
//!
//! // ... when done
//! core.stop_listening().unwrap();
//! ```
//!
//! # Phase 1 Scope
//!
//! This initial implementation provides:
//! - ✅ Working hotkey detection (Cmd+~ hardcoded)
//! - ✅ Working clipboard reading (text only)
//! - ✅ UniFFI interface for Swift/Kotlin/C# bindings
//! - ✅ Callback-based event system
//! - ✅ Trait-based architecture for testability
//!
//! Future phases will add:
//! - Phase 2: Keyboard simulation (Cmd+X, Cmd+V)
//! - Phase 3: Halo overlay integration
//! - Phase 4: AI provider clients (OpenAI, Claude, Gemini, Ollama)
//! - Phase 4: Smart routing and configuration

// Allow clippy lints for UniFFI generated code
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]
#![allow(unpredictable_function_pointer_comparisons)]

// Module declarations
// NOTE: clipboard module retained for ImageData/ImageFormat types (used by AI providers)
// Clipboard operations are handled by Swift ClipboardManager
pub mod capability;
pub mod clarification; // NEW: Phantom Flow interaction types
mod clipboard;
pub mod conversation; // NEW: Multi-turn conversation support
pub mod command; // NEW: Command completion system
mod config;
mod core;
mod error;
mod event_handler;
pub mod initialization;
pub mod intent; // NEW: Smart intent detection for conversation flow
pub mod logging;
pub mod memory;
pub mod metrics;
pub mod payload; // Structured context protocol with capability support
pub mod providers;
pub mod router;
pub mod search; // NEW: Search capability with multiple provider support
pub mod skills; // NEW: Claude Agent Skills support
pub mod semantic; // NEW: Unified semantic detection system
pub mod coordination; // NEW: Conversation-aware routing coordination layer
pub mod suggestion; // NEW: AI response suggestion parsing
pub mod utils; // NEW: Capability executor for enriching payloads
pub mod video; // NEW: Video transcript extraction (YouTube)
pub mod services; // NEW: Shared foundation services (FileOps, GitOps, SystemInfo)
pub mod mcp; // NEW: MCP (Model Context Protocol) capability
pub mod agent; // NEW: Agent loop for tool calling
pub mod dispatcher; // NEW: Intelligent tool routing (Dispatcher Layer)
pub mod routing; // NEW: Unified multi-layer routing framework
mod title_generator; // Title generation for conversation topics
pub mod tools; // NEW: Native function calling tools (AgentTool trait)
pub mod vision; // NEW: Vision capability (screen OCR, image understanding)

// Integration tests module
#[cfg(test)]
mod tests;

// Re-export public types
// NOTE: ImageData/ImageFormat still exported for AI provider image encoding
pub use crate::clipboard::{ImageData, ImageFormat};
pub use crate::command::{CommandExecutionResult, CommandNode, CommandRegistry, CommandType};
pub use crate::config::{
    BehaviorConfig, Config, ContextRuleConfig, DispatcherConfigToml, FullConfig, GeneralConfig,
    IntentDetectionConfig, KeywordRuleConfig, MemoryConfig, PIIConfig, ProviderConfig,
    ProviderConfigEntry, RoutingRuleConfig, SearchBackendConfig, SearchBackendEntry, SearchConfig,
    SearchConfigInternal, ShortcutsConfig, SkillsConfig, SmartFlowConfig, SmartMatchingConfig,
    SuggestionParsingConfig, TestConnectionResult, TriggerConfig, VideoConfig,
};
pub use crate::core::{
    AetherCore, AppMemoryInfo, CapturedContext, CompressionStats, MediaAttachment,
    MemoryEntryFFI as MemoryEntry,
};
pub use crate::memory::context::CompressionResult;
pub use crate::error::{AetherError, AetherException, Result};
pub use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
pub use crate::initialization::{
    check_embedding_model_exists, delete_skill, download_embedding_model_standalone,
    get_skills_dir, get_skills_dir_string, initialize_builtin_skills, initialize_builtin_skills_ffi,
    install_skill_from_url, install_skills_from_zip, is_fresh_install, list_installed_skills,
    run_first_time_init, InitializationProgressHandler,
};
pub use crate::logging::{create_pii_scrubbing_layer, LogLevel, PiiScrubbingLayer};
pub use crate::memory::database::MemoryStats;
pub use crate::metrics::StageTimer;
pub use crate::providers::AiProvider;
pub use crate::router::{Router, RoutingRule};
pub use crate::search::{ProviderTestResult, SearchProviderTestConfig};
pub use crate::clarification::{
    ClarificationOption, ClarificationRequest, ClarificationResult, ClarificationResultType,
    ClarificationType,
};
pub use crate::conversation::{ConversationManager, ConversationSession, ConversationTurn};
pub use crate::intent::{AiIntentDetector, AiIntentResult};
pub use crate::coordination::{ConversationAwareRouter, RoutingContext, RoutingResult};
pub use crate::suggestion::{ParsedSuggestions, SuggestionOption, SuggestionParser};
pub use crate::skills::{Skill, SkillInfo, SkillsInstaller, SkillsRegistry};
pub use crate::mcp::{
    McpEnvVar, McpServerConfig, McpServerPermissions, McpServerStatus, McpServerStatusInfo,
    McpServerType, McpServiceInfo, McpSettingsConfig, McpToolInfo,
};
pub use crate::dispatcher::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationAction, ConfirmationConfig,
    ConfirmationDecision, ConfirmationState, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult, L3RoutingOptions, L3RoutingResponse, L3RoutingResult,
    L3Router, PendingConfirmation, PendingConfirmationInfo, PendingConfirmationStore,
    PromptBuilder, PromptFormat, RoutingLayer, ToolConfirmation, ToolFilter, ToolRegistry,
    ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo, UserConfirmationDecision,
};
pub use crate::routing::{
    RoutingConfig, RoutingContext as UnifiedRoutingContext, RoutingLayerType, RoutingMatch,
    RoutingResult as UnifiedRoutingResult, UnifiedRouter,
};
pub use crate::tools::{
    // Core types
    AgentTool, NativeToolRegistry, ToolCategory, ToolDefinition, ToolResult,
    // Filesystem tools
    create_filesystem_tools, FileDeleteTool, FileListTool, FileReadTool, FileSearchTool,
    FileWriteTool, FilesystemConfig, FilesystemContext,
    // Git tools
    create_git_tools, GitBranchTool, GitConfig, GitContext, GitDiffTool, GitLogTool, GitStatusTool,
    // Shell tools
    create_shell_tools, ShellConfig, ShellContext, ShellExecuteTool,
    // System tools
    create_system_tools, SystemContext, SystemInfoTool,
    // Clipboard tools
    create_clipboard_tools, ClipboardContent, ClipboardContext, ClipboardReadTool,
    // Screen tools
    create_screen_tools, ScreenCaptureTool, ScreenConfig, ScreenContext,
    // Search tools
    create_search_tools, SearchConfig as SearchToolConfig, SearchContext, WebSearchTool,
};
pub use crate::utils::pii;
pub use crate::vision::{
    CaptureMode, VisionConfig, VisionRequest, VisionResult, VisionService, VisionTask,
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

// Include UniFFI scaffolding
// This generates all the FFI glue code at compile time
uniffi::include_scaffolding!("aether");

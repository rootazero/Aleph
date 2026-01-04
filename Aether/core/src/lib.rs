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
mod clipboard;
mod config;
mod core;
mod error;
mod event_handler;
pub mod initialization;
pub mod logging;
pub mod memory;
pub mod metrics;
pub mod prompt; // NEW: Structured context protocol
pub mod providers;
pub mod router;
pub mod utils;

// Re-export public types
// NOTE: ImageData/ImageFormat still exported for AI provider image encoding
pub use crate::clipboard::{ImageData, ImageFormat};
pub use crate::config::{
    BehaviorConfig, Config, FullConfig, GeneralConfig, MemoryConfig,
    ProviderConfig, ProviderConfigEntry, RoutingRuleConfig, ShortcutsConfig,
    TestConnectionResult,
};
pub use crate::core::{AetherCore, AppMemoryInfo, CapturedContext, MemoryEntryFFI as MemoryEntry};
pub use crate::error::{AetherError, AetherException, Result};
pub use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
pub use crate::initialization::{
    check_embedding_model_exists, download_embedding_model_standalone,
    is_fresh_install, run_first_time_init, InitializationProgressHandler,
};
pub use crate::logging::{create_pii_scrubbing_layer, LogLevel, PiiScrubbingLayer};
pub use crate::memory::database::MemoryStats;
pub use crate::metrics::StageTimer;
pub use crate::providers::AiProvider;
pub use crate::router::{Router, RoutingRule};
pub use crate::utils::pii;

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
            let filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info"));

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

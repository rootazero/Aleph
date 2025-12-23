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
mod clipboard;
mod config;
mod core;
mod error;
mod event_handler;
mod hotkey;
mod input;

// Re-export public types
pub use crate::clipboard::{ArboardManager, ClipboardManager};
pub use crate::config::Config;
pub use crate::core::AetherCore;
pub use crate::error::{AetherError, Result};
pub use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
pub use crate::hotkey::{HotkeyListener, RdevListener};
pub use crate::input::InputSimulator;

// Include UniFFI scaffolding
// This generates all the FFI glue code at compile time
uniffi::include_scaffolding!("aether");

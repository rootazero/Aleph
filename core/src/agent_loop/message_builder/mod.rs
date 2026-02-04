//! Message Builder - Converts SessionParts to LLM messages
//!
//! This module implements the message building pipeline that converts
//! ExecutionSession parts to the message format expected by LLM providers,
//! including system reminder injection.
//!
//! # Message Flow
//!
//! ```text
//! ExecutionSession.parts → filter_compacted() → parts_to_messages() → inject_reminders()
//!                                    ↓                    ↓                   ↓
//!                             [filtered parts]    [base messages]    [final messages]
//! ```
//!
//! # System Reminder Injection
//!
//! Following OpenCode's pattern, system reminders are injected by wrapping
//! the last user message with `<system-reminder>` tags when:
//! - iteration_count > reminder_threshold
//! - There are pending reminders in the session
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::agent_loop::{MessageBuilder, MessageBuilderConfig};
//! use alephcore::components::ExecutionSession;
//!
//! let config = MessageBuilderConfig::default();
//! let builder = MessageBuilder::new(config);
//!
//! let session = ExecutionSession::new();
//! let messages = builder.build_messages(&session, &session.parts);
//! ```

mod config;
mod types;
mod builder;

#[cfg(test)]
mod tests;

// Re-export public API
pub use config::MessageBuilderConfig;
pub use types::{Message, ToolCall};
pub use builder::MessageBuilder;

//! Reply Emitter - Routes Agent output back to channels
//!
//! The ReplyEmitter implements EventEmitter to capture streaming events from the
//! agent loop and route responses back to the originating channel/conversation.
//!
//! # Output Modes
//!
//! - **Streaming** (`stream_enabled = true`): Sends an initial message once a
//!   character threshold is reached, then progressively edits in real-time as
//!   tokens arrive (debounced). Uses `StreamingController` for state management.
//! - **Instant** (`stream_enabled = false`): Buffers all content, sends once on
//!   completion.
//!
//! Mode is controlled by `BehaviorConfig.output_mode` in config.toml.

mod config;
mod emitter;
mod sanitize;

#[cfg(test)]
mod tests;

pub use config::ReplyEmitterConfig;
pub use emitter::ReplyEmitter;
pub(crate) use sanitize::sanitize_llm_output;

#[cfg(test)]
pub(crate) use sanitize::split_reasoning;

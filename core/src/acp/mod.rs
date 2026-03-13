//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP harnesses.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

pub mod protocol;
pub mod session;
pub mod transport;

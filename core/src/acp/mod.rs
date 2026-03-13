//! ACP (Agent Client Protocol) module
//!
//! Manages external CLI tools (Claude Code, Codex, Gemini) as ACP harnesses.
//! Supports Tool mode (LLM-dispatched) and Agent mode (direct conversation).

pub mod harness;
pub mod harnesses;
pub mod manager;
#[cfg(test)]
pub mod mock_server;
pub mod protocol;
pub mod session;
pub mod transport;

#[cfg(test)]
mod tests;

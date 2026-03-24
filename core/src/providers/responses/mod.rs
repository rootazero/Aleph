//! Shared types and logic for the OpenAI Responses API wire format.
//!
//! Used by both the standard OpenAI Responses protocol (`/v1/responses`)
//! and the Codex protocol (`chatgpt.com/backend-api/codex/responses`).

pub mod types;
pub mod shared;

pub use types::*;
pub use shared::*;

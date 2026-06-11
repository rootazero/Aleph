//! Codex subscription provider
//!
//! Browser OAuth authentication for Codex subscription (`ChatGPT`) accounts.
//! The wire protocol is handled by the shared Responses API adapter via
//! `ResponsesVariant::codex()`, which targets
//! `chatgpt.com/backend-api/codex/responses`.

pub mod auth;

pub use auth::CodexAuth;

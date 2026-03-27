//! OpenAI-compatible API — dual-mode chat completions gateway.

pub mod auth;
pub mod completions;
pub mod models;
pub mod router;
pub mod state;
pub mod stream;
pub mod types;

pub use router::openai_routes;
pub use state::OpenAiApiState;

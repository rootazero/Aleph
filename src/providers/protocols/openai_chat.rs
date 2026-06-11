//! `OpenAI` protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: `OpenAI`, `DeepSeek`, Moonshot, Doubao, vLLM, etc.

use crate::sync_primitives::Arc;
use reqwest::Client;

// ── Tool-name sanitization ──────────────────────────────────────────────
//
// OpenAI API requires tool names to match `^[a-zA-Z0-9_-]+$`.
// Aleph tool names now use underscores (e.g. "cron_manage"), but this
// sanitizer is kept as a safety net for any external/plugin tool names.

use super::openai_common::tools::sanitize_tool_name;

/// `OpenAI` protocol adapter
pub struct OpenAiProtocol {
    client: Client,
    /// Idle timeout (seconds) for the SSE byte stream, resolved from
    /// `ProviderConfig.stream_idle_timeout_secs` in `build_request` and read
    /// in `stream_deltas`. An `AtomicU64` because `&self` is shared (`Arc`)
    /// and the value must cross into the `'static` stream closure.
    stream_idle_timeout_secs: Arc<crate::sync_primitives::AtomicU64>,
}

mod adapter;
mod proto_impl;
mod sse;

#[cfg(test)]
mod tests;

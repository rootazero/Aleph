//! OpenAI protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: OpenAI, DeepSeek, Moonshot, Doubao, vLLM, etc.

use reqwest::Client;

// ── Tool-name sanitization ──────────────────────────────────────────────
//
// OpenAI API requires tool names to match `^[a-zA-Z0-9_-]+$`.
// Aleph tool names now use underscores (e.g. "cron_manage"), but this
// sanitizer is kept as a safety net for any external/plugin tool names.

use super::openai_common::tools::sanitize_tool_name;

/// OpenAI protocol adapter
pub struct OpenAiProtocol {
    client: Client,
}

mod proto_impl;
mod adapter;
mod sse;

#[cfg(test)]
mod tests;

//! Google Gemini protocol adapter
//!
//! Handles Google Generative AI API format.

use crate::sync_primitives::Arc;
use reqwest::Client;

/// Google Gemini protocol adapter
pub struct GeminiProtocol {
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

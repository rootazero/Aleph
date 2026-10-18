//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::config::ProviderConfig;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

use super::delta::ProviderDelta;

// These are the API-owned types, not parallel Core definitions.
pub use aleph_slot_api::ai::{RequestPayload, ToolChoice};
pub use aleph_slot_api::ai_response::{
    NativeToolCall, ProviderResponse, StopReason, TokenCost, TokenUsage,
};

/// Protocol adapter trait for building requests and streaming responses
///
/// Each protocol (`OpenAI`, Anthropic, Gemini, etc.) implements this trait
/// to handle protocol-specific serialization and deserialization.
///
/// All adapters are stream-first: `stream_deltas()` is the only required
/// output path. Non-streaming and legacy `parse_response()` / `parse_stream()`
/// paths have been removed.
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build an HTTP request from the payload.
    ///
    /// All requests are stream-first: adapters should configure the request
    /// for streaming output (e.g. `"stream": true` for OpenAI-compatible APIs).
    ///
    /// # Arguments
    /// * `payload` - The unified request payload
    /// * `config` - Provider configuration (API key, model, etc.)
    ///
    /// # Returns
    /// A configured `reqwest::RequestBuilder` ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder>;

    /// Whether this protocol supports native `tool_use`
    ///
    /// Protocols that support native tool calling (e.g., Anthropic, `OpenAI`)
    /// return `true` to enable tool extraction from delta streams.
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Whether this protocol supports strict JSON schema mode
    fn supports_strict_schema(&self) -> bool {
        false
    }

    /// Canonicalise a user-supplied model id to the form this protocol expects.
    ///
    /// Mirrors openclaw's `ProviderPlugin.normalizeModelId` hook — lets each
    /// protocol accept common alias forms (`gemini-pro-1.5` → `gemini-1.5-pro`,
    /// `claude-3.5-sonnet` → `claude-3-5-sonnet`, `gpt4o` → `gpt-4o`) without
    /// the caller knowing the canonical wire name. Default impl passes through.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        std::borrow::Cow::Borrowed(model_id)
    }

    /// Stream fine-grained delta events from an HTTP response.
    ///
    /// All adapters must implement this method directly — there is no default
    /// bridge implementation. Each adapter parses its own SSE/streaming format
    /// and emits typed [`ProviderDelta`] events.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>>;

    /// Get the protocol name for logging
    fn name(&self) -> &'static str;

    /// The wire family this adapter speaks — the key for per-family policy
    /// (reasoning replay, model-behaviour fallback). Defaults to
    /// [`name`](Self::name); an adapter that delegates its wire to another (a
    /// YAML protocol that `extends` a built-in) reports that one's family, so
    /// a user-chosen protocol name never reads as "unknown protocol".
    fn wire_family(&self) -> &'static str {
        self.name()
    }

    /// Whether a request built from `config` asks the server to clear old tool
    /// results itself (Anthropic `context_management`). The same derivation
    /// `build_request` uses to put the edit on the wire; the local passes that
    /// rewrite old tool results stand down when it is true. Default `false`.
    fn clears_tool_results_server_side(&self, _config: &ProviderConfig) -> bool {
        false
    }
}

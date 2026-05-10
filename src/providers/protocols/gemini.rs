//! Google Gemini protocol adapter
//!
//! Handles Google Generative AI API format.

use reqwest::Client;

/// Google Gemini protocol adapter
pub struct GeminiProtocol {
    client: Client,
}

mod proto_impl;
mod adapter;
mod sse;

#[cfg(test)]
mod tests;

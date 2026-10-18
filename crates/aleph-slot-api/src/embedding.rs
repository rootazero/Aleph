//! Embedding provider contract — the pure trait half of what was
//! `alephcore::memory::embedding_provider`.
//!
//! This module owns only the [`EmbeddingProvider`] trait, moved here in
//! Task 3 of the P0 modularization. Everything with a dependency the API
//! crate may not take stays in `alephcore::memory::embedding_provider`:
//!
//! * `RemoteEmbeddingProvider` (reqwest client, URL/SSRF validation,
//!   `create_provider` factory),
//! * `truncate_and_normalize` (an implementation helper of the remote
//!   provider's response path),
//! * the test-only `MockEmbeddingProvider`.
//!
//! The trait returns [`crate::error::AlephError`] — the workspace's single
//! error authority since Task 1 — so Core implementations and API-side
//! consumers name the same error type.
//!
//! `alephcore::memory::embedding_provider` re-exports this trait; it is a
//! compatibility shim, not a second definition.

use crate::error::AlephError;

/// Abstract embedding provider
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "BAAI/bge-m3")
    fn model_name(&self) -> &str;

    /// Get the provider id (e.g., "siliconflow")
    fn provider_id(&self) -> &str;
}

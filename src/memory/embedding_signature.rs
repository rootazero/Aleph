//! Embedding vector-space signature.
//!
//! Note vectors are only comparable when produced by the same embedding model.
//! Switching the active provider/model (or its output dimension) silently
//! invalidates every stored vector: new query vectors land in a different space
//! and cosine similarity against the old vectors becomes meaningless — recall
//! degrades with no error raised. The kernel cannot tell from the vectors alone
//! which model produced them, so it records a signature of the space the current
//! vectors live in (after each `reembed_all`) and compares it against the active
//! provider to recommend a reembed when they diverge.

use crate::memory::EmbeddingProvider;

/// Stable identifier of an embedding vector space: `provider:model:dim`.
///
/// The provider id is part of the signature because two providers serving the
/// "same" model name are not guaranteed to produce identical weights — a true
/// passthrough keeps the triple stable, an independent implementation does not.
#[must_use]
pub fn embedding_signature(provider_id: &str, model_name: &str, dim: usize) -> String {
    format!("{provider_id}:{model_name}:{dim}")
}

/// Signature of a live provider.
pub fn provider_signature(provider: &dyn EmbeddingProvider) -> String {
    embedding_signature(
        provider.provider_id(),
        provider.model_name(),
        provider.dimensions(),
    )
}

/// Outcome of comparing the stored signature against the active provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// No signature recorded yet (fresh store, or data predating this feature).
    /// Compatibility cannot be judged — stay silent.
    Unknown,
    /// Stored signature matches the active provider — vectors are comparable.
    Match,
    /// The active provider differs from the one that produced the stored
    /// vectors — a reembed is recommended.
    Mismatch { stored: String, current: String },
}

/// Compare a stored signature against the current provider's signature.
#[must_use]
pub fn compare(stored: Option<&str>, current: &str) -> SignatureStatus {
    match stored {
        None => SignatureStatus::Unknown,
        Some(s) if s == current => SignatureStatus::Match,
        Some(s) => SignatureStatus::Mismatch {
            stored: s.to_string(),
            current: current.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_provider_model_dim() {
        assert_eq!(
            embedding_signature("openai", "text-embedding-3-small", 1536),
            "openai:text-embedding-3-small:1536"
        );
    }

    #[test]
    fn compare_none_is_unknown() {
        assert_eq!(compare(None, "openai:m:1536"), SignatureStatus::Unknown);
    }

    #[test]
    fn compare_equal_is_match() {
        assert_eq!(
            compare(Some("openai:m:1536"), "openai:m:1536"),
            SignatureStatus::Match
        );
    }

    #[test]
    fn compare_different_is_mismatch() {
        // Same model name, different provider → still a mismatch (no guarantee
        // the two providers produce identical weights).
        assert_eq!(
            compare(
                Some("openai:text-embedding-3-small:1536"),
                "azure:text-embedding-3-small:1536"
            ),
            SignatureStatus::Mismatch {
                stored: "openai:text-embedding-3-small:1536".to_string(),
                current: "azure:text-embedding-3-small:1536".to_string(),
            }
        );
    }

    #[test]
    fn compare_dimension_change_is_mismatch() {
        assert!(matches!(
            compare(Some("openai:m:1536"), "openai:m:3072"),
            SignatureStatus::Mismatch { .. }
        ));
    }
}

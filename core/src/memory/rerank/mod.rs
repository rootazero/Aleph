//! Cross-encoder reranking module
//!
//! Provides HTTP-based cross-encoder reranking with 5 provider backends:
//! Jina, SiliconFlow, Voyage, Pinecone, and vLLM.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use alephcore::memory::rerank::{build_provider, blend_scores};
//! use alephcore::memory::rerank::provider::RerankConfig;
//!
//! let config = RerankConfig::default();
//! let provider = build_provider(&config);
//! let results = provider.rerank("query", &docs, 10).await?;
//! let blended = blend_scores(&originals, &results, config.rerank_weight);
//! ```

pub mod provider;
pub mod jina;
pub mod pinecone;
pub mod siliconflow;
pub mod vllm;
pub mod voyage;

pub use provider::{RerankConfig, RerankProvider, RerankProviderType, RerankResult};

use jina::JinaRerankProvider;
use pinecone::PineconeRerankProvider;
use siliconflow::SiliconFlowRerankProvider;
use vllm::VllmRerankProvider;
use voyage::VoyageRerankProvider;

/// Build a rerank provider from configuration
pub fn build_provider(config: &RerankConfig) -> Box<dyn RerankProvider> {
    match config.provider {
        RerankProviderType::Jina => Box::new(JinaRerankProvider::new(config.clone())),
        RerankProviderType::SiliconFlow => {
            Box::new(SiliconFlowRerankProvider::new(config.clone()))
        }
        RerankProviderType::Voyage => Box::new(VoyageRerankProvider::new(config.clone())),
        RerankProviderType::Pinecone => Box::new(PineconeRerankProvider::new(config.clone())),
        RerankProviderType::Vllm => Box::new(VllmRerankProvider::new(config.clone())),
    }
}

/// Blend original retrieval scores with cross-encoder rerank scores
///
/// Formula: `final = rerank_weight * rerank_score + (1 - rerank_weight) * original_score`
///
/// Documents not present in `reranked` results get a rerank_score of 0.0.
/// Results are sorted descending by blended score.
pub fn blend_scores(
    originals: &[(String, f32)],
    reranked: &[RerankResult],
    rerank_weight: f32,
) -> Vec<(String, f32)> {
    let weight = rerank_weight.clamp(0.0, 1.0);
    let original_weight = 1.0 - weight;

    let mut blended: Vec<(String, f32)> = originals
        .iter()
        .enumerate()
        .map(|(i, (doc, orig_score))| {
            let rerank_score = reranked
                .iter()
                .find(|r| r.index == i)
                .map(|r| r.relevance_score)
                .unwrap_or(0.0);

            let final_score = weight * rerank_score + original_weight * orig_score;
            (doc.clone(), final_score)
        })
        .collect();

    blended.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    blended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_scores_basic() {
        let originals = vec![
            ("doc_a".to_string(), 0.9),
            ("doc_b".to_string(), 0.8),
            ("doc_c".to_string(), 0.7),
        ];
        let reranked = vec![
            RerankResult { index: 2, relevance_score: 0.95 },
            RerankResult { index: 0, relevance_score: 0.5 },
        ];

        let result = blend_scores(&originals, &reranked, 0.6);

        assert_eq!(result.len(), 3);
        // doc_c: 0.6*0.95 + 0.4*0.7 = 0.57 + 0.28 = 0.85
        // doc_a: 0.6*0.5  + 0.4*0.9 = 0.30 + 0.36 = 0.66
        // doc_b: 0.6*0.0  + 0.4*0.8 = 0.00 + 0.32 = 0.32
        assert_eq!(result[0].0, "doc_c");
        assert_eq!(result[1].0, "doc_a");
        assert_eq!(result[2].0, "doc_b");
    }

    #[test]
    fn test_blend_scores_zero_weight() {
        let originals = vec![
            ("a".to_string(), 0.5),
            ("b".to_string(), 0.9),
        ];
        let reranked = vec![
            RerankResult { index: 0, relevance_score: 1.0 },
        ];

        let result = blend_scores(&originals, &reranked, 0.0);
        // Pure original scores: b=0.9, a=0.5
        assert_eq!(result[0].0, "b");
        assert_eq!(result[1].0, "a");
    }

    #[test]
    fn test_blend_scores_full_weight() {
        let originals = vec![
            ("a".to_string(), 0.9),
            ("b".to_string(), 0.1),
        ];
        let reranked = vec![
            RerankResult { index: 1, relevance_score: 1.0 },
        ];

        let result = blend_scores(&originals, &reranked, 1.0);
        // Pure rerank scores: b=1.0, a=0.0
        assert_eq!(result[0].0, "b");
        assert_eq!(result[1].0, "a");
    }

    #[test]
    fn test_blend_scores_empty() {
        let result = blend_scores(&[], &[], 0.6);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_provider_variants() {
        let mut config = RerankConfig::default();
        assert_eq!(build_provider(&config).provider_id(), "jina");

        config.provider = RerankProviderType::SiliconFlow;
        assert_eq!(build_provider(&config).provider_id(), "siliconflow");

        config.provider = RerankProviderType::Voyage;
        assert_eq!(build_provider(&config).provider_id(), "voyage");

        config.provider = RerankProviderType::Pinecone;
        assert_eq!(build_provider(&config).provider_id(), "pinecone");

        config.provider = RerankProviderType::Vllm;
        assert_eq!(build_provider(&config).provider_id(), "vllm");
    }

    #[test]
    fn test_default_config() {
        let config = RerankConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider, RerankProviderType::Jina);
        assert_eq!(config.default_model(), "BAAI/bge-reranker-v2-m3");
        assert_eq!(config.timeout_ms, 5000);
        assert!((config.rerank_weight - 0.6).abs() < f32::EPSILON);
    }
}

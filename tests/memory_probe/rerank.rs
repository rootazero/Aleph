//! P3: Cross-Encoder Rerank — 6 scenarios

use alephcore::memory::rerank::{
    blend_scores, build_provider, RerankConfig, RerankProviderType, RerankResult,
};

/// Exact weighted formula: doc_a = 0.6*0.3 + 0.4*0.9 = 0.54, doc_b = 0.6*0.8 + 0.4*0.4 = 0.64
#[test]
fn p3_01_blend_scores_applies_weight_formula() {
    let originals = vec![("doc_a".to_string(), 0.9), ("doc_b".to_string(), 0.4)];
    let reranked = vec![
        RerankResult {
            index: 0,
            relevance_score: 0.3,
        },
        RerankResult {
            index: 1,
            relevance_score: 0.8,
        },
    ];

    let result = blend_scores(&originals, &reranked, 0.6);

    // doc_a: 0.6*0.3 + 0.4*0.9 = 0.18 + 0.36 = 0.54
    // doc_b: 0.6*0.8 + 0.4*0.4 = 0.48 + 0.16 = 0.64
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "doc_b");
    assert!(
        (result[0].1 - 0.64).abs() < 1e-6,
        "doc_b score: {}",
        result[0].1
    );
    assert_eq!(result[1].0, "doc_a");
    assert!(
        (result[1].1 - 0.54).abs() < 1e-6,
        "doc_a score: {}",
        result[1].1
    );
}

/// Document missing from reranked results gets rerank_score = 0.0
#[test]
fn p3_02_blend_scores_missing_rerank_uses_zero() {
    let originals = vec![("doc_a".to_string(), 0.8), ("doc_b".to_string(), 0.6)];
    // Only doc_a has a rerank result; doc_b is missing
    let reranked = vec![RerankResult {
        index: 0,
        relevance_score: 0.9,
    }];

    let result = blend_scores(&originals, &reranked, 0.6);

    // doc_a: 0.6*0.9 + 0.4*0.8 = 0.54 + 0.32 = 0.86
    // doc_b: 0.6*0.0 + 0.4*0.6 = 0.00 + 0.24 = 0.24
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "doc_a");
    assert!(
        (result[0].1 - 0.86).abs() < 1e-6,
        "doc_a score: {}",
        result[0].1
    );
    assert_eq!(result[1].0, "doc_b");
    assert!(
        (result[1].1 - 0.24).abs() < 1e-6,
        "doc_b score: {}",
        result[1].1
    );
}

/// weight=0 → pure original scores preserved
#[test]
fn p3_03_blend_scores_preserves_order_when_weight_zero() {
    let originals = vec![
        ("low".to_string(), 0.3),
        ("high".to_string(), 0.9),
        ("mid".to_string(), 0.6),
    ];
    // Rerank would invert the order, but weight=0 ignores it
    let reranked = vec![
        RerankResult {
            index: 0,
            relevance_score: 1.0,
        },
        RerankResult {
            index: 1,
            relevance_score: 0.1,
        },
        RerankResult {
            index: 2,
            relevance_score: 0.5,
        },
    ];

    let result = blend_scores(&originals, &reranked, 0.0);

    assert_eq!(result[0].0, "high");
    assert!((result[0].1 - 0.9).abs() < 1e-6);
    assert_eq!(result[1].0, "mid");
    assert!((result[1].1 - 0.6).abs() < 1e-6);
    assert_eq!(result[2].0, "low");
    assert!((result[2].1 - 0.3).abs() < 1e-6);
}

/// weight=1.0 → pure rerank scores, original scores ignored
#[test]
fn p3_04_blend_scores_full_rerank_weight_ignores_original() {
    let originals = vec![("a".to_string(), 0.99), ("b".to_string(), 0.01)];
    let reranked = vec![
        RerankResult {
            index: 0,
            relevance_score: 0.1,
        },
        RerankResult {
            index: 1,
            relevance_score: 0.95,
        },
    ];

    let result = blend_scores(&originals, &reranked, 1.0);

    // Pure rerank: b=0.95 > a=0.1
    assert_eq!(result[0].0, "b");
    assert!((result[0].1 - 0.95).abs() < 1e-6);
    assert_eq!(result[1].0, "a");
    assert!((result[1].1 - 0.1).abs() < 1e-6);
}

/// Default config: enabled=false, Jina, correct model, 5000ms, weight=0.6
#[test]
fn p3_05_rerank_config_defaults_are_sane() {
    let config = RerankConfig::default();

    assert!(!config.enabled);
    assert_eq!(config.provider, RerankProviderType::Jina);
    assert_eq!(config.models[0], "BAAI/bge-reranker-v2-m3");
    assert_eq!(config.timeout_ms, 5000);
    assert!((config.rerank_weight - 0.6).abs() < f32::EPSILON);
}

/// Each provider type builds to the correct provider_id
#[test]
fn p3_06_build_provider_returns_correct_type() {
    let cases: Vec<(RerankProviderType, &str)> = vec![
        (RerankProviderType::Jina, "jina"),
        (RerankProviderType::SiliconFlow, "siliconflow"),
        (RerankProviderType::Voyage, "voyage"),
        (RerankProviderType::Pinecone, "pinecone"),
        (RerankProviderType::Vllm, "vllm"),
    ];

    for (provider_type, expected_id) in cases {
        let mut config = RerankConfig::default();
        config.provider = provider_type;
        let provider = build_provider(&config);
        assert_eq!(
            provider.provider_id(),
            expected_id,
            "provider_id mismatch for {:?}",
            config.provider
        );
    }
}

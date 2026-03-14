//! P2: Fusion & Scoring — 8 scenarios

use alephcore::memory::hybrid_retrieval::fusion::{rrf_fuse, weighted_fuse};
use alephcore::memory::query_expander::expand;

#[test]
fn p2_01_rrf_both_sources_overlap_ranks_highest() {
    // Documents "a" and "b" appear in both sources; "c" and "d" in only one.
    let vec_results = vec![
        ("a".into(), 0.9),
        ("b".into(), 0.8),
        ("c".into(), 0.7),
    ];
    let text_results = vec![
        ("b".into(), 0.9),
        ("a".into(), 0.7),
        ("d".into(), 0.5),
    ];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.0);

    // Documents in both lists accumulate RRF from two sources → rank top-2
    let top2_ids: Vec<&str> = fused.iter().take(2).map(|f| f.id.as_str()).collect();
    assert!(top2_ids.contains(&"a"), "overlap doc 'a' should be in top-2");
    assert!(top2_ids.contains(&"b"), "overlap doc 'b' should be in top-2");

    // Single-source docs should rank lower
    let bottom_ids: Vec<&str> = fused.iter().skip(2).map(|f| f.id.as_str()).collect();
    assert!(bottom_ids.contains(&"c"), "'c' (vec-only) should rank below overlaps");
    assert!(bottom_ids.contains(&"d"), "'d' (text-only) should rank below overlaps");
}

#[test]
fn p2_02_rrf_single_source_degrades_gracefully() {
    // Empty text results → only vector results should be present, scores in [0,1]
    let vec_results = vec![
        ("x".into(), 0.9),
        ("y".into(), 0.5),
        ("z".into(), 0.3),
    ];
    let text_results: Vec<(String, f32)> = vec![];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.2);

    assert_eq!(fused.len(), 3, "all vector docs should be preserved");
    let ids: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&"x"));
    assert!(ids.contains(&"y"));
    assert!(ids.contains(&"z"));

    // All scores in [0, 1]
    for f in &fused {
        assert!(
            f.score >= 0.0 && f.score <= 1.0,
            "score {} out of [0,1] range",
            f.score
        );
    }
}

#[test]
fn p2_03_rrf_bm25_bonus_breaks_tie() {
    // "a" at rank 0 in vector only; "b" at rank 1 in vector AND rank 0 in text.
    // With k=60, "a" has RRF = 1/61 ≈ 0.01639.
    // "b" has RRF = 1/62 + 1/61 ≈ 0.01613 + 0.01639 = 0.03252.
    // "b" already wins due to dual-source. But let's craft a scenario where
    // bonus is the decider: "a" and "b" both in vector (ranks 0,1), "b" only in text.
    // Without bonus: "a"=1/61, "b"=1/62+1/61. "b" already wins.
    // Better test: "a" in vec rank 0, "b" in vec rank 1 + text rank 0.
    // Without bonus (0.0): "a"=1/61≈0.01639, "b"=1/62+1/61≈0.03253 → "b" wins.
    // So use a tighter scenario:
    // "a" in vec rank 0 only, "b" in text rank 0 only. Both have same RRF = 1/61.
    // Without bonus they tie. With bonus=0.15, "b" gets 1/61 * 1.15 > 1/61.
    let vec_results = vec![("a".into(), 0.9_f32)];
    let text_results = vec![("b".into(), 0.9_f32)];

    // Without bonus: tie (both 1/61)
    let fused_no_bonus = rrf_fuse(&vec_results, &text_results, 60, 0.0);
    let a_score_nb = fused_no_bonus.iter().find(|f| f.id == "a").unwrap().score;
    let b_score_nb = fused_no_bonus.iter().find(|f| f.id == "b").unwrap().score;
    assert!(
        (a_score_nb - b_score_nb).abs() < 1e-5,
        "without bonus, a and b should have equal scores"
    );

    // With bonus=0.15: "b" should rank above "a"
    let fused_bonus = rrf_fuse(&vec_results, &text_results, 60, 0.15);
    assert_eq!(
        fused_bonus[0].id, "b",
        "bm25_bonus=0.15 should push text-matched 'b' above vec-only 'a'"
    );
    assert!(
        fused_bonus[0].score > fused_bonus[1].score,
        "b's score should be strictly greater than a's"
    );
}

#[test]
fn p2_04_weighted_fusion_exact_formula() {
    // 0.7 * 1.0 + 0.3 * 0.5 = 0.85
    let vec_results = vec![("doc1".into(), 1.0_f32)];
    let text_results = vec![("doc1".into(), 0.5_f32)];

    let fused = weighted_fuse(&vec_results, &text_results, 0.7, 0.3);

    assert_eq!(fused.len(), 1);
    let expected = 0.7 * 1.0 + 0.3 * 0.5; // 0.85
    assert!(
        (fused[0].score - expected).abs() < 1e-5,
        "expected {expected}, got {}",
        fused[0].score
    );
}

#[test]
fn p2_05_rrf_normalization_max_is_one() {
    // With any non-trivial input, the top fused score should always be 1.0
    let vec_results = vec![
        ("a".into(), 0.95),
        ("b".into(), 0.80),
        ("c".into(), 0.60),
    ];
    let text_results = vec![
        ("c".into(), 0.90),
        ("d".into(), 0.70),
        ("a".into(), 0.50),
    ];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.1);

    assert!(!fused.is_empty());
    assert!(
        (fused[0].score - 1.0).abs() < 1e-5,
        "top score should be normalised to 1.0, got {}",
        fused[0].score
    );

    // All scores in [0, 1]
    for f in &fused {
        assert!(
            f.score >= 0.0 && f.score <= 1.0 + 1e-6,
            "score {} exceeds [0,1]",
            f.score
        );
    }

    // Descending order
    for w in fused.windows(2) {
        assert!(
            w[0].score >= w[1].score - 1e-6,
            "results should be sorted descending"
        );
    }
}

#[test]
fn p2_06_query_expansion_chinese_injects_synonyms() {
    let result = expand("用户喜欢什么编程语言");

    // Original preserved verbatim
    assert_eq!(result.original, "用户喜欢什么编程语言");

    // bm25_query should be expanded with synonyms for "喜欢"
    assert_ne!(result.original, result.bm25_query, "bm25 should differ from original");
    assert!(
        result.bm25_query.contains("偏好"),
        "bm25_query '{}' should contain synonym '偏好'",
        result.bm25_query
    );
    // Original text preserved at the start of bm25_query
    assert!(
        result.bm25_query.starts_with("用户喜欢什么编程语言"),
        "bm25_query should start with original text"
    );
}

#[test]
fn p2_07_query_expansion_english_unchanged() {
    let result = expand("What programming language do you prefer?");

    assert_eq!(result.original, "What programming language do you prefer?");
    assert_eq!(
        result.original, result.bm25_query,
        "English query should not be expanded"
    );
}

#[test]
fn p2_08_query_expansion_no_known_keywords_unchanged() {
    let result = expand("天气很好");

    assert_eq!(result.original, "天气很好");
    assert_eq!(
        result.original, result.bm25_query,
        "Chinese without known keywords should not be expanded"
    );
}

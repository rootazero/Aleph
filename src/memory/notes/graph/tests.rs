// graph algorithm tests — see Task 3.4

#[test]
fn direct_link_and_type_affinity_score() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "learning/a".into(), category: "learning".into(), sources: vec![] },
            GraphNode { path: "learning/b".into(), category: "learning".into(), sources: vec![] },
        ],
        edges: vec![("learning/a".into(), "learning/b".into())],
    };
    let g = GraphIndex::build(&snap);
    let w = relevance::SignalWeights::default();
    // direct link (3) + type affinity (1) = 4
    assert!((relevance::score_pair(&g, &w, 0, 1) - 4.0).abs() < 1e-4);
}

#[test]
fn source_overlap_scores() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "p/a".into(), category: "x".into(), sources: vec!["raw/1".into()] },
            GraphNode { path: "p/b".into(), category: "y".into(), sources: vec!["raw/1".into()] },
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let w = relevance::SignalWeights::default();
    // one shared source = 4.0, no link, different type
    assert!((relevance::score_pair(&g, &w, 0, 1) - 4.0).abs() < 1e-4);
}

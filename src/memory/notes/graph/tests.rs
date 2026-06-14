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

#[test]
fn louvain_splits_barbell_into_two_communities() {
    use crate::memory::notes::graph::*;
    // Two triangles {a,b,c} and {d,e,f}, joined by a single c-d bridge edge.
    let node = |p: &str| GraphNode { path: p.into(), category: "x".into(), sources: vec![] };
    let snap = GraphSnapshot {
        nodes: vec![node("g/a"), node("g/b"), node("g/c"), node("g/d"), node("g/e"), node("g/f")],
        edges: vec![
            ("g/a".into(),"g/b".into()), ("g/b".into(),"g/c".into()), ("g/a".into(),"g/c".into()),
            ("g/d".into(),"g/e".into()), ("g/e".into(),"g/f".into()), ("g/d".into(),"g/f".into()),
            ("g/c".into(),"g/d".into()),
        ],
    };
    let g = GraphIndex::build(&snap);
    let c = community::detect(&g);
    // a,b,c same community; d,e,f same community; the two differ.
    assert_eq!(c.of_node[0], c.of_node[1]);
    assert_eq!(c.of_node[1], c.of_node[2]);
    assert_eq!(c.of_node[3], c.of_node[4]);
    assert_eq!(c.of_node[4], c.of_node[5]);
    assert_ne!(c.of_node[0], c.of_node[3]);
    // each triangle is fully cohesive (3 intra edges / 3 possible = 1.0)
    assert!((c.cohesion[c.of_node[0]] - 1.0).abs() < 1e-4);
}

#[test]
fn louvain_empty_and_edgeless() {
    use crate::memory::notes::graph::*;
    let g0 = GraphIndex::build(&GraphSnapshot::default());
    assert!(community::detect(&g0).of_node.is_empty());
    let snap = GraphSnapshot {
        nodes: vec![GraphNode{path:"p/a".into(),category:"x".into(),sources:vec![]},
                    GraphNode{path:"p/b".into(),category:"x".into(),sources:vec![]}],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let c = community::detect(&g);
    assert_ne!(c.of_node[0], c.of_node[1]); // singletons
}

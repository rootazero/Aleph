// graph algorithm tests — see Task 3.4

#[test]
fn direct_link_and_type_affinity_score() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode {
                path: "learning/a".into(),
                category: "learning".into(),
                sources: vec![],
            },
            GraphNode {
                path: "learning/b".into(),
                category: "learning".into(),
                sources: vec![],
            },
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
            GraphNode {
                path: "p/a".into(),
                category: "x".into(),
                sources: vec!["raw/1".into()],
            },
            GraphNode {
                path: "p/b".into(),
                category: "y".into(),
                sources: vec!["raw/1".into()],
            },
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
    let node = |p: &str| GraphNode {
        path: p.into(),
        category: "x".into(),
        sources: vec![],
    };
    let snap = GraphSnapshot {
        nodes: vec![
            node("g/a"),
            node("g/b"),
            node("g/c"),
            node("g/d"),
            node("g/e"),
            node("g/f"),
        ],
        edges: vec![
            ("g/a".into(), "g/b".into()),
            ("g/b".into(), "g/c".into()),
            ("g/a".into(), "g/c".into()),
            ("g/d".into(), "g/e".into()),
            ("g/e".into(), "g/f".into()),
            ("g/d".into(), "g/f".into()),
            ("g/c".into(), "g/d".into()),
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
    let empty_snap = GraphSnapshot::default();
    let g0 = GraphIndex::build(&empty_snap);
    assert!(community::detect(&g0).of_node.is_empty());
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode {
                path: "p/a".into(),
                category: "x".into(),
                sources: vec![],
            },
            GraphNode {
                path: "p/b".into(),
                category: "x".into(),
                sources: vec![],
            },
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let c = community::detect(&g);
    assert_ne!(c.of_node[0], c.of_node[1]); // singletons
}

#[test]
fn detects_isolated_and_bridge() {
    use crate::memory::notes::graph::*;
    let node = |p: &str, cat: &str| GraphNode {
        path: p.into(),
        category: cat.into(),
        sources: vec![],
    };
    // hub h bridges three otherwise-separate clusters → bridge; lone l → isolated.
    // Each cluster is a dense triangle so Louvain keeps it as its own community
    // (a single-node leaf would instead collapse into the hub's community, so
    // the hub would span only one community and never register as a bridge).
    let snap = GraphSnapshot {
        nodes: vec![
            node("c/h", "x"),
            node("c/a1", "a"),
            node("c/a2", "a"),
            node("c/a3", "a"),
            node("c/b1", "b"),
            node("c/b2", "b"),
            node("c/b3", "b"),
            node("c/d1", "d"),
            node("c/d2", "d"),
            node("c/d3", "d"),
            node("c/l", "z"),
        ],
        edges: vec![
            // three internally-dense triangles
            ("c/a1".into(), "c/a2".into()),
            ("c/a2".into(), "c/a3".into()),
            ("c/a1".into(), "c/a3".into()),
            ("c/b1".into(), "c/b2".into()),
            ("c/b2".into(), "c/b3".into()),
            ("c/b1".into(), "c/b3".into()),
            ("c/d1".into(), "c/d2".into()),
            ("c/d2".into(), "c/d3".into()),
            ("c/d1".into(), "c/d3".into()),
            // hub h links one node of each triangle → bridges 3 communities
            ("c/h".into(), "c/a1".into()),
            ("c/h".into(), "c/b1".into()),
            ("c/h".into(), "c/d1".into()),
        ],
    };
    let g = GraphIndex::build(&snap);
    let com = community::detect(&g);
    let ins = insights::detect(&g, &com);
    assert!(ins.isolated.contains(&"c/l".to_string()));
    // h's neighbours span 3 distinct communities → bridge
    assert!(ins.bridges.contains(&"c/h".to_string()));
    // cross-type edges present → surprising non-empty
    assert!(!ins.surprising.is_empty());
}

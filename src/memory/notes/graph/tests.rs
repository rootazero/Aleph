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
        edges: vec![GraphEdge {
            from: "learning/a".into(),
            to: "learning/b".into(),
            rel_type: None,
            confidence: 1.0,
        }],
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
    // one shared source cited only by these two notes (df=2) = full weight 4.0,
    // no link, different type
    assert!((relevance::score_pair(&g, &w, 0, 1) - 4.0).abs() < 1e-4);
}

#[test]
fn rare_shared_source_outscores_ubiquitous_one() {
    use crate::memory::notes::graph::*;
    let node = |p: &str, cat: &str, src: &[&str]| GraphNode {
        path: p.into(),
        category: cat.into(),
        sources: src.iter().map(|s| s.to_string()).collect(),
    };
    // "rare" is cited by exactly a,b (df=2); "common" by a,c,d (df=3).
    // Distinct categories + no edges isolate the source signal.
    let snap = GraphSnapshot {
        nodes: vec![
            node("p/a", "ca", &["rare", "common"]),
            node("p/b", "cb", &["rare"]),
            node("p/c", "cc", &["common"]),
            node("p/d", "cd", &["common"]),
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let w = relevance::SignalWeights::default();
    let s_rare = relevance::score_pair(&g, &w, 0, 1); // a—b via "rare"
    let s_common = relevance::score_pair(&g, &w, 0, 2); // a—c via "common"
                                                        // df=2 → 4.0 * ln2/ln2 = 4.0 ; df=3 → 4.0 * ln2/ln3 ≈ 2.524
    assert!((s_rare - 4.0).abs() < 1e-4);
    assert!((s_common - 4.0 * (2.0_f32).ln() / (3.0_f32).ln()).abs() < 1e-4);
    assert!(
        s_rare > s_common,
        "rare source ({s_rare}) must outscore ubiquitous one ({s_common})"
    );
}

#[test]
fn source_sharers_inverted_index_matches_full_scan() {
    use crate::memory::notes::graph::*;
    let node = |p: &str, src: &[&str]| GraphNode {
        path: p.into(),
        category: "x".into(),
        sources: src.iter().map(|s| s.to_string()).collect(),
    };
    let snap = GraphSnapshot {
        nodes: vec![
            node("p/a", &["s1", "s2"]),
            node("p/b", &["s1"]),
            node("p/c", &["s2"]),
            node("p/d", &["s3"]),
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    // a shares s1 with b and s2 with c, but nothing with d.
    let sharers = g.source_sharers(0);
    assert!(sharers.contains(&1));
    assert!(sharers.contains(&2));
    assert!(!sharers.contains(&3));
    assert!(!sharers.contains(&0)); // never includes the seed itself
    assert_eq!(g.source_df("s1"), 2);
    assert_eq!(g.source_df("s2"), 2);
    assert_eq!(g.source_df("missing"), 0);
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
            GraphEdge {
                from: "g/a".into(),
                to: "g/b".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/b".into(),
                to: "g/c".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/a".into(),
                to: "g/c".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/d".into(),
                to: "g/e".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/e".into(),
                to: "g/f".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/d".into(),
                to: "g/f".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "g/c".into(),
                to: "g/d".into(),
                rel_type: None,
                confidence: 1.0,
            },
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
fn refine_splits_internally_disconnected_community() {
    use crate::memory::notes::graph::*;
    // Two disjoint edges: {0-1} and {2-3}. If a (buggy) Louvain partition ever
    // lumps all four into one community, the Leiden-style refinement must split
    // it back into the two connected components — the guarantee cbm's Leiden
    // refinement provides. Tested directly with a synthetic assignment so it
    // does not depend on tricking Louvain into the disconnected state.
    let node = |p: &str| GraphNode {
        path: p.into(),
        category: "x".into(),
        sources: vec![],
    };
    let edge = |from: &str, to: &str| GraphEdge {
        from: from.into(),
        to: to.into(),
        rel_type: None,
        confidence: 1.0,
    };
    let snap = GraphSnapshot {
        nodes: vec![node("g/n0"), node("g/n1"), node("g/n2"), node("g/n3")],
        edges: vec![edge("g/n0", "g/n1"), edge("g/n2", "g/n3")],
    };
    let g = GraphIndex::build(&snap);
    // Artificially place all four nodes in ONE community.
    let refined = community::refine_connected(&g, &[0, 0, 0, 0]);
    assert_eq!(refined[0], refined[1], "connected pair stays together");
    assert_eq!(refined[2], refined[3], "connected pair stays together");
    assert_ne!(refined[0], refined[2], "disconnected blobs are split apart");

    // Already-connected community is a no-op (one label for a connected pair).
    let connected = community::refine_connected(&g, &[7, 7, 9, 9]);
    assert_eq!(connected[0], connected[1]);
    assert_eq!(connected[2], connected[3]);
    assert_ne!(connected[0], connected[2]);
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
            GraphEdge {
                from: "c/a1".into(),
                to: "c/a2".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/a2".into(),
                to: "c/a3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/a1".into(),
                to: "c/a3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/b1".into(),
                to: "c/b2".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/b2".into(),
                to: "c/b3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/b1".into(),
                to: "c/b3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/d1".into(),
                to: "c/d2".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/d2".into(),
                to: "c/d3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/d1".into(),
                to: "c/d3".into(),
                rel_type: None,
                confidence: 1.0,
            },
            // hub h links one node of each triangle → bridges 3 communities
            GraphEdge {
                from: "c/h".into(),
                to: "c/a1".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/h".into(),
                to: "c/b1".into(),
                rel_type: None,
                confidence: 1.0,
            },
            GraphEdge {
                from: "c/h".into(),
                to: "c/d1".into(),
                rel_type: None,
                confidence: 1.0,
            },
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

#[test]
fn edge_confidence_is_max_over_directions() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode {
                path: "g/a".into(),
                category: "x".into(),
                sources: vec![],
            },
            GraphNode {
                path: "g/b".into(),
                category: "x".into(),
                sources: vec![],
            },
        ],
        edges: vec![
            GraphEdge {
                from: "g/a".into(),
                to: "g/b".into(),
                rel_type: Some("cites".into()),
                confidence: 0.4,
            },
            GraphEdge {
                from: "g/b".into(),
                to: "g/a".into(),
                rel_type: None,
                confidence: 0.9,
            },
        ],
    };
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    assert!((g.edge_confidence(a, b) - 0.9).abs() < 1e-6);
    // Undirected adjacency still symmetric (community/AA depend on it).
    assert!(g.adj[a].contains(&b) && g.adj[b].contains(&a));
}

#[test]
fn missing_edge_confidence_is_zero() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode {
                path: "g/a".into(),
                category: "x".into(),
                sources: vec![],
            },
            GraphNode {
                path: "g/b".into(),
                category: "x".into(),
                sources: vec![],
            },
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    assert_eq!(g.edge_confidence(a, b), 0.0);
}

#[test]
fn half_confidence_edge_yields_half_direct_link_contribution() {
    use crate::memory::notes::graph::*;
    // Two isolated same-category nodes, single directed edge.
    // score = direct_link * conf + type_affinity (no AA, no source overlap).
    let nodes = vec![
        GraphNode {
            path: "g/a".into(),
            category: "x".into(),
            sources: vec![],
        },
        GraphNode {
            path: "g/b".into(),
            category: "x".into(),
            sources: vec![],
        },
    ];
    let snap_full = GraphSnapshot {
        nodes: nodes.clone(),
        edges: vec![GraphEdge {
            from: "g/a".into(),
            to: "g/b".into(),
            rel_type: None,
            confidence: 1.0,
        }],
    };
    let snap_half = GraphSnapshot {
        nodes,
        edges: vec![GraphEdge {
            from: "g/a".into(),
            to: "g/b".into(),
            rel_type: None,
            confidence: 0.5,
        }],
    };
    let w = relevance::SignalWeights::default();
    let g_full = GraphIndex::build(&snap_full);
    let g_half = GraphIndex::build(&snap_half);
    let score_full = relevance::score_pair(&g_full, &w, 0, 1);
    let score_half = relevance::score_pair(&g_half, &w, 0, 1);
    // With conf=1.0: direct_link*1 + type_affinity = 3.0 + 1.0 = 4.0
    // With conf=0.5: direct_link*0.5 + type_affinity = 1.5 + 1.0 = 2.5
    assert!(
        (score_full - 4.0).abs() < 1e-4,
        "full-conf score = {score_full}"
    );
    assert!(
        (score_half - 2.5).abs() < 1e-4,
        "half-conf score = {score_half}"
    );
    assert!(score_half < score_full);
}

fn two_node_snapshot(
    edges: Vec<crate::memory::notes::graph::GraphEdge>,
) -> crate::memory::notes::graph::GraphSnapshot {
    use crate::memory::notes::graph::{GraphNode, GraphSnapshot};
    GraphSnapshot {
        nodes: vec![
            GraphNode {
                path: "g/a".into(),
                category: "x".into(),
                sources: vec![],
            },
            GraphNode {
                path: "g/b".into(),
                category: "x".into(),
                sources: vec![],
            },
        ],
        edges,
    }
}

#[test]
fn co_recall_edge_is_excluded_from_semantic_confidence() {
    use crate::memory::notes::graph::*;
    let snap = two_node_snapshot(vec![GraphEdge {
        from: "g/a".into(),
        to: "g/b".into(),
        rel_type: Some(CO_RECALLED_RELATION.into()),
        confidence: 0.8,
    }]);
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    // Behavioral edge never leaks into the semantic direct-link signal…
    assert_eq!(g.edge_confidence(a, b), 0.0);
    // …but carries the fifth signal, symmetric over directions.
    assert!((g.co_recall_confidence(a, b) - 0.8).abs() < 1e-6);
    assert!((g.co_recall_confidence(b, a) - 0.8).abs() < 1e-6);
    // And still participates in the undirected adjacency (candidates/AA).
    assert!(g.adj[a].contains(&b));
}

#[test]
fn semantic_edge_contributes_no_co_recall_signal() {
    use crate::memory::notes::graph::*;
    let snap = two_node_snapshot(vec![GraphEdge {
        from: "g/a".into(),
        to: "g/b".into(),
        rel_type: Some("cites".into()),
        confidence: 0.9,
    }]);
    let g = GraphIndex::build(&snap);
    let (a, b) = (g.index_of("g/a").unwrap(), g.index_of("g/b").unwrap());
    assert!((g.edge_confidence(a, b) - 0.9).abs() < 1e-6);
    assert_eq!(g.co_recall_confidence(a, b), 0.0);
}

#[test]
fn co_recall_edge_scores_with_fifth_signal_weight() {
    use crate::memory::notes::graph::*;
    let snap = two_node_snapshot(vec![GraphEdge {
        from: "g/a".into(),
        to: "g/b".into(),
        rel_type: Some(CO_RECALLED_RELATION.into()),
        confidence: 0.5,
    }]);
    let w = relevance::SignalWeights::default();
    let g = GraphIndex::build(&snap);
    // score = co_recall*0.5 + type_affinity = 2.0*0.5 + 1.0 = 2.0
    // (direct_link contributes nothing: the edge is behavioral).
    let score = relevance::score_pair(&g, &w, 0, 1);
    assert!((score - 2.0).abs() < 1e-4, "co-recall score = {score}");
}

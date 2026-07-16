//! `GraphRecompute` stage — materialize the note knowledge graph.
//!
//! Loads the full graph snapshot, runs the 5-signal / Louvain / insights
//! algorithms inside `spawn_blocking` (CPU-bound, std-thread parallel), and
//! upserts `notes_graph_cache` + `notes_graph_insights`. Pure deterministic
//! aggregation — zero LLM call (R7/R10-safe analytics infrastructure).

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::graph::relevance::{all_related, SignalWeights};
use crate::memory::notes::graph::{community, insights, GraphIndex, GraphSnapshot};
use crate::memory::notes::store::NoteStore;

use super::DreamStage;

/// Minimum MinHash Jaccard estimate for a content-similarity edge.
const MINHASH_THRESHOLD: f32 = 0.82;
/// Max MinHash similarity edges materialized per note (hub-explosion guard).
const MINHASH_MAX_EDGES_PER_NODE: usize = 8;

pub struct GraphRecomputeStage;

#[async_trait]
impl DreamStage for GraphRecomputeStage {
    fn name(&self) -> &'static str {
        "graph_recompute"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // rust-doctor-disable-next-line excessive-clone
        let agent_id = ctx.agent_id.clone();
        // rust-doctor-disable-next-line excessive-clone
        let store = ctx.indexer.store().clone();
        let snapshot: GraphSnapshot = store.load_graph_snapshot(&agent_id).await?;

        // CPU-bound compute off the async runtime.
        let computed = tokio::task::spawn_blocking(move || compute(&snapshot))
            .await
            .map_err(|e| AlephError::other(format!("graph recompute join: {e}")))?;

        // MinHash similarity edges (content-based; the structural snapshot has
        // no bodies). Non-fatal: failure → skip, keep 5-signal edges.
        let docs: Vec<(String, String)> = match async {
            let entries = store.list_notes(&agent_id).await?;
            let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
            let hydrated = store.get_notes_with_content(&agent_id, &paths).await?;
            Ok::<_, AlephError>(hydrated.into_iter().map(|r| (r.path, r.content)).collect())
        }
        .await
        {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "graph recompute: body load failed, skipping minhash");
                Vec::new()
            }
        };

        let related = if docs.len() >= 2 {
            let mh = tokio::task::spawn_blocking(move || {
                let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
                crate::memory::notes::graph::minhash::similarity_edges(
                    &docs,
                    MINHASH_THRESHOLD,
                    MINHASH_MAX_EDGES_PER_NODE,
                    threads,
                )
            })
            .await
            .map_err(|e| AlephError::other(format!("minhash join: {e}")))?;
            merge_related(computed.related, mh)
        } else {
            computed.related
        };

        store
            .replace_graph_cache(&agent_id, &computed.cache)
            .await?;
        store
            .replace_graph_insights(&agent_id, &computed.insights)
            .await?;
        store.replace_graph_related(&agent_id, &related).await?;
        tracing::info!(agent = %agent_id, nodes = computed.node_count, "graph cache recomputed");
        Ok(ctx)
    }
}

struct Computed {
    /// `(node_path, community_id, cohesion, degree)`.
    cache: Vec<(String, usize, f32, usize)>,
    /// `(kind, json_payload)`.
    insights: Vec<(String, String)>,
    /// `(node_path, related_path, score)` — top-K 5-signal relatedness edges.
    related: Vec<(String, String, f32)>,
    node_count: usize,
}

/// Pure: build the index, detect communities + insights, and shape the rows
/// for materialization. No LLM, no IO.
fn compute(snap: &GraphSnapshot) -> Computed {
    let g = GraphIndex::build(snap);
    if g.is_empty() {
        return Computed {
            cache: vec![],
            insights: vec![],
            related: vec![],
            node_count: 0,
        };
    }
    let com = community::detect(&g);
    let ins = insights::detect(&g, &com);

    // 5-signal relatedness: materialize each node's top-K scored peers, flattened
    // into `(seed, peer, score)` rows. CPU-bound, std-thread parallel (already
    // inside `spawn_blocking`).
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let related_rows: Vec<(String, String, f32)> =
        all_related(&g, &SignalWeights::default(), 8, threads)
            .into_iter()
            .flat_map(|(seed, peers)| {
                peers.into_iter().map(move |(p, s)| {
                    // rust-doctor-disable-next-line excessive-clone
                    (seed.clone(), p, s)
                })
            })
            .collect();

    let cache = (0..g.len())
        .map(|i| {
            let cid = com.of_node[i];
            // rust-doctor-disable-next-line excessive-clone
            (g.nodes[i].path.clone(), cid, com.cohesion[cid], g.degree(i))
        })
        .collect();

    // Serialize insights to JSON rows (one row per kind).
    let insights_rows = vec![
        (
            "isolated".to_string(),
            serde_json::to_string(&ins.isolated).unwrap_or_else(|_| "[]".into()),
        ),
        (
            "sparse".to_string(),
            serde_json::to_string(
                &ins.sparse_communities
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "community_id": s.community_id,
                            "size": s.size,
                            "cohesion": s.cohesion,
                            "exemplar": s.exemplar,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
        ),
        (
            "bridge".to_string(),
            serde_json::to_string(&ins.bridges).unwrap_or_else(|_| "[]".into()),
        ),
        (
            "surprising".to_string(),
            serde_json::to_string(
                &ins.surprising
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "from": e.from,
                            "to": e.to,
                            "score": e.score,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
        ),
    ];

    Computed {
        cache,
        insights: insights_rows,
        related: related_rows,
        node_count: g.len(),
    }
}

/// Merge two `(seed, peer, score)` edge lists, deduped by `(seed, peer)` keeping
/// the max score (explicit/5-signal edges beat lexical-similarity edges on ties).
fn merge_related(
    a: Vec<(String, String, f32)>,
    b: Vec<(String, String, f32)>,
) -> Vec<(String, String, f32)> {
    use std::collections::HashMap;
    let mut best: HashMap<(String, String), f32> = HashMap::new();
    for (s, p, sc) in a.into_iter().chain(b) {
        best.entry((s, p))
            .and_modify(|e| {
                if sc > *e {
                    *e = sc;
                }
            })
            .or_insert(sc);
    }
    let mut out: Vec<(String, String, f32)> =
        best.into_iter().map(|((s, p), sc)| (s, p, sc)).collect();
    out.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(&y.1)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::graph::{GraphEdge, GraphNode, GraphSnapshot};

    #[test]
    fn merge_related_keeps_max_per_pair() {
        let four_signal = vec![("a".to_string(), "b".to_string(), 4.0)];
        let minhash = vec![
            ("a".to_string(), "b".to_string(), 2.5), // same pair, lower → dropped
            ("a".to_string(), "c".to_string(), 2.5), // new pair → kept
        ];
        let merged = merge_related(four_signal, minhash);
        let ab = merged
            .iter()
            .find(|(s, p, _)| s == "a" && p == "b")
            .unwrap();
        assert!((ab.2 - 4.0).abs() < 1e-6, "max score wins");
        assert!(merged.iter().any(|(s, p, _)| s == "a" && p == "c"));
    }

    #[test]
    fn compute_empty_graph_yields_nothing() {
        let c = compute(&GraphSnapshot::default());
        assert_eq!(c.node_count, 0);
        assert!(c.cache.is_empty());
        assert!(c.insights.is_empty());
        assert!(c.related.is_empty());
    }

    #[test]
    fn compute_emits_one_cache_row_per_node_and_four_insight_kinds() {
        let node = |p: &str| GraphNode {
            path: p.into(),
            category: "x".into(),
            sources: vec![],
        };
        let snap = GraphSnapshot {
            nodes: vec![node("g/a"), node("g/b"), node("g/c")],
            edges: vec![GraphEdge {
                from: "g/a".into(),
                to: "g/b".into(),
                rel_type: None,
                confidence: 1.0,
            }],
        };
        let c = compute(&snap);
        assert_eq!(c.node_count, 3);
        assert_eq!(c.cache.len(), 3);
        // Exactly four insight kinds, in order.
        let kinds: Vec<&str> = c.insights.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, vec!["isolated", "sparse", "bridge", "surprising"]);
        // The directly-linked a—b pair yields scored relatedness edges in both
        // directions; the isolated c contributes none.
        assert!(
            !c.related.is_empty(),
            "connected graph must materialize relatedness edges"
        );
        assert!(
            c.related
                .iter()
                .any(|(seed, peer, score)| seed == "g/a" && peer == "g/b" && *score > 0.0),
            "expected a scored g/a -> g/b related edge, got: {:?}",
            c.related
        );
    }
}

//! `GraphRecompute` stage — materialize the note knowledge graph.
//!
//! Loads the full graph snapshot, runs the 4-signal / Louvain / insights
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
        let agent_id = ctx.agent_id.clone();
        let store = ctx.indexer.store().clone();
        let snapshot: GraphSnapshot = store.load_graph_snapshot(&agent_id).await?;

        // CPU-bound compute off the async runtime.
        let computed = tokio::task::spawn_blocking(move || compute(&snapshot))
            .await
            .map_err(|e| AlephError::other(format!("graph recompute join: {e}")))?;

        store
            .replace_graph_cache(&agent_id, &computed.cache)
            .await?;
        store
            .replace_graph_insights(&agent_id, &computed.insights)
            .await?;
        store
            .replace_graph_related(&agent_id, &computed.related)
            .await?;
        tracing::info!(agent = %agent_id, nodes = computed.node_count, "graph cache recomputed");
        Ok(ctx)
    }
}

struct Computed {
    /// `(node_path, community_id, cohesion, degree)`.
    cache: Vec<(String, usize, f32, usize)>,
    /// `(kind, json_payload)`.
    insights: Vec<(String, String)>,
    /// `(node_path, related_path, score)` — top-K 4-signal relatedness edges.
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

    // 4-signal relatedness: materialize each node's top-K scored peers, flattened
    // into `(seed, peer, score)` rows. CPU-bound, std-thread parallel (already
    // inside `spawn_blocking`).
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let related_rows: Vec<(String, String, f32)> =
        all_related(&g, &SignalWeights::default(), 8, threads)
            .into_iter()
            .flat_map(|(seed, peers)| {
                peers.into_iter().map(move |(p, s)| (seed.clone(), p, s))
            })
            .collect();

    let cache = (0..g.len())
        .map(|i| {
            let cid = com.of_node[i];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::graph::{GraphNode, GraphSnapshot};

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
            edges: vec![("g/a".into(), "g/b".into())],
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

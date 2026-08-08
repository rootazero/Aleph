//! `CoRecallEdges` stage — materialize behavioral co-recall edges.
//!
//! Notes retrieved together by the same query events (codebase-memory-mcp's
//! `FILE_CHANGES_WITH` transposed to recall) become weighted `co_recalled`
//! rows in `notes_links`, giving the 5-signal relevance scorer its behavioral
//! signal. Pure deterministic aggregation over `recall_signals` — zero LLM
//! call (R7/R10-safe analytics infrastructure, same class as
//! `GraphRecomputeStage`, which runs right after and consumes these edges).
//!
//! Edge discipline (anti-mush): pairs need ≥ [`MIN_CO_HITS`] distinct
//! co-occurrences, each node keeps at most [`MAX_EDGES_PER_NODE`] behavioral
//! edges (strongest first), and an existing semantic link for a pair always
//! wins over the behavioral edge (enforced by the store).

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;

use super::DreamStage;

/// Minimum distinct query events two notes must share to earn an edge.
/// A single co-occurrence is coincidence; two is a pattern worth an edge.
const MIN_CO_HITS: i64 = 2;
/// Per-node cap on behavioral edges (strongest kept) — hub-explosion guard,
/// mirroring `MINHASH_MAX_EDGES_PER_NODE` in the graph recompute.
const MAX_EDGES_PER_NODE: usize = 10;
/// Upper bound on candidate pairs pulled from SQL per cycle. The per-node cap
/// prunes far below this; the bound just keeps a pathological signals table
/// from ballooning one cycle.
const MAX_PAIRS_SCANNED: usize = 512;
/// Confidence saturation: `co_hits / (co_hits + K)`. With `K = 2`, the
/// minimum qualifying pair (2 co-hits) starts at 0.5 and approaches 1.0 —
/// always below a full-confidence semantic wikilink.
const CONFIDENCE_SATURATION: f32 = 2.0;

pub struct CoRecallEdgesStage;

#[async_trait]
impl DreamStage for CoRecallEdgesStage {
    fn name(&self) -> &'static str {
        "co_recall_edges"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let pairs = match ctx.database.co_recall_pairs(
            &ctx.agent_id,
            MIN_CO_HITS,
            MAX_PAIRS_SCANNED,
        ) {
            Ok(p) => p,
            Err(e) => {
                // Non-fatal: a broken signals table must not stall the dream
                // pipeline; the next cycle retries from scratch.
                tracing::warn!(error = %e, "co_recall_edges: pair aggregation failed, skipping");
                return Ok(ctx);
            }
        };

        // Recall signals may reference notes that were since merged or
        // archived — only pairs whose both endpoints still exist get edges.
        let known: std::collections::HashSet<&str> =
            ctx.notes.iter().map(|n| n.path.as_str()).collect();
        let live: Vec<(String, String, i64)> = pairs
            .into_iter()
            .filter(|(a, b, _)| known.contains(a.as_str()) && known.contains(b.as_str()))
            .collect();

        let edges = discipline_edges(&live, MAX_EDGES_PER_NODE);
        let edge_count = edges.len();
        ctx.indexer
            .store()
            .replace_co_recall_links(&ctx.agent_id, &edges)
            .await?;

        tracing::info!(agent = %ctx.agent_id, edges = edge_count, "co-recall edges materialized");
        Ok(ctx)
    }
}

/// Pure edge discipline: walk pairs strongest-first, accept a pair only while
/// both endpoints are under `max_per_node`, and map co-hit counts onto a
/// saturating confidence in `[0.5, 1)`. Input pairs are expected sorted by
/// weight descending (as `co_recall_pairs` returns them).
#[must_use]
pub fn discipline_edges(
    pairs: &[(String, String, i64)],
    max_per_node: usize,
) -> Vec<(String, String, f32)> {
    let mut node_degree: HashMap<&str, usize> = HashMap::new();
    let mut edges = Vec::new();
    for (a, b, co_hits) in pairs {
        let deg_a = node_degree.get(a.as_str()).copied().unwrap_or(0);
        let deg_b = node_degree.get(b.as_str()).copied().unwrap_or(0);
        if deg_a >= max_per_node || deg_b >= max_per_node {
            continue;
        }
        *node_degree.entry(a.as_str()).or_insert(0) += 1;
        *node_degree.entry(b.as_str()).or_insert(0) += 1;
        let w = *co_hits as f32;
        edges.push((a.clone(), b.clone(), w / (w + CONFIDENCE_SATURATION)));
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(a: &str, b: &str, w: i64) -> (String, String, i64) {
        (a.into(), b.into(), w)
    }

    #[test]
    fn stage_name() {
        assert_eq!(CoRecallEdgesStage.name(), "co_recall_edges");
    }

    #[test]
    fn confidence_saturates_below_semantic_full_confidence() {
        let edges = discipline_edges(&[pair("n/a", "n/b", 2), pair("n/a", "n/c", 8)], 10);
        assert_eq!(edges.len(), 2);
        // 2 co-hits → 0.5; 8 co-hits → 0.8; never reaches 1.0.
        assert!((edges[0].2 - 0.5).abs() < 1e-6);
        assert!((edges[1].2 - 0.8).abs() < 1e-6);
        assert!(edges.iter().all(|(_, _, c)| *c < 1.0));
    }

    #[test]
    fn per_node_cap_keeps_strongest_edges() {
        // Hub h co-recalled with three peers, cap 2: the two strongest
        // (first in sorted order) survive; the weakest is dropped.
        let pairs = vec![
            pair("n/h", "n/a", 9),
            pair("n/h", "n/b", 5),
            pair("n/h", "n/c", 2),
        ];
        let edges = discipline_edges(&pairs, 2);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().any(|(_, b, _)| b == "n/a"));
        assert!(edges.iter().any(|(_, b, _)| b == "n/b"));
        assert!(!edges.iter().any(|(_, b, _)| b == "n/c"));
    }

    #[test]
    fn capped_node_does_not_block_other_pairs() {
        // h saturates at cap 2 after its two strongest pairs; h—c is dropped,
        // but the later, h-independent (a, b) pair must still land.
        let pairs = vec![
            pair("n/h", "n/a", 9),
            pair("n/h", "n/b", 5),
            pair("n/h", "n/c", 4),
            pair("n/a", "n/b", 3),
        ];
        let edges = discipline_edges(&pairs, 2);
        assert_eq!(edges.len(), 3);
        assert!(!edges.iter().any(|(_, b, _)| b == "n/c"), "h—c over cap");
        assert!(
            edges.iter().any(|(a, b, _)| a == "n/a" && b == "n/b"),
            "pair after a blocked one must still be considered"
        );
    }

    #[test]
    fn empty_input_yields_no_edges() {
        assert!(discipline_edges(&[], 10).is_empty());
    }
}

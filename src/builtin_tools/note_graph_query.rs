//! `note_graph_query` — read-only interrogation of the long-term memory
//! knowledge graph.
//!
//! The retrieval pipeline only ever surfaces a note's *1-hop* peers. This tool
//! lets the model explore the graph directly: introspect its shape (`schema`),
//! walk an N-hop neighborhood (`neighbors`), list a note's cluster
//! (`community`), fetch its scored related peers (`related`), or trace the
//! shortest connection between two notes (`path`). It is a pure read executor
//! over the existing `NoteStore` graph methods — the model writes the query, the
//! store answers deterministically (R7: retrieval, not reasoning). No writes, no
//! LLM calls.

use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which graph query to run.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphOp {
    /// Introspect the graph: node categories + counts, edge relation-type
    /// vocabulary + counts, totals, community count. Needs no `target`.
    Schema,
    /// BFS neighborhood around `target` up to `depth` hops (nodes + edges).
    Neighbors,
    /// Notes in the same community/cluster as `target`.
    Community,
    /// Top related peers to `target` by the 5-signal relatedness score.
    Related,
    /// Shortest connection path between `target` and `to` (both required),
    /// walking edges in either direction up to `depth` hops.
    Path,
}

/// Arguments for the `note_graph_query` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NoteGraphQueryArgs {
    /// Which query to run.
    pub op: GraphOp,
    /// Note path (`category/name`) to center on. Required for `neighbors`,
    /// `community`, `related`, and `path` (the path source); ignored for
    /// `schema`.
    #[serde(default)]
    pub target: Option<String>,
    /// Destination note path (`category/name`) for the `path` op. Required for
    /// `path`; ignored by every other op.
    #[serde(default)]
    pub to: Option<String>,
    /// BFS depth: `neighbors` (1–3, default 1) or `path` (1–6, default 2).
    /// Ignored by other ops.
    #[serde(default)]
    pub depth: Option<u8>,
    /// Max results (default 20, capped at 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A note node in the graph.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub path: String,
    pub category: String,
    pub link_count: usize,
}

/// A directed edge between two notes.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

/// A related peer with its 5-signal relatedness score.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedPeer {
    pub path: String,
    pub score: f32,
}

/// One hop along a connection `path`: the walk moves `from` → `to`, connected
/// by an edge labelled `relation` (a plain wikilink has no relation). The edge
/// may be stored in either direction — `relation` names the connection, query
/// the note itself for a typed relation's exact orientation.
#[derive(Debug, Clone, Serialize)]
pub struct PathHop {
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
}

/// Graph shape for the `schema` op.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSchema {
    /// Total notes for this agent.
    pub total_notes: usize,
    /// `(category, count)`, most populous first.
    pub categories: Vec<(String, usize)>,
    /// `(relation_label, edge_count)`, most frequent first. A plain body
    /// wikilink is labelled `"link"`; typed dream-stage edges keep their label
    /// (e.g. `supersedes`, `contradicts`, `mention`, `semantic`, `co_recalled`).
    pub relation_types: Vec<(String, i64)>,
    /// Number of detected communities (0 before the first dream graph-recompute).
    pub community_count: usize,
}

/// Advisory metadata about the caps applied to a paging op (`neighbors`,
/// `community`, `related`), so the model can distinguish a **truncated** result
/// (a cap was hit — the graph holds more) from a genuinely **complete** one.
/// Silent caps otherwise read as "this is everything", misleading exploration.
#[derive(Debug, Clone, Serialize)]
pub struct QueryAdvisory {
    /// Result limit actually applied after clamping the request into `[1, 200]`.
    pub applied_limit: usize,
    /// True when the returned set reached `applied_limit` — more peers/nodes may
    /// exist beyond what is shown. Re-query with a higher `limit` to see more.
    pub truncated: bool,
    /// Effective BFS depth after clamping into `[1, 3]` (present for `neighbors`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_depth: Option<u8>,
}

/// Result of a `note_graph_query`. Only the field(s) relevant to `op` are
/// populated; the rest serialize away (empty / `null`).
#[derive(Debug, Clone, Serialize, Default)]
pub struct NoteGraphQueryResult {
    /// `schema` op output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<GraphSchema>,
    /// `neighbors` op nodes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<GraphNode>,
    /// `neighbors` op edges.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<GraphEdge>,
    /// `community` op peer paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub community: Vec<String>,
    /// `related` op scored peers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedPeer>,
    /// `path` op: ordered hops from `target` to `to`. Empty when the two notes
    /// are the same (self path) or when no path was found — read `path_found`
    /// to disambiguate.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<PathHop>,
    /// `path` op: whether a connection was found within `depth`. `Some(false)`
    /// with `advisory.truncated == true` means "not found within the depth cap
    /// — retry with a higher `depth`"; `Some(false)` with `truncated == false`
    /// means the notes are provably disconnected in the reachable graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_found: Option<bool>,
    /// Cap advisory for paging ops (`neighbors`/`community`/`related`) and the
    /// depth advisory for `path`. Absent for `schema` (which has no limit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisory: Option<QueryAdvisory>,
}

/// Read-only knowledge-graph query tool.
pub struct NoteGraphQueryTool {
    db: MemoryBackend,
    agent_id: String,
}

impl NoteGraphQueryTool {
    /// Tool identifier registered with the agent runtime.
    pub const NAME: &'static str = "note_graph_query";

    /// Tool description shown to the LLM in its system prompt.
    pub const DESCRIPTION: &'static str =
        "Interrogate the long-term memory knowledge graph (read-only). ops: \
         `schema` introspects node categories, edge relation-types, and totals \
         (needs no target); `neighbors` returns the N-hop neighborhood around a \
         note path (depth 1–3); `community` lists notes in the same cluster; \
         `related` returns the top related peers by relatedness score; `path` \
         finds the shortest connection between `target` and `to` (both note \
         paths, depth 1–6) — the ordered chain of notes/edges linking them, or \
         `path_found:false` if unconnected within the depth. Use it to discover \
         how knowledge connects — before or instead of full-text search. Paging \
         ops report an `advisory` with the applied limit/depth and a `truncated` \
         flag so a capped result is never mistaken for a complete one.";

    /// Create a new `NoteGraphQueryTool`.
    pub fn new(db: MemoryBackend, agent_id: impl Into<String>) -> Self {
        Self {
            db,
            agent_id: agent_id.into(),
        }
    }

    /// Execute one graph query.
    pub async fn call_impl(
        &self,
        args: NoteGraphQueryArgs,
    ) -> anyhow::Result<NoteGraphQueryResult> {
        let agent = &self.agent_id;
        let limit = args.limit.unwrap_or(20).clamp(1, 200);

        // Ops other than `schema` require a `target` note path.
        let target = || -> anyhow::Result<&str> {
            args.target
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("op requires a `target` note path (\"category/name\")")
                })
        };

        let mut result = NoteGraphQueryResult::default();
        match args.op {
            GraphOp::Schema => {
                let notes = self
                    .db
                    .list_notes(agent)
                    .await
                    .map_err(|e| anyhow::anyhow!("list_notes: {e}"))?;
                let total_notes = notes.len();
                let mut cat_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for n in &notes {
                    *cat_counts.entry(n.category.clone()).or_default() += 1;
                }
                let mut categories: Vec<(String, usize)> = cat_counts.into_iter().collect();
                categories.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

                let relation_types = self
                    .db
                    .relation_type_counts(agent)
                    .await
                    .map_err(|e| anyhow::anyhow!("relation_type_counts: {e}"))?;

                let community_count = {
                    let ids = self
                        .db
                        .community_ids(agent)
                        .await
                        .map_err(|e| anyhow::anyhow!("community_ids: {e}"))?;
                    ids.values().collect::<std::collections::HashSet<_>>().len()
                };

                result.schema = Some(GraphSchema {
                    total_notes,
                    categories,
                    relation_types,
                    community_count,
                });
            }
            GraphOp::Neighbors => {
                let center = target()?;
                let depth = args.depth.unwrap_or(1).clamp(1, 3);
                let (nodes, edges, truncated) = self
                    .db
                    .get_neighbors(center, agent, depth, limit)
                    .await
                    .map_err(|e| anyhow::anyhow!("get_neighbors: {e}"))?;
                result.nodes = nodes
                    .into_iter()
                    .map(|n| GraphNode {
                        path: n.path,
                        category: n.category,
                        link_count: n.link_count,
                    })
                    .collect();
                result.edges = edges
                    .into_iter()
                    .map(|(from, to)| GraphEdge { from, to })
                    .collect();
                // `truncated` comes from the store's BFS frontier, not the node
                // count — dangling endpoints eat cap slots without adding nodes.
                result.advisory = Some(QueryAdvisory {
                    applied_limit: limit,
                    truncated,
                    applied_depth: Some(depth),
                });
            }
            GraphOp::Community => {
                let node_path = target()?;
                result.community = self
                    .db
                    .community_peers(agent, node_path, limit)
                    .await
                    .map_err(|e| anyhow::anyhow!("community_peers: {e}"))?;
                result.advisory = Some(QueryAdvisory {
                    applied_limit: limit,
                    truncated: result.community.len() >= limit,
                    applied_depth: None,
                });
            }
            GraphOp::Related => {
                let node_path = target()?;
                result.related = self
                    .db
                    .related_peers(agent, node_path, limit)
                    .await
                    .map_err(|e| anyhow::anyhow!("related_peers: {e}"))?
                    .into_iter()
                    .map(|(path, score)| RelatedPeer { path, score })
                    .collect();
                result.advisory = Some(QueryAdvisory {
                    applied_limit: limit,
                    truncated: result.related.len() >= limit,
                    applied_depth: None,
                });
            }
            GraphOp::Path => {
                let source = target()?;
                let dest = args
                    .to
                    .as_deref()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("path op requires a `to` note path (\"category/name\")")
                    })?;
                let depth = args.depth.unwrap_or(2).clamp(1, 6);
                let (found_path, truncated) = self
                    .db
                    .find_path(source, dest, agent, depth)
                    .await
                    .map_err(|e| anyhow::anyhow!("find_path: {e}"))?;
                result.path_found = Some(found_path.is_some());
                if let Some(hops) = found_path {
                    result.path = hops
                        .into_iter()
                        .map(|(from, to, relation)| PathHop { from, to, relation })
                        .collect();
                }
                // `applied_limit` is not meaningful for `path` (no result cap);
                // `truncated` here means the depth/visit cap was hit before
                // reaching `to`, so a longer path may exist.
                result.advisory = Some(QueryAdvisory {
                    applied_limit: limit,
                    truncated,
                    applied_depth: Some(depth),
                });
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;
    use std::sync::Arc;

    fn backend() -> MemoryBackend {
        Arc::new(SqliteMemoryBackend::in_memory().unwrap())
    }

    fn note(title: &str, category: &str, links: &[&str]) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            facts: vec![format!("{title} fact")],
            links: links.iter().map(|s| s.to_string()).collect(),
            created_at: 1000,
            updated_at: 1000,
            content_hash: format!("hash_{title}"),
            ..Default::default()
        }
    }

    async fn seed(db: &MemoryBackend) {
        // Index beta first so alpha's `[[beta]]` wikilink resolves to an
        // `active` edge at index time (calling index_note directly bypasses the
        // indexer's inbound-link backfill, so order matters here).
        db.index_note(&note("beta", "learning", &[]), "agent1", "learning")
            .await
            .unwrap();
        db.index_note(
            &note("alpha", "reference", &["beta"]),
            "agent1",
            "reference",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn schema_reports_categories_and_relation_vocabulary() {
        let db = backend();
        seed(&db).await;
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let out = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Schema,
                target: None,
                to: None,
                depth: None,
                limit: None,
            })
            .await
            .unwrap();
        let schema = out.schema.expect("schema populated");
        assert_eq!(schema.total_notes, 2);
        assert!(schema
            .categories
            .iter()
            .any(|(c, n)| c == "reference" && *n == 1));
        // The alpha→beta body wikilink is an untyped edge, labelled "link".
        assert!(schema
            .relation_types
            .iter()
            .any(|(rel, n)| rel == "link" && *n >= 1));
    }

    #[tokio::test]
    async fn neighbors_walks_from_center() {
        let db = backend();
        seed(&db).await;
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let out = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Neighbors,
                target: Some("reference/alpha".to_string()),
                to: None,
                depth: Some(1),
                limit: Some(10),
            })
            .await
            .unwrap();
        assert!(
            out.nodes.iter().any(|n| n.path == "reference/alpha"),
            "center should be in the neighborhood"
        );
        // Paging ops carry an honest advisory: the effective depth is reported.
        let adv = out.advisory.expect("neighbors advisory populated");
        assert_eq!(adv.applied_depth, Some(1));
    }

    #[tokio::test]
    async fn advisory_reports_clamped_limit() {
        let db = backend();
        seed(&db).await;
        let tool = NoteGraphQueryTool::new(db, "agent1");
        // Request an over-cap limit; the advisory must report the clamped value
        // so the model can tell a cap from a complete result.
        let out = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Neighbors,
                target: Some("reference/alpha".to_string()),
                to: None,
                depth: Some(9),
                limit: Some(500),
            })
            .await
            .unwrap();
        let adv = out.advisory.expect("advisory populated");
        assert_eq!(adv.applied_limit, 200, "limit clamped to 200");
        assert_eq!(adv.applied_depth, Some(3), "depth clamped to 3");
    }

    #[tokio::test]
    async fn neighbors_without_target_errors() {
        let db = backend();
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let err = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Neighbors,
                target: None,
                to: None,
                depth: None,
                limit: None,
            })
            .await;
        assert!(err.is_err(), "neighbors without a target must error");
    }

    #[tokio::test]
    async fn path_finds_connection_between_notes() {
        let db = backend();
        seed(&db).await; // reference/alpha --[[beta]]--> learning/beta
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let out = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Path,
                target: Some("reference/alpha".to_string()),
                to: Some("learning/beta".to_string()),
                depth: Some(3),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(out.path_found, Some(true), "alpha and beta are linked");
        assert!(!out.path.is_empty(), "path must contain the connecting hop");
        // The walk starts at the source and ends at the destination.
        assert_eq!(out.path.first().unwrap().from, "reference/alpha");
        assert_eq!(out.path.last().unwrap().to, "learning/beta");
        let adv = out.advisory.expect("path advisory populated");
        assert_eq!(adv.applied_depth, Some(3));
    }

    #[tokio::test]
    async fn path_reports_not_found_for_disconnected_target() {
        let db = backend();
        seed(&db).await;
        // An indexed but unconnected note (no edges into alpha's component).
        db.index_note(&note("gamma", "reference", &[]), "agent1", "reference")
            .await
            .unwrap();
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let out = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Path,
                target: Some("reference/alpha".to_string()),
                to: Some("reference/gamma".to_string()),
                depth: Some(3),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(out.path_found, Some(false), "gamma is disconnected");
        assert!(out.path.is_empty());
        // Fully explored within depth → not truncated → provably disconnected.
        assert!(!out.advisory.unwrap().truncated);
    }

    #[tokio::test]
    async fn path_without_to_errors() {
        let db = backend();
        seed(&db).await;
        let tool = NoteGraphQueryTool::new(db, "agent1");
        let err = tool
            .call_impl(NoteGraphQueryArgs {
                op: GraphOp::Path,
                target: Some("reference/alpha".to_string()),
                to: None,
                depth: None,
                limit: None,
            })
            .await;
        assert!(err.is_err(), "path without a `to` must error");
    }
}

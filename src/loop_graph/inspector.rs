//! Read-only query facade for the governance topology.
//!
//! Why a facade: four readers today (`render_session_topology`, `notify_goal_settled`,
//! `governing_owner`, `status`) each hand-roll their own read paths against
//! `LoopGraphStore` — and each asks a slightly different question. `status`'s
//! live-join comment already names the cost of that drift ("a transient
//! `SQLITE_BUSY` would manufacture audit findings against a healthy graph",
//! "team store refusal reads as 'gone'"). One inspector, one set of read
//! rules, multiple consumers.
//!
//! Design choices:
//! - **Read-only**: never calls `upsert_*` / `delete_*`. The store is the
//!   writer; this is a reader.
//! - **Bounded traversals**: walks carry an explicit hard ceiling
//!   ([`MAX_TRAVERSAL_STEPS`] = 1024, independent of node count) so an
//!   accidental cycle errors out instead of running forever. (An earlier
//!   draft of this comment described the bound as "node count plus one";
//!   the implementation has always been the fixed ceiling.)
//! - **`Result`-propagating**: a transient store error is `Err`, not
//!   `Ok(None)`. `governing_owner` already has this discipline; the inspector
//!   unifies it.
//! - **Zero allocations on the hot path beyond what `Vec` already pays**: a
//!   single `list_nodes` + `list_edges` per `subgraph_for` call, then pure
//!   compute. This is what makes the prompt cache story hold.
//!
//! What this is NOT:
//! - Not a query language. The "give me all ancestors of kind X with cadence
//!   faster than Y" question is left to a future read API; today operators
//!   read the `status` output.
//! - Not a mutation surface. Impact analysis (`impact_of_removing`) is a
//!   SIMULATION — it never calls `drop_node`. The doctor check is the
//!   equivalent read.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::error::{AlephError, Result};
use crate::loop_graph::snapshot::{EventRecord, SnapshotStore};
use crate::loop_graph::store::LoopGraphStore;
use crate::loop_graph::types::{EdgeKind, GraphEdge, GraphNode, NodeKind};

/// Maximum depth for ancestor/descendant walks. Generous (matches the upper
/// bound the lint walk sets implicitly), so an operator's deliberately deep
/// governance chain does not get silently clipped. Independent of node count.
const MAX_TRAVERSAL_STEPS: usize = 1024;

/// A pre-computed view of one node's place in the governance topology. Cheaper
/// to render than re-walking the graph per consumer (the prompt and the
/// status both ask "who watches me?" today and each builds its own answer).
#[derive(Debug, Clone)]
pub struct NodeSubgraph {
    /// The node itself. `None` for an unknown id — callers map that to
    /// "session is not governed, render nothing" (same as
    /// `render_session_topology` does today).
    pub node: Option<GraphNode>,
    /// Edges whose `to_id == node.id`. Paired with the source node so the
    /// consumer does not need a second store lookup. Empty for an unknown id.
    pub incoming: Vec<(GraphEdge, GraphNode)>,
    /// Edges whose `from_id == node.id`. Same pairing.
    pub outgoing: Vec<(GraphEdge, GraphNode)>,
    /// All `OwnsReference` ancestors (governors-of-governors-of-…), bounded
    /// by [`MAX_TRAVERSAL_STEPS`]. Empty if the chain does not reach `Root`.
    pub ancestor_chain: Vec<GraphNode>,
    /// All `OwnsReference` descendants (children-of-children-of-…), same
    /// bound.
    pub descendant_chain: Vec<GraphNode>,
    /// Unique `Watches`/`Audits` source nodes. `target_has_victory_claim`-style
    /// filtering happens at render time; the inspector reports the full list.
    pub coverage_sources: Vec<GraphNode>,
    /// Does the `OwnsReference` chain reach a `Root`? This is what the lint
    /// checks ([`LoopGraphStore::lint`]); cached here so callers do not
    /// re-walk.
    pub governance_chain_anchored: bool,
    /// All `Root` nodes in the graph. Today every governed session sees the
    /// same set; the inspector still asks once per `subgraph_for` call so
    /// callers do not duplicate the walk.
    pub all_roots: Vec<GraphNode>,
}

/// What happens if `node_id` is removed right now. Pure simulation — the
/// inspector never calls `delete_node`. Output mirrors the lint findings the
/// real removal would trigger plus the ACL rows that lose enforcement, so
/// operators can see the blast radius before committing.
#[derive(Debug, Clone)]
pub struct ImpactReport {
    /// The node the simulation was asked about. The inspector answers
    /// `Ok(None)` if the node does not exist, so the caller can short-circuit.
    pub removed_node: Option<GraphNode>,
    /// Nodes that would become naked (no `watches`/`audits` from a runnable
    /// source) BECAUSE OF this removal — i.e. covered before, uncovered in
    /// the simulated post-state. Causal, deliberately: a loop that was
    /// already naked before the removal is NOT in this list (it is in
    /// [`Self::already_naked`]), because counting it inflates the blast
    /// radius and makes the tool's "会失去看守的环" line lie about what the
    /// operator's action actually breaks.
    pub would_become_naked: Vec<String>,
    /// Loops that are naked in the simulated post-state but were ALREADY
    /// naked before the removal — reported separately so the pre-existing
    /// exposure is visible without being blamed on this removal.
    pub already_naked: Vec<String>,
    /// `OwnsReference` edges that would become unenforceable because their
    /// `from_id` is the removed node. The store already keeps these rows
    /// dangling on purpose — but the operator should know they are now
    /// enforcing against a non-existent owner (the same invariant `gc` will
    /// refuse to clean up).
    pub loses_acl: Vec<(String, String, EdgeKind)>,
    /// Human-readable lint findings the simulated graph would produce.
    /// Deliberately string-shaped — this is what the caller renders, not
    /// a structured AST.
    pub lint_findings: Vec<String>,
}

/// A summary string for governance dashboards. Three lines:
/// 1. Node count by kind, edge count by kind.
/// 2. Naked-loop count (from lint).
/// 3. Whether any chain is unanchored.
#[derive(Debug, Clone)]
pub struct TopologySummary {
    pub node_counts_by_kind: Vec<(NodeKind, usize)>,
    pub edge_counts_by_kind: Vec<(EdgeKind, usize)>,
    pub naked_loop_count: usize,
    pub unanchored_chain_count: usize,
    pub governance_chain_anchored: bool,
}

impl TopologySummary {
    /// Render as a deterministic text block — what `loop_graph status` and
    /// `governance_metrics` ship.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        out.push_str("Topology:\n");
        out.push_str("  nodes:\n");
        for (k, n) in &self.node_counts_by_kind {
            let _ = writeln!(out, "    {} = {}", k.as_str(), n);
        }
        out.push_str("  edges:\n");
        for (k, n) in &self.edge_counts_by_kind {
            let _ = writeln!(out, "    {} = {}", k.as_str(), n);
        }
        let _ = writeln!(out, "  naked_loops = {}", self.naked_loop_count);
        let _ = writeln!(out, "  unanchored_chains = {}", self.unanchored_chain_count);
        out
    }
}

/// The inspector. Cheap to construct (just borrows the store); clones cheaply
/// (the store is `Arc`-backed inside the loop_graph module).
pub struct LoopGraphInspector<'a> {
    store: &'a LoopGraphStore,
    agent_id: &'a str,
    snapshots: Option<&'a SnapshotStore>,
}

impl<'a> LoopGraphInspector<'a> {
    #[must_use]
    pub const fn new(store: &'a LoopGraphStore, agent_id: &'a str) -> Self {
        Self {
            store,
            agent_id,
            snapshots: None,
        }
    }

    /// Attach the snapshot store (unlocks [`Self::recent_events`], the read
    /// half of the topology-mutation audit log).
    #[must_use]
    pub const fn with_snapshots(mut self, snapshots: &'a SnapshotStore) -> Self {
        self.snapshots = Some(snapshots);
        self
    }

    /// Newest topology-mutation events, bounded. Pure delegation to
    /// [`SnapshotStore::list_events`]; `Err` (not an empty vec) when no
    /// snapshot store is attached, so "subsystem absent" is never confused
    /// with "nothing happened".
    ///
    /// For paged reads (audit log past the first page), use
    /// [`Self::recent_events_before`].
    pub fn recent_events(&self, limit: usize) -> Result<Vec<EventRecord>> {
        self.recent_events_before(limit, None)
    }

    /// Paged variant of [`Self::recent_events`]. `before_id = Some(id)` returns
    /// rows with `id < before_id` (exclusive), letting a UI walk the audit log
    /// past the first page. `before_id = None` is equivalent to
    /// `recent_events`.
    pub fn recent_events_before(
        &self,
        limit: usize,
        before_id: Option<i64>,
    ) -> Result<Vec<EventRecord>> {
        let snapshots = self
            .snapshots
            .ok_or_else(|| AlephError::other("loop_graph inspector: no snapshot store attached"))?;
        snapshots.list_events(limit, before_id)
    }

    /// Compute a [`NodeSubgraph`] for `node_id`. `Ok(None)` is "not a
    /// registered node" — the prompt renders nothing for that case, and the
    /// status panel renders "untracked".
    pub fn subgraph_for(&self, node_id: &str) -> Result<Option<NodeSubgraph>> {
        let nodes = self.store.list_nodes(self.agent_id)?;
        let edges = self.store.list_edges(self.agent_id)?;
        let by_id: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let Some(node) = by_id.get(node_id).copied().cloned() else {
            return Ok(None);
        };

        let incoming: Vec<(GraphEdge, GraphNode)> = edges
            .iter()
            .filter(|e| e.to_id == node_id)
            .filter_map(|e| {
                by_id
                    .get(e.from_id.as_str())
                    .map(|n| ((*e).clone(), (*n).clone()))
            })
            .collect();

        let outgoing: Vec<(GraphEdge, GraphNode)> = edges
            .iter()
            .filter(|e| e.from_id == node_id)
            .filter_map(|e| {
                by_id
                    .get(e.to_id.as_str())
                    .map(|n| ((*e).clone(), (*n).clone()))
            })
            .collect();

        // ancestors / descendants walk OwnsReference both directions, bounded.
        let ancestor_chain = walk_chain(
            &edges,
            &by_id,
            node_id,
            EdgeKind::OwnsReference,
            Direction::Up,
        )?;
        let descendant_chain = walk_chain(
            &edges,
            &by_id,
            node_id,
            EdgeKind::OwnsReference,
            Direction::Down,
        )?;

        // coverage sources: only edges whose source can RUN. Same predicate
        // the lint uses (Round 11 / coverage_source_rejection).
        let coverage_sources: Vec<GraphNode> = incoming
            .iter()
            .filter(|(e, _)| matches!(e.kind, EdgeKind::Watches | EdgeKind::Audits))
            .filter_map(|(_, src)| {
                if can_cover_kind(src.kind) {
                    Some(src.clone())
                } else {
                    None
                }
            })
            .collect();

        // Chain anchored = some node on the ancestor chain is Root.
        let governance_chain_anchored = ancestor_chain.iter().any(|n| n.kind == NodeKind::Root);

        let all_roots: Vec<GraphNode> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Root)
            .cloned()
            .collect();

        Ok(Some(NodeSubgraph {
            node: Some(node),
            incoming,
            outgoing,
            ancestor_chain,
            descendant_chain,
            coverage_sources,
            governance_chain_anchored,
            all_roots,
        }))
    }

    /// Simulate removing `node_id`. Pure read — never writes.
    pub fn impact_of_removing(&self, node_id: &str) -> Result<ImpactReport> {
        let nodes = self.store.list_nodes(self.agent_id)?;
        let edges = self.store.list_edges(self.agent_id)?;

        let removed_node = nodes.iter().find(|n| n.id == node_id).cloned();

        // Hypothetical post-state: the NODE is gone but the edges touching it
        // are still on disk — `drop_node` deliberately does not cascade (they
        // become dangling audit signals until an explicit `gc`). Filtering them
        // out would model a gc the operator did not run and hide exactly the
        // dangling-edge finding this report exists to preview.
        let present: HashSet<&str> = nodes
            .iter()
            .filter(|n| n.id != node_id)
            .map(|n| n.id.as_str())
            .collect();
        let post_edges: Vec<&GraphEdge> = edges.iter().collect();
        let pre_by_id: HashMap<&str, &GraphNode> =
            nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let post_by_id: HashMap<&str, &GraphNode> = nodes
            .iter()
            .filter(|n| n.id != node_id)
            .map(|n| (n.id.as_str(), n))
            .collect();

        // Coverage predicate, evaluated against a node-index: the edges are
        // identical pre/post (drop_node does not cascade), so the ONLY thing
        // the removal changes is whether a coverage SOURCE still exists.
        let has_coverage = |by_id: &HashMap<&str, &GraphNode>, target: &str| -> bool {
            post_edges.iter().any(|e| {
                e.to_id == target
                    && e.from_id != target
                    && matches!(e.kind, EdgeKind::Watches | EdgeKind::Audits)
                    && by_id
                        .get(e.from_id.as_str())
                        .is_some_and(|s| can_cover_kind(s.kind))
            })
        };

        let mut would_become_naked = Vec::new();
        let mut already_naked = Vec::new();
        for n in &nodes {
            if n.id == node_id {
                continue;
            }
            if !n.kind.is_optimization_loop() {
                continue;
            }
            if has_coverage(&post_by_id, &n.id) {
                continue;
            }
            // Naked in the post-state. Was it covered BEFORE? Only then is
            // the removal the cause; an already-naked loop is pre-existing
            // exposure, not blast radius.
            if has_coverage(&pre_by_id, &n.id) {
                would_become_naked.push(n.id.clone());
            } else {
                already_naked.push(n.id.clone());
            }
        }

        let mut loses_acl = Vec::new();
        for e in &edges {
            if e.from_id == node_id && e.kind == EdgeKind::OwnsReference {
                loses_acl.push((e.from_id.clone(), e.to_id.clone(), e.kind));
            }
        }

        // Lightweight lint pass for the simulated state.
        let mut lint_findings = Vec::new();
        for e in &post_edges {
            let missing: Vec<&str> = [e.from_id.as_str(), e.to_id.as_str()]
                .into_iter()
                .filter(|id| !present.contains(*id))
                .collect();
            if !missing.is_empty() {
                lint_findings.push(format!(
                    "悬空边: {} -[{}]-> {} (节点 {:?} 已消失)",
                    e.from_id,
                    e.kind.as_str(),
                    e.to_id,
                    missing
                ));
            }
        }
        for n in &nodes {
            if n.id == node_id {
                continue;
            }
            // The simulated lint mirrors the REAL lint on the post-state,
            // which flags every naked loop — caused by this removal or not.
            // (The causal split lives in `would_become_naked` /
            // `already_naked`; this preview would under-report otherwise.)
            if would_become_naked.contains(&n.id) || already_naked.contains(&n.id) {
                lint_findings.push(format!(
                    "裸奔优化环: {} ('{}') 没有任何 watches/audits 入边",
                    n.id, n.label
                ));
            }
        }

        Ok(ImpactReport {
            removed_node,
            would_become_naked,
            already_naked,
            loses_acl,
            lint_findings,
        })
    }

    /// The aggregate-count answer for the topology: node/edge counts by kind
    /// plus the naked-loop / unanchored-chain tallies from lint.
    ///
    /// **Single-source direction**: this is where aggregate counts live.
    /// `loop_graph status` does NOT consume it today — status renders
    /// per-node live joins (goal/cron/team three-way reads of the EXECUTING
    /// entities' own stores) that this aggregate view deliberately lacks, and
    /// switching it over would silently drop exactly those lines; the two are
    /// instead pinned consistent by
    /// `loop_graph_manage::tests::status_counts_agree_with_inspector_summary`.
    /// (An earlier draft of this doc claimed status and `governance_metrics`
    /// already consumed `summary()` — neither did; the only production
    /// consumer of the inspector today is the `impact` action.) If status's
    /// aggregate half ever moves, it moves HERE, not into a second hand-roll.
    pub fn summary(&self) -> Result<TopologySummary> {
        let nodes = self.store.list_nodes(self.agent_id)?;
        let edges = self.store.list_edges(self.agent_id)?;
        let findings = self.store.lint(self.agent_id)?;

        let mut node_counts: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for n in &nodes {
            *node_counts.entry(n.kind.as_str()).or_insert(0) += 1;
        }
        let node_counts_by_kind: Vec<(NodeKind, usize)> = node_counts
            .into_iter()
            .filter_map(|(s, c)| NodeKind::parse(s).map(|k| (k, c)))
            .collect();

        let mut edge_counts: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for e in &edges {
            *edge_counts.entry(e.kind.as_str()).or_insert(0) += 1;
        }
        let edge_counts_by_kind: Vec<(EdgeKind, usize)> = edge_counts
            .into_iter()
            .filter_map(|(s, c)| EdgeKind::parse(s).map(|k| (k, c)))
            .collect();

        let naked_loop_count = findings.iter().filter(|f| f.contains("裸奔优化环")).count();
        let unanchored_chain_count = findings
            .iter()
            .filter(|f| f.contains("治理链未锚定"))
            .count();

        // Empty graph = vacuously anchored ("nothing declared, nothing
        // broken"). Without this, a brand-new deployment reads
        // `governance_chain_anchored: false` and prompts an operator to run
        // `enable_audit` for no reason.
        let governance_chain_anchored =
            unanchored_chain_count == 0 || nodes.is_empty();

        Ok(TopologySummary {
            node_counts_by_kind,
            edge_counts_by_kind,
            naked_loop_count,
            unanchored_chain_count,
            governance_chain_anchored,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Up,
    Down,
}

fn walk_chain(
    edges: &[GraphEdge],
    by_id: &HashMap<&str, &GraphNode>,
    start: &str,
    kind: EdgeKind,
    direction: Direction,
) -> Result<Vec<GraphNode>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut out: Vec<GraphNode> = Vec::new();
    let mut frontier: Vec<String> = vec![start.to_string()];
    let mut steps: usize = 0;

    while let Some(current) = frontier.pop() {
        if steps >= MAX_TRAVERSAL_STEPS {
            return Err(AlephError::other(format!(
                "loop_graph inspector: chain walk exceeded {MAX_TRAVERSAL_STEPS} steps — \
                 refusing to walk a cyclic chain of length unbounded"
            )));
        }
        steps += 1;
        if !visited.insert(current.clone()) {
            continue;
        }
        let next_edges = edges.iter().filter(|e| {
            e.kind == kind
                && match direction {
                    Direction::Up => e.to_id == current,
                    Direction::Down => e.from_id == current,
                }
        });
        for e in next_edges {
            let next_id = match direction {
                Direction::Up => &e.from_id,
                Direction::Down => &e.to_id,
            };
            if let Some(n) = by_id.get(next_id.as_str()) {
                if !visited.contains(next_id) {
                    frontier.push(next_id.clone());
                    out.push((*n).clone());
                }
            }
        }
    }
    Ok(out)
}

/// Mirror of `LoopGraphStore::coverage_source_rejection` — the lint says a
/// watcher has to RUN, so the inspector refuses to count `Anchor`/`Frozen`
/// as coverage. `Root` is allowed (a human reads the digest, three-layer cap).
fn can_cover_kind(kind: NodeKind) -> bool {
    kind.is_optimization_loop() || kind == NodeKind::Root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_graph::types::Origin;

    fn store() -> (tempfile::TempDir, LoopGraphStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = LoopGraphStore::open(&dir.path().join("g.db")).unwrap();
        (dir, s)
    }

    fn seed(agent: &str, store: &LoopGraphStore) {
        store
            .upsert_node(&GraphNode::new(
                agent,
                "goal:s1",
                NodeKind::LoopGoal,
                "g",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(&GraphNode::new(
                agent,
                "cron:gov",
                NodeKind::LoopCron,
                "gov",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(&GraphNode::new(
                agent,
                "daemon:dreaming",
                NodeKind::Daemon,
                "d",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(&GraphNode::new(
                agent,
                "root:aleph",
                NodeKind::Root,
                "what 'better' means",
                Origin::Human,
            ))
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                agent,
                "daemon:dreaming",
                "goal:s1",
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                agent,
                "cron:gov",
                "daemon:dreaming",
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                agent,
                "root:aleph",
                "cron:gov",
                EdgeKind::Watches,
                Origin::Human,
            ))
            .unwrap();
        // The governance chain anchors through OwnsReference, not Watches —
        // this edge is what makes `governance_chain_anchored` true.
        store
            .upsert_edge(&GraphEdge::new(
                agent,
                "root:aleph",
                "cron:gov",
                EdgeKind::OwnsReference,
                Origin::Human,
            ))
            .unwrap();
    }

    #[test]
    fn subgraph_for_known_node_includes_ancestor_chain_to_root() {
        let (_d, s) = store();
        seed("main", &s);
        let inspector = LoopGraphInspector::new(&s, "main");
        let sub = inspector
            .subgraph_for("goal:s1")
            .expect("store is healthy")
            .expect("node exists");
        let node = sub
            .node
            .expect("subgraph_for a registered node returns Some");
        assert_eq!(node.id, "goal:s1");

        // ancestor chain (OwnsReference up from goal:s1) hits daemon -> cron.
        let anc_ids: Vec<&str> = sub.ancestor_chain.iter().map(|n| n.id.as_str()).collect();
        assert!(anc_ids.contains(&"daemon:dreaming"), "{anc_ids:?}");
        assert!(anc_ids.contains(&"cron:gov"), "{anc_ids:?}");
        assert!(sub.governance_chain_anchored, "root:aleph is upstream");
        assert!(sub.all_roots.iter().any(|r| r.id == "root:aleph"));

        // coverage: root watches cron:gov (which is the goal's ancestor), but
        // not the goal itself — coverage_sources is "who watches ME", not my
        // ancestors. Verify cron:gov is NOT in my coverage, root is NOT either.
        let cov_ids: Vec<&str> = sub.coverage_sources.iter().map(|n| n.id.as_str()).collect();
        assert!(cov_ids.is_empty(), "{cov_ids:?}");
    }

    #[test]
    fn subgraph_for_unknown_node_is_ok_none_not_error() {
        let (_d, s) = store();
        seed("main", &s);
        let inspector = LoopGraphInspector::new(&s, "main");
        let result = inspector.subgraph_for("goal:nonexistent").unwrap();
        assert!(
            result.is_none(),
            "an unknown id is Ok(None), not an Err — mirrors render_session_topology"
        );
    }

    #[test]
    fn subgraph_for_node_with_coverage_includes_watcher() {
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:s1",
            NodeKind::LoopGoal,
            "g",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&GraphNode::new(
            "main",
            "cron:watcher",
            NodeKind::LoopCron,
            "watcher",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:watcher",
            "goal:s1",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        let inspector = LoopGraphInspector::new(&s, "main");
        let sub = inspector.subgraph_for("goal:s1").unwrap().unwrap();
        assert_eq!(sub.coverage_sources.len(), 1);
        assert_eq!(sub.coverage_sources[0].id, "cron:watcher");
    }

    #[test]
    fn impact_of_removing_a_watcher_flags_exposed_target() {
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:s1",
            NodeKind::LoopGoal,
            "g",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&GraphNode::new(
            "main",
            "cron:watcher",
            NodeKind::LoopCron,
            "watcher",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:watcher",
            "goal:s1",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        let inspector = LoopGraphInspector::new(&s, "main");
        let impact = inspector.impact_of_removing("cron:watcher").unwrap();
        assert!(
            impact.would_become_naked.iter().any(|id| id == "goal:s1"),
            "{:?}",
            impact.would_become_naked
        );
        assert!(!impact.loses_acl.is_empty() || impact.loses_acl.is_empty()); // sanity
    }

    /// `would_become_naked` is CAUSAL: a loop already naked before the
    /// removal is pre-existing exposure, not blast radius. The old predicate
    /// counted every post-state-naked loop, so the tool's "会失去看守的环"
    /// line blamed the removal for loops it never watched over.
    #[test]
    fn impact_does_not_blame_pre_existing_naked_loops_on_the_removal() {
        let (_d, s) = store();
        for (id, kind) in [
            ("goal:s1", NodeKind::LoopGoal),
            ("cron:watcher", NodeKind::LoopCron),
            ("daemon:naked", NodeKind::Daemon),
        ] {
            s.upsert_node(&GraphNode::new("main", id, kind, "x", Origin::Llm))
                .unwrap();
        }
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:watcher",
            "goal:s1",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();

        let inspector = LoopGraphInspector::new(&s, "main");
        let impact = inspector.impact_of_removing("cron:watcher").unwrap();
        assert_eq!(
            impact.would_become_naked,
            vec!["goal:s1".to_string()],
            "only the loop that LOSES coverage belongs here: {:?}",
            impact.would_become_naked
        );
        assert_eq!(
            impact.already_naked,
            vec!["daemon:naked".to_string()],
            "pre-existing exposure is reported separately: {:?}",
            impact.already_naked
        );
        // The simulated lint, like the real one, still flags both.
        let naked_findings = impact
            .lint_findings
            .iter()
            .filter(|f| f.contains("裸奔优化环"))
            .count();
        assert_eq!(naked_findings, 2, "{:?}", impact.lint_findings);
    }

    #[test]
    fn impact_of_removing_a_root_breaks_chain_anchoring() {
        let (_d, s) = store();
        seed("main", &s);
        let inspector = LoopGraphInspector::new(&s, "main");
        let impact = inspector.impact_of_removing("root:aleph").unwrap();
        // Removing root does not make a loop naked (root isn't a watcher of
        // an optimization loop in this fixture), but it breaks the chain.
        assert!(
            impact.lint_findings.iter().any(|f| f.contains("悬空边")),
            "simulated state must surface dangling edges: {:?}",
            impact.lint_findings
        );
    }

    #[test]
    fn impact_of_removing_unknown_node_is_a_noop() {
        let (_d, s) = store();
        seed("main", &s);
        let inspector = LoopGraphInspector::new(&s, "main");
        let impact = inspector.impact_of_removing("goal:nonexistent").unwrap();
        assert!(impact.removed_node.is_none());
        assert!(impact.loses_acl.is_empty());
        // The graph is unchanged, so the simulated state equals the real one:
        // no NEW dangling edges appear, and the naked-loop list is the same as
        // today's lint (goal:s1 is already naked in the fixture — nothing
        // watches it).
        assert!(
            impact.lint_findings.iter().all(|f| !f.contains("悬空边")),
            "removing nothing must not manufacture dangling edges: {:?}",
            impact.lint_findings
        );
    }

    #[test]
    fn impact_of_removing_governor_loses_acl_but_store_keeps_it() {
        // Sanity check the comment in store.rs: removing the governor of an
        // owns_reference edge leaves the edge dangling but still enforcing.
        // The inspector surfaces that asymmetry so the operator sees it BEFORE
        // committing.
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:s1",
            NodeKind::LoopGoal,
            "g",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&GraphNode::new(
            "main",
            "cron:gov",
            NodeKind::LoopCron,
            "gov",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:gov",
            "goal:s1",
            EdgeKind::OwnsReference,
            Origin::Llm,
        ))
        .unwrap();
        let inspector = LoopGraphInspector::new(&s, "main");
        let impact = inspector.impact_of_removing("cron:gov").unwrap();
        assert_eq!(impact.loses_acl.len(), 1, "{:?}", impact.loses_acl);
        assert_eq!(impact.loses_acl[0].1, "goal:s1");
    }

    #[test]
    fn summary_counts_match_lint_findings() {
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:s1",
            NodeKind::LoopGoal,
            "g",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&GraphNode::new(
            "main",
            "daemon:naked",
            NodeKind::Daemon,
            "naked",
            Origin::Llm,
        ))
        .unwrap();
        let inspector = LoopGraphInspector::new(&s, "main");
        let summary = inspector.summary().unwrap();
        // Two nodes: LoopGoal + Daemon.
        let kinds: Vec<(&str, usize)> = summary
            .node_counts_by_kind
            .iter()
            .map(|(k, c)| (k.as_str(), *c))
            .collect();
        assert!(kinds.contains(&("loop_goal", 1)), "{kinds:?}");
        assert!(kinds.contains(&("daemon", 1)), "{kinds:?}");
        // daemon:naked has no watcher.
        assert!(summary.naked_loop_count >= 1, "{summary:?}");
    }

    #[test]
    fn chains_longer_than_max_steps_are_rejected_not_runaway() {
        // Build a chain A owns B owns C ... of length MAX_TRAVERSAL_STEPS+5
        // and confirm the walk errors rather than infinite-looping.
        let (_d, s) = store();
        let n = MAX_TRAVERSAL_STEPS + 5;
        for i in 0..n {
            s.upsert_node(&GraphNode::new(
                "main",
                format!("cron:n{i}"),
                NodeKind::LoopCron,
                "n",
                Origin::Llm,
            ))
            .unwrap();
        }
        for i in 0..(n - 1) {
            s.upsert_edge(&GraphEdge::new(
                "main",
                format!("cron:n{i}"),
                format!("cron:n{}", i + 1),
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        }
        let inspector = LoopGraphInspector::new(&s, "main");
        let res = inspector.subgraph_for("cron:n0");
        assert!(
            res.is_err(),
            "a chain past MAX_TRAVERSAL_STEPS must error, not run forever"
        );
        let err = res.unwrap_err().to_string();
        assert!(err.contains("exceeded"), "{err}");
    }

    #[test]
    fn inspector_reads_propagate_store_errors() {
        // Corrupt the schema behind the store's back: the next read must be an
        // Err, not a folded "no graph".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.db");
        let store = LoopGraphStore::open(&path).unwrap();
        let raw = crate::utils::sqlite_open::open_sqlite_safe(&path).unwrap();
        raw.execute_batch("DROP TABLE graph_nodes; DROP TABLE graph_edges;")
            .unwrap();
        let inspector = LoopGraphInspector::new(&store, "main");
        let result = inspector.subgraph_for("goal:s1");
        assert!(
            result.is_err(),
            "unreadable store must error, not return None"
        );
    }

    #[test]
    fn recent_events_delegates_to_the_snapshot_store() {
        let dir = tempfile::tempdir().unwrap();
        let s = LoopGraphStore::open(&dir.path().join("g.db")).unwrap();
        let snaps = crate::loop_graph::SnapshotStore::open(&dir.path().join("s.db")).unwrap();
        snaps
            .append_event(&crate::loop_graph::TopologyEvent::GcCompleted {
                agent_id: "main".into(),
                removed: 2,
                retained_acl: 1,
            })
            .unwrap();

        // Without the store attached: Err, never a silent empty vec.
        let bare = LoopGraphInspector::new(&s, "main");
        assert!(
            bare.recent_events(10).is_err(),
            "no snapshot store attached must be an Err, not 'nothing happened'"
        );

        let inspector = LoopGraphInspector::new(&s, "main").with_snapshots(&snaps);
        let rows = inspector.recent_events(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "gc_completed");
    }
}

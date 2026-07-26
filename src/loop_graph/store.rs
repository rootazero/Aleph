//! `LoopGraphStore` — `SQLite` persistence for the governance topology.
//!
//! Its own small DB (`loop_graph.db`), NOT the notes store: the topology is
//! the held-out layer that watches the optimizers, so it must live outside
//! every optimizer's writable domain (dreaming rewrites notes nightly; it
//! must never be able to rewrite who watches it). Opens via the process-safe
//! helper (`open_sqlite_safe`, Spec C).
//!
//! No FK cascade by design: an edge whose endpoint disappeared is an AUDIT
//! SIGNAL ("the governed loop vanished"), surfaced by [`LoopGraphStore::lint`]
//! and only removed by an explicit `gc`.

use std::path::Path;

use crate::error::{AlephError, Result};
use crate::loop_graph::types::{cadence_rank, EdgeKind, GraphEdge, GraphNode, NodeKind, Origin};
use crate::sync_primitives::{Mutex, MutexGuard};

pub struct LoopGraphStore {
    conn: Mutex<rusqlite::Connection>,
}

impl LoopGraphStore {
    /// Open (creating if needed) the graph DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AlephError::other(e.to_string()))?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("loop_graph store open: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graph_nodes (
                 agent_id      TEXT NOT NULL,
                 id            TEXT NOT NULL,
                 kind          TEXT NOT NULL,
                 label         TEXT NOT NULL,
                 body          TEXT,
                 cadence       TEXT,
                 origin        TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 PRIMARY KEY (agent_id, id)
             );
             CREATE TABLE IF NOT EXISTS graph_edges (
                 agent_id      TEXT NOT NULL,
                 from_id       TEXT NOT NULL,
                 to_id         TEXT NOT NULL,
                 kind          TEXT NOT NULL,
                 note          TEXT,
                 origin        TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 PRIMARY KEY (agent_id, from_id, to_id, kind)
             );",
        )
        .map_err(|e| AlephError::other(format!("loop_graph store init: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an existing DB read-only (doctor/lint path). Errors if missing —
    /// callers treat "no file" as "no graph yet", not a fault.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| AlephError::other(format!("loop_graph store open_readonly: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert or update a node. Structural invariant (the machine-unreachable
    /// root, §7.3 of the design): a `root:` node whose origin is not `human`
    /// is rejected at the store level — every root reference is forced to
    /// carry the "a human set this" attestation.
    pub fn upsert_node(&self, node: &GraphNode) -> Result<()> {
        if node.kind == NodeKind::Root && node.origin != Origin::Human {
            return Err(AlephError::other(
                "loop_graph invariant: root nodes must have origin=human — \
                 the root reference is supplied by a person from outside the graph",
            ));
        }
        self.lock()
            .execute(
                "INSERT INTO graph_nodes
                     (agent_id, id, kind, label, body, cadence, origin, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(agent_id, id) DO UPDATE SET
                     kind = excluded.kind,
                     label = excluded.label,
                     body = excluded.body,
                     cadence = excluded.cadence,
                     origin = excluded.origin,
                     updated_at_ms = excluded.updated_at_ms",
                rusqlite::params![
                    node.agent_id,
                    node.id,
                    node.kind.as_str(),
                    node.label,
                    node.body,
                    node.cadence,
                    node.origin.as_str(),
                    node.updated_at_ms,
                ],
            )
            .map_err(|e| AlephError::other(format!("loop_graph node upsert: {e}")))?;
        Ok(())
    }

    /// Fetch one node. Missing row or unknown enum text (schema drift) is `None`.
    pub fn get_node(&self, agent_id: &str, id: &str) -> Result<Option<GraphNode>> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT agent_id, id, kind, label, body, cadence, origin,
                        created_at_ms, updated_at_ms
                 FROM graph_nodes WHERE agent_id = ?1 AND id = ?2",
                rusqlite::params![agent_id, id],
                row_to_node,
            )
            .optional()
            .map_err(|e| AlephError::other(format!("loop_graph node get: {e}")))?;
        Ok(row.flatten())
    }

    /// Delete a node. Edges referencing it are left in place on purpose —
    /// they become dangling audit signals until an explicit `gc`.
    pub fn delete_node(&self, agent_id: &str, id: &str) -> Result<bool> {
        let n = self
            .lock()
            .execute(
                "DELETE FROM graph_nodes WHERE agent_id = ?1 AND id = ?2",
                rusqlite::params![agent_id, id],
            )
            .map_err(|e| AlephError::other(format!("loop_graph node delete: {e}")))?;
        Ok(n > 0)
    }

    pub fn list_nodes(&self, agent_id: &str) -> Result<Vec<GraphNode>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, id, kind, label, body, cadence, origin,
                        created_at_ms, updated_at_ms
                 FROM graph_nodes WHERE agent_id = ?1 ORDER BY kind, id",
            )
            .map_err(|e| AlephError::other(format!("loop_graph nodes prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_node)
            .map_err(|e| AlephError::other(format!("loop_graph nodes query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(Some(n)) = r {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// Insert or update an edge. Both endpoints must exist at creation time
    /// (catches typos); dangling only ever arises from later node deletion.
    pub fn upsert_edge(&self, edge: &GraphEdge) -> Result<()> {
        let conn = self.lock();
        for endpoint in [&edge.from_id, &edge.to_id] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM graph_nodes WHERE agent_id = ?1 AND id = ?2)",
                    rusqlite::params![edge.agent_id, endpoint],
                    |r| r.get(0),
                )
                .map_err(|e| AlephError::other(format!("loop_graph edge check: {e}")))?;
            if !exists {
                return Err(AlephError::other(format!(
                    "loop_graph edge: node '{endpoint}' does not exist — register it first \
                     with action='node'"
                )));
            }
        }
        conn.execute(
            "INSERT INTO graph_edges
                 (agent_id, from_id, to_id, kind, note, origin, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(agent_id, from_id, to_id, kind) DO UPDATE SET
                 note = excluded.note,
                 origin = excluded.origin",
            rusqlite::params![
                edge.agent_id,
                edge.from_id,
                edge.to_id,
                edge.kind.as_str(),
                edge.note,
                edge.origin.as_str(),
                edge.created_at_ms,
            ],
        )
        .map_err(|e| AlephError::other(format!("loop_graph edge upsert: {e}")))?;
        Ok(())
    }

    pub fn delete_edge(
        &self,
        agent_id: &str,
        from_id: &str,
        to_id: &str,
        kind: EdgeKind,
    ) -> Result<bool> {
        let n = self
            .lock()
            .execute(
                "DELETE FROM graph_edges
                 WHERE agent_id = ?1 AND from_id = ?2 AND to_id = ?3 AND kind = ?4",
                rusqlite::params![agent_id, from_id, to_id, kind.as_str()],
            )
            .map_err(|e| AlephError::other(format!("loop_graph edge delete: {e}")))?;
        Ok(n > 0)
    }

    pub fn list_edges(&self, agent_id: &str) -> Result<Vec<GraphEdge>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, from_id, to_id, kind, note, origin, created_at_ms
                 FROM graph_edges WHERE agent_id = ?1 ORDER BY from_id, to_id, kind",
            )
            .map_err(|e| AlephError::other(format!("loop_graph edges prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_edge)
            .map_err(|e| AlephError::other(format!("loop_graph edges query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(Some(e)) = r {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Remove dangling edges (endpoint node gone). Explicit only — never
    /// automatic. Returns human-readable descriptions of what was removed.
    pub fn gc(&self, agent_id: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, id, kind, label, body, cadence, origin,
                        created_at_ms, updated_at_ms
                 FROM graph_nodes WHERE agent_id = ?1",
            )
            .map_err(|e| AlephError::other(format!("loop_graph gc nodes prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_node)
            .map_err(|e| AlephError::other(format!("loop_graph gc nodes query: {e}")))?;
        let ids: std::collections::HashSet<String> = rows
            .filter_map(|r| r.ok().and_then(|n| n.map(|n| n.id)))
            .collect();
        drop(stmt);

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, from_id, to_id, kind, note, origin, created_at_ms
                 FROM graph_edges WHERE agent_id = ?1",
            )
            .map_err(|e| AlephError::other(format!("loop_graph gc edges prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_edge)
            .map_err(|e| AlephError::other(format!("loop_graph gc edges query: {e}")))?;
        let edges: Vec<GraphEdge> = rows.filter_map(|r| r.ok().and_then(|e| e)).collect();
        drop(stmt);

        let mut removed = Vec::new();
        for e in &edges {
            if !ids.contains(&e.from_id) || !ids.contains(&e.to_id) {
                conn.execute(
                    "DELETE FROM graph_edges
                     WHERE agent_id = ?1 AND from_id = ?2 AND to_id = ?3 AND kind = ?4",
                    rusqlite::params![agent_id, e.from_id, e.to_id, e.kind.as_str()],
                )
                .map_err(|err| AlephError::other(format!("loop_graph gc: {err}")))?;
                removed.push(format!(
                    "{} -[{}]-> {}",
                    e.from_id,
                    e.kind.as_str(),
                    e.to_id
                ));
            }
        }
        Ok(removed)
    }

    /// Structural lint — pure graph checks, zero semantics (semantic verdicts
    /// belong to the audit loop's LLM turn):
    /// - dangling edges (endpoint vanished);
    /// - naked optimization loops (no `watches`/`audits` in-edge);
    /// - governors whose `owns_reference` chain does not terminate at a
    ///   `root:` node (including cycles);
    /// - fast loops owning slower loops' references (only when both declare
    ///   a known cadence class).
    pub fn lint(&self, agent_id: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, id, kind, label, body, cadence, origin,
                        created_at_ms, updated_at_ms
                 FROM graph_nodes WHERE agent_id = ?1 ORDER BY kind, id",
            )
            .map_err(|e| AlephError::other(format!("loop_graph lint nodes prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_node)
            .map_err(|e| AlephError::other(format!("loop_graph lint nodes query: {e}")))?;
        let mut nodes = Vec::new();
        for r in rows {
            if let Ok(Some(n)) = r {
                nodes.push(n);
            }
        }
        drop(stmt);

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, from_id, to_id, kind, note, origin, created_at_ms
                 FROM graph_edges WHERE agent_id = ?1 ORDER BY from_id, to_id, kind",
            )
            .map_err(|e| AlephError::other(format!("loop_graph lint edges prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_edge)
            .map_err(|e| AlephError::other(format!("loop_graph lint edges query: {e}")))?;
        let mut edges = Vec::new();
        for r in rows {
            if let Ok(Some(e)) = r {
                edges.push(e);
            }
        }
        drop(stmt);

        let by_id: std::collections::HashMap<&str, &GraphNode> =
            nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let mut findings = lint_dangling_edges(&edges, &by_id);
        findings.extend(lint_naked_loops(&nodes, &edges));
        findings.extend(lint_governance_chain(&nodes, &edges, &by_id));
        findings.extend(lint_cadence_mismatch(&edges, &by_id));
        Ok(findings)
    }
}

fn lint_dangling_edges(
    edges: &[GraphEdge],
    by_id: &std::collections::HashMap<&str, &GraphNode>,
) -> Vec<String> {
    let mut findings = Vec::new();
    for e in edges {
        let missing: Vec<&str> = [e.from_id.as_str(), e.to_id.as_str()]
            .into_iter()
            .filter(|id| !by_id.contains_key(id))
            .collect();
        if !missing.is_empty() {
            findings.push(format!(
                "悬空边: {} -[{}]-> {}（节点 {:?} 已消失——被治理的环不见了，需审计裁决或 gc）",
                e.from_id,
                e.kind.as_str(),
                e.to_id,
                missing
            ));
        }
    }
    findings
}

fn lint_naked_loops(nodes: &[GraphNode], edges: &[GraphEdge]) -> Vec<String> {
    let mut findings = Vec::new();
    for n in nodes {
        if !n.kind.is_optimization_loop() {
            continue;
        }
        let watched = edges
            .iter()
            .any(|e| e.to_id == n.id && matches!(e.kind, EdgeKind::Watches | EdgeKind::Audits));
        if !watched {
            findings.push(format!(
                "裸奔优化环: {}（'{}'）没有任何 watches/audits 入边",
                n.id, n.label
            ));
        }
    }
    findings
}

/// Walk each governor upward through incoming `owns_reference` edges; bounded
/// by node count so a cycle cannot loop forever.
fn lint_governance_chain(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    by_id: &std::collections::HashMap<&str, &GraphNode>,
) -> Vec<String> {
    let mut findings = Vec::new();
    let governors: Vec<&GraphNode> = nodes
        .iter()
        .filter(|n| {
            edges
                .iter()
                .any(|e| e.from_id == n.id && e.kind == EdgeKind::OwnsReference)
        })
        .collect();
    for g in governors {
        let mut current = g.id.as_str();
        let mut steps = 0;
        let terminated_at_root = loop {
            if by_id.get(current).is_some_and(|n| n.kind == NodeKind::Root) {
                break true;
            }
            let owner = edges
                .iter()
                .find(|e| e.to_id == current && e.kind == EdgeKind::OwnsReference)
                .map(|e| e.from_id.as_str());
            match owner {
                Some(o) if steps < nodes.len() => {
                    current = o;
                    steps += 1;
                }
                _ => break false,
            }
        };
        if !terminated_at_root {
            findings.push(format!(
                "治理链未锚定: {} 拥有他环参照，但其向上的 owns_reference 链不汇于任何 root 节点（或成环）",
                g.id
            ));
        }
    }
    findings
}

fn lint_cadence_mismatch(
    edges: &[GraphEdge],
    by_id: &std::collections::HashMap<&str, &GraphNode>,
) -> Vec<String> {
    let mut findings = Vec::new();
    for e in edges {
        if e.kind != EdgeKind::OwnsReference {
            continue;
        }
        let (Some(owner), Some(child)) =
            (by_id.get(e.from_id.as_str()), by_id.get(e.to_id.as_str()))
        else {
            continue;
        };
        if let (Some(oc), Some(cc)) = (owner.cadence.as_deref(), child.cadence.as_deref()) {
            if let (Some(or), Some(cr)) = (cadence_rank(oc), cadence_rank(cc)) {
                if or < cr {
                    findings.push(format!(
                        "快环拥有慢环参照: {}（{oc}）owns_reference {}（{cc}）——参照的所有者必须比被治理者更慢",
                        e.from_id, e.to_id
                    ));
                }
            }
        }
    }
    findings
}

type NodeRow = std::result::Result<Option<GraphNode>, rusqlite::Error>;

fn row_to_node(r: &rusqlite::Row<'_>) -> NodeRow {
    let kind: String = r.get(2)?;
    let origin: String = r.get(6)?;
    let (Some(kind), Some(origin)) = (NodeKind::parse(&kind), Origin::parse(&origin)) else {
        return Ok(None); // unknown enum text: skip row rather than wedge the caller
    };
    Ok(Some(GraphNode {
        agent_id: r.get(0)?,
        id: r.get(1)?,
        kind,
        label: r.get(3)?,
        body: r.get(4)?,
        cadence: r.get(5)?,
        origin,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
    }))
}

type EdgeRow = std::result::Result<Option<GraphEdge>, rusqlite::Error>;

fn row_to_edge(r: &rusqlite::Row<'_>) -> EdgeRow {
    let kind: String = r.get(3)?;
    let origin: String = r.get(5)?;
    let (Some(kind), Some(origin)) = (EdgeKind::parse(&kind), Origin::parse(&origin)) else {
        return Ok(None);
    };
    Ok(Some(GraphEdge {
        agent_id: r.get(0)?,
        from_id: r.get(1)?,
        to_id: r.get(2)?,
        kind,
        note: r.get(4)?,
        origin,
        created_at_ms: r.get(6)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, LoopGraphStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = LoopGraphStore::open(&dir.path().join("g.db")).unwrap();
        (dir, s)
    }

    fn node(id: &str, kind: NodeKind, origin: Origin) -> GraphNode {
        GraphNode::new("main", id, kind, format!("label-{id}"), origin)
    }

    #[test]
    fn node_roundtrip_and_update_preserves_created_at() {
        let (_d, s) = store();
        let n = node("daemon:dreaming", NodeKind::Daemon, Origin::Llm).with_cadence("nightly");
        s.upsert_node(&n).unwrap();
        let got = s.get_node("main", "daemon:dreaming").unwrap().unwrap();
        assert_eq!(got.cadence.as_deref(), Some("nightly"));

        let updated = GraphNode {
            label: "记忆整理夜巡".into(),
            updated_at_ms: got.updated_at_ms + 10,
            ..got.clone()
        };
        s.upsert_node(&updated).unwrap();
        let got2 = s.get_node("main", "daemon:dreaming").unwrap().unwrap();
        assert_eq!(got2.label, "记忆整理夜巡");
        assert_eq!(got2.created_at_ms, got.created_at_ms);
    }

    #[test]
    fn root_requires_human_origin() {
        let (_d, s) = store();
        let bad = node("root:aleph", NodeKind::Root, Origin::Llm);
        assert!(
            s.upsert_node(&bad).is_err(),
            "llm-origin root must be rejected"
        );
        let good = node("root:aleph", NodeKind::Root, Origin::Human);
        s.upsert_node(&good).unwrap();
    }

    #[test]
    fn edge_requires_existing_endpoints() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        let e = GraphEdge::new(
            "main",
            "heartbeat:hb1",
            "daemon:dreaming",
            EdgeKind::Watches,
            Origin::Llm,
        );
        assert!(
            s.upsert_edge(&e).is_err(),
            "missing endpoint must be rejected"
        );
        s.upsert_node(&node("heartbeat:hb1", NodeKind::LoopHeartbeat, Origin::Llm))
            .unwrap();
        s.upsert_edge(&e).unwrap();
        assert_eq!(s.list_edges("main").unwrap().len(), 1);
    }

    #[test]
    fn deleting_node_leaves_dangling_edge_lint_flags_it_gc_removes_it() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        s.upsert_node(&node("heartbeat:hb1", NodeKind::LoopHeartbeat, Origin::Llm))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "heartbeat:hb1",
            "daemon:dreaming",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        assert!(s.delete_node("main", "heartbeat:hb1").unwrap());

        let lint = s.lint("main").unwrap();
        assert!(lint.iter().any(|f| f.contains("悬空边")), "lint: {lint:?}");

        let removed = s.gc("main").unwrap();
        assert_eq!(removed.len(), 1);
        assert!(s.list_edges("main").unwrap().is_empty());
    }

    #[test]
    fn lint_flags_naked_loop_and_clears_when_watched() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        let lint = s.lint("main").unwrap();
        assert!(lint.iter().any(|f| f.contains("裸奔优化环")));

        s.upsert_node(&node("cron:audit", NodeKind::LoopCron, Origin::Llm).with_cadence("weekly"))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:audit",
            "daemon:dreaming",
            EdgeKind::Audits,
            Origin::Llm,
        ))
        .unwrap();
        let lint = s.lint("main").unwrap();
        assert!(!lint
            .iter()
            .any(|f| f.contains("daemon:dreaming") && f.contains("裸奔")));
    }

    #[test]
    fn lint_flags_unanchored_governance_chain_and_fast_owns_slow() {
        let (_d, s) = store();
        s.upsert_node(&node("cron:gov", NodeKind::LoopCron, Origin::Llm).with_cadence("per_turn"))
            .unwrap();
        s.upsert_node(&node("goal:g1", NodeKind::LoopGoal, Origin::Llm).with_cadence("weekly"))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:gov",
            "goal:g1",
            EdgeKind::OwnsReference,
            Origin::Llm,
        ))
        .unwrap();
        let lint = s.lint("main").unwrap();
        assert!(
            lint.iter().any(|f| f.contains("治理链未锚定")),
            "lint: {lint:?}"
        );
        assert!(
            lint.iter().any(|f| f.contains("快环拥有慢环参照")),
            "lint: {lint:?}"
        );

        // Anchor the chain: a human root owns the governor.
        s.upsert_node(&node("root:aleph", NodeKind::Root, Origin::Human))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:aleph",
            "cron:gov",
            EdgeKind::OwnsReference,
            Origin::Human,
        ))
        .unwrap();
        let lint = s.lint("main").unwrap();
        assert!(
            !lint.iter().any(|f| f.contains("治理链未锚定")),
            "lint: {lint:?}"
        );
    }

    #[test]
    fn agent_scoping_isolates_graphs() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        assert!(s.list_nodes("other").unwrap().is_empty());
    }
}

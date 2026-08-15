//! Time-travel snapshots for the role graph.
//!
//! Why snapshots: per 《什么是图工程》/ GRAPH_LAYER §1, the role graph is **慢变、可审计**
//! — auditable means replayable. Today the graph is a moving target with no history:
//! an audit verdict written six weeks ago ("verdict=drift on daemon:dreaming,
//! evidence_…") references a topology the operator cannot recover without restoring
//! from backup. This module makes the history a first-class artefact.
//!
//! Design choices:
//! - **Separate SQLite file** (`loop_graph_snapshots.db`), NOT a table inside
//!   `loop_graph.db`: snapshots are an audit trail, not governance state, and
//!   the store's write gate (dreaming has no write path here) should not even
//!   parse them. Co-locating them would also widen the lock surface for every
//!   audit capture.
//! - **JSON blobs for nodes/edges** (not row-per-row): snapshots are written
//!   occasionally and read for human review / diff; the parse cost is invisible
//!   at human timescales and the simpler schema is easier to diff against a
//!   later snapshot.
//! - **No restore-from-snapshot to live store**: a snapshot is a record, not
//!   a rollback primitive. Restoring would silently re-introduce rows the
//!   operator deliberately deleted (e.g. an `owns_reference` edge whose owner
//!   is known-dead). If a future use case emerges, the audit template's
//!   "re-adopt orphan" pattern is the precedent — explicit, carded, traced.
//! - **Compact label** (caller-supplied, ≤ 200 chars) so audit log rows can
//!   reference a snapshot by name without a join.
//!
//! What this is NOT:
//! - Not a WAL. The store's mutations are durable by SQLite's own WAL; this
//!   is a higher-level "topology at moment T" record.
//! - Not a fork primitive. LangGraph's time-travel can branch the live graph
//!   into a parallel one for what-if replay. Role-graph YAGNI (GRAPH_LAYER
//!   §7 NOT-build #1, time-travel).

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::loop_graph::store::LoopGraphStore;
use crate::loop_graph::types::{EdgeKind, GraphEdge, GraphNode, NodeKind};
use crate::sync_primitives::{Mutex, MutexGuard};

/// Maximum length of a caller-supplied snapshot label. Picked to fit a one-line
/// commit-style message ("pre-pair cron:nightwatcher for daemon:dreaming").
const LABEL_MAX_CHARS: usize = 200;

/// Public summary — what `list_snapshots` returns. Compact, safe to ship to a
/// Panel that just wants to render a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub id: i64,
    pub label: String,
    pub taken_at_ms: i64,
    pub taken_at_iso: String,
    pub node_count: usize,
    pub edge_count: usize,
}

/// Public full record — what `get_snapshot` returns, with the parsed rows
/// restored to typed values (so callers do not have to round-trip through JSON
/// to do anything useful with the contents).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub summary: SnapshotSummary,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// What `diff_snapshots` returns — every node/edge row that changed between
/// `from` and `to`. Symmetric set semantics: a node present in `from` but
/// absent in `to` is `Removed`; absent in `from`, present in `to` is `Added`;
/// present in both but with different fields is `Modified` (with the field-
/// level diff in `changed_fields`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopologyDiff {
    Added {
        id: String,
        node_kind: NodeKind,
    },
    Removed {
        id: String,
        node_kind: NodeKind,
    },
    Modified {
        id: String,
        changed_fields: Vec<String>,
    },
    EdgeAdded {
        from_id: String,
        to_id: String,
        edge_kind: EdgeKind,
    },
    EdgeRemoved {
        from_id: String,
        to_id: String,
        edge_kind: EdgeKind,
    },
    EdgeModified {
        from_id: String,
        to_id: String,
        edge_kind: EdgeKind,
        changed_fields: Vec<String>,
    },
}

/// Storage for the snapshot history. Same pattern as `LoopGraphStore`: own
/// mutex-protected `rusqlite::Connection`, opened via the project helper.
pub struct SnapshotStore {
    conn: Mutex<rusqlite::Connection>,
}

impl SnapshotStore {
    /// Open (creating if needed) the snapshot DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AlephError::other(format!("snapshot store create_dir: {e}")))?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("snapshot store open: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS snapshots (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 label         TEXT NOT NULL,
                 taken_at_ms   INTEGER NOT NULL,
                 node_count    INTEGER NOT NULL,
                 edge_count    INTEGER NOT NULL,
                 nodes_json    TEXT NOT NULL,
                 edges_json    TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_snapshots_taken_at ON snapshots(taken_at_ms);",
        )
        .map_err(|e| AlephError::other(format!("snapshot store init: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, rusqlite::Connection> {
        // Same poison-handling as `LoopGraphStore::lock` — never propagate.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Capture the current state of `agent_id`'s graph under `label`.
    /// `label` is truncated to [`LABEL_MAX_CHARS`]; pass a short, meaningful
    /// name (the audit log will reference it).
    pub fn capture(&self, graph: &LoopGraphStore, agent_id: &str, label: &str) -> Result<i64> {
        let label = if label.chars().count() > LABEL_MAX_CHARS {
            // Hard cap, not `truncate_text` (which appends "..." and exceeds
            // the limit by the marker length): a label is a lookup key in the
            // audit log, not prose, so the cap must be exact.
            label.chars().take(LABEL_MAX_CHARS).collect()
        } else {
            label.to_string()
        };
        let nodes = graph.list_nodes(agent_id)?;
        let edges = graph.list_edges(agent_id)?;
        let nodes_json = serde_json::to_string(&nodes)
            .map_err(|e| AlephError::other(format!("snapshot serialize nodes: {e}")))?;
        let edges_json = serde_json::to_string(&edges)
            .map_err(|e| AlephError::other(format!("snapshot serialize edges: {e}")))?;
        let now_ms = Utc::now().timestamp_millis();
        self.lock()
            .execute(
                "INSERT INTO snapshots
                     (label, taken_at_ms, node_count, edge_count, nodes_json, edges_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    label,
                    now_ms,
                    nodes.len() as i64,
                    edges.len() as i64,
                    nodes_json,
                    edges_json,
                ],
            )
            .map_err(|e| AlephError::other(format!("snapshot insert: {e}")))?;
        // `execute` returns rows-affected (1), not the id — `get_snapshot` keyed
        // on that would read the FIRST row forever and every diff came out
        // empty. `last_insert_rowid` is the only correct answer.
        Ok(self.lock().last_insert_rowid())
    }

    /// List snapshots, newest first. No filtering; the typical operator query
    /// is "show me the last 10 audit checkpoints" and a Panel UI can paginate.
    pub fn list_snapshots(&self) -> Result<Vec<SnapshotSummary>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, label, taken_at_ms, node_count, edge_count
                 FROM snapshots ORDER BY id DESC",
            )
            .map_err(|e| AlephError::other(format!("snapshot list prepare: {e}")))?;
        let rows = stmt
            .query_map([], row_to_summary)
            .map_err(|e| AlephError::other(format!("snapshot list query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::other(format!("snapshot list row: {e}")))?);
        }
        Ok(out)
    }

    /// Fetch one full snapshot by id. Returns `Ok(None)` for an unknown id so
    /// callers can map the panel's "deleted by another tab" state without a
    /// separate error path.
    pub fn get_snapshot(&self, id: i64) -> Result<Option<Snapshot>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT id, label, taken_at_ms, node_count, edge_count,
                        nodes_json, edges_json
                 FROM snapshots WHERE id = ?1",
                rusqlite::params![id],
                row_to_full,
            )
            .optional()
            .map_err(|e| AlephError::other(format!("snapshot get: {e}")))?;
        let Some(row) = row else { return Ok(None) };
        let nodes: Vec<GraphNode> = serde_json::from_str(&row.nodes_json)
            .map_err(|e| AlephError::other(format!("snapshot parse nodes: {e}")))?;
        let edges: Vec<GraphEdge> = serde_json::from_str(&row.edges_json)
            .map_err(|e| AlephError::other(format!("snapshot parse edges: {e}")))?;
        Ok(Some(Snapshot {
            summary: SnapshotSummary {
                id: row.id,
                label: row.label,
                taken_at_ms: row.taken_at_ms,
                taken_at_iso: iso_from_ms(row.taken_at_ms),
                node_count: row.node_count as usize,
                edge_count: row.edge_count as usize,
            },
            nodes,
            edges,
        }))
    }

    /// Compute the diff between two snapshots. Symmetric: passing `(a, b)` and
    /// `(b, a)` swap `Added`↔`Removed`. Order is irrelevant for `Modified`.
    ///
    /// Implementation note — BTreeMap keyed by stable id: nodes are keyed by
    /// `(agent_id, id)`, edges by `(agent_id, from_id, to_id, kind)`. The agent
    /// half is constant for both sides (snapshots are always per-agent), so
    /// the maps use just the structural key.
    pub fn diff_snapshots(&self, from: i64, to: i64) -> Result<Vec<TopologyDiff>> {
        let a = self
            .get_snapshot(from)?
            .ok_or_else(|| AlephError::other(format!("snapshot {from} not found")))?;
        let b = self
            .get_snapshot(to)?
            .ok_or_else(|| AlephError::other(format!("snapshot {to} not found")))?;
        Ok(diff_inner(&a, &b))
    }

    /// Delete a snapshot by id. Returns whether anything was deleted.
    /// Snapshots are audit trails, NOT governance state — deletion is the
    /// operator's responsibility ("GDPR delete", "audit log retention cut").
    /// Deliberately NOT exposed through the `loop_graph` tool today; only a
    /// future admin RPC will reach it.
    pub fn delete_snapshot(&self, id: i64) -> Result<bool> {
        let n = self
            .lock()
            .execute("DELETE FROM snapshots WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| AlephError::other(format!("snapshot delete: {e}")))?;
        Ok(n > 0)
    }
}

fn diff_inner(a: &Snapshot, b: &Snapshot) -> Vec<TopologyDiff> {
    let an = map_nodes(a);
    let bn = map_nodes(b);
    let ae = map_edges(a);
    let be = map_edges(b);

    let mut out = Vec::new();

    // Nodes: added / removed / modified.
    for (id, nb) in &bn {
        match an.get(id) {
            None => out.push(TopologyDiff::Added {
                id: (**id).to_string(),
                node_kind: nb.kind,
            }),
            Some(na) => {
                let changed = node_diff_fields(na, nb);
                if !changed.is_empty() {
                    out.push(TopologyDiff::Modified {
                        id: (**id).to_string(),
                        changed_fields: changed,
                    });
                }
            }
        }
    }
    for (id, na) in &an {
        if !bn.contains_key(id) {
            out.push(TopologyDiff::Removed {
                id: (**id).to_string(),
                node_kind: na.kind,
            });
        }
    }

    // Edges: same shape.
    for (key, eb) in &be {
        let kind = EdgeKind::parse(key.2).unwrap_or(EdgeKind::Feeds);
        match ae.get(key) {
            None => out.push(TopologyDiff::EdgeAdded {
                from_id: key.0.to_string(),
                to_id: key.1.to_string(),
                edge_kind: kind,
            }),
            Some(ea) => {
                let changed = edge_diff_fields(ea, eb);
                if !changed.is_empty() {
                    out.push(TopologyDiff::EdgeModified {
                        from_id: key.0.to_string(),
                        to_id: key.1.to_string(),
                        edge_kind: kind,
                        changed_fields: changed,
                    });
                }
            }
        }
    }
    for key in ae.keys() {
        if !be.contains_key(key) {
            let kind = EdgeKind::parse(key.2).unwrap_or(EdgeKind::Feeds);
            out.push(TopologyDiff::EdgeRemoved {
                from_id: key.0.to_string(),
                to_id: key.1.to_string(),
                edge_kind: kind,
            });
        }
    }

    // Stable order: nodes first (by id), then edges (by from_id+to_id+kind).
    // Determinism is what makes a diff reproducible for the audit log.
    out.sort_by_key(diff_sort_key);
    out
}

fn map_nodes(s: &Snapshot) -> std::collections::BTreeMap<&str, &GraphNode> {
    s.nodes.iter().map(|n| (n.id.as_str(), n)).collect()
}

fn map_edges(s: &Snapshot) -> std::collections::BTreeMap<(&str, &str, &str), &GraphEdge> {
    s.edges
        .iter()
        .map(|e| ((e.from_id.as_str(), e.to_id.as_str(), e.kind.as_str()), e))
        .collect()
}

fn diff_sort_key(d: &TopologyDiff) -> (u8, String, String, String, String) {
    match d {
        TopologyDiff::Added { id, .. } => {
            (0, id.clone(), String::new(), String::new(), String::new())
        }
        TopologyDiff::Removed { id, .. } => {
            (1, id.clone(), String::new(), String::new(), String::new())
        }
        TopologyDiff::Modified { id, .. } => {
            (2, id.clone(), String::new(), String::new(), String::new())
        }
        TopologyDiff::EdgeAdded {
            from_id,
            to_id,
            edge_kind,
            ..
        } => (
            3,
            from_id.clone(),
            to_id.clone(),
            edge_kind.as_str().to_string(),
            String::new(),
        ),
        TopologyDiff::EdgeRemoved {
            from_id,
            to_id,
            edge_kind,
            ..
        } => (
            4,
            from_id.clone(),
            to_id.clone(),
            edge_kind.as_str().to_string(),
            String::new(),
        ),
        TopologyDiff::EdgeModified {
            from_id,
            to_id,
            edge_kind,
            ..
        } => (
            5,
            from_id.clone(),
            to_id.clone(),
            edge_kind.as_str().to_string(),
            String::new(),
        ),
    }
}

fn node_diff_fields(a: &GraphNode, b: &GraphNode) -> Vec<String> {
    let mut fields = Vec::new();
    if a.kind != b.kind {
        fields.push("kind".into());
    }
    if a.label != b.label {
        fields.push("label".into());
    }
    if a.body != b.body {
        fields.push("body".into());
    }
    if a.cadence != b.cadence {
        fields.push("cadence".into());
    }
    if a.origin != b.origin {
        fields.push("origin".into());
    }
    fields
}

fn edge_diff_fields(a: &GraphEdge, b: &GraphEdge) -> Vec<String> {
    let mut fields = Vec::new();
    if a.note != b.note {
        fields.push("note".into());
    }
    if a.origin != b.origin {
        fields.push("origin".into());
    }
    fields
}

fn iso_from_ms(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn row_to_summary(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotSummary> {
    Ok(SnapshotSummary {
        id: r.get(0)?,
        label: r.get(1)?,
        taken_at_ms: r.get(2)?,
        taken_at_iso: iso_from_ms(r.get(2)?),
        node_count: r.get::<_, i64>(3)? as usize,
        edge_count: r.get::<_, i64>(4)? as usize,
    })
}

struct SnapshotFullRow {
    id: i64,
    label: String,
    taken_at_ms: i64,
    node_count: i64,
    edge_count: i64,
    nodes_json: String,
    edges_json: String,
}

fn row_to_full(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotFullRow> {
    Ok(SnapshotFullRow {
        id: r.get(0)?,
        label: r.get(1)?,
        taken_at_ms: r.get(2)?,
        node_count: r.get(3)?,
        edge_count: r.get(4)?,
        nodes_json: r.get(5)?,
        edges_json: r.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_graph::types::Origin;

    fn store() -> (tempfile::TempDir, LoopGraphStore, SnapshotStore) {
        let dir = tempfile::tempdir().unwrap();
        let g = LoopGraphStore::open(&dir.path().join("g.db")).unwrap();
        let s = SnapshotStore::open(&dir.path().join("s.db")).unwrap();
        (dir, g, s)
    }

    fn goal(session: &str) -> GraphNode {
        GraphNode::new(
            "main",
            format!("goal:{session}"),
            NodeKind::LoopGoal,
            "g",
            Origin::Llm,
        )
    }
    fn daemon(name: &str) -> GraphNode {
        GraphNode::new(
            "main",
            format!("daemon:{name}"),
            NodeKind::Daemon,
            "d",
            Origin::Llm,
        )
    }

    #[test]
    fn capture_then_list_round_trips() {
        let (_d, g, s) = store();
        g.upsert_node(&goal("s1")).unwrap();
        g.upsert_node(&daemon("dreaming")).unwrap();
        g.upsert_edge(&GraphEdge::new(
            "main",
            "daemon:dreaming",
            "goal:s1",
            EdgeKind::OwnsReference,
            Origin::Llm,
        ))
        .unwrap();

        let id = s.capture(&g, "main", "pre-pair").unwrap();
        assert!(id > 0);

        let summaries = s.list_snapshots().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, id);
        assert_eq!(summaries[0].label, "pre-pair");
        assert_eq!(summaries[0].node_count, 2);
        assert_eq!(summaries[0].edge_count, 1);
    }

    #[test]
    fn get_snapshot_restores_typed_rows() {
        let (_d, g, s) = store();
        let n = goal("s1");
        g.upsert_node(&n).unwrap();
        let id = s.capture(&g, "main", "v1").unwrap();

        let full = s.get_snapshot(id).unwrap().expect("snapshot present");
        assert_eq!(full.summary.id, id);
        assert_eq!(full.nodes.len(), 1);
        assert_eq!(full.nodes[0].id, "goal:s1");
        assert_eq!(full.nodes[0].kind, NodeKind::LoopGoal);
        assert!(full.edges.is_empty());
    }

    #[test]
    fn get_snapshot_unknown_id_is_none_not_error() {
        let (_d, _g, s) = store();
        let none = s.get_snapshot(999_999).unwrap();
        assert!(none.is_none(), "unknown id must read as Ok(None)");
    }

    #[test]
    fn diff_detects_added_removed_modified_in_stable_order() {
        let (_d, g, s) = store();
        g.upsert_node(&goal("s1")).unwrap();
        g.upsert_node(&daemon("dreaming")).unwrap();
        let snap_a = s.capture(&g, "main", "a").unwrap();

        // Mutate: drop daemon, add a cron watcher, change goal's label.
        g.delete_node("main", "daemon:dreaming").unwrap();
        g.upsert_node(&GraphNode::new(
            "main",
            "cron:watcher",
            NodeKind::LoopCron,
            "watcher",
            Origin::Llm,
        ))
        .unwrap();
        g.upsert_edge(&GraphEdge::new(
            "main",
            "cron:watcher",
            "goal:s1",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        let updated_goal = GraphNode {
            label: "renamed goal".into(),
            updated_at_ms: goal("s1").updated_at_ms + 10,
            ..goal("s1")
        };
        g.upsert_node(&updated_goal).unwrap();

        let snap_b = s.capture(&g, "main", "b").unwrap();
        let diff = s.diff_snapshots(snap_a, snap_b).unwrap();

        // Five changes: goal label modified, daemon removed, cron added,
        // watches edge added. The daemon→goal owns_reference edge is NOT
        // `edge_removed`: `delete_node` deliberately leaves edges dangling
        // (audit signals until gc), so snap_b still holds the row.
        let kinds: Vec<&'static str> = diff
            .iter()
            .map(|d| match d {
                TopologyDiff::Added { .. } => "added",
                TopologyDiff::Removed { .. } => "removed",
                TopologyDiff::Modified { .. } => "modified",
                TopologyDiff::EdgeAdded { .. } => "edge_added",
                TopologyDiff::EdgeRemoved { .. } => "edge_removed",
                TopologyDiff::EdgeModified { .. } => "edge_modified",
            })
            .collect();
        assert!(kinds.contains(&"removed"), "{kinds:?}");
        assert!(kinds.contains(&"added"), "{kinds:?}");
        assert!(kinds.contains(&"modified"), "{kinds:?}");
        assert!(kinds.contains(&"edge_added"), "{kinds:?}");
        assert!(
            !kinds.contains(&"edge_removed"),
            "drop_node does not cascade — the dangling edge is still in the snapshot: {kinds:?}"
        );

        // The diff must be deterministic across calls — the audit log relies on it.
        let diff_again = s.diff_snapshots(snap_a, snap_b).unwrap();
        assert_eq!(diff, diff_again);
    }

    #[test]
    fn diff_is_symmetric_added_and_removed_swap() {
        let (_d, g, s) = store();
        g.upsert_node(&goal("s1")).unwrap();
        let snap_a = s.capture(&g, "main", "a").unwrap();

        g.delete_node("main", "goal:s1").unwrap();
        let snap_b = s.capture(&g, "main", "b").unwrap();

        let ab = s.diff_snapshots(snap_a, snap_b).unwrap();
        let ba = s.diff_snapshots(snap_b, snap_a).unwrap();
        // Forward: Removed. Backward: Added.
        assert!(matches!(ab[0], TopologyDiff::Removed { ref id, .. } if id == "goal:s1"));
        assert!(matches!(ba[0], TopologyDiff::Added { ref id, .. } if id == "goal:s1"));
    }

    #[test]
    fn snapshot_label_is_truncated_not_rejected() {
        let (_d, g, s) = store();
        let huge = "x".repeat(LABEL_MAX_CHARS + 50);
        let id = s.capture(&g, "main", &huge).unwrap();
        let snap = s.get_snapshot(id).unwrap().unwrap();
        assert!(
            snap.summary.label.chars().count() <= LABEL_MAX_CHARS,
            "label must be capped: {} chars",
            snap.summary.label.chars().count()
        );
    }

    #[test]
    fn delete_removes_a_snapshot() {
        let (_d, g, s) = store();
        g.upsert_node(&goal("s1")).unwrap();
        let id = s.capture(&g, "main", "transient").unwrap();
        assert_eq!(s.list_snapshots().unwrap().len(), 1);
        assert!(s.delete_snapshot(id).unwrap());
        assert!(s.list_snapshots().unwrap().is_empty());
        assert!(!s.delete_snapshot(id).unwrap(), "second delete is a no-op");
    }

    /// The contract the audit template relies on: `nodes_json` is a faithful
    /// round-trip of the live rows. A row the store dropped (here by enum-text
    /// spoofing) is not in the snapshot — that is acceptable, the snapshot
    /// captures the same fail-soft view every reader has.
    #[test]
    fn snapshot_inherits_store_fail_soft_view() {
        let (d, g, s) = store();
        g.upsert_node(&goal("s1")).unwrap();
        // Force an unknown kind in the live store. `LoopGraphStore::lock` is
        // private to store.rs, so write the spoof through a second connection.
        let conn = crate::utils::sqlite_open::open_sqlite_safe(&d.path().join("g.db"))
            .expect("open raw db");
        conn.execute(
            "UPDATE graph_nodes SET kind = 'loop_from_the_future' WHERE id = 'goal:s1'",
            [],
        )
        .unwrap();
        let id = s.capture(&g, "main", "drift").unwrap();
        let snap = s.get_snapshot(id).unwrap().unwrap();
        assert!(
            snap.nodes.is_empty(),
            "snapshot must mirror the store's fail-soft view: {:?}",
            snap.nodes
        );
    }
}

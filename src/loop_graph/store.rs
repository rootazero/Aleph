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

/// What one `gc` pass did. `retained_acl` is not an error and not a leftover:
/// dangling `owns_reference` rows are deliberately kept (see
/// [`LoopGraphStore::gc`]), and the tool has to say so — silently keeping a row
/// the caller asked to clear is the same class of lie as silently deleting one.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub removed: Vec<String>,
    pub retained_acl: Vec<String>,
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
        // Same project helper the writer uses, not a hand-rolled
        // `open_with_flags`: it sets `busy_timeout=5000`, without which the
        // doctor lint returns `SQLITE_BUSY` the instant it lands mid-write
        // (`enable_audit` wires N edges in a loop) and reports a healthy graph
        // as "Graph DB unreadable".
        let conn = crate::utils::sqlite_open::open_sqlite_readonly(path)
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
    ///
    /// `origin` is write-once, like `created_at_ms`: an upsert never rewrites
    /// it. It is the provenance the audit template is told to check, and the
    /// model supplies it verbatim (`args.origin.unwrap_or(Origin::Llm)`), so
    /// letting a later upsert flip a human-attested row to `llm` (or the
    /// reverse) erased the only record there was — `updated_at_ms` moves, but
    /// nothing says what changed. Changing provenance now requires an explicit
    /// delete + recreate, which is the documented escape hatch and does leave a
    /// trace. This is NOT a defence against a hostile model; that is the Auto-
    /// tier approval card's job for `root:`/`frozen:` ids.
    ///
    /// `body` and `cadence` are **omission-preserving** (`COALESCE`), and for
    /// the same reason `origin` is write-once. The only writer builds its row
    /// with `#[serde(default)] Option<String>` args, so "the model re-registered
    /// this node to fix the label" arrived here as `body = None` and a plain
    /// `SET body = excluded.body` wrote SQL NULL over it. On a `root:` node that
    /// silently erased **the human reference text itself** — the one thing in
    /// this store no machine is allowed to supply — and both readers render a
    /// root only `if let Some(body)`, so the line vanished from every governed
    /// session's prompt with nothing logged. The same stroke wiped `cadence`,
    /// the sole input to `lint_cadence_mismatch`, silently retiring that lint
    /// for the node. Clearing a field is still expressible: pass `""` (an empty
    /// string is not NULL, so `COALESCE` takes it).
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
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(agent_id, id) DO UPDATE SET
                     kind = excluded.kind,
                     label = excluded.label,
                     body = COALESCE(excluded.body, body),
                     cadence = COALESCE(excluded.cadence, cadence),
                     updated_at_ms = excluded.updated_at_ms",
                rusqlite::params![
                    node.agent_id,
                    node.id,
                    node.kind.as_str(),
                    node.label,
                    node.body,
                    node.cadence,
                    node.origin.as_str(),
                    node.created_at_ms,
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
    ///
    /// Structural invariant (the held-out-watcher rule): a governance edge may
    /// not point at its own source. `lint_naked_loops` asks only "does some
    /// `watches`/`audits` edge END here", so `x -[watches]-> x` satisfies it —
    /// one `link` call from the very optimizer this layer exists to watch, and
    /// the graph's ONLY self-certification detector goes silent while `lint`,
    /// `doctor core/loop-graph` and the weekly audit template all report a
    /// sound topology. The rule is enforced here rather than in the tool so
    /// every writer inherits it.
    pub fn upsert_edge(&self, edge: &GraphEdge) -> Result<()> {
        if edge.from_id == edge.to_id
            && matches!(
                edge.kind,
                EdgeKind::Watches | EdgeKind::Audits | EdgeKind::OwnsReference
            )
        {
            return Err(AlephError::other(format!(
                "loop_graph invariant: '{}' cannot {} itself — the watcher must be a \
                 held-out loop, or the check certifies nothing",
                edge.from_id,
                edge.kind.as_str()
            )));
        }
        let conn = self.lock();
        let mut from_kind: Option<String> = None;
        for (endpoint, is_source) in [(&edge.from_id, true), (&edge.to_id, false)] {
            use rusqlite::OptionalExtension;
            let kind: Option<String> = conn
                .query_row(
                    "SELECT kind FROM graph_nodes WHERE agent_id = ?1 AND id = ?2",
                    rusqlite::params![edge.agent_id, endpoint],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| AlephError::other(format!("loop_graph edge check: {e}")))?;
            let Some(kind) = kind else {
                return Err(AlephError::other(format!(
                    "loop_graph edge: node '{endpoint}' does not exist — register it first \
                     with action='node'"
                )));
            };
            if is_source {
                from_kind = Some(kind);
            }
        }
        if let Some(kind) = from_kind.as_deref().and_then(NodeKind::parse) {
            if let Some(reason) = coverage_source_rejection(kind, edge.kind) {
                return Err(AlephError::other(format!(
                    "loop_graph invariant: '{}' cannot {} '{}' — {reason}",
                    edge.from_id,
                    edge.kind.as_str(),
                    edge.to_id
                )));
            }
        }
        conn.execute(
            "INSERT INTO graph_edges
                 (agent_id, from_id, to_id, kind, note, origin, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(agent_id, from_id, to_id, kind) DO UPDATE SET
                 note = COALESCE(excluded.note, note)",
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

    /// Ids of the loops that own `to_id`'s reference, read from raw columns.
    ///
    /// Deliberately NOT `list_edges().filter(...)`: `row_to_edge` is fail-soft
    /// (an unknown `kind`/`origin` text yields `Ok(None)` so one bad row cannot
    /// wedge every reader), and `list_edges` therefore silently DROPS such a
    /// row. For a lint that is an acceptable blind spot; for an ACL it is a
    /// grant — a governed loop whose `owns_reference` row happens to be
    /// unreadable (a rolled-back build meeting a newer `Origin` variant; both
    /// enums are `#[non_exhaustive]`, so the vocabulary is expected to grow)
    /// would be answered "ungoverned" and allowed to rewrite its own objective.
    /// Existence questions read the columns that cannot fail to parse, exactly
    /// as [`node_ids_present`] does.
    pub fn owns_reference_sources(&self, agent_id: &str, to_id: &str) -> Result<Vec<String>> {
        self.edge_sources(agent_id, to_id, EdgeKind::OwnsReference, "owns_reference")
    }

    /// Ids of the loops watching `to_id`, read from raw columns.
    ///
    /// Same discipline as [`Self::owns_reference_sources`], and for a cost that
    /// is just as one-way: this feeds the victory-claim poke, whose caller
    /// treats an empty answer as "nothing to review, keep the one-shot claim".
    /// A dropped row therefore does not degrade the review, it CANCELS it, for
    /// a completion whose claim key can never change again.
    pub fn watches_sources(&self, agent_id: &str, to_id: &str) -> Result<Vec<String>> {
        self.edge_sources(agent_id, to_id, EdgeKind::Watches, "watches")
    }

    fn edge_sources(
        &self,
        agent_id: &str,
        to_id: &str,
        kind: EdgeKind,
        ctx: &str,
    ) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT from_id FROM graph_edges
                 WHERE agent_id = ?1 AND to_id = ?2 AND kind = ?3
                 ORDER BY from_id",
            )
            .map_err(|e| AlephError::other(format!("loop_graph {ctx} prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id, to_id, kind.as_str()], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| AlephError::other(format!("loop_graph {ctx} query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::other(format!("loop_graph {ctx}: {e}")))?);
        }
        Ok(out)
    }

    /// Ids of the nodes whose `body` is exactly `body`, read from raw columns.
    ///
    /// Same reasoning as [`node_ids_present`] / `owns_reference_sources`: this
    /// answers an EXISTENCE question (`enable_audit`'s "is an audit ring
    /// already installed"), and `list_nodes` fail-softs an unreadable row to
    /// nothing. An audit node this build cannot parse — a downgrade past a new
    /// `NodeKind`, which `#[non_exhaustive]` says to expect — would read as "no
    /// audit ring" and mint a second one, the exact outcome the guard exists to
    /// prevent. `id`/`body` are TEXT columns that cannot fail to decode.
    pub fn node_ids_with_body(&self, agent_id: &str, body: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id FROM graph_nodes
                 WHERE agent_id = ?1 AND body = ?2 ORDER BY id",
            )
            .map_err(|e| AlephError::other(format!("loop_graph body scan prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id, body], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::other(format!("loop_graph body scan query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::other(format!("loop_graph body scan: {e}")))?);
        }
        Ok(out)
    }

    /// Remove dangling edges (endpoint node gone). Explicit only — never
    /// automatic.
    ///
    /// "Gone" is decided by [`node_ids_present`] — raw `id` text, not a parsed
    /// row. `row_to_node` fail-softs an unknown `kind`/`origin` to `Ok(None)`
    /// so one odd row cannot wedge a reader, but feeding that skip into a
    /// DELETE predicate turns "I could not read this node" into "this node
    /// does not exist" and irreversibly deletes every edge touching it. Both
    /// enums are `#[non_exhaustive]`, i.e. the vocabulary is expected to grow,
    /// so a downgrade after a new kind ships is the realistic trigger.
    ///
    /// **`owns_reference` is never collected.** It is not decoration: it is the
    /// row `governing_owner` reads to refuse a governed loop's rewrite of its
    /// own objective. Deleting the governor node leaves that edge dangling but
    /// still ENFORCING — which is correct, "my governor vanished" must not be a
    /// self-service unlock. A gc that swept it turned `drop_node` + `gc`, two
    /// calls neither of which names a protected id, into the second door out of
    /// §6.2 write protection, right next to the `unlink` door the Auto-tier card
    /// now covers. The one way out stays the carded `unlink`.
    pub fn gc(&self, agent_id: &str) -> Result<GcReport> {
        let mut conn = self.lock();
        let ids = node_ids_present(&conn, agent_id, "gc")?;

        let mut stmt = conn
            .prepare(
                "SELECT agent_id, from_id, to_id, kind, note, origin, created_at_ms
                 FROM graph_edges WHERE agent_id = ?1",
            )
            .map_err(|e| AlephError::other(format!("loop_graph gc edges prepare: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id], row_to_edge)
            .map_err(|e| AlephError::other(format!("loop_graph gc edges query: {e}")))?;
        let mut edges = Vec::new();
        for r in rows {
            // Mid-iteration decode errors must propagate here (not be
            // silently dropped): a `row_to_edge` that fails to parse one row
            // would otherwise leave `gc` deleting edges against an INCOMPLETE
            // `node_ids_present` picture — the exact reverse of the "do not
            // delete edges into a node I merely failed to parse" rule the
            // rest of this function enforces.
            let row = r.map_err(|e| AlephError::other(format!("loop_graph gc row: {e}")))?;
            if let Some(e) = row {
                edges.push(e);
            }
        }
        drop(stmt);

        // Build the report BEFORE the transaction so a failed delete still
        // returns a structured "what would have been removed" instead of
        // either an empty report or a half-truth.
        let mut report = GcReport::default();
        let mut to_delete: Vec<&GraphEdge> = Vec::new();
        for e in &edges {
            if ids.contains(&e.from_id) && ids.contains(&e.to_id) {
                continue;
            }
            let described = format!("{} -[{}]-> {}", e.from_id, e.kind.as_str(), e.to_id);
            if e.kind == EdgeKind::OwnsReference {
                report.retained_acl.push(described);
                continue;
            }
            to_delete.push(e);
            report.removed.push(described);
        }

        // Atomic delete: `gc` converges the graph, so a partial delete that
        // leaves some dangling rows and some deleted rows would mismatch the
        // report (and the next `lint` against the same store). One
        // transaction: either every deletable edge is gone and the report
        // matches, or nothing was deleted and the caller still has the
        // pre-gc graph.
        let tx = conn
            .transaction()
            .map_err(|e| AlephError::other(format!("loop_graph gc begin: {e}")))?;
        for e in &to_delete {
            tx.execute(
                "DELETE FROM graph_edges
                     WHERE agent_id = ?1 AND from_id = ?2 AND to_id = ?3 AND kind = ?4",
                rusqlite::params![agent_id, e.from_id, e.to_id, e.kind.as_str()],
            )
            .map_err(|err| AlephError::other(format!("loop_graph gc: {err}")))?;
        }
        tx.commit()
            .map_err(|e| AlephError::other(format!("loop_graph gc commit: {e}")))?;
        Ok(report)
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
        // Read nodes, edges, and presence into local Vec/HashSet, then drop
        // the lock BEFORE running the five pure-CPU lint passes. Holding the
        // store mutex through every pass blocks every concurrent get_node /
        // upsert_node / list_edges for the full duration of lint, which is
        // unjustified: lint is read-only and computes only on the snapshot
        // it just read.
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
            // Mid-iteration decode errors are best-effort skipped with a warn:
            // lint is a snapshot diagnostic, not a destructive op, so a few
            // unreadable rows do not justify failing the whole pass. They
            // were previously silently dropped, which obscured real DB
            // problems from operators.
            let row = r.map_err(|e| AlephError::other(format!("loop_graph lint node: {e}")))?;
            if let Some(n) = row {
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
            let row = r.map_err(|e| AlephError::other(format!("loop_graph lint edge: {e}")))?;
            if let Some(e) = row {
                edges.push(e);
            }
        }
        drop(stmt);

        // Existence is asked of the raw id column, not of `by_id`: a row that
        // is present but unparseable must not be reported as "节点已消失"
        // (same reasoning as `gc`). The parsed map still answers every
        // question that needs the node's fields.
        let present = node_ids_present(&conn, agent_id, "lint")?;
        // Lock released here — the rest of this function is pure compute on
        // local data.
        drop(conn);

        let by_id: std::collections::HashMap<&str, &GraphNode> =
            nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let mut findings = lint_dangling_edges(&edges, &present);
        findings.extend(lint_naked_loops(&nodes, &edges, &by_id));
        findings.extend(lint_forged_coverage(&nodes, &edges, &by_id));
        findings.extend(lint_governance_chain(&nodes, &edges, &by_id));
        findings.extend(lint_cadence_mismatch(&edges, &by_id));
        Ok(findings)
    }
}

/// May a node of `source` kind be the SOURCE of a `watches` / `audits` edge?
/// `Some(reason)` rejects.
///
/// Round 9 made the coverage exemption unforgeable at length 1 (no self-edge),
/// round 10 at length 2 (`lint_forged_coverage` walks the ring). Both closed
/// holes in the *shape* of the coverage graph while leaving its *vocabulary*
/// open, and the third instance of the same failure is cheaper than either: an
/// `anchor:` node costs one un-carded `node` call (the approval card matches
/// only `root:`/`frozen:` ids), and one un-carded `link anchor:x -[watches]->
/// daemon:dreaming` satisfies `lint_naked_loops` AND grounds
/// `lint_forged_coverage` — the graph's whole self-certification detector goes
/// quiet, permanently, for two calls that raise nothing.
///
/// The rule is what "watching" means: a watcher has to RUN. `Anchor` is a
/// measurement declaration and `Frozen` is a rule enforced somewhere else —
/// neither has a session, a schedule or a turn, so neither can review anything.
/// `Root` stays allowed and means what GRAPH_LAYER.md §4.4 documents it to
/// mean: a person reads the digest — the third and topmost layer of the
/// 三层封顶, supplied from outside the graph by a human-origin node the store
/// itself attests.
/// Can a node of this kind cover another one (be a `watches`/`audits` source)?
/// The positive form of [`coverage_source_rejection`], for readers that filter
/// legacy rows rather than reject writes. One vocabulary, two directions.
fn can_cover(kind: NodeKind) -> bool {
    kind.is_optimization_loop() || kind == NodeKind::Root
}

fn coverage_source_rejection(source: NodeKind, edge: EdgeKind) -> Option<&'static str> {
    if !matches!(edge, EdgeKind::Watches | EdgeKind::Audits) {
        return None;
    }
    match source {
        NodeKind::Anchor => Some(
            "anchor 节点是一次测量声明，没有会话也没有节奏，跑不起来 —— \
             它只能作为 anchored_by 的目标。要看守就接一个能跑的环（cron:/goal:/daemon:/…），\
             或由人接 root: 一条（三层封顶的第三层）",
        ),
        NodeKind::Frozen => Some(
            "frozen 节点登记的是别处执法的规则，它自己不运行，复核不了任何东西 —— \
             正确形状是反过来：让一个能跑的环 audits 这条冻结规则",
        ),
        _ => None,
    }
}

/// Ids of every node row that EXISTS, read straight from the `id` column.
///
/// Deliberately does not go through `row_to_node`: presence is a question
/// about the row, not about whether this build understands its enum text.
fn node_ids_present(
    conn: &rusqlite::Connection,
    agent_id: &str,
    ctx: &str,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn
        .prepare("SELECT id FROM graph_nodes WHERE agent_id = ?1")
        .map_err(|e| AlephError::other(format!("loop_graph {ctx} node ids prepare: {e}")))?;
    let rows = stmt
        .query_map(rusqlite::params![agent_id], |r| r.get::<_, String>(0))
        .map_err(|e| AlephError::other(format!("loop_graph {ctx} node ids query: {e}")))?;
    let mut out = std::collections::HashSet::new();
    for r in rows {
        out.insert(r.map_err(|e| AlephError::other(format!("loop_graph {ctx} node id: {e}")))?);
    }
    Ok(out)
}

fn lint_dangling_edges(
    edges: &[GraphEdge],
    present: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut findings = Vec::new();
    for e in edges {
        let missing: Vec<&str> = [e.from_id.as_str(), e.to_id.as_str()]
            .into_iter()
            .filter(|id| !present.contains(*id))
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

/// Flag optimization loops nobody watches.
///
/// A self `watches`/`audits` edge does NOT count as being watched. New ones
/// are rejected by `upsert_edge`; discounting them here keeps any legacy row
/// from silencing the check. Same for a coverage edge whose SOURCE cannot run
/// (`anchor:`/`frozen:` — see [`coverage_source_rejection`]): the write path
/// now refuses those, and this reader discounts the ones already on disk, so
/// closing the hole does not depend on a migration.
///
/// Deliberately NOT exempted: a node that merely HAS an outgoing `audits`
/// edge. That would make the exemption edge-grantable — `daemon:dreaming` could
/// audit anything and permanently excuse itself from the one check that says
/// nobody is watching it, which is the same hole as the self-edge above. The
/// only exemption is a watcher that can actually run, or a `Root` (a human),
/// neither of which an edge write can forge. The audit loop `enable_audit`
/// installs therefore does name itself until a human grounds it
/// (`root:<slug> -[watches]-> cron:<audit>`), which is the documented 三层封顶
/// and clears the finding with no code.
fn lint_naked_loops(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    by_id: &std::collections::HashMap<&str, &GraphNode>,
) -> Vec<String> {
    let mut findings = Vec::new();
    for n in nodes {
        if !n.kind.is_optimization_loop() {
            continue;
        }
        let watched = edges.iter().any(|e| {
            e.to_id == n.id
                && e.from_id != n.id
                && matches!(e.kind, EdgeKind::Watches | EdgeKind::Audits)
                && by_id
                    .get(e.from_id.as_str())
                    .is_some_and(|s| can_cover(s.kind))
        });
        if !watched {
            findings.push(format!(
                "裸奔优化环: {}（'{}'）没有任何 watches/audits 入边",
                n.id, n.label
            ));
        }
    }
    findings
}

/// Is any loop's coverage self-granted — a `watches`/`audits` ring with no
/// unforgeable node above it?
///
/// `lint_naked_loops` asks "does an edge point at me", and round 9 made that
/// unforgeable at length 1 (`from_id != n.id`, enforced at write time too).
/// Length 2 is still free: two `link` calls make an optimizer and its own
/// auditor watch each other, and both go silent — the optimizer granted itself
/// the exemption, which is precisely the measurement-decay failure this layer
/// exists to detect.
///
/// The predicate is deliberately narrow: a ring is only reported when NOTHING
/// above it is unforgeable. `Root` is the one kind a loop cannot award itself
/// (root writes are human-origin by store invariant and gated at the tool
/// boundary), so a ring with a root upstream is genuinely covered — that is the
/// documented 三层封顶 (优化环 → 审计环 → 人). A merely *un-anchored* linear
/// chain is not reported here — its top element already shows up as a naked
/// loop, and repeating the same root cause once per node below it would drown
/// the real finding.
///
/// Round 11 note: `Frozen`/`Anchor` used to count as grounding here too, which
/// was the hole — they cost one un-carded `node` call, so the "unforgeable"
/// exemption was one `link` away for any optimizer. Now they cannot be coverage
/// SOURCES at all ([`coverage_source_rejection`]), and the upward walk only
/// ever visits sources, so `Root` is the whole grounding set by construction.
fn lint_forged_coverage(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    by_id: &std::collections::HashMap<&str, &GraphNode>,
) -> Vec<String> {
    // Legacy rows with a non-runnable source are discounted here exactly as in
    // `lint_naked_loops`, so both checks agree on what "covered" means.
    let is_coverage = |e: &GraphEdge| {
        matches!(e.kind, EdgeKind::Watches | EdgeKind::Audits)
            && by_id
                .get(e.from_id.as_str())
                .is_some_and(|s| can_cover(s.kind))
    };
    let mut findings = Vec::new();
    for n in nodes {
        if !n.kind.is_optimization_loop() {
            continue;
        }
        // Walk upward through everything that claims to cover `n`.
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut frontier: Vec<&str> = edges
            .iter()
            .filter(|e| is_coverage(e) && e.to_id == n.id)
            .map(|e| e.from_id.as_str())
            .collect();
        if frontier.is_empty() {
            continue; // uncovered — that is `lint_naked_loops`' finding, not ours
        }
        let mut grounded = false;
        let mut returns_to_self = false;
        while let Some(current) = frontier.pop() {
            if current == n.id {
                returns_to_self = true;
            }
            if !visited.insert(current) {
                continue;
            }
            if by_id.get(current).is_some_and(|c| c.kind == NodeKind::Root) {
                grounded = true;
                break;
            }
            frontier.extend(
                edges
                    .iter()
                    .filter(|e| is_coverage(e) && e.to_id == current)
                    .map(|e| e.from_id.as_str()),
            );
        }
        if returns_to_self && !grounded {
            findings.push(format!(
                "伪造的看守覆盖: {}（'{}'）的 watches/audits 覆盖绕回它自己，\
                 且这个环之上没有任何 root —— 等于它自己给自己发了豁免。\
                 请由人接一条 root:… -[watches]-> 该环",
                n.id, n.label
            ));
        }
    }
    findings
}

/// Does every governor's `owns_reference` chain reach a human root?
///
/// The invariant GRAPH_LAYER.md states is "SOME upward path terminates at a
/// root", so the walk explores ALL incoming `owns_reference` edges, not just
/// the first one `list_edges` happens to order first. With two owners — say
/// `root:aleph` and `cron:orphan` both owning `cron:gov` — the old single-path
/// walk deterministically picked the lexicographically smaller `from_id`, hit
/// the dead end, and reported a correctly-anchored graph as unanchored: a
/// permanent doctor Warning the auditor cannot reconcile against a graph that
/// plainly shows the root edge. The `visited` set also makes cycle detection
/// exact, replacing the `steps < nodes.len()` bound.
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
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut frontier = vec![g.id.as_str()];
        let mut terminated_at_root = false;
        while let Some(current) = frontier.pop() {
            if !visited.insert(current) {
                continue;
            }
            if by_id.get(current).is_some_and(|n| n.kind == NodeKind::Root) {
                terminated_at_root = true;
                break;
            }
            frontier.extend(
                edges
                    .iter()
                    .filter(|e| e.to_id == current && e.kind == EdgeKind::OwnsReference)
                    .map(|e| e.from_id.as_str()),
            );
        }
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

    /// Round 9 made the naked-loop exemption unforgeable at length 1
    /// (`x -[watches]-> x` is refused at write time). Length 2 was still free:
    /// two `link` calls make an optimizer and its auditor watch each other and
    /// both go quiet. The exemption must come from something a loop cannot
    /// award itself.
    /// Round 11's hole, and the third instance of the same shape rounds 9 and
    /// 10 closed: the exemption was unforgeable by node KIND, but nothing said
    /// which kinds may SOURCE a coverage edge. An `anchor:` node costs one
    /// un-carded `node` call (the approval card matches only `root:`/`frozen:`),
    /// so two free calls used to silence `lint_naked_loops` AND ground
    /// `lint_forged_coverage` for any loop, permanently.
    #[test]
    fn a_thing_that_cannot_run_cannot_be_a_watcher() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        let mut anchor = node("anchor:tests", NodeKind::Anchor, Origin::Llm);
        anchor.body = Some("probe=cargo test, truth=exit_code".into());
        s.upsert_node(&anchor).unwrap();
        let mut frozen = node("frozen:budget", NodeKind::Frozen, Origin::Llm);
        frozen.body = Some("budget.rs ratchet".into());
        s.upsert_node(&frozen).unwrap();

        for (from, kind) in [
            ("anchor:tests", EdgeKind::Watches),
            ("anchor:tests", EdgeKind::Audits),
            ("frozen:budget", EdgeKind::Watches),
            ("frozen:budget", EdgeKind::Audits),
        ] {
            let err = s
                .upsert_edge(&GraphEdge::new(
                    "main",
                    from,
                    "daemon:dreaming",
                    kind,
                    Origin::Llm,
                ))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("loop_graph invariant"),
                "{from} {kind:?}: {err}"
            );
        }
        // The shapes that DO make sense still work: a loop audits a frozen rule,
        // a loop anchors itself, and a human root watches a loop.
        s.upsert_edge(&GraphEdge::new(
            "main",
            "daemon:dreaming",
            "frozen:budget",
            EdgeKind::Audits,
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "daemon:dreaming",
            "anchor:tests",
            EdgeKind::AnchoredBy,
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&node("root:aleph", NodeKind::Root, Origin::Human))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:aleph",
            "daemon:dreaming",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        assert!(
            !s.lint("main")
                .unwrap()
                .iter()
                .any(|f| f.contains("裸奔优化环")),
            "a human root IS a valid watcher — that is the documented 三层封顶"
        );
    }

    /// The write guard above only protects rows written from now on. A row
    /// already on disk must not silence the lint either — same belt-and-braces
    /// as the self-edge rule.
    #[test]
    fn a_legacy_anchor_sourced_watch_row_does_not_count_as_coverage() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        let mut anchor = node("anchor:tests", NodeKind::Anchor, Origin::Llm);
        anchor.body = Some("probe=cargo test, truth=exit_code".into());
        s.upsert_node(&anchor).unwrap();
        // Written the way a build without the invariant would have.
        s.lock()
            .execute(
                "INSERT INTO graph_edges (agent_id, from_id, to_id, kind, note, origin, created_at_ms)
                 VALUES ('main', 'anchor:tests', 'daemon:dreaming', 'watches', NULL, 'llm', 0)",
                [],
            )
            .unwrap();
        let findings = s.lint("main").unwrap();
        assert!(
            findings.iter().any(|f| f.contains("裸奔优化环")),
            "a stored anchor-sourced watch row must not read as 'watched': {findings:?}"
        );
    }

    /// `owns_reference` is the row the objective ACL reads. Sweeping it because
    /// its source node vanished turns `drop_node` + `gc` — two calls neither of
    /// which names a protected id — into a self-service unlock of §6.2 write
    /// protection, right beside the `unlink` door the approval card covers.
    #[test]
    fn gc_never_collects_the_objective_acl() {
        let (_d, s) = store();
        s.upsert_node(&node("goal:sess-1", NodeKind::LoopGoal, Origin::Llm))
            .unwrap();
        s.upsert_node(&node("cron:steward", NodeKind::LoopCron, Origin::Llm))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:steward",
            "goal:sess-1",
            EdgeKind::OwnsReference,
            Origin::Llm,
        ))
        .unwrap();
        assert!(s.delete_node("main", "cron:steward").unwrap());

        let report = s.gc("main").unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.retained_acl.len(), 1, "{report:?}");
        assert_eq!(
            s.owns_reference_sources("main", "goal:sess-1").unwrap(),
            vec!["cron:steward".to_string()],
            "the ACL must still answer after gc — the governed loop stays governed"
        );
    }

    /// `enable_audit`'s idempotency reads this, and it must answer from raw
    /// columns: an audit node this build cannot parse reading as "no audit
    /// ring" mints a second one.
    #[test]
    fn body_scan_sees_a_row_list_nodes_would_drop() {
        let (_d, s) = store();
        let mut n = node("cron:auditor", NodeKind::LoopCron, Origin::Llm);
        n.body = Some(crate::loop_graph::AUDIT_NODE_BODY.to_string());
        s.upsert_node(&n).unwrap();
        s.lock()
            .execute(
                "UPDATE graph_nodes SET kind = 'loop_from_the_future' WHERE id = 'cron:auditor'",
                [],
            )
            .unwrap();
        assert!(
            s.list_nodes("main").unwrap().is_empty(),
            "list_nodes is fail-soft — that is what makes it the wrong reader here"
        );
        assert_eq!(
            s.node_ids_with_body("main", crate::loop_graph::AUDIT_NODE_BODY)
                .unwrap(),
            vec!["cron:auditor".to_string()]
        );
    }

    #[test]
    fn mutual_watching_between_two_loops_is_not_a_valid_exemption() {
        let (_d, store) = store();
        let a = GraphNode::new(
            "main",
            "cron:aud",
            NodeKind::LoopCron,
            "auditor",
            Origin::Llm,
        );
        let b = GraphNode::new(
            "main",
            "daemon:dreaming",
            NodeKind::Daemon,
            "optimiser",
            Origin::Llm,
        );
        store.upsert_node(&a).unwrap();
        store.upsert_node(&b).unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                "main",
                "cron:aud",
                "daemon:dreaming",
                EdgeKind::Audits,
                Origin::Llm,
            ))
            .unwrap();
        // The forgery: the watched loop now "watches" its own watcher.
        store
            .upsert_edge(&GraphEdge::new(
                "main",
                "daemon:dreaming",
                "cron:aud",
                EdgeKind::Watches,
                Origin::Llm,
            ))
            .unwrap();

        let findings = store.lint("main").unwrap();
        assert!(
            findings.iter().any(|f| f.contains("伪造的看守覆盖")),
            "a two-node watch ring with nothing unforgeable above it must be reported: {findings:?}"
        );

        // A human root above the ring is a real exemption, and silences it.
        store
            .upsert_node(&GraphNode::new(
                "main",
                "root:aleph",
                NodeKind::Root,
                "human reference",
                Origin::Human,
            ))
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                "main",
                "root:aleph",
                "cron:aud",
                EdgeKind::Watches,
                Origin::Human,
            ))
            .unwrap();
        let findings = store.lint("main").unwrap();
        assert!(
            !findings.iter().any(|f| f.contains("伪造的看守覆盖")),
            "a root above the ring is an unforgeable exemption: {findings:?}"
        );
    }

    /// Round 9 gave `upsert_node` COALESCE semantics for `body`/`cadence` so a
    /// re-registration cannot null out human prose. Edges carry the same kind of
    /// prose in `note` and were left on the old全量-overwrite path.
    #[test]
    fn relinking_without_a_note_keeps_the_existing_rationale() {
        let (_d, store) = store();
        for id in ["cron:counter", "daemon:dreaming"] {
            store
                .upsert_node(&GraphNode::new(
                    "main",
                    id,
                    NodeKind::LoopCron,
                    id,
                    Origin::Human,
                ))
                .unwrap();
        }
        store
            .upsert_edge(
                &GraphEdge::new(
                    "main",
                    "cron:counter",
                    "daemon:dreaming",
                    EdgeKind::Watches,
                    Origin::Human,
                )
                .with_note("用户纠正率是唯一反指标，勿改"),
            )
            .unwrap();
        // Re-link without a note (confirming the pairing, a re-registration sweep…)
        store
            .upsert_edge(&GraphEdge::new(
                "main",
                "cron:counter",
                "daemon:dreaming",
                EdgeKind::Watches,
                Origin::Human,
            ))
            .unwrap();
        let edges = store.list_edges("main").unwrap();
        let e = edges.iter().find(|e| e.to_id == "daemon:dreaming").unwrap();
        assert_eq!(
            e.note.as_deref(),
            Some("用户纠正率是唯一反指标，勿改"),
            "omitting `note` on a re-link must keep the rationale, not NULL it"
        );
    }

    /// The objective ACL must not read "I could not decode this row" as
    /// "ungoverned" — that is a grant. `list_edges` is fail-soft by design, so
    /// the ACL reads raw columns instead.
    #[test]
    fn owns_reference_survives_a_row_list_edges_cannot_decode() {
        let (_d, store) = store();
        store
            .upsert_node(&GraphNode::new(
                "main",
                "cron:steward",
                NodeKind::LoopCron,
                "steward",
                Origin::Human,
            ))
            .unwrap();
        store
            .upsert_node(&GraphNode::new(
                "main",
                "goal:sess-1",
                NodeKind::LoopGoal,
                "governed goal",
                Origin::Human,
            ))
            .unwrap();
        {
            let conn = store.lock();
            conn.execute(
                "INSERT INTO graph_edges (agent_id, from_id, to_id, kind, note, origin, created_at_ms)
                 VALUES (?1, 'cron:steward', 'goal:sess-1', 'owns_reference', NULL, 'from_the_future', 1)",
                rusqlite::params!["main"],
            )
            .unwrap();
        }
        assert!(
            store.list_edges("main").unwrap().is_empty(),
            "precondition: the fail-soft reader drops this row"
        );
        assert_eq!(
            store
                .owns_reference_sources("main", "goal:sess-1")
                .unwrap()
                .as_slice(),
            ["cron:steward".to_string()],
            "the ACL must still see the governance edge"
        );
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
    fn governance_self_edge_is_rejected_and_never_counts_as_watched() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        assert!(
            s.lint("main")
                .unwrap()
                .iter()
                .any(|f| f.contains("裸奔优化环")),
            "baseline: an unwatched optimizer must be flagged"
        );

        // The self-certification move: watch/audit/own-reference yourself.
        for kind in [EdgeKind::Watches, EdgeKind::Audits, EdgeKind::OwnsReference] {
            let err = s
                .upsert_edge(&GraphEdge::new(
                    "main",
                    "daemon:dreaming",
                    "daemon:dreaming",
                    kind,
                    Origin::Llm,
                ))
                .unwrap_err()
                .to_string();
            assert!(err.contains("held-out"), "{kind:?}: {err}");
        }
        assert!(
            s.lint("main")
                .unwrap()
                .iter()
                .any(|f| f.contains("裸奔优化环")),
            "the naked-loop finding must survive every self-edge attempt"
        );

        // Belt and braces for any legacy row that predates the store guard:
        // the lint predicate itself discounts self-edges.
        let self_edge = GraphEdge::new(
            "main",
            "daemon:dreaming",
            "daemon:dreaming",
            EdgeKind::Watches,
            Origin::Llm,
        );
        let dreaming = node("daemon:dreaming", NodeKind::Daemon, Origin::Llm);
        let nodes = [dreaming];
        let by_id: std::collections::HashMap<&str, &GraphNode> =
            nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert!(
            lint_naked_loops(&nodes, &[self_edge], &by_id)
                .iter()
                .any(|f| f.contains("裸奔优化环")),
            "a stored self-edge must not read as 'watched'"
        );
    }

    #[test]
    fn auditing_something_does_not_exempt_you_from_being_watched() {
        // Exemption must never be edge-grantable: an optimizer that adds one
        // `audits` edge would otherwise excuse itself from the only check that
        // says nobody is watching it. Grounding by a human root DOES clear it.
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        s.upsert_node(&node("cron:audit", NodeKind::LoopCron, Origin::Llm))
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
        assert!(
            lint.iter().any(|f| f.contains("cron:audit")),
            "an ungrounded auditor is still a naked loop: {lint:?}"
        );
        assert!(
            !lint.iter().any(|f| f.contains("daemon:dreaming")),
            "the audited loop IS covered: {lint:?}"
        );

        s.upsert_node(&node("root:aleph", NodeKind::Root, Origin::Human))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:aleph",
            "cron:audit",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        assert!(
            !s.lint("main")
                .unwrap()
                .iter()
                .any(|f| f.contains("裸奔优化环")),
            "human grounding is the documented resolution and needs no code"
        );
    }

    #[test]
    fn governance_chain_is_anchored_if_any_upward_path_reaches_a_root() {
        // Two owners, only one of which reaches the root. The old single-path
        // walk took whichever `from_id` sorted first and reported a correctly
        // anchored graph as unanchored — a permanent doctor Warning the audit
        // loop could not reconcile against a graph plainly showing the edge.
        let (_d, s) = store();
        for (id, kind, origin) in [
            ("root:aleph", NodeKind::Root, Origin::Human),
            ("cron:gov", NodeKind::LoopCron, Origin::Llm),
            ("cron:orphan", NodeKind::LoopCron, Origin::Llm),
            ("goal:g1", NodeKind::LoopGoal, Origin::Llm),
        ] {
            s.upsert_node(&node(id, kind, origin)).unwrap();
        }
        for (from, to) in [
            ("root:aleph", "cron:gov"),
            ("cron:orphan", "cron:gov"),
            ("cron:gov", "goal:g1"),
        ] {
            s.upsert_edge(&GraphEdge::new(
                "main",
                from,
                to,
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        }

        let lint = s.lint("main").unwrap();
        assert!(
            !lint
                .iter()
                .any(|f| f.contains("治理链未锚定") && f.contains("cron:gov")),
            "cron:gov IS owned by root:aleph via the second in-edge: {lint:?}"
        );
        assert!(
            lint.iter()
                .any(|f| f.contains("治理链未锚定") && f.contains("cron:orphan")),
            "cron:orphan really is unanchored and must still be named: {lint:?}"
        );

        // A cycle with no root still terminates and IS reported.
        let (_d2, s2) = store();
        for id in ["cron:a", "cron:b"] {
            s2.upsert_node(&node(id, NodeKind::LoopCron, Origin::Llm))
                .unwrap();
        }
        for (from, to) in [("cron:a", "cron:b"), ("cron:b", "cron:a")] {
            s2.upsert_edge(&GraphEdge::new(
                "main",
                from,
                to,
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        }
        assert!(
            s2.lint("main")
                .unwrap()
                .iter()
                .any(|f| f.contains("治理链未锚定")),
            "a cycle reaches no root"
        );
    }

    #[test]
    fn provenance_is_write_once() {
        // `origin` is what the audit template is told to check, and the model
        // supplies it verbatim — an in-place rewrite would erase the only
        // record there was.
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Human))
            .unwrap();
        s.upsert_node(
            &node("daemon:dreaming", NodeKind::Daemon, Origin::Llm).with_body("re-registered"),
        )
        .unwrap();
        let got = s.get_node("main", "daemon:dreaming").unwrap().unwrap();
        assert_eq!(got.origin, Origin::Human, "provenance must survive upsert");
        assert_eq!(
            got.body.as_deref(),
            Some("re-registered"),
            "body still updates"
        );
    }

    /// A re-registration that omits `body` must not erase it.
    ///
    /// The only writer builds its row from `#[serde(default)] Option<String>`
    /// args, so "fix the label on `root:aleph`" arrived as `body = None` and a
    /// plain `SET body = excluded.body` wrote NULL over the **human reference
    /// text** — the one thing in this store no machine may supply. Asserted at
    /// the consumer that actually matters: the rendered session topology, which
    /// emits a root line only `if let Some(body)`, so the erasure showed up as
    /// a line silently missing from every governed session's prompt.
    #[test]
    fn label_only_reregistration_keeps_the_human_root_reference() {
        let (_d, s) = store();
        s.upsert_node(
            &node("root:aleph", NodeKind::Root, Origin::Human).with_body("对用户真实有用"),
        )
        .unwrap();
        s.upsert_node(&node("goal:sess-1", NodeKind::LoopGoal, Origin::Llm))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:aleph",
            "goal:sess-1",
            EdgeKind::OwnsReference,
            Origin::Human,
        ))
        .unwrap();

        // Re-register with a new label and NOTHING else — body/cadence omitted.
        let mut relabel = node("root:aleph", NodeKind::Root, Origin::Human);
        relabel.label = "根参照（改名）".into();
        s.upsert_node(&relabel).unwrap();

        let rendered = crate::loop_graph::service::render_session_topology_in(&s, "sess-1")
            .expect("governed session renders");
        assert!(
            rendered.contains("对用户真实有用"),
            "the human root reference must survive a label-only upsert: {rendered}"
        );

        // Clearing is still expressible — an empty string is not NULL.
        s.upsert_node(&node("root:aleph", NodeKind::Root, Origin::Human).with_body(""))
            .unwrap();
        assert_eq!(
            s.get_node("main", "root:aleph").unwrap().unwrap().body,
            Some(String::new()),
            "an explicit empty body still clears"
        );
    }

    /// A node row this build cannot PARSE is still a node row that EXISTS.
    ///
    /// `row_to_node` fail-softs unknown enum text to `Ok(None)` so one odd row
    /// cannot wedge a reader — but feeding that skip into `gc`'s DELETE
    /// predicate turned "I could not read this node" into "this node is gone"
    /// and irreversibly deleted every edge touching it. `NodeKind` is
    /// `#[non_exhaustive]`, so a downgrade after a new kind ships is the
    /// realistic trigger.
    #[test]
    fn gc_and_lint_treat_an_unparseable_node_row_as_present() {
        let (_d, s) = store();
        s.upsert_node(&node("daemon:dreaming", NodeKind::Daemon, Origin::Llm))
            .unwrap();
        s.upsert_node(&node("cron:watch", NodeKind::LoopCron, Origin::Llm))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:watch",
            "daemon:dreaming",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();

        // Simulate a row written by a newer build.
        s.lock()
            .execute(
                "UPDATE graph_nodes SET kind = 'loop_future' WHERE id = 'daemon:dreaming'",
                [],
            )
            .unwrap();

        assert!(
            s.gc("main").unwrap().removed.is_empty(),
            "gc must not delete edges into a node it merely failed to parse"
        );
        assert_eq!(
            s.list_edges("main").unwrap().len(),
            1,
            "the edge must survive"
        );
        assert!(
            !s.lint("main").unwrap().iter().any(|f| f.contains("悬空边")),
            "lint must not report a present-but-unparseable node as vanished"
        );
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
        assert_eq!(removed.removed.len(), 1);
        assert!(removed.retained_acl.is_empty());
        assert!(s.list_edges("main").unwrap().is_empty());
    }

    /// `gc` is supposed to converge the graph: either every deletable dangling
    /// edge is gone and the report matches, or none is deleted. The previous
    /// implementation deleted edges one statement at a time, so a mid-loop
    /// failure left a half-deleted graph and a report that lied about what
    /// had actually changed. Asserted by making the first delete fail
    /// (`owns_reference` is normally retained, not deleted; we instead make
    /// the first non-protected delete fail by attaching a CHECK constraint
    /// to the edges table only after the first edge is queued for removal).
    ///
    /// This test uses a simpler construction: make the FIRST queued edge's
    /// delete fail by tampering with its primary key after the snapshot was
    /// read, so the second delete then succeeds. With a real transaction
    /// the second one rolls back; without one, it stays deleted.
    #[test]
    fn gc_is_atomic_across_multiple_dangling_edges() {
        let (_d, s) = store();
        // Three optimizer nodes plus one watcher; deleting the watcher leaves
        // three dangling `watches` edges that `gc` must remove atomically.
        for id in ["daemon:d1", "daemon:d2", "daemon:d3"] {
            s.upsert_node(&node(id, NodeKind::Daemon, Origin::Llm))
                .unwrap();
        }
        s.upsert_node(&node(
            "heartbeat:gone",
            NodeKind::LoopHeartbeat,
            Origin::Llm,
        ))
        .unwrap();
        for to in ["daemon:d1", "daemon:d2", "daemon:d3"] {
            s.upsert_edge(&GraphEdge::new(
                "main",
                "heartbeat:gone",
                to,
                EdgeKind::Watches,
                Origin::Llm,
            ))
            .unwrap();
        }
        assert!(s.delete_node("main", "heartbeat:gone").unwrap());

        // Force the FIRST DELETE in the transaction to fail: arm a
        // SQL trigger that raises an error on the first row touched. The
        // remaining two DELETEs in the loop must roll back.
        s.lock()
            .execute_batch(
                // The latch table is created FIRST: the trigger's `WHEN` clause
                // reads it, and a fixture that depends on SQLite deferring that
                // resolution is a fixture that breaks on an engine upgrade.
                //
                // `VALUES (1)`, not `DEFAULT VALUES`: SQLite does not accept the
                // `DEFAULT VALUES` form inside a TRIGGER body. rustc does not read
                // this string, so the error surfaced only as a failing test —
                // 判据 §10, "the language embedded in a string literal is one the
                // compiler never looks at". The `WHEN NOT EXISTS` guard is what
                // keeps the fixed constant from colliding on a second fire.
                "CREATE TABLE _gc_break_fired (id INTEGER PRIMARY KEY);
                 CREATE TRIGGER gc_break_first_delete BEFORE DELETE ON graph_edges
                 WHEN NOT EXISTS (SELECT 1 FROM _gc_break_fired)
                 BEGIN
                     INSERT INTO _gc_break_fired (id) VALUES (1);
                     SELECT RAISE(ABORT, 'gc_fail_simulation');
                 END;",
            )
            .unwrap();

        let report = s.gc("main");
        assert!(
            report.is_err(),
            "the simulated delete failure must propagate, not be swallowed"
        );

        // Atomicity: even if the SECOND and THIRD DELETEs in the loop ran
        // before the failure (rusqlite executes them sequentially within one
        // transaction), the transaction commit must roll them all back. The
        // graph must look exactly as it did before `gc` was called.
        let remaining = s.list_edges("main").unwrap();
        assert_eq!(
            remaining.len(),
            3,
            "gc atomicity: a failed gc must leave every dangling edge in place \
             (the report cannot lie about what was removed): {remaining:?}"
        );

        // Drop the trigger and re-run: gc now succeeds and removes all three.
        s.lock()
            .execute_batch("DROP TRIGGER gc_break_first_delete;")
            .unwrap();
        let report = s.gc("main").unwrap();
        assert_eq!(report.removed.len(), 3, "{report:?}");
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

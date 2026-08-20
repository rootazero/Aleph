//! `core/loop-graph` — structural lint of the governance topology.
//!
//! A cheap, always-available observation surface between audit-loop ticks:
//! dangling edges (a governed loop vanished), naked optimization loops
//! (nothing watches/audits them), unanchored governance chains, and fast
//! loops owning slower loops' references. Read-only by design — the graph
//! is the held-out layer, so even doctor never repairs it mechanically;
//! findings route to the audit loop / user instead.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::loop_graph::LoopGraphStore;

const ID: &str = "core/loop-graph";
const DB_FILENAME: &str = "loop_graph.db";

/// Agent scope to lint. Doctor is a path-based offline check with no agent
/// registry handle; it reads the same single constant every other loop_graph
/// reader does rather than re-spelling the literal.
const DEFAULT_AGENT: &str = crate::routing::DEFAULT_AGENT_ID;

pub struct LoopGraphCheck {
    data_dir: PathBuf,
}

impl LoopGraphCheck {
    #[must_use]
    pub const fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait]
impl HealthCheck for LoopGraphCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Loop graph"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let path = self.data_dir.join(DB_FILENAME);
        if !path.exists() {
            return vec![Finding::ok(
                ID,
                "No graph yet",
                "loop_graph.db absent — no governance topology declared (zero cost).",
            )];
        }
        /// Outcome of the blocking store probe (`open` + `lint` + emptiness
        /// check), one enum so a single `spawn_blocking` covers all three
        /// synchronous SQLite calls.
        enum Probe {
            Unreadable(String),
            LintFailed(String),
            Done { findings: Vec<String>, empty: bool },
        }

        // rusqlite is synchronous — keep open/lint/list_nodes off the async executor.
        let probe = tokio::task::spawn_blocking(move || {
            let store = match LoopGraphStore::open_readonly(&path) {
                Ok(s) => s,
                Err(e) => return Probe::Unreadable(format!("{e}")),
            };
            let findings = match store.lint(DEFAULT_AGENT) {
                Ok(f) => f,
                Err(e) => return Probe::LintFailed(format!("{e}")),
            };
            let empty = findings.is_empty()
                && matches!(store.list_nodes(DEFAULT_AGENT), Ok(n) if n.is_empty());
            Probe::Done { findings, empty }
        })
        .await;

        let (findings, empty) = match probe {
            Ok(Probe::Done { findings, empty }) => (findings, empty),
            Ok(Probe::Unreadable(e)) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Graph DB unreadable",
                    e,
                )];
            }
            Ok(Probe::LintFailed(e)) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Graph lint failed",
                    e,
                )];
            }
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Graph lint failed",
                    format!("the graph lint task failed to run: {e}"),
                )];
            }
        };
        if findings.is_empty() {
            // "Nothing is wrong" and "nothing is declared" are different
            // answers, and only one of them is reassuring. The `!path.exists()`
            // branch above cannot tell them apart in production: the daemon
            // creates `loop_graph.db` unconditionally at boot
            // (`builtin_registry/builder/constructor`), so the file is always
            // there and an EMPTY graph — no loop registered, nothing watching
            // anything — used to report as "Topology sound". A governance layer
            // that certifies its own absence is the failure it exists to catch.
            if empty {
                return vec![Finding::ok(
                    ID,
                    "No topology declared",
                    "loop_graph.db exists but holds no nodes — no loop is registered, so nothing \
                     is watched, anchored or grounded. Zero cost, and zero coverage: \
                     `loop_graph(action='enable_audit')` + `pair` are how it starts.",
                )];
            }
            return vec![Finding::ok(
                ID,
                "Topology sound",
                "No dangling edges, naked optimization loops, or unanchored governance chains.",
            )];
        }
        findings
            .into_iter()
            .map(|f| {
                Finding::problem(ID, Severity::Warning, "Topology finding", f).with_fix_hint(
                    "结构信号，非机械修复项：交给审计环裁决（loop_graph status / graph-audit note），\
                     确认实体已消失的悬空边可 loop_graph(action='gc')",
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_graph::{GraphNode, NodeKind, Origin};

    #[tokio::test]
    async fn absent_db_is_ok_naked_loop_is_warning() {
        let dir = tempfile::tempdir().unwrap();
        let check = LoopGraphCheck::new(dir.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert!(findings.iter().all(|f| !f.is_problem()));

        let store = LoopGraphStore::open(&dir.path().join(DB_FILENAME)).unwrap();
        store
            .upsert_node(&GraphNode::new(
                DEFAULT_AGENT,
                "daemon:dreaming",
                NodeKind::Daemon,
                "夜巡",
                Origin::Llm,
            ))
            .unwrap();
        drop(store);

        let findings = check.run(Posture::Inspect).await;
        assert!(
            findings.iter().any(Finding::is_problem),
            "naked loop must be a warning: {findings:?}"
        );
    }

    /// The production shape: the daemon always creates the DB, so `!exists()`
    /// never fires and an empty graph reached the "Topology sound" line —
    /// certifying its own absence.
    #[tokio::test]
    async fn an_existing_but_empty_graph_is_not_reported_as_sound() {
        let dir = tempfile::tempdir().unwrap();
        drop(LoopGraphStore::open(&dir.path().join(DB_FILENAME)).unwrap());
        let findings = LoopGraphCheck::new(dir.path().to_path_buf())
            .run(Posture::Inspect)
            .await;
        assert!(findings.iter().all(|f| !f.is_problem()));
        let rendered = format!("{findings:?}");
        assert!(rendered.contains("No topology declared"), "{rendered}");
        assert!(!rendered.contains("Topology sound"), "{rendered}");
    }
}

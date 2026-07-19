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

/// Agent scopes to lint. Doctor is a path-based offline check with no agent
/// registry handle, so it lints the default scope only; other scopes are
/// covered by their own audit loops via `loop_graph(status)`.
const DEFAULT_AGENT: &str = "main";

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
        let store = match LoopGraphStore::open_readonly(&path) {
            Ok(s) => s,
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Graph DB unreadable",
                    format!("{e}"),
                )];
            }
        };
        let findings = match store.lint(DEFAULT_AGENT) {
            Ok(f) => f,
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Graph lint failed",
                    format!("{e}"),
                )];
            }
        };
        if findings.is_empty() {
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
}

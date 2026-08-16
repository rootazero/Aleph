//! Read-only exporters for the governance topology.
//!
//! Why two formats:
//! - **DOT** (Graphviz): operator-facing, for visual review of a non-trivial
//!   governance graph. The audit template already tells the model to look at
//!   `loop_graph status`; a picture is what an operator uses to verify the
//!   graph is wired as intended.
//! - **JSON**: machine-facing, for the Panel to render its own visualization
//!   (e.g. via a force-directed canvas) and for `governance_metrics` to
//!   serialize the topology without round-tripping through `list_nodes`/
//!   `list_edges`.
//!
//! Design choices:
//! - **No external dependency for DOT** — the format is small enough to
//!   template inline, and adding `petgraph`/`graphviz-rust` for one exporter
//!   is the kind of churn this layer rejects per R3.
//! - **Deterministic bytes** — sorted by (kind, id) for nodes, by
//!   (from_id, to_id, kind) for edges. The audit log ships these for replay;
//!   a non-deterministic sort would make byte-diffs meaningless.
//! - **No body in DOT** — bodies can be many KB (root references are
//!   deliberately large; see `service.rs::MAX_ROOT_BODY_CHARS`). The DOT
//!   label is just `id` + `kind`, with body as an HTML `TITLE` tooltip the
//!   user can hover. Bodies go in JSON, where size is honest.
//!
//! What this is NOT:
//! - Not a renderer. There is no live UI in this crate (the Panel's graph
//!   canvas lives in `interfaces/webchat/.../views/canvas/`).
//! - Not a write format. There is no `from_dot`/`from_json` — the graph's
//!   vocabulary is closed (GRAPH_LAYER §7 NOT-build #2: "graph database
//!   interchange"), and re-importing would also need to enforce every store
//!   invariant the live writer enforces.

use std::fmt::Write as _;

use crate::error::Result;
use crate::loop_graph::store::LoopGraphStore;
use crate::loop_graph::types::{EdgeKind, NodeKind};

/// Format the graph as a Graphviz DOT document. The output is stable for a
/// stable graph (sorted nodes, sorted edges, deterministic attribute order),
/// so caching by bytes is safe.
pub fn to_dot(store: &LoopGraphStore, agent_id: &str) -> Result<String> {
    let mut nodes = store.list_nodes(agent_id)?;
    let mut edges = store.list_edges(agent_id)?;
    nodes.sort_by(|a, b| (a.kind.as_str(), a.id.as_str()).cmp(&(b.kind.as_str(), b.id.as_str())));
    edges.sort_by(|a, b| {
        (a.from_id.as_str(), a.to_id.as_str(), a.kind.as_str()).cmp(&(
            b.from_id.as_str(),
            b.to_id.as_str(),
            b.kind.as_str(),
        ))
    });

    let mut out = String::new();
    out.push_str(&format!("// loop_graph export, agent={agent_id}\n"));
    out.push_str("digraph loop_graph {\n");
    out.push_str("  rankdir=LR;\n");
    out.push_str("  graph [fontname=\"Helvetica\", fontsize=10];\n");
    out.push_str("  node  [fontname=\"Helvetica\", fontsize=10, shape=box, style=\"rounded\"];\n");
    out.push_str("  edge  [fontname=\"Helvetica\", fontsize=10];\n\n");

    // Shape by kind — quick visual grouping for the operator.
    for n in &nodes {
        let shape = shape_for(n.kind);
        let color = color_for(n.kind);
        // Stable DOT id: replace `:` with `_` (DOT ids may not contain `:`).
        let dot_id = dot_id(&n.id);
        let label = dot_escape(&n.label);
        // Tooltip carries the body verbatim if present (≤ 200 chars to keep
        // Graphviz responsive); full body still goes through JSON.
        let tooltip = n
            .body
            .as_deref()
            .map(|b| truncate_chars(b, 200))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {dot_id} [label=\"{label}\", shape={shape}, color=\"{color}\", fillcolor=\"{color}\", style=\"filled,rounded\", tooltip=\"{}\"];",
            dot_escape(&tooltip)
        );
    }

    out.push('\n');
    for e in &edges {
        let from = dot_id(&e.from_id);
        let to = dot_id(&e.to_id);
        let color = edge_color_for(e.kind);
        let label = e.kind.as_str();
        let _ = writeln!(
            out,
            "  {from} -> {to} [label=\"{label}\", color=\"{color}\", fontcolor=\"{color}\"];"
        );
    }

    out.push_str("}\n");
    Ok(out)
}

/// Format the graph as compact JSON. The Panel uses this to render its own
/// graph view without re-reading SQLite, and `governance_metrics` uses it to
/// log the topology at audit boundaries.
pub fn to_json(store: &LoopGraphStore, agent_id: &str) -> Result<String> {
    let mut nodes = store.list_nodes(agent_id)?;
    let mut edges = store.list_edges(agent_id)?;
    nodes.sort_by(|a, b| (a.kind.as_str(), a.id.as_str()).cmp(&(b.kind.as_str(), b.id.as_str())));
    // Same total order as `to_dot` — (from_id, to_id, kind) lexicographic. An
    // earlier draft cross-wired the tuple fields ((a.from, b.from, a.kind)
    // against (b.from, a.to, b.kind)), which is not a total order at all: it
    // compared one edge's kind against the OTHER edge's to_id. `sort_by`
    // tolerated it silently and produced an arbitrary permutation, so DOT and
    // JSON disagreed about edge order and byte-diffs against the audit log
    // were meaningless — the exact property the header comment promises.
    edges.sort_by(|a, b| {
        (a.from_id.as_str(), a.to_id.as_str(), a.kind.as_str()).cmp(&(
            b.from_id.as_str(),
            b.to_id.as_str(),
            b.kind.as_str(),
        ))
    });

    let payload = serde_json::json!({
        "agent_id": agent_id,
        "node_count": nodes.len(),
        "edge_count": edges.len(),
        "nodes": nodes,
        "edges": edges,
    });
    serde_json::to_string_pretty(&payload).map_err(Into::into)
}

fn shape_for(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Root => "note",
        NodeKind::Frozen => "octagon",
        NodeKind::Anchor => "diamond",
        NodeKind::Daemon | NodeKind::LoopCron | NodeKind::LoopHeartbeat => "box",
        NodeKind::LoopGoal => "ellipse",
        NodeKind::Team => "hexagon",
    }
}

fn color_for(kind: NodeKind) -> &'static str {
    match kind {
        // Hex codes deliberately picked so the colors are distinguishable in
        // both light and dark themes and don't clash with Graphviz's defaults.
        NodeKind::Root => "#FFD580",          // warm cream
        NodeKind::Frozen => "#B0B0B0",        // grey
        NodeKind::Anchor => "#A6E3A1",        // green
        NodeKind::Daemon => "#89B4FA",        // blue
        NodeKind::LoopCron => "#89B4FA",      // blue
        NodeKind::LoopHeartbeat => "#89B4FA", // blue
        NodeKind::LoopGoal => "#F5C2E7",      // pink
        NodeKind::Team => "#FAB387",          // orange
    }
}

fn edge_color_for(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Watches => "#A6E3A1",
        EdgeKind::OwnsReference => "#F38BA8",
        EdgeKind::Arbitrates => "#FAB387",
        EdgeKind::Audits => "#89B4FA",
        EdgeKind::AnchoredBy => "#A6ADC8",
        EdgeKind::Feeds => "#CDD6F4",
    }
}

fn dot_id(id: &str) -> String {
    // DOT accepts [_A-Za-z0-9]; everything else becomes `_`.
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn dot_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_graph::types::{GraphEdge, GraphNode, Origin};

    fn store() -> (tempfile::TempDir, LoopGraphStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = LoopGraphStore::open(&dir.path().join("g.db")).unwrap();
        (dir, s)
    }

    #[test]
    fn dot_is_deterministic_for_an_empty_graph() {
        let (_d, s) = store();
        let a = to_dot(&s, "main").unwrap();
        let b = to_dot(&s, "main").unwrap();
        assert_eq!(a, b);
        assert!(a.contains("digraph loop_graph"));
        assert!(a.contains("}"));
    }

    #[test]
    fn dot_includes_nodes_and_edges() {
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
            "root:aleph",
            NodeKind::Root,
            "root",
            Origin::Human,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:aleph",
            "goal:s1",
            EdgeKind::Watches,
            Origin::Human,
        ))
        .unwrap();

        let dot = to_dot(&s, "main").unwrap();
        assert!(
            dot.contains("root_aleph"),
            "id colon must be replaced: {dot}"
        );
        assert!(dot.contains("goal_s1"));
        assert!(
            dot.contains("watches"),
            "edge verb must appear as label: {dot}"
        );
        assert!(dot.contains("root_aleph -> goal_s1"));
    }

    #[test]
    fn dot_escapes_quotes_in_labels() {
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:q",
            NodeKind::LoopGoal,
            r#"has "quotes" and \backslashes"#,
            Origin::Llm,
        ))
        .unwrap();
        let dot = to_dot(&s, "main").unwrap();
        assert!(dot.contains(r#"has \"quotes\" and \\backslashes"#), "{dot}");
    }

    #[test]
    fn json_round_trips_through_serde_with_a_typed_read() {
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
            "w",
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

        let json = to_json(&s, "main").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["agent_id"], "main");
        assert_eq!(parsed["node_count"], 2);
        assert_eq!(parsed["edge_count"], 1);
        assert!(parsed["nodes"].is_array());
        assert!(parsed["edges"].is_array());
    }

    #[test]
    fn json_is_deterministic_for_a_stable_graph() {
        let (_d, s) = store();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:a",
            NodeKind::LoopGoal,
            "a",
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_node(&GraphNode::new(
            "main",
            "goal:b",
            NodeKind::LoopGoal,
            "b",
            Origin::Llm,
        ))
        .unwrap();
        let j1 = to_json(&s, "main").unwrap();
        let j2 = to_json(&s, "main").unwrap();
        assert_eq!(j1, j2);
    }

    /// The cross-wired comparator this guards against compared one edge's kind
    /// against the OTHER edge's to_id, so DOT (correct) and JSON (broken)
    /// disagreed about edge order. The adversarial case: same `from_id`, and
    /// a pair whose (kind, to_id) strings order OPPOSITELY under the broken
    /// tuple — `watches` > `goal:a` while `audits` < `goal:b` — so the broken
    /// comparator emits the audits edge first where the correct one emits the
    /// edge to `goal:a` first.
    #[test]
    fn json_edge_order_matches_dot_edge_order() {
        let (_d, s) = store();
        for id in ["cron:w", "goal:a", "goal:b"] {
            let kind = if id.starts_with("cron:") {
                NodeKind::LoopCron
            } else {
                NodeKind::LoopGoal
            };
            s.upsert_node(&GraphNode::new("main", id, kind, "x", Origin::Llm))
                .unwrap();
        }
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:w",
            "goal:a",
            EdgeKind::Watches,
            Origin::Llm,
        ))
        .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "cron:w",
            "goal:b",
            EdgeKind::Audits,
            Origin::Llm,
        ))
        .unwrap();

        let dot = to_dot(&s, "main").unwrap();
        let dot_order: Vec<&str> = dot
            .lines()
            .filter(|l| l.contains("->"))
            .map(|l| l.trim())
            .collect();
        assert_eq!(dot_order.len(), 2, "{dot}");
        assert!(
            dot_order[0].starts_with("cron_w -> goal_a"),
            "dot must order by to_id first: {dot_order:?}"
        );

        let json = to_json(&s, "main").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let json_order: Vec<&str> = parsed["edges"]
            .as_array()
            .expect("edges array")
            .iter()
            .map(|e| e["to_id"].as_str().expect("to_id"))
            .collect();
        assert_eq!(
            json_order,
            vec!["goal:a", "goal:b"],
            "json must use the same (from_id, to_id, kind) order as dot"
        );
    }

    /// Determinism against INSERTION order, not just against a second read of
    /// the same store: the same edge set written in two different orders must
    /// serialize to identical bytes (the audit log byte-diffs these). The
    /// store's own ORDER BY feeds `to_json` a sorted vec, so this only fails
    /// if the in-memory sort is not a genuine total order — exactly the
    /// regression this test pins.
    #[test]
    fn json_bytes_are_identical_for_shuffled_insertion_order() {
        let edges: Vec<(&str, &str, EdgeKind)> = vec![
            ("cron:w", "goal:b", EdgeKind::Audits),
            ("cron:w", "goal:a", EdgeKind::Watches),
            ("cron:y", "cron:w", EdgeKind::OwnsReference),
            ("cron:x", "goal:a", EdgeKind::Feeds),
        ];
        let mut shuffled = edges.clone();
        shuffled.reverse();

        let build = |order: &[(&str, &str, EdgeKind)]| {
            let (_d, s) = store();
            // upsert_edge requires both endpoints to exist. Pin every
            // timestamp: created_at_ms/updated_at_ms serialize into the JSON
            // and a millisecond boundary between the two builds would flake
            // the byte comparison for a reason that has nothing to do with
            // ordering.
            for id in ["cron:w", "cron:x", "cron:y", "goal:a", "goal:b"] {
                let kind = if id.starts_with("cron:") {
                    NodeKind::LoopCron
                } else {
                    NodeKind::LoopGoal
                };
                let mut n = GraphNode::new("main", id, kind, "x", Origin::Llm);
                n.created_at_ms = 1_700_000_000_000;
                n.updated_at_ms = 1_700_000_000_000;
                s.upsert_node(&n).unwrap();
            }
            for (from, to, kind) in order {
                let mut e = GraphEdge::new("main", *from, *to, *kind, Origin::Llm);
                e.created_at_ms = 1_700_000_000_000;
                s.upsert_edge(&e).unwrap();
            }
            to_json(&s, "main").unwrap()
        };
        assert_eq!(build(&edges), build(&shuffled));
    }
    #[test]
    fn export_handles_large_graph_within_reasonable_byte_size() {
        let (_d, s) = store();
        // 50 nodes, 50 edges — well within any panel's render budget.
        for i in 0..50 {
            s.upsert_node(&GraphNode::new(
                "main",
                format!("goal:s{i}"),
                NodeKind::LoopGoal,
                format!("label-{i}"),
                Origin::Llm,
            ))
            .unwrap();
        }
        for i in 0..49 {
            s.upsert_edge(&GraphEdge::new(
                "main",
                format!("goal:s{i}"),
                format!("goal:s{}", i + 1),
                EdgeKind::Feeds,
                Origin::Llm,
            ))
            .unwrap();
        }
        let dot = to_dot(&s, "main").unwrap();
        let json = to_json(&s, "main").unwrap();
        assert!(dot.len() < 200_000);
        assert!(json.len() < 200_000);
    }
}

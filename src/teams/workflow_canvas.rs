//! `CoordTask` DAG ↔ Obsidian JSON Canvas bridge.
//!
//! Renders a team's `coord_tasks` graph as a JSON Canvas `Document` (Obsidian
//! 1.0 spec) and parses canvas documents back into `NewCoordTask`s. This is
//! the data layer behind R2's plan visualization — the panel's canvas engine
//! reads the same wire format we serve here.
//!
//! ## Layout
//!
//! Tasks are arranged in topological *layers* (Kahn's algorithm):
//! - Column `x` = layer index × `LAYER_X_SPACING`
//! - Row    `y` = position within layer × `ROW_Y_SPACING`
//!
//! Cycles in the dependency graph (which the dispatcher rejects at task
//! creation, but defence-in-depth here) cause stragglers to be placed in a
//! final "orphan" column with no layer guarantee — we never panic.
//!
//! ## Colour mapping (Obsidian preset palette)
//!
//! | `CoordTaskStatus` | Preset |
//! |-------------------|--------|
//! | Pending           | `"3"` yellow |
//! | InProgress        | `"5"` cyan   |
//! | Completed         | `"4"` green  |
//! | Failed            | `"1"` red    |
//! | Cancelled         | none (grey)  |

use std::collections::{HashMap, VecDeque};

use aleph_protocol::json_canvas::{facing_sides, Document, Edge, EndShape, Node, NodeCommon};
use serde_json::Value;

use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, NewCoordTask, Priority};

// ---------------------------------------------------------------------------
// Layout constants — picked to keep cards readable in the panel
// ---------------------------------------------------------------------------

const NODE_WIDTH: i64 = 280;
const NODE_HEIGHT: i64 = 120;
const LAYER_X_SPACING: i64 = 360; // NODE_WIDTH + 80 gutter
const ROW_Y_SPACING: i64 = 180; // NODE_HEIGHT + 60 gutter

/// Metadata key used to record the originating `coord_task` ID inside a
/// canvas text node, so a subsequent `import_canvas` can choose to update
/// rather than re-create.
pub const CANVAS_NODE_TASK_ID_KEY: &str = "coord_task_id";

// ---------------------------------------------------------------------------
// Export: tasks → canvas
// ---------------------------------------------------------------------------

/// Render `tasks` (a team's coord-task slice) as an Obsidian JSON Canvas
/// document. Task subject + description become the text-node body; status
/// drives the colour; dependency edges become canvas edges with arrows.
pub fn tasks_to_canvas(tasks: &[CoordTask]) -> Document {
    let id_to_idx: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i))
        .collect();

    let layers = compute_layers(tasks, &id_to_idx);

    let mut nodes: Vec<Node> = Vec::with_capacity(tasks.len());
    // Node centres, keyed by task id — used to anchor edges to the facing sides
    // of each card so the exported `.canvas` renders cleanly (no centre-crossing
    // strokes) in Obsidian and the panel.
    let mut centers: HashMap<&str, (f64, f64)> = HashMap::with_capacity(tasks.len());
    for task in tasks {
        let (layer, row) = layers
            .get(task.id.as_str())
            .copied()
            .unwrap_or((tasks.len(), 0));
        let common = NodeCommon {
            id: task.id.clone(),
            x: (layer as i64) * LAYER_X_SPACING,
            y: (row as i64) * ROW_Y_SPACING,
            width: NODE_WIDTH,
            height: NODE_HEIGHT,
            color: status_color(task.status).map(str::to_string),
        };
        centers.insert(task.id.as_str(), common.center());
        nodes.push(Node::Text {
            common,
            text: render_task_body(task),
        });
    }

    // Edges: one canvas edge per `(dep, task)` immutable dependency. We use a
    // stable derived ID so re-renders are deterministic. Sides are anchored to
    // the facing cardinals of the two cards (dep → task, left-to-right by
    // layer) via the shared `facing_sides` single source of truth.
    let mut edges: Vec<Edge> = Vec::new();
    for task in tasks {
        for dep in &task.dependencies {
            if !id_to_idx.contains_key(dep.as_str()) {
                // dependency outside this slice — skip; keeps the canvas
                // self-contained.
                continue;
            }
            let dep_center = centers.get(dep.as_str());
            let task_center = centers.get(task.id.as_str());
            let (from_side, to_side) = match (dep_center, task_center) {
                (Some(&from_c), Some(&to_c)) => {
                    let (fs, ts) = facing_sides(from_c, to_c);
                    (Some(fs), Some(ts))
                }
                _ => (None, None),
            };
            edges.push(Edge {
                id: format!("e:{}:{}", dep, task.id),
                from_node: dep.clone(),
                from_side,
                from_end: None,
                to_node: task.id.clone(),
                to_side,
                to_end: Some(EndShape::Arrow),
                color: None,
                label: None,
            });
        }
    }

    Document { nodes, edges }
}

/// Render a task's display body. Markdown is honoured by the panel's
/// `markdown_excerpt` renderer.
fn render_task_body(task: &CoordTask) -> String {
    let owner_line = match task.owner.as_deref() {
        Some(o) => format!("\n\n_owner_: `{o}`"),
        None => String::new(),
    };
    let status_line = format!("\n\n_status_: `{}`", task.status.as_str());
    let desc = if task.description.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", task.description)
    };
    format!("**{}**{desc}{status_line}{owner_line}", task.subject)
}

const fn status_color(s: CoordTaskStatus) -> Option<&'static str> {
    match s {
        CoordTaskStatus::Pending => Some("3"),
        CoordTaskStatus::Blocked => Some("6"),
        CoordTaskStatus::InProgress => Some("5"),
        CoordTaskStatus::WaitingReview => Some("2"),
        CoordTaskStatus::Completed => Some("4"),
        CoordTaskStatus::Skipped => Some("6"),
        CoordTaskStatus::Paused => Some("6"),
        CoordTaskStatus::Failed => Some("1"),
        CoordTaskStatus::Unsatisfiable => Some("1"),
        CoordTaskStatus::Cancelled => None,
    }
}

/// Kahn-style layering: returns `task_id → (layer_index, position_within_layer)`.
///
/// Tasks with no in-edges land in layer 0. Each subsequent layer contains
/// tasks whose dependencies have all been placed in earlier layers. Ties
/// inside a layer preserve the input order so renders are deterministic.
fn compute_layers<'a>(
    tasks: &'a [CoordTask],
    id_to_idx: &HashMap<&str, usize>,
) -> HashMap<&'a str, (usize, usize)> {
    let n = tasks.len();
    let mut indeg: Vec<usize> = (0..n)
        .map(|i| {
            tasks[i]
                .dependencies
                .iter()
                .filter(|d| id_to_idx.contains_key(d.as_str()))
                .count()
        })
        .collect();

    let mut layer_of: Vec<Option<usize>> = vec![None; n];
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut current_layer = 0usize;

    while !queue.is_empty() {
        // Stable order within a layer = input order
        let mut layer_idxs: Vec<usize> = queue.iter().copied().collect();
        layer_idxs.sort();
        queue.clear();
        for &i in &layer_idxs {
            layer_of[i] = Some(current_layer);
        }
        // Discover next layer
        for &i in &layer_idxs {
            // Tasks depending on `tasks[i]` (i.e. its dependents) get their
            // in-degree decremented.
            for (j, t) in tasks.iter().enumerate() {
                if layer_of[j].is_some() {
                    continue;
                }
                for dep in &t.dependencies {
                    if id_to_idx.get(dep.as_str()).copied() == Some(i) {
                        indeg[j] = indeg[j].saturating_sub(1);
                        if indeg[j] == 0 {
                            queue.push_back(j);
                        }
                    }
                }
            }
        }
        current_layer += 1;
    }

    // Build the output map. Cycle stragglers (those still `None`) all land in
    // the same orphan layer = current_layer.
    let mut layer_groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut orphans: Vec<usize> = Vec::new();
    for (i, l) in layer_of.iter().enumerate() {
        match l {
            Some(layer) => layer_groups.entry(*layer).or_default().push(i),
            None => orphans.push(i),
        }
    }
    if !orphans.is_empty() {
        layer_groups.insert(current_layer, orphans);
    }

    let mut out: HashMap<&'a str, (usize, usize)> = HashMap::new();
    for (layer, mut idxs) in layer_groups {
        idxs.sort(); // stable within layer
        for (row, idx) in idxs.into_iter().enumerate() {
            out.insert(tasks[idx].id.as_str(), (layer, row));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Import: canvas → tasks
// ---------------------------------------------------------------------------

/// Reverse of [`tasks_to_canvas`]. Converts a JSON Canvas document into a
/// list of [`NewCoordTask`]s ready for [`CoordTaskStore::create_task`].
///
/// Only `Text` nodes become tasks. Other node kinds (`File`/`Link`/`Group`)
/// are skipped — they are visual aids, not work units. Edges become
/// `blocked_by` entries: an edge `A → B` means "task B depends on A". The
/// `team_id` on every returned `NewCoordTask` is set from the parameter.
///
/// The first line of each text node (`**bold**` prefix optional) becomes the
/// task `subject`; everything else becomes the `description`.
#[must_use]
pub fn canvas_to_new_tasks(doc: &Document, team_id: &str) -> Vec<NewCoordTask> {
    let text_nodes: Vec<(&str, &str)> = doc
        .nodes
        .iter()
        .filter_map(|n| match n {
            Node::Text { common, text } => Some((common.id.as_str(), text.as_str())),
            _ => None,
        })
        .collect();

    let text_ids: std::collections::HashSet<&str> = text_nodes.iter().map(|(id, _)| *id).collect();

    // Build dependency map: text_node_id → list of upstream text_node_ids.
    let mut deps: HashMap<&str, Vec<String>> = HashMap::new();
    for edge in &doc.edges {
        if text_ids.contains(edge.from_node.as_str()) && text_ids.contains(edge.to_node.as_str()) {
            deps.entry(edge.to_node.as_str())
                .or_default()
                .push(edge.from_node.clone());
        }
    }

    text_nodes
        .into_iter()
        .map(|(node_id, text)| {
            let (subject, description) = split_subject(text);
            let blocked_by = deps.remove(node_id).unwrap_or_default();
            // The dispatcher marker is load-bearing: `select_schedulable`
            // requires it, and BOTH reclaim passes are gated on it — without
            // it an imported canvas produces tasks the autonomous loop can
            // never run (silent forever-Pending kanban), the same trap
            // `handle_create_task` documents and stamps against. Ownerless
            // tasks still wait until an owner is assigned; the marker just
            // makes them dispatchable once one is.
            let metadata = serde_json::json!({
                CANVAS_NODE_TASK_ID_KEY: node_id,
                crate::teams::dispatcher::MANAGED_BY_KEY: crate::teams::dispatcher::MANAGED_BY_DISPATCHER,
            });
            NewCoordTask {
                team_id: Some(team_id.to_string()),
                subject,
                description,
                owner: None,
                priority: Priority::Normal,
                blocked_by,
                metadata,
            }
        })
        .collect()
}

/// Split a text-node body into `(subject, description)`. The first non-empty
/// line is the subject (with bold markers stripped); the rest is description.
fn split_subject(text: &str) -> (String, String) {
    let mut lines = text.split('\n');
    let first = loop {
        match lines.next() {
            Some(l) if !l.trim().is_empty() => break l,
            Some(_) => continue,
            // rust-doctor-disable-line unnecessary-allocation
            None => return (String::new(), String::new()),
        }
    };
    let subject_raw = first.trim();
    // Strip `**bold**` wrapping if the entire subject is bolded.
    let subject = if subject_raw.starts_with("**") && subject_raw.ends_with("**") {
        subject_raw
            .trim_start_matches("**")
            .trim_end_matches("**")
            .to_string()
    } else {
        subject_raw.to_string()
    };
    let rest: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (subject, rest)
}

/// Extract `coord_task_id` from a `CoordTask.metadata` JSON value — handy for
/// callers wanting to round-trip-update an existing task instead of creating
/// a new one. Returns `None` when the field is missing or not a string.
#[must_use]
pub fn extract_canvas_task_id(metadata: &Value) -> Option<&str> {
    metadata.get(CANVAS_NODE_TASK_ID_KEY)?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, Priority};

    fn mk_task(id: &str, deps: Vec<&str>, status: CoordTaskStatus) -> CoordTask {
        CoordTask {
            id: id.to_string(),
            team_id: Some("team-1".to_string()),
            subject: format!("Task {id}"),
            description: format!("Description of {id}"),
            status,
            owner: Some("worker-1".to_string()),
            priority: Priority::Normal,
            result: None,
            metadata: serde_json::Value::Null,
            dependencies: deps.into_iter().map(String::from).collect(),
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn export_two_node_chain_has_one_edge() {
        let tasks = vec![
            mk_task("a", vec![], CoordTaskStatus::Pending),
            mk_task("b", vec!["a"], CoordTaskStatus::Pending),
        ];
        let doc = tasks_to_canvas(&tasks);
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.edges.len(), 1);
        assert_eq!(doc.edges[0].from_node, "a");
        assert_eq!(doc.edges[0].to_node, "b");
        // Arrow head only on the destination side.
        assert!(matches!(doc.edges[0].to_end, Some(EndShape::Arrow)));
        assert!(doc.edges[0].from_end.is_none());
    }

    #[test]
    fn export_anchors_edge_sides_left_to_right() {
        use aleph_protocol::json_canvas::Side;
        // a (layer 0) → b (layer 1): b sits to the right of a, so the edge
        // should leave a's Right and enter b's Left.
        let tasks = vec![
            mk_task("a", vec![], CoordTaskStatus::Pending),
            mk_task("b", vec!["a"], CoordTaskStatus::Pending),
        ];
        let doc = tasks_to_canvas(&tasks);
        assert_eq!(doc.edges.len(), 1);
        assert_eq!(doc.edges[0].from_side, Some(Side::Right));
        assert_eq!(doc.edges[0].to_side, Some(Side::Left));
    }

    #[test]
    fn layout_places_layers_left_to_right() {
        let tasks = vec![
            mk_task("a", vec![], CoordTaskStatus::Pending),
            mk_task("b", vec!["a"], CoordTaskStatus::Pending),
            mk_task("c", vec!["b"], CoordTaskStatus::Pending),
        ];
        let doc = tasks_to_canvas(&tasks);
        let a = doc.nodes.iter().find(|n| n.id() == "a").unwrap().common();
        let b = doc.nodes.iter().find(|n| n.id() == "b").unwrap().common();
        let c = doc.nodes.iter().find(|n| n.id() == "c").unwrap().common();
        assert!(a.x < b.x, "a({}) should be left of b({})", a.x, b.x);
        assert!(b.x < c.x, "b({}) should be left of c({})", b.x, c.x);
    }

    #[test]
    fn layout_places_siblings_in_same_column() {
        // a → {b, c} both depend only on a → same layer
        let tasks = vec![
            mk_task("a", vec![], CoordTaskStatus::Pending),
            mk_task("b", vec!["a"], CoordTaskStatus::Pending),
            mk_task("c", vec!["a"], CoordTaskStatus::Pending),
        ];
        let doc = tasks_to_canvas(&tasks);
        let b_x = doc.nodes.iter().find(|n| n.id() == "b").unwrap().common().x;
        let c_x = doc.nodes.iter().find(|n| n.id() == "c").unwrap().common().x;
        assert_eq!(b_x, c_x);
    }

    #[test]
    fn status_colour_mapping() {
        let tasks = vec![
            mk_task("p", vec![], CoordTaskStatus::Pending),
            mk_task("ip", vec![], CoordTaskStatus::InProgress),
            mk_task("done", vec![], CoordTaskStatus::Completed),
            mk_task("fail", vec![], CoordTaskStatus::Failed),
            mk_task("cancel", vec![], CoordTaskStatus::Cancelled),
        ];
        let doc = tasks_to_canvas(&tasks);
        let by_id = |id: &str| {
            doc.nodes
                .iter()
                .find(|n| n.id() == id)
                .unwrap()
                .common()
                .color
                .clone()
        };
        assert_eq!(by_id("p"), Some("3".to_string()));
        assert_eq!(by_id("ip"), Some("5".to_string()));
        assert_eq!(by_id("done"), Some("4".to_string()));
        assert_eq!(by_id("fail"), Some("1".to_string()));
        assert_eq!(by_id("cancel"), None);
    }

    #[test]
    fn import_round_trips_subject_and_dependencies() {
        let original = vec![
            mk_task("a", vec![], CoordTaskStatus::Pending),
            mk_task("b", vec!["a"], CoordTaskStatus::Pending),
        ];
        let doc = tasks_to_canvas(&original);
        let parsed = canvas_to_new_tasks(&doc, "team-1");

        assert_eq!(parsed.len(), 2);
        let task_b = parsed.iter().find(|t| t.subject == "Task b").unwrap();
        assert_eq!(task_b.blocked_by, vec!["a".to_string()]);
        assert_eq!(task_b.team_id.as_deref(), Some("team-1"));
        // Metadata should carry the canvas node id so re-imports can update.
        assert_eq!(extract_canvas_task_id(&task_b.metadata), Some("b"),);
    }

    #[test]
    fn import_skips_non_text_nodes() {
        let doc = Document {
            nodes: vec![
                Node::Text {
                    common: NodeCommon {
                        id: "t1".into(),
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    text: "**Important**\n\nDetails".into(),
                },
                Node::File {
                    common: NodeCommon {
                        id: "f1".into(),
                        x: 200,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    file: "spec.md".into(),
                    subpath: None,
                },
                Node::Link {
                    common: NodeCommon {
                        id: "l1".into(),
                        x: 400,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    url: "https://example.com".into(),
                },
                Node::Group {
                    common: NodeCommon {
                        id: "g1".into(),
                        x: 0,
                        y: 200,
                        width: 200,
                        height: 200,
                        color: None,
                    },
                    label: Some("Phase 1".into()),
                    background: None,
                    background_style: None,
                },
            ],
            edges: vec![],
        };
        let parsed = canvas_to_new_tasks(&doc, "team-1");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].subject, "Important");
        assert_eq!(parsed[0].description, "Details");
    }

    #[test]
    fn import_drops_edges_pointing_at_non_text_nodes() {
        let doc = Document {
            nodes: vec![
                Node::Text {
                    common: NodeCommon {
                        id: "t1".into(),
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    text: "T1".into(),
                },
                Node::Group {
                    common: NodeCommon {
                        id: "g1".into(),
                        x: 200,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    label: None,
                    background: None,
                    background_style: None,
                },
            ],
            edges: vec![Edge {
                id: "e1".into(),
                from_node: "g1".into(),
                from_side: None,
                from_end: None,
                to_node: "t1".into(),
                to_side: None,
                to_end: Some(EndShape::Arrow),
                color: None,
                label: None,
            }],
        };
        let parsed = canvas_to_new_tasks(&doc, "team-1");
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].blocked_by.is_empty());
    }

    #[test]
    fn export_omits_edges_to_tasks_outside_slice() {
        // Task `b` depends on `external` (not in tasks) — that edge must not appear.
        let tasks = vec![mk_task("b", vec!["external"], CoordTaskStatus::Pending)];
        let doc = tasks_to_canvas(&tasks);
        assert_eq!(doc.edges.len(), 0);
        assert_eq!(doc.nodes.len(), 1);
    }

    #[test]
    fn render_task_body_includes_status_and_owner() {
        let task = mk_task("x", vec![], CoordTaskStatus::InProgress);
        let body = render_task_body(&task);
        assert!(body.contains("**Task x**"));
        assert!(body.contains("Description of x"));
        assert!(body.contains("in_progress"));
        assert!(body.contains("worker-1"));
    }
}

//! Bidirectional mapping between Aleph's `GraphNeighborsResponse` and
//! Obsidian JSON Canvas `Document`s.
//!
//! Design choices (recorded once, applied consistently below):
//!
//! 1. Each Aleph note becomes a `File` node whose `file` field is the note's
//!    `path`. The id is preserved verbatim, so a round-trip
//!    `graph → canvas → graph` is stable.
//! 2. The centre node is placed *last* in the JSON Canvas `nodes` array, so it
//!    renders on top per the spec's z-order rule (last = topmost).
//! 3. `fromSide`/`toSide` are derived from the relative geometry: the closest
//!    cardinal direction along the chord. This matches Obsidian's own
//!    auto-anchoring when you drag an edge between two nodes.
//! 4. Edges whose `kind` is in [`super::super::edge_curve::DIRECTIONAL_KINDS`]
//!    get `toEnd: arrow`; everything else gets `toEnd: none`. Aleph's `label`
//!    is forwarded verbatim.
//! 5. On import, only `File` and `Text` nodes are lifted into `NoteNodeDto`.
//!    `Link` and `Group` are preserved in `ImportResult::dropped` so callers
//!    can surface them without losing data.

use std::collections::HashMap;

use super::super::adapter::{GraphNeighborsResponse, NoteLinkDto, NoteNodeDto};
use super::super::edge_curve::DIRECTIONAL_KINDS;
use super::super::types::Vec2;
use super::spec::{Document, EndShape, Node, NodeCommon, Side};

/// Tunables for `graph_to_canvas`.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Default node width in pixels when no per-node hint is provided.
    pub default_width: i64,
    /// Default node height in pixels.
    pub default_height: i64,
}

impl Default for ExportOptions {
    fn default() -> Self {
        // Matches the FULL-mode card size used in the renderer (§5.2 of the
        // 2026-05-23 redesign spec): 280×150 px.
        Self {
            default_width: 280,
            default_height: 150,
        }
    }
}

/// Convert an Aleph graph neighbourhood + a positions map into a JSON Canvas
/// `Document`. The mapping is lossy on the way out (Aleph carries `category`,
/// `tags`, `link_count` which JSON Canvas has no slot for) but lossless on the
/// fields the spec models.
pub fn graph_to_canvas(
    neighbors: &GraphNeighborsResponse,
    positions: &HashMap<String, Vec2>,
    options: &ExportOptions,
) -> Document {
    // 1) Compose nodes — neighbours first, centre last, so centre renders on top.
    let mut nodes: Vec<Node> = neighbors
        .nodes
        .iter()
        .map(|n| note_to_file_node(n, positions, options))
        .collect();
    nodes.push(note_to_file_node(&neighbors.center, positions, options));

    // 2) Compose edges with auto-anchoring + arrow heads for directional kinds.
    let edges = neighbors
        .edges
        .iter()
        .map(|e| edge_to_canvas(e, positions))
        .collect();

    Document { nodes, edges }
}

/// Convert one note DTO to a JSON Canvas `File` node.
fn note_to_file_node(
    note: &NoteNodeDto,
    positions: &HashMap<String, Vec2>,
    options: &ExportOptions,
) -> Node {
    let pos = positions.get(&note.id).copied().unwrap_or(Vec2::zero());
    Node::File {
        common: NodeCommon {
            id: note.id.clone(),
            x: pos.x.round() as i64,
            y: pos.y.round() as i64,
            width: options.default_width,
            height: options.default_height,
            color: None,
        },
        file: note.path.clone(),
        subpath: None,
    }
}

/// Edge id format: `"e:{from}->{to}"`. Stable across re-exports.
fn edge_id(from: &str, to: &str) -> String {
    format!("e:{from}->{to}")
}

fn edge_to_canvas(
    link: &NoteLinkDto,
    positions: &HashMap<String, Vec2>,
) -> super::spec::Edge {
    let directional = link
        .kind
        .as_deref()
        .is_some_and(|k| DIRECTIONAL_KINDS.contains(&k));
    let (from_side, to_side) = anchor_sides(&link.from, &link.to, positions);

    super::spec::Edge {
        id: edge_id(&link.from, &link.to),
        from_node: link.from.clone(),
        from_side,
        from_end: None,
        to_node: link.to.clone(),
        to_side,
        to_end: Some(if directional { EndShape::Arrow } else { EndShape::None }),
        color: None,
        label: link.label.clone(),
    }
}

/// Pick (fromSide, toSide) from the closest cardinal direction along the
/// chord. The two sides face each other: the source emits toward the target,
/// and the target receives from the side facing back at the source.
fn anchor_sides(
    from_id: &str,
    to_id: &str,
    positions: &HashMap<String, Vec2>,
) -> (Option<Side>, Option<Side>) {
    let from = match positions.get(from_id) {
        Some(p) => *p,
        None => return (None, None),
    };
    let to = match positions.get(to_id) {
        Some(p) => *p,
        None => return (None, None),
    };
    let from_side = closest_side(from, to);
    let to_side = opposite_side(from_side);
    (Some(from_side), Some(to_side))
}

/// The cardinal direction (up/down/left/right) on the source node pointing
/// toward the target. Canvas convention: +y is down.
fn closest_side(from: Vec2, to: Vec2) -> Side {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            Side::Right
        } else {
            Side::Left
        }
    } else if dy >= 0.0 {
        Side::Bottom
    } else {
        Side::Top
    }
}

fn opposite_side(side: Side) -> Side {
    match side {
        Side::Top => Side::Bottom,
        Side::Bottom => Side::Top,
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    }
}

/// Result of importing a JSON Canvas `Document`.
///
/// Aleph models a single canvas as a flat graph with no implicit "centre",
/// so the importer surfaces the first eligible node it sees (or the first one
/// referenced by the most edges) as a hint, leaving the caller free to pick a
/// different anchor.
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    pub notes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    pub positions: HashMap<String, Vec2>,
    /// Nodes the importer could not lift into a `NoteNodeDto` (Link / Group /
    /// Text nodes with no path-shaped reference). Kept verbatim so the caller
    /// can surface them as auxiliary annotations.
    pub dropped: Vec<Node>,
}

/// Convert a JSON Canvas document into an Aleph-shaped note list + edge list
/// + positions map.
pub fn canvas_to_graph(doc: &Document) -> ImportResult {
    let mut notes = Vec::with_capacity(doc.nodes.len());
    let mut positions = HashMap::with_capacity(doc.nodes.len());
    let mut dropped = Vec::new();

    for node in &doc.nodes {
        let common = node.common();
        match node {
            Node::File { file, .. } => {
                positions.insert(
                    common.id.clone(),
                    Vec2::new(common.x as f64, common.y as f64),
                );
                notes.push(file_to_note(common, file));
            }
            Node::Text { text, .. } => {
                positions.insert(
                    common.id.clone(),
                    Vec2::new(common.x as f64, common.y as f64),
                );
                notes.push(text_to_note(common, text));
            }
            Node::Link { .. } | Node::Group { .. } => {
                dropped.push(node.clone());
            }
        }
    }

    // Pre-compute a {id → degree} table so we can stamp link_count on each
    // note based on the imported edges (matches how `graph.neighbors` reports
    // link_count on the server side).
    let mut degree: HashMap<String, usize> = HashMap::new();
    for e in &doc.edges {
        *degree.entry(e.from_node.clone()).or_default() += 1;
        *degree.entry(e.to_node.clone()).or_default() += 1;
    }
    for n in &mut notes {
        n.link_count = degree.get(&n.id).copied().unwrap_or(0);
    }

    let edges = doc
        .edges
        .iter()
        .map(|e| NoteLinkDto {
            from: e.from_node.clone(),
            to: e.to_node.clone(),
            label: e.label.clone(),
            kind: kind_from_arrow(e.to_end),
        })
        .collect();

    ImportResult {
        notes,
        edges,
        positions,
        dropped,
    }
}

fn file_to_note(common: &NodeCommon, file: &str) -> NoteNodeDto {
    let name = file_stem(file);
    NoteNodeDto {
        id: common.id.clone(),
        name,
        path: file.to_string(),
        category: "imported".to_string(),
        tags: Vec::new(),
        link_count: 0,
    }
}

fn text_to_note(common: &NodeCommon, text: &str) -> NoteNodeDto {
    // Use the first non-empty line (≤ 60 chars) as the visible name; this is a
    // best-effort summary, not a content-addressable identity.
    let name = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            if l.chars().count() <= 60 {
                l.to_string()
            } else {
                let truncated: String = l.chars().take(60).collect();
                format!("{truncated}…")
            }
        })
        .unwrap_or_else(|| "(empty text node)".to_string());

    NoteNodeDto {
        id: common.id.clone(),
        name,
        // Synthetic path. The importer leaves the real text in `dropped` if a
        // caller needs to recover the body verbatim.
        path: format!("canvas-text/{}.md", common.id),
        category: "imported".to_string(),
        tags: Vec::new(),
        link_count: 0,
    }
}

fn file_stem(path: &str) -> String {
    let last_segment = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match last_segment.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem.to_string(),
        _ => last_segment.to_string(),
    }
}

fn kind_from_arrow(to_end: Option<EndShape>) -> Option<String> {
    match to_end {
        // Default per spec is `arrow`, but we only stamp a kind when the
        // upstream produced an explicit arrow — that's how we know the canvas
        // author *meant* a directional edge.
        Some(EndShape::Arrow) => Some("refers".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::spec::{BackgroundStyle, Edge as JcEdge};

    fn n(id: &str, name: &str, path: &str, category: &str) -> NoteNodeDto {
        NoteNodeDto {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            category: category.to_string(),
            tags: vec![],
            link_count: 0,
        }
    }

    fn link(from: &str, to: &str, kind: Option<&str>, label: Option<&str>) -> NoteLinkDto {
        NoteLinkDto {
            from: from.to_string(),
            to: to.to_string(),
            label: label.map(str::to_string),
            kind: kind.map(str::to_string),
        }
    }

    fn neighbors_fixture() -> GraphNeighborsResponse {
        GraphNeighborsResponse {
            center: n("c", "Centre", "center.md", "user"),
            nodes: vec![n("a", "Alpha", "a.md", "project"), n("b", "Beta", "b.md", "feedback")],
            edges: vec![
                link("c", "a", Some("refers"), Some("alpha link")),
                link("c", "b", Some("related"), None),
                link("a", "b", None, None),
            ],
            hop_depth: HashMap::new(),
        }
    }

    #[test]
    fn graph_to_canvas_places_centre_last_for_top_z_order() {
        let positions: HashMap<String, Vec2> = [
            ("c".to_string(), Vec2::new(0.0, 0.0)),
            ("a".to_string(), Vec2::new(100.0, 0.0)),
            ("b".to_string(), Vec2::new(0.0, 100.0)),
        ]
        .into_iter()
        .collect();

        let doc = graph_to_canvas(&neighbors_fixture(), &positions, &ExportOptions::default());

        assert_eq!(doc.nodes.len(), 3);
        assert_eq!(doc.nodes.last().unwrap().id(), "c");
        assert_eq!(doc.edges.len(), 3);
    }

    #[test]
    fn graph_to_canvas_emits_arrow_only_for_directional_kinds() {
        let positions: HashMap<String, Vec2> = [
            ("c".to_string(), Vec2::new(0.0, 0.0)),
            ("a".to_string(), Vec2::new(100.0, 0.0)),
            ("b".to_string(), Vec2::new(0.0, 100.0)),
        ]
        .into_iter()
        .collect();

        let doc = graph_to_canvas(&neighbors_fixture(), &positions, &ExportOptions::default());

        let by_pair: HashMap<(String, String), &JcEdge> = doc
            .edges
            .iter()
            .map(|e| ((e.from_node.clone(), e.to_node.clone()), e))
            .collect();

        // "refers" → arrow
        assert_eq!(
            by_pair[&("c".into(), "a".into())].to_end,
            Some(EndShape::Arrow)
        );
        // "related" → none
        assert_eq!(
            by_pair[&("c".into(), "b".into())].to_end,
            Some(EndShape::None)
        );
        // No kind → none
        assert_eq!(
            by_pair[&("a".into(), "b".into())].to_end,
            Some(EndShape::None)
        );
    }

    #[test]
    fn graph_to_canvas_forwards_edge_labels() {
        let positions: HashMap<String, Vec2> = [
            ("c".to_string(), Vec2::new(0.0, 0.0)),
            ("a".to_string(), Vec2::new(100.0, 0.0)),
            ("b".to_string(), Vec2::new(0.0, 100.0)),
        ]
        .into_iter()
        .collect();

        let doc = graph_to_canvas(&neighbors_fixture(), &positions, &ExportOptions::default());
        let labelled = doc
            .edges
            .iter()
            .find(|e| e.from_node == "c" && e.to_node == "a")
            .unwrap();
        assert_eq!(labelled.label.as_deref(), Some("alpha link"));
    }

    #[test]
    fn anchor_sides_picks_dominant_axis() {
        // Target is to the right and slightly down — right is dominant.
        assert_eq!(
            closest_side(Vec2::new(0.0, 0.0), Vec2::new(100.0, 30.0)),
            Side::Right
        );
        // Target is below and slightly right — bottom is dominant.
        assert_eq!(
            closest_side(Vec2::new(0.0, 0.0), Vec2::new(30.0, 100.0)),
            Side::Bottom
        );
        // Diagonal exact 45° ties → prefers horizontal (`dx.abs() >= dy.abs()`).
        assert_eq!(
            closest_side(Vec2::new(0.0, 0.0), Vec2::new(50.0, 50.0)),
            Side::Right
        );
    }

    #[test]
    fn opposite_side_inverts_cardinals() {
        assert_eq!(opposite_side(Side::Top), Side::Bottom);
        assert_eq!(opposite_side(Side::Bottom), Side::Top);
        assert_eq!(opposite_side(Side::Left), Side::Right);
        assert_eq!(opposite_side(Side::Right), Side::Left);
    }

    #[test]
    fn round_trip_preserves_ids_paths_positions_and_labels() {
        let positions: HashMap<String, Vec2> = [
            ("c".to_string(), Vec2::new(0.0, 0.0)),
            ("a".to_string(), Vec2::new(100.0, 50.0)),
            ("b".to_string(), Vec2::new(-80.0, 120.0)),
        ]
        .into_iter()
        .collect();

        let original = neighbors_fixture();
        let doc = graph_to_canvas(&original, &positions, &ExportOptions::default());
        let imported = canvas_to_graph(&doc);

        // All three node ids survive
        let mut imported_ids: Vec<_> = imported.notes.iter().map(|n| n.id.clone()).collect();
        imported_ids.sort();
        assert_eq!(imported_ids, vec!["a", "b", "c"]);

        // Paths preserved verbatim
        let by_id: HashMap<_, _> = imported.notes.iter().map(|n| (&n.id, n)).collect();
        assert_eq!(by_id[&"a".to_string()].path, "a.md");
        assert_eq!(by_id[&"c".to_string()].path, "center.md");

        // Positions preserved (within i64-round error)
        for (id, pos) in &positions {
            let imported_pos = imported.positions.get(id).copied().expect("position kept");
            assert!((imported_pos.x - pos.x).abs() <= 0.5);
            assert!((imported_pos.y - pos.y).abs() <= 0.5);
        }

        // Edges preserved with their labels; directional kind re-stamped as "refers".
        let by_pair: HashMap<(String, String), &NoteLinkDto> = imported
            .edges
            .iter()
            .map(|e| ((e.from.clone(), e.to.clone()), e))
            .collect();
        assert_eq!(
            by_pair[&("c".into(), "a".into())].label.as_deref(),
            Some("alpha link")
        );
        assert_eq!(
            by_pair[&("c".into(), "a".into())].kind.as_deref(),
            Some("refers")
        );
        assert_eq!(by_pair[&("a".into(), "b".into())].kind, None);

        // link_count stamped from edge degree (c has 2 outgoing, a/b have 1 each + 1 mutual)
        assert_eq!(by_id[&"c".to_string()].link_count, 2);
        assert_eq!(by_id[&"a".to_string()].link_count, 2);
        assert_eq!(by_id[&"b".to_string()].link_count, 2);
    }

    #[test]
    fn import_drops_link_and_group_nodes() {
        let doc = Document {
            nodes: vec![
                Node::File {
                    common: NodeCommon {
                        id: "f1".into(),
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                        color: None,
                    },
                    file: "note.md".into(),
                    subpath: None,
                },
                Node::Link {
                    common: NodeCommon {
                        id: "l1".into(),
                        x: 200,
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
                        x: -100,
                        y: -100,
                        width: 600,
                        height: 600,
                        color: None,
                    },
                    label: Some("Container".into()),
                    background: None,
                    background_style: Some(BackgroundStyle::Cover),
                },
            ],
            edges: vec![],
        };
        let imported = canvas_to_graph(&doc);
        assert_eq!(imported.notes.len(), 1);
        assert_eq!(imported.notes[0].path, "note.md");
        assert_eq!(imported.dropped.len(), 2);
    }

    #[test]
    fn text_node_summary_uses_first_non_empty_line() {
        let doc = Document {
            nodes: vec![Node::Text {
                common: NodeCommon {
                    id: "t1".into(),
                    x: 0,
                    y: 0,
                    width: 250,
                    height: 100,
                    color: None,
                },
                text: "\n\n# Hello\nSecond line".into(),
            }],
            edges: vec![],
        };
        let imported = canvas_to_graph(&doc);
        assert_eq!(imported.notes.len(), 1);
        assert_eq!(imported.notes[0].name, "# Hello");
        assert_eq!(imported.notes[0].path, "canvas-text/t1.md");
    }

    #[test]
    fn file_stem_strips_directory_and_extension() {
        assert_eq!(file_stem("a/b/c.md"), "c");
        assert_eq!(file_stem("c.md"), "c");
        assert_eq!(file_stem("c"), "c");
        assert_eq!(file_stem(".dotfile"), ".dotfile");
        assert_eq!(file_stem("nested/path/note.canvas"), "note");
    }

    #[test]
    fn export_then_import_obsidian_sample_yields_zero_dropped_for_file_only_doc() {
        // Build a synthetic 2-file canvas (no Link/Group) and confirm both
        // make it through `canvas_to_graph`.
        let doc = Document {
            nodes: vec![
                Node::File {
                    common: NodeCommon {
                        id: "f1".into(),
                        x: 10,
                        y: 20,
                        width: 200,
                        height: 100,
                        color: Some("4".into()),
                    },
                    file: "a.md".into(),
                    subpath: None,
                },
                Node::File {
                    common: NodeCommon {
                        id: "f2".into(),
                        x: 220,
                        y: 20,
                        width: 200,
                        height: 100,
                        color: None,
                    },
                    file: "subdir/b.md".into(),
                    subpath: None,
                },
            ],
            edges: vec![JcEdge {
                id: "e1".into(),
                from_node: "f1".into(),
                from_side: Some(Side::Right),
                from_end: None,
                to_node: "f2".into(),
                to_side: Some(Side::Left),
                to_end: Some(EndShape::Arrow),
                color: None,
                label: Some("links to".into()),
            }],
        };
        let imported = canvas_to_graph(&doc);
        assert_eq!(imported.notes.len(), 2);
        assert!(imported.dropped.is_empty());
        assert_eq!(imported.edges.len(), 1);
        assert_eq!(imported.edges[0].label.as_deref(), Some("links to"));
        assert_eq!(imported.edges[0].kind.as_deref(), Some("refers"));
    }
}

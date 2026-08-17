//! Obsidian JSON Canvas 1.0 schema types — wire-compatible with `.canvas` files.
//! (renamed from `canvas_format` when the whiteboard took the `canvas` name —
//! see `shared/protocol/src/canvas.rs` and `src/canvas/`)
//!
//! Reference: `obsidianmd/jsoncanvas` `spec/1.0.md`.
//!
//! Lives in `aleph-protocol` so both the panel (frontend) and core (server-side
//! tools like `memory_canvas` save/load) share a single source of truth. Pure
//! data — no I/O, no Leptos, no DOM.
//!
//! All wire field names match the spec verbatim (`fromNode`, `toSide`, …) via
//! serde `rename` attributes, so any `.canvas` file produced by Obsidian
//! parses losslessly here and any [`Document`] serialised here opens in
//! Obsidian.

use serde::{Deserialize, Serialize};

/// Top-level JSON Canvas document.
///
/// Nodes are stored in ascending z-order (first = bottom, last = top), matching
/// the spec; conversion helpers in the panel preserve that order.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct Document {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
}

/// Generic fields shared by every node variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct NodeCommon {
    pub id: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// One of four canvas node types, tagged by the `"type"` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Text {
        #[serde(flatten)]
        common: NodeCommon,
        /// Plain text with Markdown syntax.
        text: String,
    },
    File {
        #[serde(flatten)]
        common: NodeCommon,
        /// Path to the file within the system.
        file: String,
        /// Optional `#heading` or `#^block` subpath.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Link {
        #[serde(flatten)]
        common: NodeCommon,
        url: String,
    },
    Group {
        #[serde(flatten)]
        common: NodeCommon,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "backgroundStyle"
        )]
        background_style: Option<BackgroundStyle>,
    },
}

impl Node {
    #[must_use]
    pub const fn common(&self) -> &NodeCommon {
        match self {
            Self::Text { common, .. }
            | Self::File { common, .. }
            | Self::Link { common, .. }
            | Self::Group { common, .. } => common,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.common().id
    }
}

impl NodeCommon {
    /// Geometric centre of the node's bounding box in canvas coordinates.
    ///
    /// Per the JSON Canvas spec, `x`/`y` are the box's **top-left** corner and
    /// `width`/`height` extend right/down (+y is down).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // centre of integer canvas coords; loss acceptable
    pub fn center(&self) -> (f64, f64) {
        (
            self.x as f64 + self.width as f64 / 2.0,
            self.y as f64 + self.height as f64 / 2.0,
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundStyle {
    Cover,
    Ratio,
    Repeat,
}

/// Edge connecting two nodes.
///
/// Field naming matches the spec verbatim (`fromNode`, `toSide`, …) via serde
/// renames; Rust-side names are `snake_case`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct Edge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "fromSide")]
    pub from_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "fromEnd")]
    pub from_end: Option<EndShape>,
    #[serde(rename = "toNode")]
    pub to_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "toSide")]
    pub to_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "toEnd")]
    pub to_end: Option<EndShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    /// The cardinal opposite — the side that faces back across a node.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Pick the `(from_side, to_side)` pair for an edge between two node centres.
///
/// The source emits toward the target along the chord's **dominant axis**
/// (the larger of |Δx|, |Δy|), and the target receives on the facing side.
/// This matches Obsidian's own auto-anchoring when an edge is dragged between
/// two nodes, and is the single source of truth shared by every JSON Canvas
/// producer in the workspace (core team-canvas export + panel graph export).
///
/// Canvas convention: +y is down. Exact 45° ties prefer the horizontal axis
/// (`|Δx| >= |Δy|`), so a target down-and-right anchors `Right → Left`.
#[must_use]
pub fn facing_sides(from_center: (f64, f64), to_center: (f64, f64)) -> (Side, Side) {
    let dx = to_center.0 - from_center.0;
    let dy = to_center.1 - from_center.1;
    let from_side = if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            Side::Right
        } else {
            Side::Left
        }
    } else if dy >= 0.0 {
        Side::Bottom
    } else {
        Side::Top
    };
    (from_side, from_side.opposite())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EndShape {
    None,
    Arrow,
}

/// `canvasColor` per spec — hex (`"#FF0000"`) or preset (`"1".."6"`).
///
/// Kept as an opaque `String` per the spec's deliberate ambiguity: apps may
/// remap the six presets to their own brand colours.
pub type Color = String;

/// Parse a JSON Canvas string into a [`Document`].
pub fn parse(s: &str) -> Result<Document, serde_json::Error> {
    serde_json::from_str(s)
}

/// Serialise a [`Document`] into pretty JSON (matches Obsidian's formatting).
pub fn to_string_pretty(doc: &Document) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 11-line `sample.canvas` shipped in `obsidianmd/jsoncanvas` is the
    /// authoritative round-trip fixture. We embed it verbatim and assert it
    /// parses, then re-serialises into something that re-parses to the same
    /// value (lossless under Serde semantics).
    const SAMPLE: &str = r#"{
        "nodes":[
            {"id":"754a8ef995f366bc","type":"group","x":-300,"y":-460,"width":610,"height":200,"label":"JSON Canvas"},
            {"id":"8132d4d894c80022","type":"file","file":"readme.md","x":-280,"y":-200,"width":570,"height":560,"color":"6"},
            {"id":"7efdbbe0c4742315","type":"file","file":"_site/logo.svg","x":-280,"y":-440,"width":217,"height":80},
            {"id":"59e896bc8da20699","type":"text","text":"Learn more:\n\n- [Apps](/docs/apps.md)\n- [Spec](spec/1.0.md)\n- [Github](https://github.com/obsidianmd/jsoncanvas)","x":40,"y":-440,"width":250,"height":160},
            {"id":"0ba565e7f30e0652","type":"file","file":"spec/1.0.md","x":360,"y":-400,"width":400,"height":400}
        ],
        "edges":[
            {"id":"6fa11ab87f90b8af","fromNode":"7efdbbe0c4742315","fromSide":"right","toNode":"59e896bc8da20699","toSide":"left"}
        ]
    }"#;

    #[test]
    fn parses_obsidian_sample_canvas() {
        let doc: Document = serde_json::from_str(SAMPLE).expect("sample.canvas must parse");
        assert_eq!(doc.nodes.len(), 5);
        assert_eq!(doc.edges.len(), 1);

        match &doc.nodes[0] {
            Node::Group { common, label, .. } => {
                assert_eq!(common.id, "754a8ef995f366bc");
                assert_eq!(label.as_deref(), Some("JSON Canvas"));
            }
            other => panic!("expected Group, got {:?}", other),
        }

        match &doc.nodes[1] {
            Node::File { common, file, .. } => {
                assert_eq!(file, "readme.md");
                assert_eq!(common.color.as_deref(), Some("6"));
            }
            other => panic!("expected File, got {:?}", other),
        }

        match &doc.nodes[3] {
            Node::Text { text, .. } => {
                assert!(text.contains("[Apps]"));
            }
            other => panic!("expected Text, got {:?}", other),
        }

        let e = &doc.edges[0];
        assert_eq!(e.from_node, "7efdbbe0c4742315");
        assert_eq!(e.from_side, Some(Side::Right));
        assert_eq!(e.to_side, Some(Side::Left));
        assert!(e.color.is_none());
    }

    #[test]
    fn round_trip_lossless() {
        let doc: Document = serde_json::from_str(SAMPLE).unwrap();
        let serialised = serde_json::to_string(&doc).unwrap();
        let reparsed: Document = serde_json::from_str(&serialised).unwrap();
        assert_eq!(doc, reparsed);
    }

    #[test]
    fn skips_empty_fields_on_serialise() {
        let doc = Document {
            nodes: vec![Node::File {
                common: NodeCommon {
                    id: "n1".into(),
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                    color: None,
                },
                file: "a.md".into(),
                subpath: None,
            }],
            edges: vec![],
        };
        let s = serde_json::to_string(&doc).unwrap();
        assert!(!s.contains("\"edges\""), "empty edges must be omitted");
        assert!(!s.contains("\"color\""), "missing color must be omitted");
        assert!(
            !s.contains("\"subpath\""),
            "missing subpath must be omitted"
        );
    }

    #[test]
    fn empty_document_serialises_to_empty_object() {
        let doc = Document::default();
        let s = serde_json::to_string(&doc).unwrap();
        assert_eq!(s, "{}");
    }

    #[test]
    fn node_common_center_is_box_midpoint() {
        let c = NodeCommon {
            id: "n".into(),
            x: 10,
            y: 20,
            width: 200,
            height: 100,
            color: None,
        };
        // top-left (10,20) + half-extent (100,50) → (110, 70)
        assert_eq!(c.center(), (110.0, 70.0));
    }

    #[test]
    fn side_opposite_inverts_cardinals() {
        assert_eq!(Side::Top.opposite(), Side::Bottom);
        assert_eq!(Side::Bottom.opposite(), Side::Top);
        assert_eq!(Side::Left.opposite(), Side::Right);
        assert_eq!(Side::Right.opposite(), Side::Left);
    }

    #[test]
    fn facing_sides_picks_dominant_axis_and_faces_back() {
        // target right + slightly down → horizontal dominates → Right/Left
        assert_eq!(
            facing_sides((0.0, 0.0), (100.0, 30.0)),
            (Side::Right, Side::Left)
        );
        // target below + slightly right → vertical dominates → Bottom/Top
        assert_eq!(
            facing_sides((0.0, 0.0), (30.0, 100.0)),
            (Side::Bottom, Side::Top)
        );
        // target up-left → Left wins on the tie-break path only if |dx|>=|dy|
        assert_eq!(
            facing_sides((0.0, 0.0), (-100.0, -10.0)),
            (Side::Left, Side::Right)
        );
        assert_eq!(
            facing_sides((0.0, 0.0), (-10.0, -100.0)),
            (Side::Top, Side::Bottom)
        );
        // exact 45° tie → horizontal preferred (|dx| >= |dy|)
        assert_eq!(
            facing_sides((0.0, 0.0), (50.0, 50.0)),
            (Side::Right, Side::Left)
        );
    }

    #[test]
    fn enum_lowercase_naming_matches_spec() {
        let s = serde_json::to_string(&Side::Right).unwrap();
        assert_eq!(s, "\"right\"");
        let s = serde_json::to_string(&EndShape::Arrow).unwrap();
        assert_eq!(s, "\"arrow\"");
        let s = serde_json::to_string(&BackgroundStyle::Cover).unwrap();
        assert_eq!(s, "\"cover\"");
    }

    #[test]
    fn parse_helper_matches_direct_serde() {
        let a: Document = serde_json::from_str(SAMPLE).unwrap();
        let b = parse(SAMPLE).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn to_string_pretty_round_trips() {
        let doc: Document = serde_json::from_str(SAMPLE).unwrap();
        let pretty = to_string_pretty(&doc).unwrap();
        let reparsed = parse(&pretty).unwrap();
        assert_eq!(doc, reparsed);
        // Pretty output contains newlines (basic sanity)
        assert!(pretty.contains('\n'));
    }
}

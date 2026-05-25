//! Obsidian JSON Canvas 1.0 interop for Aleph's memory canvas.
//!
//! Spec types live in `aleph_protocol::canvas_format` (shared with core so
//! server-side tools like `memory_canvas` use the same on-wire schema).
//! This module re-exports them as a `spec` submodule and adds panel-side
//! conversion in [`convert`].

pub mod convert;

/// Re-export of the shared schema, available as `json_canvas::spec::Document`
/// (and friends) — same import paths the convert layer already uses.
pub mod spec {
    pub use aleph_protocol::canvas_format::*;
}

pub use convert::{canvas_to_graph, graph_to_canvas, ExportOptions, ImportResult};
pub use spec::{
    parse, to_string_pretty, BackgroundStyle, Color, Document, Edge, EndShape, Node, NodeCommon,
    Side,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_export_paths_resolve() {
        // Sanity: the re-exports compile and round-trip via the shared spec.
        let original = Document {
            nodes: vec![Node::File {
                common: NodeCommon {
                    id: "f1".into(),
                    x: 0,
                    y: 0,
                    width: 200,
                    height: 100,
                    color: None,
                },
                file: "n.md".into(),
                subpath: None,
            }],
            edges: vec![],
        };
        let s = to_string_pretty(&original).unwrap();
        let reparsed = parse(&s).unwrap();
        assert_eq!(original, reparsed);
    }
}

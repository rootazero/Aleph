//! Canvas interaction events emitted by the galaxy renderer.
//!
//! After the Canvas2D radial engine was retired (Task 17), the only surviving
//! type here is [`CanvasEvent`] — the event the WebGL [`GalaxyCanvas`] emits to
//! its host view. The former 2D hit-test / pan-zoom / keyboard-nav state lived
//! in this module too but went away with the radial path.
//!
//! [`GalaxyCanvas`]: crate::views::memory::galaxy

/// Pointer events the galaxy emits to the host view.
///
/// The shared `Node` suffix is deliberate and not redundancy: these are the
/// node-directed events specifically, and the graph emits camera and
/// selection-box events elsewhere. Dropping it would leave `CanvasEvent::Select`,
/// which does not say what was selected.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum CanvasEvent {
    /// A node was clicked.
    SelectNode(String),
    /// Empty space was clicked — clear the current selection.
    DeselectNode,
    /// The hovered node changed (edge-triggered; `None` = pointer left all nodes).
    HoverNode(Option<String>),
}

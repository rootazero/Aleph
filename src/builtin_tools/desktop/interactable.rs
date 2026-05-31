//! Shared selection of *actionable* AX elements.
//!
//! The role allowlist and the depth-first collector below were previously
//! duplicated between [`super::ax`] (the `desktop_ax_snapshot` tool) and
//! [`super::gui_locate`] (which kept a hand-copied "mirror" of the list).
//! `desktop_som` needs the exact same notion of "what can a user click", so
//! the canonical version lives here and both older callers re-use it rather
//! than carrying a third copy.

use aleph_protocol::desktop_bridge::methods::ax::AxElement;

/// AX roles a user can act on. Containers and decorative text are excluded so
/// a snapshot stays a short, actionable list rather than a wall of nodes.
pub(super) const INTERACTABLE_ROLES: &[&str] = &[
    "AXButton",
    "AXMenuButton",
    "AXPopUpButton",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXCheckBox",
    "AXRadioButton",
    "AXDisclosureTriangle",
    "AXTextField",
    "AXTextArea",
    "AXSearchField",
    "AXSecureTextField",
    "AXComboBox",
    "AXLink",
    "AXSlider",
    "AXIncrementor",
    "AXStepper",
    "AXColorWell",
    "AXSegmentedControl",
];

/// Return `(x, y, width, height)` when the element has a non-degenerate
/// bounding rectangle — elements with no bounds or a zero-size rect cannot be
/// clicked and are excluded from any actionable list.
pub(super) fn usable_bounds(node: &AxElement) -> Option<(f64, f64, f64, f64)> {
    node.bounds.as_ref().and_then(|b| {
        if b.width > 1.0 && b.height > 1.0 {
            Some((b.x, b.y, b.width, b.height))
        } else {
            None
        }
    })
}

/// Depth-first collection of interactable, clickable elements in document
/// order (the order a person reads the UI).
pub(super) fn collect_interactable<'a>(node: &'a AxElement, out: &mut Vec<&'a AxElement>) {
    if INTERACTABLE_ROLES.contains(&node.role.as_str()) && usable_bounds(node).is_some() {
        out.push(node);
    }
    for child in &node.children {
        collect_interactable(child, out);
    }
}

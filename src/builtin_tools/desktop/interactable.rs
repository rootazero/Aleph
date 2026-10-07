//! Shared selection *and* model-safe projection of AX elements.
//!
//! The role allowlist and the depth-first collector below were previously
//! duplicated between [`super::ax`] (the `desktop_ax_snapshot` tool) and
//! [`super::gui_locate`] (which kept a hand-copied "mirror" of the list).
//! `desktop_som` needs the exact same notion of "what can a user click", so
//! the canonical version lives here and both older callers re-use it rather
//! than carrying a third copy.
//!
//! [`safe_value`] and [`affordance_fields`] exist for the same reason: three
//! hand-written JSON projections (`ax::element_to_json`,
//! `set_of_marks::build_marks`, `gui_locate::success_output_ax`) render AX
//! elements into model-visible payloads. A password field's `AXValue` must be
//! unable to reach *any* of them, so the redaction lives at the single point
//! all three go through rather than being re-remembered at each call site.

use serde_json::{json, Map, Value};

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

/// `true` when the element masks its content — a password box.
///
/// The role check is a second, older signal: a helper that predates the
/// `secure` affordance still reports the subrole as its role, and a password
/// box must be recognised as one on that binary too.
pub(super) fn is_secure(node: &AxElement) -> bool {
    node.secure == Some(true) || node.role == "AXSecureTextField"
}

/// The only sanctioned read of an element's string value.
///
/// Returns `None` for a secure field, so a password cannot reach a model-facing
/// projection, a match score, the transcript, or memory. Blank/whitespace-only
/// values are also dropped — every caller treated them as absent anyway.
///
/// `secure: None` (an older helper that does not report the affordance) is
/// deliberately *not* treated as secure: doing so would blank out every value
/// on that binary. The role fallback in [`is_secure`] is what covers the
/// password box in that case.
pub(super) fn safe_value(node: &AxElement) -> Option<&str> {
    if is_secure(node) {
        return None;
    }
    node.value.as_deref().filter(|s| !s.trim().is_empty())
}

/// Deep copy of an element tree with every secure node's value removed.
///
/// The raw AX tools (`desktop_ax_query_focused` / `_query_tree` /
/// `_query_by_role`) hand the wire type straight to `serde_json`, so they have
/// no projection to redact in — the `AxElement` *is* the response. They pass it
/// through here first, which is the only reason a password box can be returned
/// as an element at all: the model needs to know the field exists (and that it
/// is secure) to reason about the form; it must never learn what is in it.
///
/// Takes the tree by value (the caller owns what the limb just handed back) so
/// redacting a 10k-node Chromium tree costs no clones.
pub(super) fn redact_secure_values(mut node: AxElement) -> AxElement {
    if is_secure(&node) {
        node.value = None;
    }
    node.children = node
        .children
        .into_iter()
        .map(redact_secure_values)
        .collect();
    node
}

/// Affordance keys shared by every model-facing element projection.
///
/// Emission is deliberately asymmetric, because a snapshot can carry hundreds
/// of nodes and every key is paid for in tokens on every subsequent turn:
///
/// * `actions` — whenever the element reports any. This is the field worth its
///   tokens: it is the element's *own* list of AX action names, so `ax_action`
///   stops being a guess among a few hardcoded ones.
/// * `enabled` — only when `false`. A greyed-out control is the surprising
///   state and the one the model must see ("Submit is disabled → fill the form
///   first"); `true` is the default assumption and says nothing.
/// * `settable` — only when `false`: "this value cannot be written, do not
///   reach for set_value".
/// * `secure` — only when `true`, and the value is withheld by [`safe_value`].
///
/// `None` means "the limb did not tell us" and is never rendered as a negative.
pub(super) fn affordance_fields(node: &AxElement) -> Map<String, Value> {
    let mut map = Map::new();
    if is_secure(node) {
        map.insert("secure".into(), json!(true));
    }
    if node.enabled == Some(false) {
        map.insert("enabled".into(), json!(false));
    }
    if node.settable == Some(false) {
        map.insert("settable".into(), json!(false));
    }
    match node.actions.as_deref() {
        Some(actions) if !actions.is_empty() => {
            map.insert("actions".into(), json!(actions));
        }
        _ => {}
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str) -> AxElement {
        AxElement {
            role: role.to_string(),
            pid: 1,
            ..Default::default()
        }
    }

    #[test]
    fn safe_value_withholds_a_secure_fields_contents() {
        let mut node = el("AXTextField");
        node.value = Some("hunter2".into());
        node.secure = Some(true);
        assert_eq!(safe_value(&node), None);
    }

    #[test]
    fn safe_value_withholds_on_the_secure_role_alone() {
        // Older helper: no `secure` affordance on the wire, subrole-as-role.
        let mut node = el("AXSecureTextField");
        node.value = Some("hunter2".into());
        assert_eq!(node.secure, None);
        assert_eq!(safe_value(&node), None);
    }

    #[test]
    fn safe_value_passes_a_normal_value_through() {
        let mut node = el("AXTextField");
        node.value = Some("a@b.c".into());
        assert_eq!(safe_value(&node), Some("a@b.c"));

        node.secure = Some(false);
        assert_eq!(safe_value(&node), Some("a@b.c"));
    }

    #[test]
    fn safe_value_drops_blank_values() {
        let mut node = el("AXTextField");
        node.value = Some("   ".into());
        assert_eq!(safe_value(&node), None);
    }

    #[test]
    fn affordances_emit_only_the_actionable_states() {
        let mut node = el("AXButton");
        node.enabled = Some(true);
        node.settable = Some(true);
        node.actions = Some(vec![]);
        // enabled:true / settable:true / actions:[] carry no information the
        // model does not already assume — they must not cost a token.
        assert!(affordance_fields(&node).is_empty());

        node.enabled = Some(false);
        node.actions = Some(vec!["AXPress".into(), "AXShowMenu".into()]);
        let fields = affordance_fields(&node);
        assert_eq!(fields["enabled"], json!(false));
        assert_eq!(fields["actions"], json!(["AXPress", "AXShowMenu"]));
        assert!(!fields.contains_key("settable"));
        assert!(!fields.contains_key("secure"));
    }

    #[test]
    fn affordances_are_empty_when_the_limb_says_nothing() {
        assert!(affordance_fields(&el("AXButton")).is_empty());
    }
}

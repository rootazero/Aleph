//! Model-facing compression of AX trees: elide pure layout wrappers.
//!
//! Ported from open-codex-computer-use's `shouldElideNode`
//! (AccessibilitySnapshot.swift): accessibility trees are full of
//! `AXGroup`/`AXUnknown` wrappers that carry no semantics of their own —
//! no label, no value, no actions — and exist only because the toolkit
//! nested its layout that way. On an Electron/WebView tree those wrappers
//! eat most of the node budget, so the truncation marker fires while the
//! actual controls were never reached.
//!
//! The pass is **render-only**: it runs on the tree about to be handed to the
//! model (`desktop_ax_query_tree`), never on the tree `verify_state` /
//! `gui_locate` / `set_of_marks` match against. An elided node's children are
//! rehomed to its parent at the same position, so structure the model may
//! navigate by is preserved; only the noise is gone.
//!
//! Pure functions over [`AxElement`], host-testable on any OS — the role
//! strings are already normalised to the macOS `"AX*"` vocabulary by every
//! limb (AT-SPI `roles.rs`, UIA `role_map.rs`, macOS natively).

use aleph_protocol::desktop_bridge::methods::ax::AxElement;

/// Container roles with no semantics of their own. Everything else — buttons,
/// text fields, static text, images — survives regardless of how empty it
/// looks, because the role itself is the information.
const WRAPPER_ROLES: &[&str] = &["AXGroup", "AXUnknown"];

/// A wrapper earns its place in the tree only if it carries at least one
/// piece of information: a label, a value, a link, an action set, or an
/// affordance flag. Bounds alone do not count — a bare rectangle with no
/// semantics is decoration, and "it has a frame" would keep every wrapper.
fn is_elidable_wrapper(node: &AxElement) -> bool {
    if !WRAPPER_ROLES.contains(&node.role.as_str()) {
        return false;
    }
    // A wrapper that can be *acted on* (e.g. an icon-only web button rendered
    // as a group with AXPress) is a target, not decoration — keep it.
    node.title.as_deref().is_none_or(str::is_empty)
        && node.value.as_deref().is_none_or(str::is_empty)
        && node.url.is_none()
        && node.actions.as_deref().is_none_or(|a| a.is_empty())
        && node.enabled.is_none()
        && node.settable.is_none()
        && node.secure.is_none()
}

/// Elide wrapper nodes below `node`, rehoming their children in place, and
/// return how many nodes were elided. `node` itself is never elided — the
/// caller owns the root's identity.
///
/// Children are processed bottom-up first, so a chain of nested wrappers
/// collapses in a single pass.
pub(super) fn elide_wrapper_nodes(node: &mut AxElement) -> usize {
    let mut elided = 0;
    for child in &mut node.children {
        elided += elide_wrapper_nodes(child);
    }
    let mut kept = Vec::with_capacity(node.children.len());
    for child in std::mem::take(&mut node.children) {
        if is_elidable_wrapper(&child) {
            elided += 1;
            // Rehome: the grandchildren take the wrapper's slot, in order.
            kept.extend(child.children);
        } else {
            kept.push(child);
        }
    }
    node.children = kept;
    elided
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, children: Vec<AxElement>) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: None,
            value: None,
            bounds: None,
            pid: 1,
            secure: None,
            enabled: None,
            settable: None,
            actions: None,
            url: None,
            children,
        }
    }

    fn titled(role: &str, title: &str, children: Vec<AxElement>) -> AxElement {
        AxElement {
            title: Some(title.to_string()),
            ..node(role, children)
        }
    }

    #[test]
    fn empty_wrapper_is_elided_and_children_rehomed_in_order() {
        let mut root = node(
            "AXWindow",
            vec![
                node("AXGroup", vec![node("AXButton", vec![])]),
                node("AXStaticText", vec![]),
            ],
        );
        let elided = elide_wrapper_nodes(&mut root);
        assert_eq!(elided, 1);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].role, "AXButton");
        assert_eq!(root.children[1].role, "AXStaticText");
    }

    #[test]
    fn nested_wrapper_chain_collapses_in_one_pass() {
        let mut root = node(
            "AXWindow",
            vec![node(
                "AXGroup",
                vec![node("AXUnknown", vec![node("AXButton", vec![])])],
            )],
        );
        let elided = elide_wrapper_nodes(&mut root);
        assert_eq!(elided, 2);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].role, "AXButton");
    }

    #[test]
    fn wrapper_with_a_label_survives() {
        let mut root = node(
            "AXWindow",
            vec![titled("AXGroup", "Sidebar", vec![node("AXButton", vec![])])],
        );
        assert_eq!(elide_wrapper_nodes(&mut root), 0);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].title.as_deref(), Some("Sidebar"));
    }

    #[test]
    fn wrapper_with_actions_is_a_target_not_decoration() {
        let mut actionable = node("AXGroup", vec![]);
        actionable.actions = Some(vec!["AXPress".to_string()]);
        let mut root = node("AXWindow", vec![actionable]);
        assert_eq!(elide_wrapper_nodes(&mut root), 0);
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn wrapper_with_affordance_flags_survives() {
        let mut flagged = node("AXGroup", vec![]);
        flagged.enabled = Some(false);
        let mut root = node("AXWindow", vec![flagged]);
        assert_eq!(elide_wrapper_nodes(&mut root), 0);
    }

    #[test]
    fn childless_wrapper_is_dropped() {
        let mut root = node("AXWindow", vec![node("AXGroup", vec![])]);
        assert_eq!(elide_wrapper_nodes(&mut root), 1);
        assert!(root.children.is_empty());
    }

    #[test]
    fn non_wrapper_roles_are_never_elided_even_when_empty() {
        let mut root = node("AXWindow", vec![node("AXImage", vec![])]);
        assert_eq!(elide_wrapper_nodes(&mut root), 0);
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn the_root_itself_is_never_elided() {
        let mut root = node("AXGroup", vec![]);
        assert_eq!(elide_wrapper_nodes(&mut root), 0);
        assert_eq!(root.role, "AXGroup");
    }
}

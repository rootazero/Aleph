//! accname-lite (spec §4.1), in order; the first rule that produces a value
//! wins.
//!
//! 1. `aria-labelledby` — the referenced nodes' text, in the order written
//! 2. `aria-label`
//! 3. `<label for=…>` or a wrapping `<label>`
//! 4. `value`, for `<input type=button|submit|reset>` only
//! 5. `placeholder`
//! 6. `alt` — and on an `<img>`, `alt=""` is an author DECISION to have no
//!    name, so it stops the chain rather than falling through
//! 7. `title`
//! 8. visible descendant text, whitespace-collapsed, capped at
//!    [`NAME_MAX_CHARS`] — **only for roles that take their name from their
//!    contents** ([`super::Role::supports_name_from_content`]). Everything
//!    else ends nameless, which for a landmark is the right answer: a
//!    `<header>` named by its whole nav bar is a name nobody can use.
//!
//! Rules 4 and 6's empty-alt case go beyond the spec's five-step list. Both
//! are named here, because a reader comparing the two documents would
//! otherwise read the difference as drift: without rule 4 the busiest button
//! on most forms is nameless, and without the `alt=""` stop a decorative image
//! is named by its tooltip.

use std::collections::HashMap;

use super::raw::{RawFrame, RawNodeKind};

/// The cap on a name derived from descendant text (spec §4.1: ≤80 chars).
pub const NAME_MAX_CHARS: usize = 80;

/// One frame, indexed for the lookups accname needs: id → node, `label[for]` →
/// node, parent → children.
///
/// Built once per frame by `PageState::build`. Per node it would be quadratic
/// on a page the size of Hacker News (~700 nodes).
pub struct FrameIndex<'a> {
    pub frame: &'a RawFrame,
    by_id: HashMap<&'a str, usize>,
    label_for: HashMap<&'a str, usize>,
    children: Vec<Vec<usize>>,
}

impl<'a> FrameIndex<'a> {
    #[must_use]
    pub fn build(frame: &'a RawFrame) -> Self {
        let mut by_id: HashMap<&'a str, usize> = HashMap::new();
        let mut label_for: HashMap<&'a str, usize> = HashMap::new();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); frame.nodes.len()];
        for (i, node) in frame.nodes.iter().enumerate() {
            if let Some((_, id)) = node
                .attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("id"))
            {
                by_id.entry(id.as_str()).or_insert(i);
            }
            if node.tag_lower() == "label" {
                if let Some((_, target)) = node
                    .attrs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("for"))
                {
                    label_for.entry(target.as_str()).or_insert(i);
                }
            }
            // Only backwards edges are trusted; a forward or out-of-range
            // parent is dropped rather than guessed (see `build`).
            if let Some(p) = node.parent {
                if p < i {
                    children[p].push(i);
                }
            }
        }
        Self {
            frame,
            by_id,
            label_for,
            children,
        }
    }

    #[must_use]
    pub fn children_of(&self, index: usize) -> &[usize] {
        self.children.get(index).map_or(&[], Vec::as_slice)
    }

    /// The ancestor chain of `index`, nearest first. Bounded by the node
    /// count, so a malformed `parent` chain cannot spin here.
    #[must_use]
    pub fn ancestors(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cursor = index;
        for _ in 0..self.frame.nodes.len() {
            let Some(parent) = self.frame.nodes[cursor].parent else {
                break;
            };
            if parent >= cursor {
                break;
            }
            out.push(parent);
            cursor = parent;
        }
        out
    }
}

/// The accessible name of `node_index` in `fx`'s frame.
#[must_use]
pub fn accessible_name(node_index: usize, fx: &FrameIndex<'_>) -> String {
    let Some(node) = fx.frame.nodes.get(node_index) else {
        return String::new();
    };
    let tag = node.tag_lower();

    // 1. aria-labelledby
    if let Some(list) = node.attr("aria-labelledby") {
        let joined: Vec<String> = list
            .split_whitespace()
            .filter_map(|id| fx.by_id.get(id).copied())
            .map(|i| descendant_text(i, fx))
            .filter(|s| !s.is_empty())
            .collect();
        if !joined.is_empty() {
            return cap(&joined.join(" "));
        }
        // A dangling reference is an author mistake, not a decision to have no
        // name: fall through rather than returning "".
    }

    // 2. aria-label
    if let Some(v) = node.attr("aria-label") {
        let v = normalize(v);
        if !v.is_empty() {
            return cap(&v);
        }
    }

    // 3. <label for> / wrapping <label>
    if let Some(id) = node.attr("id") {
        if let Some(&label) = fx.label_for.get(id) {
            let v = descendant_text(label, fx);
            if !v.is_empty() {
                return cap(&v);
            }
        }
    }
    for ancestor in fx.ancestors(node_index) {
        if fx.frame.nodes[ancestor].tag_lower() == "label" {
            let v = descendant_text(ancestor, fx);
            if !v.is_empty() {
                return cap(&v);
            }
            break;
        }
    }

    // 4. <input type=button|submit|reset> value
    if tag == "input" {
        let ty = node.attr("type").unwrap_or("text").to_ascii_lowercase();
        if matches!(ty.as_str(), "button" | "submit" | "reset") {
            if let Some(v) = node.attr("value") {
                let v = normalize(v);
                if !v.is_empty() {
                    return cap(&v);
                }
            }
        }
    }

    // 5. placeholder
    if let Some(v) = node.attr("placeholder") {
        let v = normalize(v);
        if !v.is_empty() {
            return cap(&v);
        }
    }

    // 6. alt — on an <img>, even an empty one decides.
    if let Some(v) = node.attr("alt") {
        let v = normalize(v);
        if !v.is_empty() || tag == "img" {
            return cap(&v);
        }
    }

    // 7. title
    if let Some(v) = node.attr("title") {
        let v = normalize(v);
        if !v.is_empty() {
            return cap(&v);
        }
    }

    // 8. visible descendant text — for the roles that take their name from
    // their contents, and no others. A landmark named by everything inside it
    // is a name nobody can use, and it turns every `<header>` into a rendered
    // line quoting its whole nav bar.
    if super::roles::role_for(&tag, &node.attrs).supports_name_from_content() {
        return cap(&descendant_text(node_index, fx));
    }
    String::new()
}

/// The visible text under `index`, in document order, whitespace-collapsed.
///
/// "Visible" is `build::visibility_of` — deliberately the same derivation the
/// tree uses, not a second one: a name built from text the tree calls
/// invisible is a name for something the user cannot read.
fn descendant_text(index: usize, fx: &FrameIndex<'_>) -> String {
    let mut out = String::new();
    let mut stack = vec![index];
    let mut budget = fx.frame.nodes.len();
    while let Some(i) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let node = &fx.frame.nodes[i];
        if node.kind == RawNodeKind::Text {
            if super::build::visibility_of(node) {
                if let Some(t) = &node.text {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(t);
                }
            }
            continue;
        }
        // Push children in reverse so the pop order is document order.
        for &child in fx.children_of(i).iter().rev() {
            stack.push(child);
        }
    }
    normalize(&out)
}

/// Collapse every run of whitespace (newlines and tabs included) to one space
/// and trim. Doing it here means the renderer never has to repair a name.
#[must_use]
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to [`NAME_MAX_CHARS`] characters on a char boundary, marking the
/// cut. Chars, never bytes (P7).
fn cap(s: &str) -> String {
    match s.char_indices().nth(NAME_MAX_CHARS) {
        Some((idx, _)) => {
            let mut out = s[..idx].to_string();
            out.push('…');
            out
        }
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::raw::{RawFrame, RawNode, RawNodeKind, Rect};
    use super::*;

    fn rect() -> Option<Rect> {
        Some(Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        })
    }

    fn el(backend: u64, parent: Option<usize>, tag: &str, attrs: &[(&str, &str)]) -> RawNode {
        RawNode {
            backend_node_id: backend,
            parent,
            kind: RawNodeKind::Element,
            tag: Some(tag.to_string()),
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            text: None,
            rect: rect(),
            computed: None,
            clickable_hint: None,
            focused: None,
        }
    }

    fn txt(backend: u64, parent: usize, text: &str) -> RawNode {
        RawNode {
            backend_node_id: backend,
            parent: Some(parent),
            kind: RawNodeKind::Text,
            tag: None,
            attrs: vec![],
            text: Some(text.to_string()),
            rect: rect(),
            computed: None,
            clickable_hint: None,
            focused: None,
        }
    }

    fn frame(nodes: Vec<RawNode>) -> RawFrame {
        RawFrame {
            frame_id: "F".into(),
            loader_id: "L".into(),
            offset: (0, 0),
            nodes,
        }
    }

    fn name_of(f: &RawFrame, index: usize) -> String {
        accessible_name(index, &FrameIndex::build(f))
    }

    /// Rule 1 beats rule 2: `aria-labelledby` wins over `aria-label`, and
    /// concatenates its referenced nodes in the order the attribute writes
    /// them — not in document order.
    #[test]
    fn labelledby_wins_over_aria_label_and_joins_in_attribute_order() {
        let f = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "span", &[("id", "b")]),
            txt(3, 1, "World"),
            el(4, Some(0), "span", &[("id", "a")]),
            txt(5, 3, "Hello"),
            el(
                6,
                Some(0),
                "button",
                &[("aria-labelledby", "a b"), ("aria-label", "ignored")],
            ),
        ]);
        assert_eq!(name_of(&f, 5), "Hello World");
    }

    /// …and an `aria-labelledby` whose ids resolve to NOTHING falls through
    /// instead of producing an empty name. A dangling reference is an author
    /// mistake, not a decision to leave the control nameless.
    #[test]
    fn a_dangling_labelledby_falls_through_to_the_next_rule() {
        let f = frame(vec![
            el(1, None, "div", &[]),
            el(
                2,
                Some(0),
                "button",
                &[("aria-labelledby", "nope"), ("aria-label", "Save")],
            ),
        ]);
        assert_eq!(name_of(&f, 1), "Save");
    }

    /// Rule 2 beats rule 3: `aria-label` wins over a `<label for>`.
    #[test]
    fn aria_label_wins_over_a_label_element() {
        let f = frame(vec![
            el(1, None, "form", &[]),
            el(2, Some(0), "label", &[("for", "q")]),
            txt(3, 1, "Query"),
            el(
                4,
                Some(0),
                "input",
                &[
                    ("id", "q"),
                    ("type", "text"),
                    ("aria-label", "Search the site"),
                ],
            ),
        ]);
        assert_eq!(name_of(&f, 3), "Search the site");
    }

    /// Rule 3 beats rule 5, in both of the shapes an HTML form actually uses:
    /// an explicit `<label for>` and a wrapping `<label>`.
    #[test]
    fn label_for_and_wrapping_label_both_win_over_placeholder() {
        let explicit = frame(vec![
            el(1, None, "form", &[]),
            el(2, Some(0), "label", &[("for", "q")]),
            txt(3, 1, "Query"),
            el(
                4,
                Some(0),
                "input",
                &[("id", "q"), ("type", "text"), ("placeholder", "type here")],
            ),
        ]);
        assert_eq!(name_of(&explicit, 3), "Query");

        let wrapping = frame(vec![
            el(1, None, "form", &[]),
            el(2, Some(0), "label", &[]),
            txt(3, 1, "Email"),
            el(
                4,
                Some(1),
                "input",
                &[("type", "text"), ("placeholder", "you@example.com")],
            ),
        ]);
        assert_eq!(name_of(&wrapping, 3), "Email");
    }

    /// Rule 4 beats rule 5: an `<input type=submit>` is named by its `value`.
    /// Without this the busiest button on most forms is nameless — and a text
    /// input's `value` is CONTENT, not a name, which is the counter-case.
    #[test]
    fn a_submit_input_is_named_by_its_value_and_a_text_input_is_not() {
        let f = frame(vec![
            el(1, None, "form", &[]),
            el(
                2,
                Some(0),
                "input",
                &[
                    ("type", "submit"),
                    ("value", "Go"),
                    ("placeholder", "unused"),
                ],
            ),
            el(
                3,
                Some(0),
                "input",
                &[
                    ("type", "text"),
                    ("value", "typed"),
                    ("placeholder", "Search"),
                ],
            ),
        ]);
        assert_eq!(name_of(&f, 1), "Go");
        assert_eq!(name_of(&f, 2), "Search");
    }

    /// Rule 6 beats rule 7, and `alt=""` on an image is an explicit decision
    /// to have no name: it stops the chain rather than falling through to
    /// `title`.
    #[test]
    fn alt_wins_over_title_and_an_empty_alt_is_a_decision() {
        let f = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "img", &[("alt", "Logo"), ("title", "ignored")]),
            el(3, Some(0), "img", &[("alt", ""), ("title", "decorative")]),
            el(4, Some(0), "abbr", &[("title", "World Wide Web")]),
        ]);
        assert_eq!(name_of(&f, 1), "Logo");
        assert_eq!(name_of(&f, 2), "");
        assert_eq!(name_of(&f, 3), "World Wide Web");
    }

    /// Rule 8 applies to roles that take their name from their contents, and
    /// to nothing else.
    ///
    /// Without the gate the fixture's `<header>` is named `"Hacker News"` by
    /// its first link, `anchor_flags` then makes it a printed node, and the
    /// golden's `- banner` line becomes `- banner "Hacker News"`. On the real
    /// Hacker News it would be named by the entire nav bar.
    #[test]
    fn a_landmark_is_not_named_by_everything_inside_it() {
        let f = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "header", &[]),
            el(3, Some(1), "a", &[("href", "/")]),
            txt(4, 2, "Hacker News"),
            el(5, Some(0), "main", &[]),
            txt(6, 4, "body text"),
        ]);
        assert_eq!(name_of(&f, 1), "", "a banner is not named by its contents");
        assert_eq!(name_of(&f, 4), "", "neither is a main landmark");
        assert_eq!(
            name_of(&f, 2),
            "Hacker News",
            "but the link inside it still is — the gate is per role, not global"
        );
        // An explicit label beats the gate: it is rule 2, not rule 8.
        let labelled = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "header", &[("aria-label", "Site header")]),
            txt(3, 1, "ignored"),
        ]);
        assert_eq!(name_of(&labelled, 1), "Site header");
    }

    /// Rule 8 is last, is built from VISIBLE descendants only, collapses
    /// whitespace, and is capped at `NAME_MAX_CHARS`.
    #[test]
    fn descendant_text_is_the_last_resort_and_is_bounded() {
        let mut hidden = txt(4, 1, "SHOULD NOT APPEAR");
        hidden.rect = None;
        let f = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "button", &[]),
            txt(3, 1, "  Sign\n  in  "),
            hidden,
        ]);
        assert_eq!(name_of(&f, 1), "Sign in");

        let long = "x".repeat(NAME_MAX_CHARS + 40);
        let f2 = frame(vec![
            el(1, None, "div", &[]),
            el(2, Some(0), "button", &[]),
            txt(3, 1, &long),
        ]);
        let name = name_of(&f2, 1);
        assert_eq!(
            name.chars().count(),
            NAME_MAX_CHARS + 1,
            "the cap plus the ellipsis that marks the cut"
        );
        assert!(name.ends_with('…'));
    }
}

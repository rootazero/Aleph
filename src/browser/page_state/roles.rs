//! Role mapping and the interactivity hint (spec §4.1).
//!
//! An explicit `role` attribute wins; otherwise the tag decides. The table is
//! a list, and a list only covers the world of the day it was written
//! (判据 §5) — which is exactly why `is_interactive` is SIX signals rather
//! than "is the role in this set". A tag this table has never heard of can
//! still be clickable, and the model is told so.

use super::raw::{attr_of, RawNode};
use super::Role;

/// An ARIA `role` token, or `None` when the token is one this build does not
/// model. `None` means "fall through to the tag", never "generic": an unknown
/// token is not a reason to throw away what the tag already said.
#[must_use]
pub fn role_from_aria(token: &str) -> Option<Role> {
    let t = token.trim().to_ascii_lowercase();
    Some(match t.as_str() {
        "link" => Role::Link,
        "button" => Role::Button,
        "textbox" | "searchbox" => Role::Textbox,
        "checkbox" | "switch" => Role::Checkbox,
        "radio" => Role::Radio,
        "combobox" => Role::Combobox,
        "listbox" => Role::Listbox,
        "option" => Role::Option,
        "menu" | "menubar" => Role::Menu,
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => Role::MenuItem,
        "tab" => Role::Tab,
        "heading" => Role::Heading,
        "img" | "image" | "figure" => Role::Image,
        "banner" => Role::Banner,
        "main" => Role::Main,
        "navigation" => Role::Navigation,
        "contentinfo" => Role::Contentinfo,
        "list" => Role::List,
        "listitem" => Role::ListItem,
        "table" | "grid" => Role::Table,
        "row" => Role::Row,
        "cell" | "gridcell" | "columnheader" | "rowheader" => Role::Cell,
        "article" => Role::Article,
        "form" | "search" => Role::Form,
        "dialog" | "alertdialog" => Role::Dialog,
        // An author saying "this markup carries no semantics" is a DECISION,
        // not an unknown, so it maps rather than falling through.
        "presentation" | "none" | "generic" => Role::Generic,
        _ => return None,
    })
}

/// The role of an element with `tag` and `attrs`.
#[must_use]
pub fn role_for(tag: &str, attrs: &[(String, String)]) -> Role {
    if let Some(explicit) = attr_of(attrs, "role").and_then(role_from_aria) {
        return explicit;
    }
    let tag = tag.to_ascii_lowercase();
    match tag.as_str() {
        // An <a> is a link only with an href. Without one it is a named
        // anchor, and calling it a link makes the model click on nothing.
        "a" | "area" => {
            if attr_of(attrs, "href").is_some() {
                Role::Link
            } else {
                Role::Generic
            }
        }
        "button" | "summary" => Role::Button,
        "input" => input_role(attrs),
        "textarea" => Role::Textbox,
        "select" => {
            let multiple = attr_of(attrs, "multiple").is_some();
            let rows = attr_of(attrs, "size")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(1);
            if multiple || rows > 1 {
                Role::Listbox
            } else {
                Role::Combobox
            }
        }
        "option" => Role::Option,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Role::Heading,
        "img" | "svg" | "picture" => Role::Image,
        "header" => Role::Banner,
        "footer" => Role::Contentinfo,
        "nav" => Role::Navigation,
        "main" => Role::Main,
        "ul" | "ol" | "menu" => Role::List,
        "li" => Role::ListItem,
        "table" => Role::Table,
        "tr" => Role::Row,
        "td" | "th" => Role::Cell,
        "article" => Role::Article,
        "form" => Role::Form,
        "dialog" => Role::Dialog,
        _ => Role::Generic,
    }
}

/// `<input>` is nine controls wearing one tag.
///
/// `range`, `color`, `file` and the date family land on `Textbox` because this
/// build has no separate role for them and `Textbox` is the role whose verbs
/// (`fill`, `type_text`, `upload`) are the ones a caller would reach for.
/// Named here rather than left to look deliberate.
fn input_role(attrs: &[(String, String)]) -> Role {
    let ty = attr_of(attrs, "type")
        .unwrap_or("text")
        .trim()
        .to_ascii_lowercase();
    match ty.as_str() {
        "checkbox" => Role::Checkbox,
        "radio" => Role::Radio,
        "button" | "submit" | "reset" | "image" => Role::Button,
        "hidden" => Role::Generic,
        _ => Role::Textbox,
    }
}

/// Whether the model should treat this node as something it can act on.
///
/// Six independent signals, OR-ed (spec §4.1). A HINT printed beside the node,
/// not a filter deciding what enters the tree: a name-list filter only covers
/// the world of the day it was written, and the elements it misses are exactly
/// the hand-rolled widgets a page is most likely to have (判据 §5).
#[must_use]
pub fn is_interactive(role: Role, node: &RawNode) -> bool {
    if role.is_interactive_role() {
        return true;
    }
    if node
        .attr("tabindex")
        .and_then(|t| t.trim().parse::<i32>().ok())
        .is_some_and(|t| t >= 0)
    {
        return true;
    }
    if node.has_attr("onclick") {
        return true;
    }
    if node
        .attr("contenteditable")
        .is_some_and(|v| !v.eq_ignore_ascii_case("false"))
    {
        return true;
    }
    if node.computed.is_some_and(|c| c.cursor_pointer) {
        return true;
    }
    node.clickable_hint == Some(true)
}

#[cfg(test)]
mod tests {
    use super::super::raw::{node_with, Computed};
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The explicit `role` attribute wins over the tag, in both directions —
    /// including the direction that DOWNGRADES a semantic tag, which is what
    /// `role="presentation"` exists for.
    #[test]
    fn an_explicit_role_attribute_beats_the_tag_mapping() {
        assert_eq!(role_for("div", &attrs(&[("role", "button")])), Role::Button);
        assert_eq!(role_for("a", &attrs(&[("href", "/x")])), Role::Link);
        assert_eq!(
            role_for("a", &attrs(&[("href", "/x"), ("role", "button")])),
            Role::Button
        );
        assert_eq!(
            role_for("table", &attrs(&[("role", "presentation")])),
            Role::Generic
        );
        // An unknown ARIA token is not a reason to throw away what the tag
        // already said.
        assert_eq!(
            role_for("button", &attrs(&[("role", "quux")])),
            Role::Button
        );
    }

    /// `<a>` without `href` is not a link. The commonest false positive in a
    /// hand-rolled role table, and the model pays for it by clicking something
    /// that navigates nowhere.
    #[test]
    fn an_anchor_without_href_is_not_a_link() {
        assert_eq!(role_for("a", &attrs(&[])), Role::Generic);
        assert_eq!(role_for("a", &attrs(&[("name", "top")])), Role::Generic);
        assert_eq!(role_for("a", &attrs(&[("href", "")])), Role::Link);
    }

    /// `<input>` is nine controls wearing one tag.
    #[test]
    fn input_type_decides_the_role() {
        for (ty, expected) in [
            ("checkbox", Role::Checkbox),
            ("radio", Role::Radio),
            ("submit", Role::Button),
            ("button", Role::Button),
            ("reset", Role::Button),
            ("image", Role::Button),
            ("hidden", Role::Generic),
            ("text", Role::Textbox),
            ("password", Role::Textbox),
            ("search", Role::Textbox),
            ("file", Role::Textbox),
        ] {
            assert_eq!(
                role_for("input", &attrs(&[("type", ty)])),
                expected,
                "input[type={ty}]"
            );
        }
        // No `type` at all is `text`, per HTML.
        assert_eq!(role_for("input", &attrs(&[])), Role::Textbox);
        // Case-insensitive, like the HTML parser.
        assert_eq!(
            role_for("INPUT", &attrs(&[("type", "CheckBox")])),
            Role::Checkbox
        );
    }

    /// `<select>` is a combobox until it shows more than one row.
    #[test]
    fn select_is_a_listbox_only_when_it_shows_more_than_one_row() {
        assert_eq!(role_for("select", &attrs(&[])), Role::Combobox);
        assert_eq!(role_for("select", &attrs(&[("size", "1")])), Role::Combobox);
        assert_eq!(role_for("select", &attrs(&[("size", "4")])), Role::Listbox);
        assert_eq!(
            role_for("select", &attrs(&[("multiple", "")])),
            Role::Listbox
        );
    }

    /// Every landmark and structural tag in the table, so a deletion is a
    /// failing assertion rather than a silently flatter tree.
    #[test]
    fn the_structural_tag_table_is_complete() {
        for (tag, expected) in [
            ("header", Role::Banner),
            ("footer", Role::Contentinfo),
            ("nav", Role::Navigation),
            ("main", Role::Main),
            ("article", Role::Article),
            ("form", Role::Form),
            ("dialog", Role::Dialog),
            ("ul", Role::List),
            ("ol", Role::List),
            ("menu", Role::List),
            ("li", Role::ListItem),
            ("table", Role::Table),
            ("tr", Role::Row),
            ("td", Role::Cell),
            ("th", Role::Cell),
            ("h1", Role::Heading),
            ("h6", Role::Heading),
            ("img", Role::Image),
            ("svg", Role::Image),
            ("button", Role::Button),
            ("summary", Role::Button),
            ("textarea", Role::Textbox),
            ("option", Role::Option),
            ("span", Role::Generic),
            ("div", Role::Generic),
        ] {
            assert_eq!(role_for(tag, &attrs(&[])), expected, "<{tag}>");
        }
    }

    /// Interactivity is a HINT built from six independent signals, not a
    /// filter derived from the role alone (spec §4.1). Every signal is
    /// asserted on a node whose role is `Generic`, so only that signal can be
    /// responsible for the answer.
    #[test]
    fn interactivity_is_multi_signal_and_each_signal_stands_alone() {
        let plain = node_with("div", &[]);
        assert!(!is_interactive(Role::Generic, &plain));

        assert!(is_interactive(Role::Link, &plain), "role");
        assert!(
            is_interactive(Role::Generic, &node_with("div", &[("tabindex", "0")])),
            "tabindex=0"
        );
        assert!(
            !is_interactive(Role::Generic, &node_with("div", &[("tabindex", "-1")])),
            "tabindex=-1 is reachable by script, not by the user"
        );
        assert!(
            is_interactive(Role::Generic, &node_with("div", &[("onclick", "f()")])),
            "onclick"
        );
        assert!(
            is_interactive(
                Role::Generic,
                &node_with("div", &[("contenteditable", "true")])
            ),
            "contenteditable"
        );
        assert!(
            !is_interactive(
                Role::Generic,
                &node_with("div", &[("contenteditable", "false")])
            ),
            "contenteditable=false is an explicit no"
        );

        let mut pointer = node_with("div", &[]);
        pointer.computed = Some(Computed {
            display_none: false,
            visibility_hidden: false,
            opacity_zero: false,
            cursor_pointer: true,
        });
        assert!(is_interactive(Role::Generic, &pointer), "cursor:pointer");

        let mut hinted = node_with("div", &[]);
        hinted.clickable_hint = Some(true);
        assert!(is_interactive(Role::Generic, &hinted), "isClickable");
    }

    /// `Role::as_str` and the serde name are two faces of one fact, derived
    /// independently (a `match` and a `rename_all`). Pin them against each
    /// other rather than against a third typed-out list (判据 §9).
    #[test]
    fn every_role_renders_the_same_word_it_serialises_as() {
        for role in Role::ALL {
            let json = serde_json::to_string(&role).expect("Role serialises");
            assert_eq!(
                json,
                format!("\"{}\"", role.as_str()),
                "{role:?} renders and serialises differently"
            );
        }

        // `Role::ALL` is walked out of the exhaustive `Role::next` chain and
        // its LENGTH is checked at compile time by that const walk, so there is
        // no literal count left to assert here — the old
        // `assert_eq!(Role::ALL.len(), 27)` asserted a hand-written array
        // against a hand-written number and covered no new variant at all.
        //
        // What a runtime check still adds is the two ways the chain can be
        // mis-linked without changing its length: a variant visited twice
        // (which drops another), and two variants printing the same word —
        // which would make two different nodes indistinguishable in the text
        // tree and in the JSON alike.
        let visited: std::collections::HashSet<Role> = Role::ALL.into_iter().collect();
        assert_eq!(
            visited.len(),
            Role::ALL.len(),
            "the Role::next chain visits a variant twice, so it misses another"
        );
        let words: std::collections::HashSet<&str> = Role::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            words.len(),
            Role::ALL.len(),
            "two roles print the same word, so the tree cannot tell them apart"
        );
    }
}

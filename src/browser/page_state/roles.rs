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
        other => tag_role(other).unwrap_or(Role::Generic),
    }
}

/// Every tag whose role is a **constant** — the whole mapping, as data.
///
/// A table rather than more `match` arms, and the reason is a guard rather
/// than taste. `the_structural_tag_table_is_complete` used to enumerate its own
/// 23 tags, so it covered 23 of the 32 keys this file routes and its name said
/// otherwise: deleting `"h2" | "h3" | "h4" | "h5"` from the heading arm left it
/// green while every real page's structural backbone went flat (判据 §3, §5).
///
/// A `match` over `&str` cannot be made exhaustive, so the only way a test can
/// notice a deleted key is for the key set to be a **value it can read**. That
/// is this. The four tags whose role depends on their attributes — `a`, `area`,
/// `input`, `select` — cannot live here and keep their own dedicated tests.
const SIMPLE_TAGS: &[(&str, Role)] = &[
    ("button", Role::Button),
    ("summary", Role::Button),
    ("textarea", Role::Textbox),
    ("option", Role::Option),
    ("h1", Role::Heading),
    ("h2", Role::Heading),
    ("h3", Role::Heading),
    ("h4", Role::Heading),
    ("h5", Role::Heading),
    ("h6", Role::Heading),
    ("img", Role::Image),
    ("svg", Role::Image),
    ("picture", Role::Image),
    ("header", Role::Banner),
    ("footer", Role::Contentinfo),
    ("nav", Role::Navigation),
    ("main", Role::Main),
    ("ul", Role::List),
    ("ol", Role::List),
    ("menu", Role::List),
    ("li", Role::ListItem),
    ("table", Role::Table),
    ("tr", Role::Row),
    ("td", Role::Cell),
    ("th", Role::Cell),
    ("article", Role::Article),
    ("form", Role::Form),
    ("dialog", Role::Dialog),
];

/// [`SIMPLE_TAGS`] lookup. `None` is "this build models no role for that tag",
/// which [`role_for`] spends as `Generic` — never as "not an element".
fn tag_role(tag: &str) -> Option<Role> {
    SIMPLE_TAGS
        .iter()
        .find(|(k, _)| *k == tag)
        .map(|(_, role)| *role)
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

        // `<area>` shares the arm and had no assertion anywhere: it is
        // attribute-dependent, so it cannot live in `SIMPLE_TAGS`, and the
        // table guard's absence check passes whether the match still routes it
        // or not. Deleting `"area"` from the arm was green until this line —
        // measured, and the last of the six keys the review counted uncovered.
        assert_eq!(role_for("area", &attrs(&[("href", "/x")])), Role::Link);
        assert_eq!(role_for("area", &attrs(&[])), Role::Generic);
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

    /// Every constant-role tag, derived from the production table rather than
    /// re-typed beside it.
    ///
    /// The old version of this test enumerated its own 23 tags and was named
    /// "complete". It covered 23 of the 32 keys `role_for` routes, and deleting
    /// `"picture"`, `"area"` and `"h2" | "h3" | "h4" | "h5"` from the production
    /// arms left it **green** — measured. `<h2>`–`<h5>` quietly becoming
    /// `Generic` flattens the structural backbone of nearly every real page.
    ///
    /// A `match` over `&str` cannot be exhaustive, so "a deleted arm cannot
    /// pass" needs the key set to be readable data — `SIMPLE_TAGS` — plus ONE
    /// hand-written expectation of what that data should say. Any add, delete
    /// or remap reddens the first assertion with a readable diff; the second
    /// proves the table is what `role_for` actually consults rather than dead
    /// data beside a surviving match (判据 §7).
    ///
    /// **Its scope is the table, not `role_for`** — a constant-role tag added
    /// as a `match` arm instead of a row is invisible here, measured. That half
    /// belongs to
    /// `role_for_routes_no_tag_by_hand_except_the_attribute_dependent_ones`
    /// below, and the two together are what "complete" means.
    #[test]
    fn the_structural_tag_table_is_complete() {
        let actual: Vec<String> = SIMPLE_TAGS
            .iter()
            .map(|(tag, role)| format!("{tag}={}", role.as_str()))
            .collect();
        let expected = [
            "button=button",
            "summary=button",
            "textarea=textbox",
            "option=option",
            "h1=heading",
            "h2=heading",
            "h3=heading",
            "h4=heading",
            "h5=heading",
            "h6=heading",
            "img=image",
            "svg=image",
            "picture=image",
            "header=banner",
            "footer=contentinfo",
            "nav=navigation",
            "main=main",
            "ul=list",
            "ol=list",
            "menu=list",
            "li=listitem",
            "table=table",
            "tr=row",
            "td=cell",
            "th=cell",
            "article=article",
            "form=form",
            "dialog=dialog",
        ];
        assert_eq!(actual, expected, "the tag table changed");

        // Not dead data: every key routes through `role_for`.
        for (tag, role) in SIMPLE_TAGS {
            assert_eq!(role_for(tag, &attrs(&[])), *role, "<{tag}>");
        }

        // The four attribute-dependent tags are deliberately absent — their
        // role is not a constant, and each has its own test above.
        for tag in ["a", "area", "input", "select"] {
            assert!(
                !SIMPLE_TAGS.iter().any(|(k, _)| *k == tag),
                "<{tag}>'s role depends on its attributes and cannot be a \
                 constant in this table"
            );
        }

        // A tag the table has never heard of is `Generic`, not a panic and not
        // an absence.
        assert_eq!(role_for("span", &attrs(&[])), Role::Generic);
        assert_eq!(role_for("blink", &attrs(&[])), Role::Generic);
    }

    /// **That guard is complete over the TABLE. This one is complete over
    /// `role_for`.**
    ///
    /// A tag whose role is a constant belongs in `SIMPLE_TAGS`, where a
    /// deletion is visible because the keys are data a test can read. Nothing
    /// stopped the next one being written as a `match` arm instead —
    /// **measured at `3398e90e2`**: adding `"aside" => Role::Article,` to
    /// `role_for` left the whole module suite at **44 passed / 0 failed**, and
    /// the guard whose name says "complete" is one of those 44. It is the same
    /// gap that guard was written to close, one size smaller (判据 §3: a guard
    /// is worth exactly its scope, and its NAME is not its scope).
    ///
    /// A `match` over `&str` cannot be made exhaustive, so completeness over
    /// `role_for` has to be asserted over its **source**: the literals its body
    /// mentions, against a written-down list of the ones it may route by hand.
    /// Those are the four attribute-dependent tags plus the three that also sit
    /// in the table, and the attribute names the rules read. A new literal —
    /// `"aside"`, or an attribute for a new rule — reddens this with a readable
    /// diff, and the author either moves the tag into the table or re-rules
    /// this list deliberately.
    ///
    /// `code_keeping_literals`, not `code_text`: the latter strips string
    /// payloads, which is exactly what this census counts. Comment stripping
    /// comes from the lexer so a `//` inside a literal cannot cut a line short.
    #[test]
    fn role_for_routes_no_tag_by_hand_except_the_attribute_dependent_ones() {
        use crate::utils::source_scan::{code_keeping_literals, production_text};

        let rel = "src/browser/page_state/roles.rs";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let src = std::fs::read_to_string(&path).expect("roles.rs is readable");
        let code = code_keeping_literals(&production_text(std::path::Path::new(rel), &src));

        let start = code
            .find("pub fn role_for(")
            .expect("the scan is broken, not the tree: role_for is in this file");
        let end = start
            + code[start..]
                .find("\n}")
                .expect("role_for's body closes at column 0");
        let body = &code[start..end];

        let mut found: Vec<String> = Vec::new();
        let mut rest = body;
        while let Some(open) = rest.find('"') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            found.push(after[..close].to_string());
            rest = &after[close + 1..];
        }
        found.sort();
        found.dedup();

        let mut allowed: Vec<String> = [
            // Tags whose role is NOT a constant — each has its own test above.
            "a", "area", "input", "select",
            // Constant-role tags that also sit in `SIMPLE_TAGS`; the table
            // guard's `assert_eq!(role_for(tag), *role)` half pins these two
            // spellings against each other in both directions.
            "button", "summary", "textarea",
            // Attribute names the attribute-dependent rules read.
            "role", "href", "multiple", "size",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        allowed.sort();

        // Non-vacuity FIRST, so a slice that lost the function reports THAT
        // rather than a diff against an empty set (判据 §2).
        assert!(
            found.len() > 8 && found.contains(&"select".to_string()),
            "the scan, not the tree: the slice yielded {} literals ({found:?}), \
             so it is reading something other than `role_for`'s body",
            found.len()
        );
        assert_eq!(
            found, allowed,
            "the literals `role_for` routes by hand are no longer the ones \
             written down here. If a tag was ADDED to the match: its role is a \
             constant, so it belongs in SIMPLE_TAGS where the completeness \
             guard can see it — a match arm is invisible there however much \
             that guard's name promises. If one was REMOVED: re-rule this list \
             on purpose."
        );
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

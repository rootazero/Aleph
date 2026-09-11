//! The page state tree: `RawDom` → `PageState` → text + JSON (spec §4).

pub mod accname;
pub mod build;
pub mod raw;
pub mod refs;
pub mod render;
pub mod roles;

use serde::{Deserialize, Serialize};

use crate::browser::engine::Engine;

pub use accname::{accessible_name, AccName, FrameIndex, NAME_MAX_CHARS};
pub use raw::{attr_of, Computed, RawDom, RawFrame, RawNode, RawNodeKind, Rect, Viewport};
pub use refs::{FrameKey, RefEntry, RefId, RefKey, RefTable, StaleReason};
pub use render::{quote, render_text, rendered_indices, to_json, TEXT_MAX_CHARS};
pub use roles::{is_interactive, role_for, role_from_aria};

/// The roles this build models. A closed set on purpose: an open one would
/// make the renderer's vocabulary a function of whatever a page wrote in a
/// `role=` attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Link,
    Button,
    Textbox,
    Checkbox,
    Radio,
    Combobox,
    Listbox,
    Option,
    Menu,
    MenuItem,
    Tab,
    Heading,
    Image,
    Banner,
    Main,
    Navigation,
    Contentinfo,
    List,
    ListItem,
    Table,
    Row,
    Cell,
    Article,
    Form,
    Dialog,
    Generic,
    Text,
}

impl Role {
    /// How many variants there are.
    ///
    /// The one literal in this construction, and it is **checked by the
    /// compiler**: [`Self::walk`] is const-evaluated to build [`Self::ALL`],
    /// and it panics — a hard compile error in a `const` initializer — if the
    /// chain [`Self::next`] describes is any other length.
    ///
    /// `pub(crate)`: it exists to size `ALL` and to make that check possible,
    /// and nothing outside this crate reads it. A caller wanting the count has
    /// `Role::ALL.len()`, which cannot drift from the array it describes (P5).
    pub(crate) const COUNT: usize = 27;

    /// Where [`Self::ALL`] starts.
    const FIRST: Role = Role::Link;

    /// The variant after `self`, or `None` at the end of [`Self::ALL`].
    ///
    /// **Exhaustive, with no `_` arm**, which is the whole point and the same
    /// idiom `spec_7_1_signal` uses in [`crate::browser::error`]: a new variant
    /// does not compile (`E0004`) until someone says where it sits, and saying
    /// where it sits is what puts it in `ALL`. `ALL` used to be a hand-written
    /// array whose own doc claimed it was not one — nothing tied it to the
    /// enum, so a 28th variant was simply never iterated and the guard that
    /// pins [`Self::as_str`] against the serde name silently stopped covering
    /// it (判据 §5).
    ///
    /// **The hole this does NOT close**, said out loud because a guard is worth
    /// exactly its scope (判据 §3): an author who answers the forced arm with
    /// `Role::New => None` instead of linking it in leaves a second terminator,
    /// and the chain from [`Self::FIRST`] still ends where it did. **Measured,
    /// not assumed** — that mutation was run and the suite stayed green.
    /// Nothing on stable can force reachability (`variant_count` is nightly, a
    /// derive macro is a dependency R3 would ask about). What IS forced is that
    /// the author must write *an* answer, and the natural answer — append, then
    /// bump `COUNT` when the compiler tells you to — is the correct one.
    const fn next(self) -> Option<Self> {
        match self {
            Role::Link => Some(Role::Button),
            Role::Button => Some(Role::Textbox),
            Role::Textbox => Some(Role::Checkbox),
            Role::Checkbox => Some(Role::Radio),
            Role::Radio => Some(Role::Combobox),
            Role::Combobox => Some(Role::Listbox),
            Role::Listbox => Some(Role::Option),
            Role::Option => Some(Role::Menu),
            Role::Menu => Some(Role::MenuItem),
            Role::MenuItem => Some(Role::Tab),
            Role::Tab => Some(Role::Heading),
            Role::Heading => Some(Role::Image),
            Role::Image => Some(Role::Banner),
            Role::Banner => Some(Role::Main),
            Role::Main => Some(Role::Navigation),
            Role::Navigation => Some(Role::Contentinfo),
            Role::Contentinfo => Some(Role::List),
            Role::List => Some(Role::ListItem),
            Role::ListItem => Some(Role::Table),
            Role::Table => Some(Role::Row),
            Role::Row => Some(Role::Cell),
            Role::Cell => Some(Role::Article),
            Role::Article => Some(Role::Form),
            Role::Form => Some(Role::Dialog),
            Role::Dialog => Some(Role::Generic),
            Role::Generic => Some(Role::Text),
            Role::Text => None,
        }
    }

    /// Every variant, walked out of [`Self::next`] rather than typed out.
    pub const ALL: [Role; Self::COUNT] = Self::walk();

    /// [`Self::ALL`]'s body. Const-evaluated, so both panics below are compile
    /// errors and [`Self::COUNT`] cannot drift from the chain.
    const fn walk() -> [Role; Self::COUNT] {
        let mut out = [Role::Link; Self::COUNT];
        let mut i = 0;
        let mut cur = Self::FIRST;
        loop {
            out[i] = cur;
            i += 1;
            match cur.next() {
                Some(n) => cur = n,
                None => break,
            }
            if i == Self::COUNT {
                panic!("Role::COUNT is smaller than the chain Role::next walks — bump it");
            }
        }
        if i != Self::COUNT {
            panic!("Role::COUNT is larger than the chain Role::next walks");
        }
        out
    }

    /// The word the renderer prints. Pinned against the serde name by
    /// `every_role_renders_the_same_word_it_serialises_as`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Link => "link",
            Role::Button => "button",
            Role::Textbox => "textbox",
            Role::Checkbox => "checkbox",
            Role::Radio => "radio",
            Role::Combobox => "combobox",
            Role::Listbox => "listbox",
            Role::Option => "option",
            Role::Menu => "menu",
            Role::MenuItem => "menuitem",
            Role::Tab => "tab",
            Role::Heading => "heading",
            Role::Image => "image",
            Role::Banner => "banner",
            Role::Main => "main",
            Role::Navigation => "navigation",
            Role::Contentinfo => "contentinfo",
            Role::List => "list",
            Role::ListItem => "listitem",
            Role::Table => "table",
            Role::Row => "row",
            Role::Cell => "cell",
            Role::Article => "article",
            Role::Form => "form",
            Role::Dialog => "dialog",
            Role::Generic => "generic",
            Role::Text => "text",
        }
    }

    /// Whether this role takes its name from its own contents.
    ///
    /// accname's rule 8 (visible descendant text) applies to these and to
    /// nothing else. Without the gate a `<header>` is named by its entire nav
    /// bar and a `<main>` by the whole page — a name that is technically its
    /// content and useless as an identity, and one that then makes the
    /// landmark an "anchor" the renderer prints with a quoted blob.
    ///
    /// The list is ARIA's name-from-content set, narrowed to the roles this
    /// build models. Like every list it only covers the day it was written
    /// (判据 §5), which is survivable here because the failure mode is a
    /// missing name rather than a wrong one.
    #[must_use]
    pub const fn supports_name_from_content(self) -> bool {
        matches!(
            self,
            Role::Link
                | Role::Button
                | Role::Heading
                | Role::Cell
                | Role::ListItem
                | Role::Option
                | Role::MenuItem
                | Role::Tab
                | Role::Checkbox
                | Role::Radio
        )
    }

    /// The roles that are actionable by definition. ONE of the six signals
    /// `roles::is_interactive` ORs together — never the whole answer.
    #[must_use]
    pub const fn is_interactive_role(self) -> bool {
        matches!(
            self,
            Role::Link
                | Role::Button
                | Role::Textbox
                | Role::Checkbox
                | Role::Radio
                | Role::Combobox
                | Role::Listbox
                | Role::Option
                | Role::MenuItem
                | Role::Tab
        )
    }
}

/// The state bits a control carries. `Option` where "not applicable" and
/// "false" are different facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStates {
    pub disabled: bool,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub selected: bool,
    pub required: bool,
    pub readonly: bool,
    /// Does the caret live here? `None` is "no producer has looked", which is
    /// every Chromium capture until Task 17 — see [`raw::RawNode::focused`].
    ///
    /// An `Option` like `checked` and `expanded`, and for the reason this
    /// struct's own doc gives, because the two faces of this field disagree
    /// about the collapse. The text render prints nothing for `None` and
    /// nothing for `Some(false)`, so *there* they are the same observable. The
    /// JSON render is not: this struct has no `skip_serializing_if`, so a
    /// `bool` made `to_json` emit `"focused": false` on every node — a printed
    /// claim that the caret is elsewhere, about something nobody has looked at.
    /// One derivation has to serve both faces (判据 §9), so the distinction is
    /// carried rather than collapsed here and spelled `null` in JSON.
    pub focused: Option<bool>,
}

/// One node of the tree the model reads. `parent` indexes `PageState::nodes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateNode {
    pub parent: Option<usize>,
    pub r#ref: Option<RefId>,
    pub backend_node_id: u64,
    pub frame: FrameKey,
    pub role: Role,
    pub name: String,
    /// Whether [`Self::name`] is this node's **complete** visible descendant
    /// text: [`accname`]'s rule 8 built it, and the cap did not cut it.
    ///
    /// Carried rather than re-derived, because the renderer's absorption rule
    /// needs exactly this fact and the derivation already had it. Asking
    /// `name.contains(text)` instead deleted a `"$29"` leaf under a link named
    /// `"Plans from $29 per month"`; asking only "did rule 8 build it" deleted
    /// every leaf under any capped name. Both directions and the measurements
    /// are on [`accname::AccName::covers_all_text`], which is where the fact is
    /// derived — this field is its carrier and not a second account of it.
    pub name_covers_all_text: bool,
    pub value: Option<String>,
    pub states: NodeStates,
    /// Page coordinates — the frame offset is already applied.
    pub rect: Option<Rect>,
    pub interactive: bool,
    pub visible: bool,
    pub text: Option<String>,
    pub href: Option<String>,
    pub placeholder: Option<String>,
}

/// One capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    pub engine: Engine,
    pub generation: u64,
    pub url: String,
    /// The page's own `<title>`, and therefore **page-controlled** — the sixth
    /// such string this struct carries, after the accessible name, a text leaf,
    /// an `href`, a `placeholder` and `url`.
    ///
    /// `render_text` does not print it, so ruling R40's five quoting sites are
    /// the complete list *for this task's renderer* and no more than that.
    /// Task 14 renders url and title in `browser_snapshot`: **that render goes
    /// through [`render::quote`]**, which is re-exported from this module for
    /// the purpose. A page picks its own title, and a title is as good a place
    /// to forge a `[ref=` token as a placeholder was.
    pub title: String,
    pub viewport: Viewport,
    /// `(nodes with no box, nodes total)` — a runtime fact for the model, not
    /// a verdict about the page (R7).
    pub no_box: (usize, usize),
    /// How long this capture's CDP round trips took, in milliseconds.
    ///
    /// **Not "how long a barrier held us".** Neither fetcher can tell an
    /// obscura barrier apart from ordinary latency, so a `waited=` token would
    /// be a label the number cannot support — and a wrong label costs more
    /// than a missing one (判据 §17). This is elapsed fetch time and says so.
    pub fetch_ms: u64,
    pub nodes: Vec<StateNode>,
}

#[cfg(test)]
mod wire_face {
    //! The keys the model's JSON face is made of, written down.
    //!
    //! `render::to_json` is `serde_json::to_value` of [`PageState`], so every
    //! field of every struct it reaches is a wire key that some consumer — and
    //! after Task 14, a model — reads by name.

    use super::*;

    /// The sorted key set of one serialised value.
    fn keys<T: Serialize>(value: &T) -> Vec<String> {
        let value = serde_json::to_value(value).expect("the wire types serialise");
        let mut out: Vec<String> = value
            .as_object()
            .expect("a wire struct is a JSON object")
            .keys()
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// A written-down key list, sorted so the list's own order is not a
    /// contract.
    fn expected(names: &[&str]) -> Vec<String> {
        assert!(
            !names.is_empty(),
            "non-vacuity: an empty expectation passes against anything that \
             serialises to {{}}"
        );
        let mut out: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
        out.sort();
        out
    }

    fn frame_key() -> FrameKey {
        FrameKey {
            frame_id: "F".into(),
            loader_id: "L".into(),
        }
    }

    fn viewport() -> Viewport {
        Viewport {
            width: 800,
            height: 600,
            scroll_x: 0,
            scroll_y: 0,
            content_width: 800,
            content_height: 600,
            dpr: 1.0,
        }
    }

    /// **Every wire field keeps its key, whatever its value and whatever
    /// anyone writes above it.**
    ///
    /// Three ways a key can leave the model's JSON face, and this has to see
    /// all three:
    ///
    /// 1. **value-dependent** — a `skip_serializing_if` on an `Option` turns
    ///    "we looked and the answer is no" into the same silence as "nobody
    ///    looked". That is the one-line change that undoes the `focused`
    ///    ruling, and `checked`/`expanded` are `Option` for the same reason
    ///    while having no JSON assertion of their own.
    /// 2. **`#[serde(skip)]`** — the key leaves in every state.
    /// 3. **`#[serde(rename)]`** — the key every consumer reads by name
    ///    becomes a different word, and nothing that reads back what it just
    ///    wrote can tell (判据 §10).
    ///
    /// The round-3 version of this guard saw only (1), because it derived
    /// **both** sides from `NodeStates` and compared them to each other: a
    /// change that moves both sides together is invisible to it. Measured — a
    /// `skip` on `checked` and a `rename` to `is_checked` each left it green,
    /// with `checked` gone from the JSON face entirely. That is the tag-table
    /// ruling one file over and not applied here (判据 §16): a test deriving
    /// both halves from production asserts the table against itself. So the
    /// key list below is **written down**, and the struct literals are
    /// **exhaustive** — the first authority catches a rename or a skip, the
    /// second means a new field cannot be added without someone writing it
    /// here.
    ///
    /// **Scope, censused at `63e7a4b78`** (predicate: fields declared on a
    /// struct `to_value(&PageState)` reaches): `PageState` 8, `StateNode` 15,
    /// `NodeStates` 7, `Viewport` 7, `Rect` 4, `FrameKey` 2 — **43 fields, all
    /// six structs, all covered here**. `RefId` is a newtype over `String` and
    /// carries no keys; `Role` and `Engine` are enums whose *vocabulary* is a
    /// different contract (`every_role_renders_the_same_word_it_serialises_as`
    /// pins `Role`'s). `RawNode` and friends are the fetchers' INPUT face, not
    /// this one, and a rename there fails the fixture load — except on
    /// `RawNode::focused`, whose `#[serde(default)]` would absorb it silently;
    /// that face gets its producer in Tasks 11/17 and its guard with it.
    #[test]
    fn every_wire_field_keeps_its_json_key_whatever_its_value() {
        let states = expected(&[
            "disabled", "checked", "expanded", "selected", "required", "readonly", "focused",
        ]);
        assert_eq!(keys(&NodeStates::default()), states, "unset NodeStates");
        assert_eq!(
            keys(&NodeStates {
                disabled: true,
                checked: Some(true),
                expanded: Some(true),
                selected: true,
                required: true,
                readonly: true,
                focused: Some(true),
            }),
            states,
            "a state bit disappears from JSON when it is set — or the key set \
             is no longer the one written down here"
        );

        let node = expected(&[
            "parent",
            "ref",
            "backend_node_id",
            "frame",
            "role",
            "name",
            "name_covers_all_text",
            "value",
            "states",
            "rect",
            "interactive",
            "visible",
            "text",
            "href",
            "placeholder",
        ]);
        let empty = StateNode {
            parent: None,
            r#ref: None,
            backend_node_id: 1,
            frame: frame_key(),
            role: Role::Generic,
            name: String::new(),
            name_covers_all_text: false,
            value: None,
            states: NodeStates::default(),
            rect: None,
            interactive: false,
            visible: true,
            text: None,
            href: None,
            placeholder: None,
        };
        assert_eq!(keys(&empty), node, "a StateNode with nothing known");
        let full = StateNode {
            parent: Some(0),
            r#ref: Some(RefId("e1".into())),
            value: Some("v".into()),
            rect: Some(Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }),
            text: Some("t".into()),
            href: Some("/h".into()),
            placeholder: Some("p".into()),
            ..empty.clone()
        };
        assert_eq!(keys(&full), node, "a StateNode with everything known");

        assert_eq!(
            keys(&PageState {
                engine: Engine::Chromium,
                generation: 1,
                url: "https://x.test/".into(),
                title: "t".into(),
                viewport: viewport(),
                no_box: (0, 1),
                fetch_ms: 1,
                nodes: vec![full],
            }),
            expected(&[
                "engine",
                "generation",
                "url",
                "title",
                "viewport",
                "no_box",
                "fetch_ms",
                "nodes",
            ])
        );

        assert_eq!(
            keys(&viewport()),
            expected(&[
                "width",
                "height",
                "scroll_x",
                "scroll_y",
                "content_width",
                "content_height",
                "dpr",
            ])
        );
        assert_eq!(
            keys(&Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4
            }),
            expected(&["x", "y", "w", "h"])
        );
        assert_eq!(keys(&frame_key()), expected(&["frame_id", "loader_id"]));
    }
}

#[cfg(test)]
mod census {
    /// `PageState::build` is the ONE builder (spec §7.3): both fetchers
    /// produce a `RawDom`, and exactly one place turns it into a `PageState`.
    ///
    /// **"At most one", not "exactly one", and that is deliberate.** At the end
    /// of this task there are zero call sites outside this module — nothing
    /// consumes a page state yet; Task 12's `cdp_backend/snapshot.rs` is what
    /// will. The failure this guard exists for is a SECOND builder appearing
    /// (an action path re-deriving a tree with its own rules), and that is
    /// what it goes red on, today and after Task 12. When there IS one site,
    /// its identity is pinned too, so the single site cannot quietly move
    /// somewhere with different inputs.
    ///
    /// Non-vacuity is asserted first: the pattern must match inside this
    /// module, or the scan is finding nothing because it is broken rather
    /// than because there is nothing to find (判据 §2).
    #[test]
    fn page_state_has_at_most_one_builder_call_site_outside_the_module() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources_under(&root);
        assert!(
            sources.len() > 100,
            "the source walk found only {} files under src/ — the census \
             scanned nothing, which is not the same as finding nothing wrong",
            sources.len()
        );

        // Non-vacuity, anchored on text production code ACTUALLY CONTAINS.
        // `PageState::build(` never appears inside page_state/: the definition
        // is `pub fn build(` in an `impl PageState`, every doc mention is
        // stripped by `code_text`, and every call is `#[cfg(test)]`. Anchoring
        // on it would make this guard red on its own first run for a reason
        // that has nothing to do with what it guards (判据 §3).
        let build_rs = std::fs::read_to_string(root.join("browser/page_state/build.rs"))
            .expect("page_state/build.rs is readable");
        let build_prod = production_text(
            std::path::Path::new("src/browser/page_state/build.rs"),
            &build_rs,
        );
        assert!(
            build_prod.contains("impl PageState") && build_prod.contains("pub fn build("),
            "the builder is not where this guard thinks it is — the scan, not \
             the tree, is what is broken"
        );

        let mut outside: Vec<String> = Vec::new();
        for (rel, text) in sources {
            if rel.starts_with("src/browser/page_state/") {
                continue;
            }
            // `production_text`, not `production_prefix`: a whole-file test
            // module carries no `#[cfg(test)]` of its own, so the per-file cut
            // would hand this walk 100% of a test file as production.
            // `code_text` on top, so a mention inside a string literal or a
            // doc comment is not a call site.
            let code = code_text(&production_text(std::path::Path::new(&rel), &text));
            for _ in 0..code.matches("PageState::build(").count() {
                outside.push(rel.clone());
            }
        }

        assert!(
            outside.len() <= 1,
            "a second `PageState::build(` call site appeared: {outside:?}. One \
             page state comes from one builder; a second derivation is a second \
             page for the model to disagree with itself about."
        );
        if let Some(site) = outside.first() {
            assert_eq!(
                site, "src/browser/cdp_backend/snapshot.rs",
                "the single builder call site moved out of the snapshot path"
            );
        }
    }
}

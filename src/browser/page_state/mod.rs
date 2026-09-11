//! The page state tree: `RawDom` → `PageState` → text + JSON (spec §4).

pub mod accname;
pub mod build;
pub mod raw;
pub mod refs;
pub mod render;
pub mod roles;

use serde::{Deserialize, Serialize};

use crate::browser::engine::Engine;

pub use accname::{accessible_name, FrameIndex, NAME_MAX_CHARS};
pub use raw::{attr_of, Computed, RawDom, RawFrame, RawNode, RawNodeKind, Rect, Viewport};
pub use refs::{FrameKey, RefEntry, RefId, RefKey, RefTable, StaleReason};
pub use render::{render_text, rendered_indices, to_json, TEXT_MAX_CHARS};
pub use roles::{is_interactive, role_for, role_from_aria};

/// The roles this build models. A closed set on purpose: an open one would
/// make the renderer's vocabulary a function of whatever a page wrote in a
/// `role=` attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Every variant, so a census iterates the set instead of a typed-out copy
    /// of it (判据 §5).
    pub const ALL: [Role; 27] = [
        Role::Link,
        Role::Button,
        Role::Textbox,
        Role::Checkbox,
        Role::Radio,
        Role::Combobox,
        Role::Listbox,
        Role::Option,
        Role::Menu,
        Role::MenuItem,
        Role::Tab,
        Role::Heading,
        Role::Image,
        Role::Banner,
        Role::Main,
        Role::Navigation,
        Role::Contentinfo,
        Role::List,
        Role::ListItem,
        Role::Table,
        Role::Row,
        Role::Cell,
        Role::Article,
        Role::Form,
        Role::Dialog,
        Role::Generic,
        Role::Text,
    ];

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
    pub focused: bool,
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
            // `production_text`, not `production_prefix`: a whole-file test
            // module carries no `#[cfg(test)]` of its own, so the per-file cut
            // would hand this walk 100% of a test file as production.
            // `code_text` on top, so a mention inside a string literal or a
            // doc comment is not a call site.
            let code = code_text(&production_text(std::path::Path::new(&rel), &text));
            if rel.starts_with("src/browser/page_state/") {
                continue;
            }
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

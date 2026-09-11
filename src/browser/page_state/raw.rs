//! The engine-neutral DOM capture. Two fetchers produce it, one builder
//! consumes it — and one of those fetchers has a death date (spec §3.3).
//!
//! Coordinates here are **frame-local**: `RawFrame::offset` says where a child
//! document sits inside its parent, and `PageState::build` is the one place
//! that adds it. A fetcher that also added would double every child-frame
//! coordinate, and the result would still look like a coordinate.

use serde::{Deserialize, Serialize};

use crate::browser::engine::Engine;

/// A box on the page, in CSS pixels, rounded to whole pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where the model is looking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub content_width: u32,
    pub content_height: u32,
    /// The page scale factor `Page.getLayoutMetrics` reports — the ratio the
    /// geometry above is already expressed in. NOT `window.devicePixelRatio`:
    /// that is a different number, and reading this as that would mis-scale
    /// every coordinate on a zoomed page.
    pub dpr: f64,
}

/// The four computed values the fetchers ask for, and nothing else.
///
/// `overflow_clip` was here and is CUT: both fetchers filled it and nothing in
/// any part read it. A field whose only property is that it parses is the
/// abstraction R10 says to withdraw on sight. If Task 13's occlusion check
/// wants it, it comes back as that task's deliverable with a reader.
///
/// `Option<Computed>` on a node means "the style read did not survive its own
/// cross-check" (spec §3.3) — an unknown. It must not be spent as "visible",
/// and must not be spent as "hidden" either; see `build::visibility_of`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computed {
    pub display_none: bool,
    pub visibility_hidden: bool,
    pub opacity_zero: bool,
    pub cursor_pointer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawNodeKind {
    Document,
    Element,
    Text,
    Other,
}

/// One node, flattened. `parent` indexes this frame's `nodes` and always
/// points BACKWARDS — the fetchers emit document order.
///
/// ## Synthesized attributes
///
/// A fetcher may add pairs the HTML never had, when the engine reports the
/// fact somewhere other than the attribute list:
/// - `("checked", "")` from `DOMSnapshot`'s `inputChecked` — the DOM
///   *property*, which a user click changes and the attribute does not
/// - `("selected", "")` from `optionSelected`
/// - `(":focus", "")` when the fetcher ran JS and could see focus
///
/// Written as attributes rather than as new fields so the two fetchers keep
/// one shape between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawNode {
    pub backend_node_id: u64,
    pub parent: Option<usize>,
    pub kind: RawNodeKind,
    pub tag: Option<String>,
    pub attrs: Vec<(String, String)>,
    pub text: Option<String>,
    /// `None` means the engine reported no box for this node.
    ///
    /// It is **not** a verdict about visibility — see `build::visibility_of`,
    /// where style decides and a missing box only drops the geometry token.
    /// A boxless node keeps its place, keeps its ref if it is interactive or
    /// carries text, and is counted in `PageState::no_box` (ruling R12,
    /// deviation 14; this overrides spec §4.1's "`rect=None` ⇒ hidden").
    /// Never `Some(0x0)` for an absent box: a zero rectangle is a coordinate,
    /// and an absent one is an absence.
    pub rect: Option<Rect>,
    pub computed: Option<Computed>,
    /// Chrome's `DOMSnapshot` `isClickable`. `None` is "the engine did not
    /// say", not "no" — one signal of six, and never a veto.
    ///
    /// (`shadow_root` was here and is CUT for the same reason as
    /// `Computed::overflow_clip`: both fetchers wrote it, nothing read it.)
    pub clickable_hint: Option<bool>,
}

impl RawNode {
    /// This node's attribute `name`, matched case-insensitively on the name
    /// (HTML attribute names are ASCII case-insensitive).
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        attr_of(&self.attrs, name)
    }

    /// Whether the attribute is present at all, whatever its value —
    /// `disabled`, `checked`, `required` and friends are boolean attributes.
    #[must_use]
    pub fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }

    /// The lowercased tag, or `""` for a node that has none.
    #[must_use]
    pub fn tag_lower(&self) -> String {
        self.tag.as_deref().unwrap_or_default().to_ascii_lowercase()
    }
}

/// [`RawNode::attr`] as a free function, for callers holding only the pairs.
#[must_use]
pub fn attr_of<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// One document. The main frame is `frames[0]`; children follow in the order
/// the fetcher walked them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawFrame {
    pub frame_id: String,
    pub loader_id: String,
    /// Where this document's origin sits in page coordinates.
    pub offset: (i32, i32),
    pub nodes: Vec<RawNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawDom {
    pub engine: Engine,
    pub viewport: Viewport,
    pub frames: Vec<RawFrame>,
}

/// A plain element for unit tests, so `roles.rs` and `accname.rs` do not each
/// grow a constructor of their own.
#[cfg(test)]
pub(crate) fn node_with(tag: &str, attrs: &[(&str, &str)]) -> RawNode {
    RawNode {
        backend_node_id: 1,
        parent: None,
        kind: RawNodeKind::Element,
        tag: Some(tag.to_string()),
        attrs: attrs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        text: None,
        rect: Some(Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        }),
        computed: None,
        clickable_hint: None,
    }
}

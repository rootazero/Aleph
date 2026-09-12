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
    /// The **page scale factor** — pinch-zoom — that `Page.getLayoutMetrics`
    /// reports as `cssVisualViewport.scale`, and the ratio the geometry above is
    /// already expressed in.
    ///
    /// This field was called `dpr` and held this same value, which is 判据
    /// §17's 错的标签比缺的贵 in its purest form: `devicePixelRatio` and the
    /// page scale are different numbers, both are near 1 on an ordinary desktop
    /// page, and **a reader cannot tell a wrong DPR from a right one** — where
    /// "there is no DPR here" is a state any consumer can handle. The doc
    /// underneath said the right thing while the name said the wrong one, and
    /// the name is what a caller reads (判据 §1: the copy that lies is the one
    /// nobody re-reads).
    ///
    /// It reaches a model on the JSON face in Task 14, which is what made the
    /// name worth changing rather than commenting. A real device pixel ratio is
    /// **not** fetched: it would cost a `Runtime.evaluate` round trip per
    /// capture for a value nothing reads today, and a field with no reader is
    /// what this module CUT twice already (`Computed::overflow_clip`,
    /// `RawNode::shadow_root`). If Task 14 wants one, it arrives with its
    /// consumer.
    pub page_scale: f64,
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
///
/// # Obligation on the fetcher: these are CASCADED values, not declared ones
///
/// `build::visibility_of` judges each node by its own `Computed` and nothing
/// else — it does not walk ancestors. So a fetcher must report the **effective**
/// style of each node: a child of a `display: none` subtree must itself carry
/// `display_none: true`. A fetcher that reported only what each element
/// declared would leave every descendant of a hidden container reading as
/// visible, and the builder has no way to tell that apart from a genuinely
/// visible node.
///
/// This is written here, on the field the fetchers fill, rather than only at
/// the rule that consumes it: the person who must honour the contract reads
/// this file (判据 §1 — the copy that drifts is the one its owner never sees).
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
/// ## `attrs` is the PAGE's namespace, and the page is a hostile writer
///
/// Everything in `attrs` came from the document, and **a fetcher writes
/// nothing here**. It used to be allowed to add pairs the HTML never had, when
/// the engine reported a fact somewhere other than the attribute list —
/// `("checked", "")` from `DOMSnapshot`'s `inputChecked`, `("selected", "")`
/// from `optionSelected`. That is withdrawn, and for a reason beyond forgery:
/// **`attrs` is add-only, so a fetcher borrowing it could only ever push the
/// answer to `true`.** A checkbox the agent has just clicked OFF still carries
/// the markup `checked` attribute — the attribute is the *initial* state and
/// the property is the *current* one — and there was no pair a fetcher could
/// add to say "no". The model was told `[checked]` about a control it had
/// itself unchecked: the observation lying about the agent's own effect, which
/// is worse than lying about the page. See [`RawNode::checked`].
///
/// **A fact the page must NOT be able to assert does not go in `attrs`.** It
/// gets a field, because a field is a namespace the page cannot reach. This is
/// not a style preference — `:focus` used to live here, and `:` is a legal
/// HTML attribute character, so `<button :focus>` rendered `[focused]` into
/// the model's observation as an *observed* fact about where the caret was.
/// `:`-prefixed attribute names are also ordinary Vue/Alpine `:prop` output
/// left in a served DOM, so the collision was not merely adversarial. The
/// defect was two writers of different trust sharing one namespace; a name
/// filter would have needed a guard, and a guard only covers the shapes it
/// recognises (判据 §3). See [`RawNode::focused`].
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
    /// Does the caret live here? `None` is **"the fetcher did not say"**, and
    /// that is the only value any fetcher produces today: `DOMSnapshot` carries
    /// no focus bit, so only a fetcher that ran JS (Task 17's obscura one) can
    /// fill it. Until one does, `[focused]` never renders — which is the honest
    /// state, and cheaper than a wrong one (判据 §17).
    ///
    /// **A field rather than a synthesized `attrs` entry, and that is the whole
    /// point** — see this struct's doc. A page cannot write here. If Task 17
    /// does not supply a producer, CUT this field and `NodeStates::focused`
    /// together rather than shipping a predicate that is constant (判据 §2).
    ///
    /// `#[serde(default)]` because the fixtures predate the field and absent
    /// means exactly what `None` means. It is not a value being filled in: a
    /// fetcher that never mentions focus has not claimed the caret is elsewhere.
    #[serde(default)]
    pub focused: Option<bool>,
    /// Is this control checked **now**? `None` is "the fetcher did not say",
    /// and then `build` falls back to the page's `checked` / `aria-checked`
    /// attribute behind a role gate.
    ///
    /// # Obligation on the fetcher: this is the PROPERTY, not the attribute
    ///
    /// Fill it from `DOMSnapshot`'s `inputChecked` — the live DOM property. The
    /// two part company the moment anyone clicks: the content attribute is the
    /// *initial* state and never changes again, so a box the user or **the
    /// agent itself** just unchecked still carries `checked` in the markup.
    /// Reporting the attribute here re-creates exactly the defect the field
    /// exists to fix.
    ///
    /// `inputChecked` is a rare-boolean index list: a node appears in it iff
    /// the property is true. **You do not have to write `Some(false)` for the
    /// rest** — set [`RawFrame::live_properties_observed`] on the frame and
    /// `build` reads the silence correctly, because the declaration says the
    /// silence means something. That is deliberate: this doc used to ask for
    /// the per-node `Some(false)`, which is an obligation the obvious
    /// implementation (walk the list, set what it finds) violates silently on
    /// exactly the nodes that matter, and a doc comment is the copy nobody
    /// re-reads (判据 §1).
    ///
    /// A field, so the page cannot reach it — and so an answer of "no" is
    /// expressible at all, which `attrs` could not do. It **outranks the
    /// attribute unconditionally**, including the role gate: gating an
    /// observation by a `role=` the page writes would let a page suppress a
    /// true reading of its own control.
    #[serde(default)]
    pub checked: Option<bool>,
    /// Is this option selected **now**? [`Self::checked`]'s twin in every
    /// respect (判据 §16), filled from `DOMSnapshot`'s `optionSelected`, the
    /// property and never the `selected` content attribute, and governed by the
    /// same frame declaration — so silence about a selectable node is read as
    /// `false` when [`RawFrame::live_properties_observed`] is set.
    #[serde(default)]
    pub selected: Option<bool>,
    /// What is typed in this control **now**? `None` is "the fetcher did not
    /// say", and `build` then falls back to the page's `value` content
    /// attribute — unless [`RawFrame::live_properties_observed`] says the
    /// capture looked, in which case silence here means the control is empty.
    ///
    /// # Obligation on the fetcher: the PROPERTY, and the third member
    ///
    /// Fill it from `DOMSnapshot`'s `inputValue` (and `textValue` for
    /// `<textarea>`). `value` is the third attribute in this struct with a
    /// `default*` IDL twin — `defaultValue` — which is the mechanical form of
    /// "does this go stale": the content attribute is what the control started
    /// with and the property is what is in it now, and they part company the
    /// moment anyone types, including when the typist is the agent's own
    /// `fill`.
    ///
    /// It joined its twins late, and the reason is worth keeping: the rule that
    /// predicts exactly this set was written down a round earlier and then run
    /// over `NodeStates`' seven bits instead of over "the attributes the builder
    /// reads". A membership rule is worth what its enumeration is worth.
    ///
    /// Taken verbatim, not whitespace-collapsed like the attribute path: this
    /// is the user's text, a `<textarea>`'s newlines are part of it, and the
    /// JSON face escapes rather than folds. `render_text` does not print it at
    /// all today — Task 14's JSON face is where it reaches a model.
    #[serde(default)]
    pub value: Option<String>,
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
    /// **Did this capture read the live DOM properties** for the three fields
    /// that have a page-attribute fallback — [`RawNode::checked`],
    /// [`RawNode::selected`] and [`RawNode::value`]?
    ///
    /// # What it licenses
    ///
    /// `true` says: for this frame, the fetcher asked the engine for the
    /// properties. `build` then reads **only** the fields for the elements the
    /// engine actually reports on — `<input type=checkbox|radio>`, `<option>`,
    /// and inputs with a value — so the page's stale `checked` / `selected` /
    /// `value` content attributes are not consulted there at all, and a node
    /// the fetcher left `None` is one whose property is **false** or empty,
    /// because `DOMSnapshot` reports these as rare index lists where a node
    /// appears iff it has the value.
    ///
    /// It licenses nothing beyond those elements. A `<div role="checkbox">` has
    /// no checkedness for any capture to have read, so this flag says nothing
    /// about one and its `aria-checked` remains the only evidence there is —
    /// deriving `false` from a capture's silence about a property that does not
    /// exist would be the same invention in ARIA clothing.
    ///
    /// `false` says the opposite: nobody looked, so the page's markup is the
    /// only evidence there is, and it is evidence of the **initial** state.
    ///
    /// # Why this is a property of the CAPTURE and not of the node
    ///
    /// A per-node rule is one a fetcher author has to honour on every node, and
    /// it fails silently on the node nobody thought about: the obvious
    /// implementation — walk `inputChecked`, set the ones it lists — leaves
    /// `None` on every other node, and a `None` that falls back to a stale
    /// content attribute is exactly the defect the fields were added to fix.
    /// Declared once per frame, that obligation cannot be got wrong per-node.
    ///
    /// # Why the FRAME and not `RawDom`
    ///
    /// A capture is assembled per document — an out-of-process iframe is a
    /// separate `DOMSnapshot` call, and this module's fixtures carry such a
    /// pair — so one document's read can succeed while another's does not.
    /// Per-frame is never worse than per-capture and matches the unit a fetcher
    /// actually assembles. A fetcher that reads properties for the whole page
    /// writes `true` on every frame, which costs it one line.
    ///
    /// # Why there is no `#[serde(default)]`, unlike every other new field
    ///
    /// Deliberate, and the distinction is the point: [`RawNode::focused`] and
    /// friends are per-NODE data, where absent honestly means "this node was
    /// not mentioned". This is a capture's **declaration about itself**, and a
    /// capture that does not say what it looked at is one nobody can read
    /// safely (判据 §8). Omitting it is a missing-field error from serde and a
    /// compile error at every construction site, which is the whole reason to
    /// spend a required field here.
    ///
    /// **A `false` here is conservative, not neutral**, and that is why it is
    /// the shape of the type rather than a default: a fetcher that fills the
    /// fields but forgets this flag still gets the attribute fallback, and a
    /// node whose markup says `checked` but whose property is false will read
    /// `[checked]`. Defaulting the other way would be worse — it would have an
    /// unset flag manufacture `Some(false)` denials on every checkable node in
    /// every hand-written capture, which is the invention this round removed.
    pub live_properties_observed: bool,
    pub nodes: Vec<RawNode>,
}

/// A frame whose content is **not** in this capture, and the most this capture
/// knows about it.
///
/// Two variants because the two cases know different things, and an `Option`
/// that is always `Some` in one branch and always `None` in the other is a
/// shape that invites a reader to check the wrong one.
///
/// # Why this is an enum and not a `Vec<u64>`, which is not a matter of taste
///
/// A flat list of `backendNodeId`s would have had to put **something** in the
/// slot for an unplaceable document, which has no owning element and therefore
/// no `backendNodeId` at all. The only candidate is `0` — and `0` is CDP's own
/// "no node". That shape would reproduce 判据 §8 **inside the very carrier
/// built to stop an absence reading as a fact**: a caller acting on the list
/// would resolve a node that does not exist, one level in from the defect the
/// list exists to prevent.
///
/// Its twin is live in the producer: `unaccounted_frame_elements` refuses a
/// frame element whose `backendNodeId` is unreadable rather than writing `0`,
/// for exactly the same reason. An unknown is allowed to say only "I don't
/// know", and the place to say it is a refusal — never an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreachedFrame {
    /// A frame ELEMENT whose content lives in another renderer and whose
    /// capture was not supplied — carried as the element's own
    /// `backendNodeId`, which is the key the stitcher places children by and
    /// therefore the one a caller can act on.
    NotCaptured(u64),
    /// A DOCUMENT this capture contains that no element owns, so there is
    /// nowhere to put it — carried as the document's own frame id, because an
    /// unowned document is precisely the case where no owning element is
    /// known. Its nodes are dropped rather than placed at the page origin,
    /// where every one of them would have a plausible wrong coordinate.
    Unplaceable(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawDom {
    pub engine: Engine,
    pub viewport: Viewport,
    /// **What this capture could not read.** Empty means the capture is whole.
    ///
    /// # Why this is a field and not a paragraph
    ///
    /// Without it, a page whose cross-origin subtree was never captured is
    /// indistinguishable from a page whose iframe is genuinely empty: the
    /// `<iframe>` element is in [`Self::frames`] with its box, `src` and title,
    /// and its content is simply absent. An absence that reads as a fact is
    /// 判据 §17's 错的标签, and nothing downstream can recover it — a consumer
    /// of a `RawDom` cannot know a frame was never captured.
    ///
    /// It is the **output of an accounting the fetcher already performs**, not a
    /// second copy of one: a frame element is listed here exactly when it has no
    /// content document in the capture and no child capture was supplied for it,
    /// so there is nothing for anyone to remember to clear.
    ///
    /// **Precisely: [`UnreachedFrame::NotCaptured`] cannot be produced at all
    /// once children are supplied — [`UnreachedFrame::Unplaceable`] still can,
    /// because it describes a different failure.** An earlier version of this
    /// doc said "the list is empty by construction", which is true of the first
    /// variant and false of the field; a page with an unowned document and a
    /// fully enumerated set of children reaches this list. The drift argument is
    /// unaffected — neither variant is a value anyone maintains — but the
    /// sentence was wrong and a reader would have believed it (判据 §1).
    ///
    /// No `#[serde(default)]`, for [`RawFrame::live_properties_observed`]'s
    /// reason: this is a capture's declaration about **itself**, and a capture
    /// that does not say what it failed to read is one nobody can read safely
    /// (判据 §8). Omitting it is a missing-field error from serde and an
    /// `E0063` at every construction site — which is the cost that makes the
    /// declaration real, and whose absence is why an earlier round left this
    /// fact in a doc comment instead.
    pub unreached_frames: Vec<UnreachedFrame>,
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
        focused: None,
        checked: None,
        selected: None,
        value: None,
    }
}

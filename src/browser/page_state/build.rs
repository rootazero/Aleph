//! The ONE builder (spec §7.3). Two fetchers in, one `PageState` out.

use std::collections::HashMap;
use std::time::Duration;

use super::accname::{accessible_name, normalize, AccName, FrameIndex};
use super::raw::{RawDom, RawNode, RawNodeKind, Rect};
use super::refs::{FrameKey, RefKey, RefTable};
use super::roles::{is_interactive, role_for};
use super::{NodeStates, PageState, Role, StateNode};

impl PageState {
    /// Turn a capture into the tree the model reads.
    ///
    /// Resets `refs` when the MAIN frame's loader changed (spec §4.3). That
    /// lives here, not in the caller: it is a property of a capture, and a
    /// caller-side reset is a wire that can be forgotten silently — stale refs
    /// would then resolve against a new document (判据 §7).
    #[must_use]
    pub fn build(
        raw: &RawDom,
        refs: &mut RefTable,
        generation: u64,
        url: &str,
        title: &str,
        fetch: Duration,
    ) -> PageState {
        if let Some(main) = raw.frames.first() {
            refs.reset_for_document(&main.loader_id);
        }

        let mut nodes: Vec<StateNode> = Vec::new();
        for frame in &raw.frames {
            let fx = FrameIndex::build(frame);
            let frame_key = FrameKey {
                frame_id: frame.frame_id.clone(),
                loader_id: frame.loader_id.clone(),
            };
            // raw index -> index in `nodes`, for the frame being walked.
            let mut emitted: HashMap<usize, usize> = HashMap::new();
            for (i, node) in frame.nodes.iter().enumerate() {
                let Some(state) = state_node(i, node, &fx, &frame_key, frame.offset, &emitted)
                else {
                    continue;
                };
                emitted.insert(i, nodes.len());
                nodes.push(state);
            }
        }

        let no_box = (
            nodes.iter().filter(|n| n.rect.is_none()).count(),
            nodes.len(),
        );
        let mut state = PageState {
            engine: raw.engine,
            generation,
            url: url.to_string(),
            title: title.to_string(),
            viewport: raw.viewport.clone(),
            no_box,
            fetch_ms: u64::try_from(fetch.as_millis()).unwrap_or(u64::MAX),
            nodes,
        };

        // Refs are minted for exactly the nodes the RENDERER PRINTS, and in the
        // order it prints them — so `ref_count()` equals the number of `[ref=`
        // tokens the model can see, and the ids it is shown have no gaps.
        //
        // Minting for every visible interactive-or-text node instead would
        // report a count larger than the model can address, and hand out
        // numbers like `e1, e3, e4` that read as elements it was not shown
        // (判据 §18 — a number must carry the predicate its label claims).
        //
        // `rendered_indices` reads `visible`, `interactive`, `name`, `role`,
        // `text` and `parent` — never `r#ref` — so running it before minting is
        // not circular. Containers earn a line but no ref: nothing addresses a
        // `- banner`.
        for i in super::render::rendered_indices(&state) {
            let node = &state.nodes[i];
            if !node.interactive && node.text.is_none() {
                continue;
            }
            let key = RefKey {
                frame_id: node.frame.frame_id.clone(),
                loader_id: node.frame.loader_id.clone(),
                backend_node_id: node.backend_node_id,
            };
            state.nodes[i].r#ref = Some(refs.mint(&key, generation));
        }
        state
    }

    /// How many nodes the model can address — which is exactly the number of
    /// `[ref=` tokens in `render_text`'s output, because `build` mints only for
    /// nodes the renderer prints.
    /// `the_thirteen_node_fixture_renders_the_golden_tree` asserts that
    /// equality rather than trusting it.
    #[must_use]
    pub fn ref_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.r#ref.is_some()).count()
    }
}

/// Style decides when it is known; the box decides only when it is not.
///
/// **This replaces spec §4.1's "`rect=None` ⇒ hidden".** Part 5 measured real
/// obscura returning a ZERO quad for inline `<a>` / `<span>` from both
/// `getBoxModel` and `getBoundingClientRect`, while M11 measured real boxes
/// for Hacker News's `<a>`. Nobody knows which generalises, so the visibility
/// rule must not rest on it: a missing box would otherwise delete every inline
/// link on some pages and none on others, and the model would have no way to
/// tell those two worlds apart.
///
/// So a node with `computed` is judged by its styles alone. A missing box on
/// such a node is not a verdict — it is counted in `PageState::no_box`, the
/// node still earns a ref if it is interactive or carries text, and
/// `render_text` simply omits the `@x,y wxh` token. The model sees "this is
/// here and I could not measure it", which is the true statement.
///
/// `computed == None` is the cross-check failure (spec §3.3), and there the
/// box is the only evidence left. It is spent as evidence of PRESENCE, never
/// as evidence of absence beyond that (判据 §8).
#[must_use]
pub fn visibility_of(node: &RawNode) -> bool {
    match node.computed {
        Some(c) => !c.display_none && !c.visibility_hidden && !c.opacity_zero,
        None => node.rect.is_some(),
    }
}

/// One raw node → one state node, or `None` for nodes that carry nothing.
fn state_node(
    index: usize,
    node: &RawNode,
    fx: &FrameIndex<'_>,
    frame: &FrameKey,
    offset: (i32, i32),
    emitted: &HashMap<usize, usize>,
) -> Option<StateNode> {
    let text = match node.kind {
        // A document node has no role, no name and no box; a line for it would
        // be a line of nothing.
        RawNodeKind::Document | RawNodeKind::Other => return None,
        RawNodeKind::Text => {
            let t = normalize(node.text.as_deref().unwrap_or_default());
            if t.is_empty() {
                return None;
            }
            Some(t)
        }
        RawNodeKind::Element => None,
    };

    // The nearest EMITTED ancestor, so dropping a document node does not
    // orphan its children. A parent index that points forward or off the end
    // is dropped rather than guessed: the node keeps its place and loses only
    // the edge (判据 §8).
    let parent = nearest_emitted_ancestor(index, fx, emitted);

    let (role, name, interactive) = if text.is_some() {
        (Role::Text, AccName::none(), false)
    } else {
        let role = role_for(&node.tag_lower(), &node.attrs);
        (role, accessible_name(index, fx), is_interactive(role, node))
    };

    Some(StateNode {
        parent,
        r#ref: None,
        backend_node_id: node.backend_node_id,
        frame: frame.clone(),
        role,
        name_covers_all_text: name.covers_all_text,
        name: name.text,
        value: value_of(node, fx.frame.live_properties_observed),
        states: states_of(node, role, fx.frame.live_properties_observed),
        // The one place the frame offset is applied. Position moves; size does
        // not.
        rect: node.rect.as_ref().map(|r| Rect {
            x: r.x + offset.0,
            y: r.y + offset.1,
            w: r.w,
            h: r.h,
        }),
        interactive,
        visible: visibility_of(node),
        text,
        href: node.attr("href").map(str::to_string),
        placeholder: node.attr("placeholder").map(str::to_string),
    })
}

fn nearest_emitted_ancestor(
    index: usize,
    fx: &FrameIndex<'_>,
    emitted: &HashMap<usize, usize>,
) -> Option<usize> {
    let mut cursor = index;
    for _ in 0..fx.frame.nodes.len() {
        let parent = fx.frame.nodes[cursor].parent?;
        if parent >= cursor {
            return None;
        }
        if let Some(&out) = emitted.get(&parent) {
            return Some(out);
        }
        cursor = parent;
    }
    None
}

/// A control's current value. A submit button's `value` is its NAME, not its
/// value — accname rule 4 already spent it, and printing it twice would read
/// as two different facts.
///
/// **The third member of `checked` and `selected`'s class**, and it arrived a
/// round late. The membership rule is mechanical — an attribute goes stale iff
/// its IDL property has a `default*` twin — and `value`/`defaultValue` is that
/// pair exactly: the attribute is what the control started with, the property
/// is what is in it now, and the agent's own `fill` is one of the things that
/// parts them. The rule had been written down and then run over `NodeStates`'
/// seven bits rather than over the attributes the builder reads.
///
/// Same precedence as its twins: the fetcher's reading, then — only where no
/// capture looked — the page's markup.
fn value_of(node: &RawNode, live: bool) -> Option<String> {
    if node.tag_lower() == "input" {
        let ty = node.attr("type").unwrap_or("text").to_ascii_lowercase();
        if matches!(ty.as_str(), "button" | "submit" | "reset") {
            return None;
        }
    }
    node.value
        .clone()
        .or_else(|| {
            // A capture that read `inputValue` lists the controls that have
            // one, so silence is "empty" and the markup is not consulted.
            (!live).then(|| node.attr("value").map(normalize)).flatten()
        })
        .filter(|v| !v.is_empty())
}

/// The roles a `checked` answer can be about at all — a bare attribute, an
/// `inputChecked` reading, or the `false` a live capture derives from silence.
///
/// One derivation, because the gate is asked twice and two spellings of one
/// rule part company (判据 §12). Wider than HTML on purpose: `menuitem` carries
/// no bare `checked` attribute, but `role="menuitemcheckbox"` maps here and
/// does carry the state.
const fn is_checkable(role: Role) -> bool {
    matches!(role, Role::Checkbox | Role::Radio | Role::MenuItem)
}

/// [`is_checkable`]'s twin for `selected`.
const fn is_selectable(role: Role) -> bool {
    matches!(role, Role::Option | Role::Tab | Role::Row | Role::Cell)
}

/// The elements whose checkedness **the engine itself reports** —
/// `DOMSnapshot`'s `inputChecked` covers `<input type=checkbox|radio>` and
/// nothing else.
///
/// **Narrower than [`is_checkable`], and the gap is the whole point.** Only
/// here can a live capture's silence be read as `false`, because only here is
/// there a property the capture could have read. A `<div role="checkbox">` has
/// no checkedness for `inputChecked` to report, so answering `false` for one
/// out of a capture's silence would be this round's own invention aimed at
/// ARIA widgets — and there the page's `aria-checked` remains the only
/// evidence there is, live capture or not.
///
/// The `<input>` test is on the TAG and the type comes from `role`, which
/// `roles::input_role` already derived: two spellings of "is this a checkbox"
/// would be two derivations of one fact (判据 §12).
fn has_native_checkedness(node: &RawNode, role: Role) -> bool {
    node.tag_lower() == "input" && matches!(role, Role::Checkbox | Role::Radio)
}

/// `<option>` — `optionSelected`'s entire domain, and the same narrowing.
///
/// Measured as a mutation: with the live derivation gated on [`is_selectable`]
/// instead, a capture that read the properties gave **every `<td>` and `<tr>`**
/// `Some(false)` and appended `[unselected]` to its line — on a page built out
/// of table cells, which is this branch's own fixture page, that is a token on
/// nearly every line, claiming an observation of a property those elements do
/// not have.
fn has_native_selectedness(node: &RawNode, role: Role) -> bool {
    node.tag_lower() == "option" && matches!(role, Role::Option)
}

/// The state bits.
///
/// # Which bits a page is allowed to assert about itself, and which it is not
///
/// A state token is a page-controlled **predicate**, and `render::quote` cannot
/// help here — quoting defends page-controlled *strings*, while a token reaches
/// the line as itself. So the defence has to be at the derivation.
///
/// - `disabled`, `required`, `readonly` are read from the bare HTML attributes
///   with no role gate, **on purpose**. They are real attributes the page is
///   entitled to write about its own elements; a `<div required>` is an
///   authoring mistake, not a forgery, and the honest report of an authoring
///   mistake is the mistake.
/// - `aria-*` is page-authored by definition — that is what ARIA is for.
/// - `checked` and `selected` read [`RawNode::checked`] / [`RawNode::selected`]
///   **first** — the live DOM property, which only a fetcher can write — and
///   fall back to the page's attribute behind a role gate only when the fetcher
///   said nothing. The membership rule that separates these two from the three
///   above is not "is this legal HTML", it is **does this attribute's value go
///   stale**: `checked` and `selected` are the *initial* state and part company
///   from the property the moment anyone clicks, including when the clicker is
///   the agent. `disabled`, `required` and `readonly` do not.
///   The role gate stays on the fallback, where it is worth what it is worth: it
///   removes accidental mislabelling (`<div selected>` out of a framework) and
///   is **not** a trust boundary, because `role_for` takes `role=` from the page
///   and `<div role="option" selected>` satisfies it. The trust boundary is the
///   field.
/// - `focused` comes from [`RawNode::focused`], a field, and never from
///   `attrs`. It is the one bit in this set with no attribute fallback at all,
///   and it lived in `attrs` as `":focus"` until a page could forge it — see
///   `RawNode`'s doc for the mechanism.
///
/// # An absent attribute is silence, not a denial
///
/// The fallback used to answer `Some(has_attr("checked"))`, so a checkbox with
/// **no** `checked` attribute rendered `[unchecked]` — a claim that someone
/// looked and the answer was no, manufactured out of an absence (判据 §8), on
/// every checkbox in every capture, and pinned by a passing test. It is the
/// `focused` ruling inverted: *"answering 'the caret is elsewhere' would be its
/// own invention"* is the same sentence with a different subject.
///
/// The attribute's PRESENCE is what this round's finding was about — it records
/// the *initial* state and goes stale — so its ABSENCE records the initial
/// state just as weakly. What a page writes is carried; what it does not write
/// is `None`, and `state_tokens` prints nothing for `None` because only states
/// that are KNOWN print. `aria-checked="false"` is different and still lands as
/// `Some(false)`: that is the page saying "no", not the page saying nothing.
fn states_of(node: &RawNode, role: Role, live: bool) -> NodeStates {
    let aria_true = |name: &str| {
        node.attr(name)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    };
    let aria_bool = |name: &str| {
        node.attr(name)
            .and_then(|v| match v.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            })
    };
    NodeStates {
        disabled: node.has_attr("disabled") || aria_true("aria-disabled"),
        // The fetcher's reading of the live property wins outright. Where it
        // said nothing, `live` decides who answers: a capture that read the
        // properties answers `false` for a checkable node it did not list
        // (`inputChecked` is a rare-boolean list — appearing in it IS the
        // truth value), and the page's markup is not consulted at all. A
        // capture that did not look leaves the markup as the only evidence.
        // `None` throughout for anything not checkable: "this is not a
        // checkbox" and "this checkbox is off" are different facts.
        checked: node
            .checked
            .or_else(|| (live && has_native_checkedness(node, role)).then_some(false))
            .or_else(|| match role {
                _ if is_checkable(role) && node.has_attr("checked") => Some(true),
                _ => aria_bool("aria-checked"),
            }),
        expanded: aria_bool("aria-expanded"),
        // `checked`'s twin, the same precedence and the same silence (判据
        // §16). `Option<bool>` for the reason `focused` is one: a bare `bool`
        // spells "nobody looked" as `"selected": false` on the JSON face, which
        // is a denial about every node on the page.
        selected: node
            .selected
            .or_else(|| (live && has_native_selectedness(node, role)).then_some(false))
            .or_else(|| match role {
                _ if is_selectable(role) && node.has_attr("selected") => Some(true),
                _ => aria_bool("aria-selected"),
            }),
        required: node.has_attr("required") || aria_true("aria-required"),
        readonly: node.has_attr("readonly") || aria_true("aria-readonly"),
        // From the FIELD, never from `attrs` — the page cannot reach a field.
        //
        // Carried, not collapsed. An earlier version of this line spent `None`
        // as `false`, arguing that "nobody looked" and "the caret is elsewhere"
        // render identically so the difference is unobservable. That is true of
        // the TEXT face and false of the JSON one: `NodeStates` has no
        // `skip_serializing_if`, so the collapse made `to_json` print
        // `"focused": false` on every node — a positive claim about something
        // no producer has looked at until Task 17 (判据 §9: one verb, two
        // faces, and they must share a derivation).
        focused: node.focused,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{render_text, to_json, RefTable, StaleReason};
    use super::*;
    use std::time::Duration;

    const FIXTURE: &str = include_str!("fixtures/thirteen-node.rawdom.json");
    const GOLDEN: &str = include_str!("fixtures/thirteen-node.golden.txt");

    fn fixture() -> RawDom {
        serde_json::from_str(FIXTURE).expect("the thirteen-node fixture parses as a RawDom")
    }

    fn build_once(refs: &mut RefTable) -> PageState {
        PageState::build(
            &fixture(),
            refs,
            7,
            "https://example.test/hn",
            "Hacker News",
            Duration::from_millis(142),
        )
    }

    /// The whole builder in one assertion: roles, names, refs, geometry,
    /// visibility, the frame offset and the container rule.
    #[test]
    fn the_thirteen_node_fixture_renders_the_golden_tree() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        let rendered = render_text(&state);
        assert_eq!(rendered, GOLDEN.trim_end_matches('\n'));

        // The number the tool reports and the number the model can see are the
        // same number. `SnapshotOutput.ref_count` is what Part 4 shows, and a
        // count that included refs the text never printed would be a label on
        // the wrong predicate (判据 §18). Ids are contiguous for the same
        // reason: a gap reads as an element the model was not shown.
        assert_eq!(
            state.ref_count(),
            rendered.matches("[ref=").count(),
            "ref_count and the printed refs disagree:\n{rendered}"
        );
        assert_eq!(state.ref_count(), 6);
        for i in 1..=6 {
            assert!(
                rendered.contains(&format!("[ref=e{i}]")),
                "e{i} is missing, so the ids the model sees have a gap:\n{rendered}"
            );
        }
    }

    /// Thirteen raw nodes, eleven state nodes: the two `document` nodes carry
    /// no role, no name and no box, and a line for either would be a line of
    /// nothing.
    ///
    /// `no_box` counts BOTH boxless nodes — the `display: none` one and the
    /// visible inline link — because it reports how much of the capture had no
    /// geometry, which is a different question from what is visible.
    #[test]
    fn documents_are_dropped_and_everything_else_survives() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        assert_eq!(state.nodes.len(), 11);
        assert_eq!(state.no_box, (2, 11), "two nodes generate no box");
        assert_eq!(state.ref_count(), 6);
        assert_eq!(state.generation, 7);
        assert_eq!(
            state.fetch_ms, 142,
            "elapsed fetch time, not a barrier wait"
        );
    }

    /// A child frame's rects are frame-local in `RawDom` and page-global in
    /// `PageState`. The offset is `(40, 200)`, so the checkbox at `(10, 20)`
    /// must land at `(50, 220)` — and its size must not move. The main frame's
    /// rect is asserted too, so this also catches "the offset is applied twice".
    #[test]
    fn the_frame_offset_is_added_to_position_and_not_to_size() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        let checkbox = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 22)
            .expect("the checkbox is in the tree");
        let r = checkbox.rect.as_ref().expect("it has a box");
        assert_eq!((r.x, r.y, r.w, r.h), (50, 220, 13, 13));
        assert_eq!(checkbox.frame.frame_id, "F-child");
        assert_eq!(checkbox.frame.loader_id, "L-2");

        let link = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 3)
            .expect("the link is in the tree");
        let r = link.rect.as_ref().expect("it has a box");
        assert_eq!((r.x, r.y, r.w, r.h), (130, 11, 83, 15));
    }

    /// `display: none` is what makes a node invisible. It keeps its place in
    /// the JSON and gets no ref — a ref on something nothing can click is a
    /// promise the driver cannot keep.
    #[test]
    fn a_display_none_node_is_invisible_kept_in_json_and_never_given_a_ref() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        let hidden = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 6)
            .expect("the display:none div survives into the state");
        assert!(!hidden.visible);
        assert!(hidden.r#ref.is_none());
        assert!(
            !render_text(&state).contains("tooltip"),
            "an invisible node must not reach the text tree"
        );
    }

    /// **A missing box is not a verdict.** Real obscura returned a zero quad
    /// for inline `<a>` while M11 measured real boxes for the same tag on
    /// another page, so a rule that hid boxless nodes would delete every
    /// inline link on some pages and none on others — and the model could not
    /// tell those two worlds apart.
    ///
    /// So this node stays visible, keeps its ref, is counted in `no_box`, and
    /// loses exactly one thing: the `@x,y wxh` token. The negative half is the
    /// substance — asserting only that the line is present would stay green
    /// over a renderer that printed `@0,0 0x0`, which is a coordinate rather
    /// than an absence.
    #[test]
    fn a_boxless_but_styled_node_stays_visible_keeps_its_ref_and_loses_only_its_geometry() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        let inline = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 9)
            .expect("the boxless inline link is in the tree");
        assert!(inline.visible, "styles say visible, so it is visible");
        assert!(inline.rect.is_none());
        assert!(inline.interactive);
        assert_eq!(
            inline.r#ref.as_ref().map(|r| r.0.as_str()),
            Some("e3"),
            "a boxless node the model can still click must be addressable"
        );

        let line = render_text(&state)
            .lines()
            .find(|l| l.contains("Inline link"))
            .expect("it reaches the text tree")
            .to_string();
        assert_eq!(line, "  - link \"Inline link\" [ref=e3] /url: \"/inline\"");
        assert!(!line.contains('@'), "no geometry token: {line}");
        assert!(!line.contains("0x0"), "and no invented zero box: {line}");
    }

    /// States are read, not guessed. And `checked: None` on a non-checkable
    /// control is load-bearing: "this is not a checkbox" and "this checkbox is
    /// off" are different facts, and the renderer prints them differently.
    #[test]
    fn disabled_and_checked_reach_node_states() {
        let mut refs = RefTable::new();
        let state = build_once(&mut refs);
        let button = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 5)
            .expect("button");
        assert!(button.states.disabled);
        assert_eq!(button.name, "Submit query");
        assert!(button.interactive, "a disabled control is still a control");
        assert_eq!(button.states.checked, None);

        let checkbox = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 22)
            .expect("checkbox");
        assert_eq!(checkbox.states.checked, Some(true));
    }

    /// **A fetcher can say "no", and its reading beats the markup.**
    ///
    /// `checked` and `selected` used to come from the attribute alone, behind a
    /// role gate. A gate answers a different question than the one that matters
    /// here: the content attribute is the control's **initial** state and the
    /// DOM property is its **current** one, and they part company the moment
    /// anyone clicks. `DOMSnapshot` reports `inputChecked`/`optionSelected` as
    /// rare-boolean index lists — a node appears iff the property is true — so
    /// the only channel a fetcher had was *adding* an attribute pair, which can
    /// push the answer to `true` and can never take it back. A box **the agent
    /// itself had just clicked off** read `[checked]` for the rest of the
    /// session: the tree lying about the agent's own effect.
    ///
    /// Both directions on both twins (判据 §16), because a field that only ever
    /// agrees with the attribute is indistinguishable from no field at all.
    #[test]
    fn a_fetchers_live_reading_outranks_an_attribute_that_has_gone_stale() {
        let built = |raw: &RawDom| {
            PageState::build(
                raw,
                &mut RefTable::new(),
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            )
        };
        let states_of_node = |state: &PageState, id: u64| {
            state
                .nodes
                .iter()
                .find(|n| n.backend_node_id == id)
                .expect("the fixture node is in the tree")
                .states
        };

        // The fixture's checkbox is `<input type=checkbox checked>`. The agent
        // clicks it off: the attribute does not move, the property does.
        let mut raw = fixture();
        raw.frames[1].nodes[2].checked = Some(false);
        let state = built(&raw);
        assert_eq!(
            states_of_node(&state, 22).checked,
            Some(false),
            "the markup attribute outranked the fetcher's reading of the live \
             property, so the model is told a box it just unchecked is checked"
        );
        let rendered = render_text(&state);
        assert!(
            rendered.contains("[unchecked]") && !rendered.contains("[checked]"),
            "and the stale claim reached the text face:\n{rendered}"
        );

        // The fallback half: no fetcher looked, so the page's attribute is the
        // only evidence there is and it still counts.
        assert_eq!(
            states_of_node(&built(&fixture()), 22).checked,
            Some(true),
            "a silent fetcher must not erase the markup — `None` is \"I did not \
             look\", not \"it is off\""
        );

        // `selected`, the twin, on a real `<option selected>`.
        let mut raw = fixture();
        raw.frames[1].nodes[3].tag = Some("option".to_string());
        raw.frames[1].nodes[3].attrs = vec![("selected".to_string(), String::new())];
        assert_eq!(
            states_of_node(&built(&raw), 23).selected,
            Some(true),
            "fallback: the page's `selected` on an <option> is the only \
             evidence and must still reach the state"
        );
        raw.frames[1].nodes[3].selected = Some(false);
        let state = built(&raw);
        assert_eq!(
            states_of_node(&state, 23).selected,
            Some(false),
            "the fetcher saw the option deselected and the markup won anyway"
        );
        assert!(
            !render_text(&state).contains("[selected]"),
            "{}",
            render_text(&state)
        );
    }

    /// Two captures of the same document hand back the same numbers, and the
    /// second capture mints nothing new (spec §4.3).
    #[test]
    fn refs_are_stable_across_two_builds_of_the_same_document() {
        let mut refs = RefTable::new();
        let first = build_once(&mut refs);
        let second = PageState::build(
            &fixture(),
            &mut refs,
            8,
            "https://example.test/hn",
            "Hacker News",
            Duration::from_secs(0),
        );
        let ids = |s: &PageState| -> Vec<Option<String>> {
            s.nodes
                .iter()
                .map(|n| n.r#ref.clone().map(|r| r.0))
                .collect()
        };
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(refs.len(), 6, "the second build minted new numbers");
    }

    /// A navigation renumbers from scratch, inside `build` — the caller never
    /// has to remember to reset.
    #[test]
    fn a_new_main_frame_loader_resets_the_table_inside_build() {
        let mut refs = RefTable::new();
        let first = build_once(&mut refs);
        let first_link = first
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 3)
            .and_then(|n| n.r#ref.clone())
            .expect("the link had a ref");

        let mut navigated = fixture();
        navigated.frames[0].loader_id = "L-9".to_string();
        let second = PageState::build(
            &navigated,
            &mut refs,
            8,
            "https://example.test/other",
            "Other",
            Duration::from_millis(7),
        );
        assert_eq!(refs.document(), Some("L-9"));
        assert_eq!(refs.resolve(&first_link), Err(StaleReason::Navigated));
        let second_link = second
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 3)
            .and_then(|n| n.r#ref.clone())
            .expect("the link has a new ref");
        assert_ne!(second_link, first_link);
        assert_eq!(second.fetch_ms, 7);
    }

    /// A malformed `RawDom` — a parent index pointing forward, or off the end
    /// — must not panic and must not silently re-parent the node under
    /// something unrelated. Dropping the edge is the fail-closed answer: the
    /// node keeps its place and loses only its position in the tree.
    #[test]
    fn a_forward_or_out_of_range_parent_index_is_dropped_not_guessed() {
        let mut broken = fixture();
        broken.frames[0].nodes[2].parent = Some(99);
        broken.frames[0].nodes[4].parent = Some(6);
        let mut refs = RefTable::new();
        let state = PageState::build(
            &broken,
            &mut refs,
            1,
            "https://example.test/hn",
            "t",
            Duration::from_secs(0),
        );
        let link = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 3)
            .expect("the node itself survives");
        assert_eq!(link.parent, None, "an unusable parent edge becomes no edge");
        let button = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 5)
            .expect("button");
        assert_eq!(button.parent, None);
    }

    /// **"Nobody looked" and "the caret is elsewhere" are different facts, and
    /// the JSON face is where the difference is visible.**
    ///
    /// This field spent `None` as `false` for one round, on the argument that
    /// both render nothing so the collapse is unobservable. That is true of the
    /// text face only. `NodeStates` has no `skip_serializing_if`, so a `bool`
    /// made `to_json` emit `"focused": false` on **every node of every
    /// capture** — a printed claim about something no producer has looked at
    /// until Task 17. One verb, two faces, and a derivation justified by one
    /// face's observable was being applied to both (判据 §9).
    ///
    /// All three states are asserted on both faces, because the failure was
    /// precisely that one face could not tell two of them apart.
    #[test]
    fn an_unobserved_focus_is_unknown_in_json_and_not_a_denial() {
        // `.get("focused")`, deliberately, NOT `["focused"]`. serde_json's
        // `Index` answers `Value::Null` both when a key is present with a null
        // value and when the key is ABSENT — so case 1 below, the arm carrying
        // this whole ruling, expected `Null` and got `Null` whether or not the
        // field was serialised at all. Measured: adding
        // `#[serde(skip_serializing_if = "Option::is_none")]` to the field —
        // the one-line change that undoes the ruling — left this test green.
        // `get` returns `Option<&Value>` and separates the two where `[]`
        // cannot (判据 §2: the assertion that carried the ruling was the one
        // that could not see its own reversal).
        let button_states = |raw: &RawDom| -> (NodeStates, String, Option<serde_json::Value>) {
            let mut refs = RefTable::new();
            let state = PageState::build(
                raw,
                &mut refs,
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            );
            let at = state
                .nodes
                .iter()
                .position(|n| n.backend_node_id == 5)
                .expect("the button is in the tree");
            (
                state.nodes[at].states,
                render_text(&state),
                to_json(&state)["nodes"][at]["states"]
                    .get("focused")
                    .cloned(),
            )
        };

        // 1. No producer has looked — every Chromium capture, today.
        let mut raw = fixture();
        let (states, text, json) = button_states(&raw);
        assert_eq!(states.focused, None);
        assert!(!text.contains("[focused]"), "text asserts nothing:\n{text}");
        assert_eq!(
            json,
            Some(serde_json::Value::Null),
            "the JSON face must carry `focused: null` — PRESENT and unknown. \
             `None` here means the key was dropped, which is the same silence \
             a `bool` gave and is what this ruling exists to prevent; a bare \
             `Value::Null` expectation cannot tell those apart"
        );

        // 2. A fetcher looked and the caret is elsewhere. Silent in text —
        //    exactly one node per document has it, so an `[unfocused]` on all
        //    the others would be noise — but a REAL observation in JSON.
        raw.frames[0].nodes[4].focused = Some(false);
        let (states, text, json) = button_states(&raw);
        assert_eq!(states.focused, Some(false));
        assert!(!text.contains("[focused]"), "{text}");
        assert_eq!(
            json,
            Some(serde_json::json!(false)),
            "an observation that the caret is elsewhere reads as 'nobody looked'"
        );

        // 3. The caret is here.
        raw.frames[0].nodes[4].focused = Some(true);
        let (states, text, json) = button_states(&raw);
        assert_eq!(states.focused, Some(true));
        assert!(text.contains("[focused]"), "{text}");
        assert_eq!(json, Some(serde_json::json!(true)));
    }

    /// **An absent attribute is silence, and silence is not a denial.**
    ///
    /// The fallback used to answer `Some(has_attr("checked"))`, so a checkbox
    /// the page never marked — which is nearly every checkbox — rendered
    /// `[unchecked]`: "somebody looked and the answer is no", manufactured out
    /// of an absence (判据 §8). Round 3 ruled the other way for the twin, in a
    /// sentence that transfers verbatim: *"answering 'the caret is elsewhere'
    /// would be its own invention"*. It was pinned by a passing whole-line
    /// expectation in `render.rs`, which is why it survived three rounds.
    ///
    /// The three cases are asserted together because the rule is about which
    /// of them are the same: **the page saying yes**, **the page saying no**,
    /// and **the page saying nothing** — and only the third is silence.
    #[test]
    fn an_absent_attribute_is_silence_rather_than_a_denial() {
        let built = |attrs: &[(&str, &str)]| {
            let mut raw = fixture();
            raw.frames[1].nodes[2].attrs = attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            PageState::build(
                &raw,
                &mut RefTable::new(),
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            )
        };
        let checkbox = |state: &PageState| {
            state
                .nodes
                .iter()
                .find(|n| n.backend_node_id == 22)
                .expect("the fixture checkbox")
                .states
                .checked
        };

        let silent = built(&[("type", "checkbox")]);
        assert_eq!(
            checkbox(&silent),
            None,
            "a checkbox the page never marked is not a checkbox someone \
             looked at and found off"
        );
        let rendered = render_text(&silent);
        assert!(
            !rendered.contains("[unchecked]") && !rendered.contains("[checked]"),
            "an invented state token reached the text tree:\n{rendered}"
        );

        // The page saying yes. Weak evidence — it is the INITIAL state — but it
        // is evidence, and it is all there is until a fetcher looks.
        assert_eq!(
            checkbox(&built(&[("type", "checkbox"), ("checked", "")])),
            Some(true),
            "the page's own markup stopped reaching the state"
        );

        // The page saying no, which is NOT silence: `aria-checked="false"` is
        // an assertion, and the model gets it.
        let denied = built(&[("type", "checkbox"), ("aria-checked", "false")]);
        assert_eq!(checkbox(&denied), Some(false));
        assert!(
            render_text(&denied).contains("[unchecked]"),
            "a page that says \"not checked\" is answering, and the answer \
             must print — otherwise this rule is indistinguishable from one \
             that stopped reading the attribute at all"
        );
    }

    /// **A capture that read the live properties does not consult the markup —
    /// and its silence about a node is an answer, not a gap.**
    ///
    /// This is the obligation that could otherwise undo the whole round.
    /// `inputChecked` is a rare-boolean index list, so a fetcher doing the
    /// obvious thing — walk the list, set what it finds — leaves `None` on
    /// every node whose property is *false*. Per-node, that `None` is
    /// indistinguishable from "no fetcher ran", the attribute fallback takes
    /// over, and a box the agent just unchecked reads `[checked]` off stale
    /// markup with every test green.
    ///
    /// `RawFrame::live_properties_observed` moves that obligation from every
    /// node to one declaration per capture: when it is set, absence within a
    /// checkable role IS `false`, and the page's attributes are not consulted
    /// at all. Task 11 cannot get it wrong per-node because there is no
    /// per-node rule left to get wrong.
    #[test]
    fn a_capture_that_read_the_properties_never_falls_back_to_the_markup() {
        let built = |live: bool, fetcher_said: Option<bool>| {
            let mut raw = fixture();
            raw.frames[1].live_properties_observed = live;
            // Markup says checked; the fetcher's reading is the argument.
            raw.frames[1].nodes[2].checked = fetcher_said;
            PageState::build(
                &raw,
                &mut RefTable::new(),
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            )
        };
        let checkbox = |state: &PageState| {
            state
                .nodes
                .iter()
                .find(|n| n.backend_node_id == 22)
                .expect("the fixture checkbox")
                .states
                .checked
        };

        // The defect this prevents, stated as the case: the fetcher looked and
        // did not list this node, so the property is false — while the markup
        // still says `checked` because the attribute is the initial state.
        assert_eq!(
            checkbox(&built(true, None)),
            Some(false),
            "a capture that read the properties fell back to the page's stale \
             markup for a node the fetcher did not list — which is the exact \
             shape a fetcher produces for every control that is OFF"
        );
        // Same capture, same silence, but nobody looked: now the markup is the
        // only evidence there is and it counts.
        assert_eq!(
            checkbox(&built(false, None)),
            Some(true),
            "with no fetcher the page's markup is the only evidence and must \
             still reach the state"
        );
        // A node the fetcher DID list is unaffected by the flag.
        assert_eq!(checkbox(&built(true, Some(true))), Some(true));

        // Applicability still comes from the role: a live capture answers
        // `false` for things that can be checked, not for everything.
        let live = built(true, None);
        let button = live
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 5)
            .expect("the fixture button");
        assert_eq!(
            button.states.checked, None,
            "\"nothing to check here\" is not \"checked: no\" — a live capture \
             must not answer for a node the question does not apply to"
        );

        // `selected`, the twin, through the same flag.
        let mut raw = fixture();
        raw.frames[1].live_properties_observed = true;
        raw.frames[1].nodes[3].tag = Some("option".to_string());
        raw.frames[1].nodes[3].attrs = vec![("selected".to_string(), String::new())];
        let state = PageState::build(
            &raw,
            &mut RefTable::new(),
            1,
            "https://example.test/hn",
            "t",
            Duration::from_secs(0),
        );
        let option = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 23)
            .expect("the option");
        assert_eq!(
            option.states.selected,
            Some(false),
            "the `selected` twin still reads the page's markup under a capture \
             that looked (判据 §16)"
        );
        assert!(
            render_text(&state).contains("[unselected]"),
            "and the model is told so: an option nobody can tell apart from an \
             unobserved one is a line the model cannot act on"
        );

        // **A live capture answers only where the engine reports a property.**
        // `inputChecked` covers `<input type=checkbox|radio>`; `optionSelected`
        // covers `<option>`. A `<div role="checkbox">` has no checkedness for a
        // capture to have read, so its silence says nothing about one — and the
        // page's `aria-checked` stays the only evidence there is. Deriving
        // `false` there would be this round's own invention aimed at ARIA
        // widgets, and deriving it for `Row`/`Cell` — which the *attribute*
        // gate admits — put `[unselected]` on every `<td>` of a table page.
        let aria_widget = |attrs: &[(&str, &str)]| {
            let mut raw = fixture();
            raw.frames[1].live_properties_observed = true;
            raw.frames[1].nodes[3].tag = Some("div".to_string());
            raw.frames[1].nodes[3].attrs = attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            let state = PageState::build(
                &raw,
                &mut RefTable::new(),
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            );
            state
                .nodes
                .iter()
                .find(|n| n.backend_node_id == 23)
                .expect("the widget")
                .states
        };
        assert_eq!(
            aria_widget(&[("role", "checkbox")]).checked,
            None,
            "a live capture answered for an ARIA widget whose checkedness no \
             engine reports — silence about a property that does not exist is \
             not an observation of `false`"
        );
        assert_eq!(
            aria_widget(&[("role", "checkbox"), ("aria-checked", "true")]).checked,
            Some(true),
            "and the page's own assertion must still reach the model under a \
             live capture: there is no property here to outrank it"
        );
        assert_eq!(
            aria_widget(&[("role", "row")]).selected,
            None,
            "a live capture answered for a row's selectedness, which no engine \
             reports — on a table page that is `[unselected]` on nearly every \
             line"
        );
    }

    /// **A control's value is the property, not the markup — the third member
    /// of the class, and the one the membership rule named a round before
    /// anybody ran it over this function.**
    ///
    /// `value_of` read `node.attr("value")`: the content attribute, which is
    /// what the control *started* with. `value`'s IDL twin is `defaultValue`,
    /// which is the mechanical form of "does this go stale" — and it parts from
    /// the property the moment anyone types, including when the typist is the
    /// agent's own `fill`. The JSON face then carries the original text of a box
    /// the agent has already filled in.
    ///
    /// Censused at `446271740`: `value_of` had **no test of any kind** — not
    /// even for the submit-button rule its own doc explains, which is why that
    /// rule is the control here.
    #[test]
    fn a_controls_value_is_the_property_and_the_markup_is_only_a_fallback() {
        let built = |live: bool, fetcher_said: Option<&str>, attrs: &[(&str, &str)]| {
            let mut raw = fixture();
            raw.frames[1].live_properties_observed = live;
            raw.frames[1].nodes[3].value = fetcher_said.map(str::to_string);
            raw.frames[1].nodes[3].attrs = attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            let state = PageState::build(
                &raw,
                &mut RefTable::new(),
                1,
                "https://example.test/hn",
                "t",
                Duration::from_secs(0),
            );
            state
                .nodes
                .iter()
                .find(|n| n.backend_node_id == 23)
                .expect("the text input")
                .value
                .clone()
        };

        // Nobody looked: the page's markup is the only evidence there is.
        assert_eq!(
            built(false, None, &[("type", "text"), ("value", "search me")]),
            Some("search me".to_string()),
            "the markup fallback stopped working"
        );
        // The fetcher looked: what is in the box now beats what it started with.
        assert_eq!(
            built(
                false,
                Some("typed by the agent"),
                &[("type", "text"), ("value", "search me")]
            ),
            Some("typed by the agent".to_string()),
            "the page's initial value outranked the live property — the model \
             is reading back the text of a box the agent has already filled"
        );
        // A capture that read `inputValue` lists the controls that have one, so
        // silence means empty and the stale markup is not consulted at all.
        assert_eq!(
            built(true, None, &[("type", "text"), ("value", "search me")]),
            None,
            "a capture that read the values fell back to the page's markup"
        );
        // Control, and the rule `value_of`'s doc has always claimed: a submit
        // button's `value` is its NAME (accname rule 4 spent it), so it must
        // not also arrive as a value.
        assert_eq!(
            built(false, Some("Go"), &[("type", "submit"), ("value", "Go")]),
            None,
            "a submit button's value printed twice — once as its name and once \
             as its value — and the model has two facts where there is one"
        );
    }

    /// **A page cannot write a state token into the model's observation.**
    ///
    /// `render::quote` defends page-controlled *strings*; a state token is a
    /// page-controlled *predicate* and reaches the line as itself, so the
    /// defence has to be at the derivation and this test has to go through the
    /// real one. `node_states_are_printed_and_absent_states_print_nothing`
    /// cannot see this class at all: it constructs `NodeStates` by hand and
    /// never calls `states_of`.
    ///
    /// Both forged tokens were reachable before this round. `:` is a legal HTML
    /// attribute character — and `:`-prefixed names are ordinary Vue/Alpine
    /// `:prop` output left in a served DOM — so `<button :focus>` rendered
    /// `[focused]`, an *observed* claim about where the caret is. `<div
    /// selected>` rendered `[selected]` because `checked` had been role-gated
    /// and its twin had not (判据 §16).
    ///
    /// The positive halves are the substance: `aria-selected` on a real
    /// `<option>` still reaches the line, so this test cannot be satisfied by a
    /// renderer that simply stopped printing states (判据 §2).
    #[test]
    fn a_page_authored_attribute_cannot_become_a_state_token() {
        let mut forged = fixture();
        // Straight onto the fixture's own `<button>` and `<div>`, so the whole
        // builder runs exactly as it does for a real capture.
        forged.frames[0].nodes[4]
            .attrs
            .push((":focus".to_string(), String::new()));
        forged.frames[0].nodes[4]
            .attrs
            .push(("selected".to_string(), String::new()));
        let mut refs = RefTable::new();
        let state = PageState::build(
            &forged,
            &mut refs,
            1,
            "https://example.test/hn",
            "t",
            Duration::from_secs(0),
        );
        let button = state
            .nodes
            .iter()
            .find(|n| n.backend_node_id == 5)
            .expect("button");
        assert_eq!(
            button.states.focused, None,
            "a page wrote `:focus` and it reached the state — and `None` rather \
             than `Some(false)` is the claim: nobody LOOKED, so answering \
             \"the caret is elsewhere\" would be its own invention"
        );
        assert_eq!(
            button.states.selected, None,
            "a page wrote `selected` on a button and the model was told it is \
             selected — and `None` rather than `Some(false)`, because a button \
             is not selectable and \"not applicable\" is not \"not selected\""
        );
        let rendered = render_text(&state);
        assert!(
            !rendered.contains("[focused]") && !rendered.contains("[selected]"),
            "a forged state token reached the text tree:\n{rendered}"
        );

        // The positive halves, so the assertions above are not satisfied by a
        // renderer that prints no states at all.
        let mut real = fixture();
        real.frames[0].nodes[4].focused = Some(true);
        real.frames[1].nodes[2].attrs = vec![
            ("aria-label".to_string(), "Remember me".to_string()),
            ("aria-selected".to_string(), "true".to_string()),
        ];
        real.frames[1].nodes[2].tag = Some("option".to_string());
        let mut refs = RefTable::new();
        let state = PageState::build(
            &real,
            &mut refs,
            1,
            "https://example.test/hn",
            "t",
            Duration::from_secs(0),
        );
        let rendered = render_text(&state);
        assert!(
            rendered.contains("[focused]"),
            "a FETCHER said the caret is here and the model was not told:\n{rendered}"
        );
        assert!(
            rendered.contains("[selected]"),
            "`aria-selected` on an <option> is the page describing itself, \
             which it is entitled to do:\n{rendered}"
        );
    }
}

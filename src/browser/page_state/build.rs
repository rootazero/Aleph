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
        name_from_content: name.from_content,
        name: name.text,
        value: value_of(node),
        states: states_of(node, role),
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
fn value_of(node: &RawNode) -> Option<String> {
    if node.tag_lower() == "input" {
        let ty = node.attr("type").unwrap_or("text").to_ascii_lowercase();
        if matches!(ty.as_str(), "button" | "submit" | "reset") {
            return None;
        }
    }
    node.attr("value").map(normalize).filter(|v| !v.is_empty())
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
/// - `checked` and `selected` are **role-gated**, because the bare attribute is
///   meaningful only on elements that can carry the state. They are twins and
///   both get the treatment (判据 §16): a `<div selected>` used to render
///   `[selected]` because only one of the two had been gated.
/// - `focused` comes from [`RawNode::focused`], a field, and never from
///   `attrs`. It is the one bit in this set the page must not be able to
///   assert at all, and it lived in `attrs` as `":focus"` until a page could
///   forge it — see `RawNode`'s doc for the mechanism.
fn states_of(node: &RawNode, role: Role) -> NodeStates {
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
        // `None` for anything that is not checkable: "this is not a checkbox"
        // and "this checkbox is off" are different facts, and the renderer
        // prints them differently.
        checked: match role {
            Role::Checkbox | Role::Radio | Role::MenuItem => {
                Some(node.has_attr("checked") || aria_true("aria-checked"))
            }
            _ => aria_bool("aria-checked"),
        },
        expanded: aria_bool("aria-expanded"),
        // `checked`'s twin, gated the same way: the bare `selected` attribute
        // says something only on an element that can be selected. `<div
        // selected>` is not a selected anything.
        selected: match role {
            Role::Option | Role::Tab | Role::Row | Role::Cell => {
                node.has_attr("selected") || aria_true("aria-selected")
            }
            _ => aria_true("aria-selected"),
        },
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

    /// **No state bit may vanish from the JSON face when it happens to be
    /// unset.** The class, rather than the one member of it that had a ruling.
    ///
    /// `focused`'s guard above is about one field because one field had a
    /// ruling attached. The defect underneath it is not about `focused` at
    /// all: a `skip_serializing_if` on ANY of these seven turns "we looked and
    /// the answer is no" into the same silence as "nobody looked", and
    /// `checked` and `expanded` are `Option<bool>` for exactly the reason
    /// `focused` now is — while having **no JSON assertion anywhere**
    /// (censused at `59d6c8c04`: `to_json` has two call sites in tests, and the
    /// other one asserts only non-null values, where `[]` cannot hide an absent
    /// key). So they were one attribute away from losing the distinction with
    /// nothing to notice.
    ///
    /// Derived from the type on both sides rather than checked against a
    /// written-down key list (判据 §5): an all-unset `NodeStates` and an
    /// all-set one must serialise to the **same key set**, and the struct
    /// literal below is exhaustive, so an eighth field cannot be added without
    /// being written into it.
    #[test]
    fn every_state_bit_keeps_its_key_in_json_whatever_its_value() {
        let keys = |states: NodeStates| -> Vec<String> {
            let value = serde_json::to_value(states).expect("NodeStates serialises");
            let mut out: Vec<String> = value
                .as_object()
                .expect("NodeStates is a JSON object")
                .keys()
                .cloned()
                .collect();
            out.sort();
            out
        };

        let unset = keys(NodeStates::default());
        let set = keys(NodeStates {
            disabled: true,
            checked: Some(true),
            expanded: Some(true),
            selected: true,
            required: true,
            readonly: true,
            focused: Some(true),
        });
        assert_eq!(
            unset, set,
            "a state bit disappears from JSON when it is unset, so \"we looked \
             and the answer is no\" and \"nobody looked\" read identically to \
             anything consuming the attachment"
        );
        assert!(
            !unset.is_empty(),
            "non-vacuity: the key sets are equal because both are empty"
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
        assert!(
            !button.states.selected,
            "a page wrote `selected` on a button and the model was told it is selected"
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

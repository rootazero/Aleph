//! Text editing for the whiteboard — a world-positioned `<textarea>` overlay
//! over a Geo/Note/Text shape.
//!
//! # How an edit session starts and ends
//!
//! Two producers open a session (both set the editor's `text_editing`
//! signal): double-clicking a shape whose variant carries text
//! ([`text_of`] `Some`, `fresh: false`), and the Text creation tool
//! ([`super::interaction::Effect::BeginTextEdit`], `fresh: true` — the shape
//! exists only as a local preview, nothing on the wire yet). The session
//! ends through exactly one funnel (`finish` below): blur and ⌘/Ctrl+Enter
//! commit, Esc discards. The outcome rules are pure functions here
//! ([`commit_outcome`] / [`cancel_outcome`]) so the discard-empty-fresh rule
//! the plan tests is unit-testable without a DOM.
//!
//! # Commits base on the *live* shape, not the snapshot
//!
//! The doc keeps moving while the user types (broadcast frames, a conflict
//! refetch). Committing the snapshot taken at double-click would silently
//! revert any concurrent geometry change, so [`commit_outcome`] re-reads the
//! shape from the live doc and only swaps its text; the snapshot is the
//! fallback for a shape that vanished mid-edit (the commit recreates it —
//! typed text must not be lost to someone else's delete). The `fresh` undo
//! is a `DeleteShape` by construction: inverting against a doc that already
//! holds the preview would "restore" the empty preview instead of removing
//! the creation.
//!
//! # Rendering while editing
//!
//! The overlay sits in the editor's world-transformed HTML layer, so it
//! positions in world units and rides every pan/zoom for free. While a
//! session is open the editor projects the shape into the SVG layer with its
//! text cleared ([`with_text`] with `""`) — outline and card stay visible
//! under the textarea, but the text exists in exactly one place. The
//! textarea is fixed to the shape's bbox and scrolls beyond it (`overflow-y:
//! auto`) — auto-growing the *shape* while typing is deliberately not done
//! here (the SVG text overflows its box identically today; both grow
//! together or not at all).

use aleph_protocol::canvas::{CanvasDoc, CanvasOp, Shape, ShapeStyle};
use leptos::prelude::*;

use super::ops;
use super::shape_view;
use crate::state::canvas::CanvasState;

/// One open text-editing session.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TextEditState {
    /// The shape as it was when editing began. For `fresh` sessions this is
    /// the only copy that survives a conflict refetch wiping the preview.
    pub shape: Shape,
    /// Born in this gesture and never committed: an empty commit (or Esc)
    /// discards the shape instead of writing an invisible empty one.
    pub fresh: bool,
}

/// The text a shape carries, for the variants the overlay can edit.
/// Frame titles and arrow labels are deliberately not editable here — their
/// text is decoration on another gesture, not body content.
#[must_use]
pub(super) fn text_of(shape: &Shape) -> Option<&str> {
    match shape {
        Shape::Geo { text, .. } | Shape::Text { text, .. } | Shape::Note { text, .. } => Some(text),
        _ => None,
    }
}

/// The same shape with its text replaced; non-text variants pass through
/// unchanged (no caller edits them, but a clone is safer than a panic).
#[must_use]
pub(super) fn with_text(shape: &Shape, text: &str) -> Shape {
    let mut s = shape.clone();
    match &mut s {
        Shape::Geo { text: t, .. } | Shape::Text { text: t, .. } | Shape::Note { text: t, .. } => {
            *t = text.to_string();
        }
        _ => {}
    }
    s
}

fn style_of(shape: &Shape) -> Option<&ShapeStyle> {
    match shape {
        Shape::Geo { style, .. } | Shape::Text { style, .. } | Shape::Note { style, .. } => {
            Some(style)
        }
        _ => None,
    }
}

/// Textarea inset matching where `shape_view.rs` puts each variant's text
/// (Geo: x+10/y+8, Note: x+12/y+10, Text: at the corner).
#[must_use]
fn text_inset(shape: &Shape) -> (f64, f64) {
    match shape {
        Shape::Geo { .. } => (10.0, 8.0),
        Shape::Note { .. } => (12.0, 10.0),
        _ => (0.0, 0.0),
    }
}

/// What ending a session does.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum TextEditOutcome {
    /// Nothing changed — close the overlay, touch nothing.
    Keep,
    /// Remove the local preview of a fresh shape; nothing ever hit the wire.
    Discard { id: String },
    /// A real edit: optimistic-apply, record undo, send (the editor's
    /// standard commit path).
    Commit {
        redo: Vec<CanvasOp>,
        undo: Vec<CanvasOp>,
    },
}

/// Outcome of committing `text` (blur / ⌘Enter). `doc` is the live document
/// — module doc explains why the live shape, not the snapshot, is the base.
#[must_use]
pub(super) fn commit_outcome(
    edit: &TextEditState,
    doc: Option<&CanvasDoc>,
    text: &str,
) -> TextEditOutcome {
    if edit.fresh && text.trim().is_empty() {
        return TextEditOutcome::Discard {
            id: edit.shape.id().to_string(),
        };
    }
    let live = doc.and_then(|d| d.shapes.iter().find(|s| s.id() == edit.shape.id()));
    let base = live.unwrap_or(&edit.shape);
    if !edit.fresh && text_of(base) == Some(text) {
        return TextEditOutcome::Keep;
    }
    let redo = vec![CanvasOp::UpsertShape {
        shape: with_text(base, text),
    }];
    let undo = if edit.fresh {
        vec![CanvasOp::DeleteShape {
            id: edit.shape.id().to_string(),
        }]
    } else {
        vec![CanvasOp::UpsertShape {
            shape: base.clone(),
        }]
    };
    TextEditOutcome::Commit { redo, undo }
}

/// Outcome of abandoning the session (Esc): a fresh shape is discarded, an
/// existing one keeps its pre-edit text (the doc never held the draft — the
/// textarea did).
#[must_use]
pub(super) fn cancel_outcome(edit: &TextEditState) -> TextEditOutcome {
    if edit.fresh {
        TextEditOutcome::Discard {
            id: edit.shape.id().to_string(),
        }
    } else {
        TextEditOutcome::Keep
    }
}

/// The overlay: renders a textarea over the session's shape while
/// `editing` is `Some`. Mounted inside the editor's world-transformed
/// overlay layer, so all geometry below is world units.
#[component]
pub(super) fn TextEditOverlay(
    editing: RwSignal<Option<TextEditState>>,
    /// The editor's commit path: optimistic-apply + undo record + send.
    #[prop(into)]
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
) -> impl IntoView {
    move || {
        editing
            .get()
            .map(|edit| session_view(editing, on_commit, edit))
    }
}

/// One edit session's DOM. Rebuilt per session — the `editing` signal only
/// transitions None↔Some (the editor never swaps one session for another
/// without closing the first through `finish`).
fn session_view(
    editing: RwSignal<Option<TextEditState>>,
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
    edit: TextEditState,
) -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let c = edit.shape.common().clone();
    let (pad_x, pad_y) = text_inset(&edit.shape);
    let style = style_of(&edit.shape).cloned().unwrap_or_default();
    let fs = shape_view::font_size_for(style.size);
    // Notes always write in the primary text token (their card supplies the
    // color); Geo/Text follow the shape's palette slot — the same rule the
    // SVG renderer applies, via the same function.
    let color = match edit.shape {
        Shape::Note { .. } => "var(--color-text-primary)",
        _ => shape_view::text_fill(&style),
    };
    let initial = text_of(&edit.shape).unwrap_or_default().to_string();

    let ta: NodeRef<leptos::html::Textarea> = NodeRef::new();
    Effect::new(move |_| {
        if let Some(el) = ta.get() {
            let _ = el.focus();
            // Caret to the end. `set_selection_range` clamps past-the-end
            // positions, so the byte-vs-UTF-16 length mismatch is harmless.
            let len = el.value().len() as u32;
            let _ = el.set_selection_range(len, len);
        }
    });

    // The single exit funnel. Every capture is `Copy`, so the closure is too
    // (blur and keydown each hold a copy). The `editing` take-and-clear guard
    // makes a second entry (⌘Enter's commit followed by the unmount-blur) a
    // no-op instead of a double commit.
    let finish = move |discard: bool| {
        let Some(edit) = editing.try_get_untracked().flatten() else {
            return;
        };
        editing.set(None);
        let text = ta.get_untracked().map(|el| el.value()).unwrap_or_default();
        let outcome = if discard {
            cancel_outcome(&edit)
        } else {
            canvas
                .doc
                .with_untracked(|d| commit_outcome(&edit, d.as_ref(), &text))
        };
        match outcome {
            TextEditOutcome::Keep => {}
            TextEditOutcome::Discard { id } => {
                // Local removal only — the fresh preview never hit the wire,
                // so there is nothing to undo and nothing to send.
                canvas.doc.update(|d| {
                    if let Some(d) = d.as_mut() {
                        ops::apply_local(d, &[CanvasOp::DeleteShape { id: id.clone() }]);
                    }
                });
            }
            TextEditOutcome::Commit { redo, undo } => on_commit.run((redo, undo)),
        }
    };

    view! {
        <textarea
            node_ref=ta
            class="absolute block resize-none border-none outline-none bg-transparent overflow-y-auto"
            style=format!(
                "left: {}px; top: {}px; width: {}px; height: {}px; \
                 padding: {}px {}px; font-size: {}px; line-height: 1.4; \
                 color: {}; pointer-events: auto;",
                c.x,
                c.y,
                c.w.max(40.0),
                c.h.max(24.0),
                pad_y,
                pad_x,
                fs,
                color,
            )
            prop:value=initial
            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:dblclick=|ev: web_sys::MouseEvent| ev.stop_propagation()
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                let key = ev.key();
                if key == "Escape" {
                    // Ours alone: the editor's window listener would read an
                    // Esc as "cancel the (nonexistent) drag".
                    ev.stop_propagation();
                    ev.prevent_default();
                    finish(true);
                } else if key == "Enter" && (ev.meta_key() || ev.ctrl_key()) {
                    ev.prevent_default();
                    finish(false);
                }
            }
            on:blur=move |_| finish(false)
        ></textarea>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon};

    fn common(id: &str, x: f64) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x,
            y: 0.0,
            w: 200.0,
            h: 100.0,
            z: FracIndex::first(),
            parent_id: None,
        }
    }

    fn text_shape(id: &str, x: f64, text: &str) -> Shape {
        Shape::Text {
            common: common(id, x),
            style: ShapeStyle::default(),
            text: text.to_string(),
        }
    }

    fn doc_with(shapes: Vec<Shape>) -> CanvasDoc {
        CanvasDoc {
            id: "cv-1".to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes,
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn committing_text_on_a_fresh_create_upserts_and_undoes_by_delete() {
        let edit = TextEditState {
            shape: text_shape("t1", 0.0, ""),
            fresh: true,
        };
        let doc = doc_with(vec![edit.shape.clone()]); // the local preview
        let TextEditOutcome::Commit { redo, undo } = commit_outcome(&edit, Some(&doc), "hello")
        else {
            panic!("a fresh shape with text must commit");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("commit upserts");
        };
        assert_eq!(text_of(shape), Some("hello"));
        assert_eq!(
            undo,
            vec![CanvasOp::DeleteShape { id: "t1".into() }],
            "undoing a fresh creation deletes it — inverting against the doc \
             would resurrect the empty preview instead"
        );
    }

    #[test]
    fn an_empty_or_whitespace_text_on_a_fresh_create_discards_the_shape() {
        let edit = TextEditState {
            shape: text_shape("t1", 0.0, ""),
            fresh: true,
        };
        let doc = doc_with(vec![edit.shape.clone()]);
        for empty in ["", "   ", "\n\t "] {
            assert_eq!(
                commit_outcome(&edit, Some(&doc), empty),
                TextEditOutcome::Discard { id: "t1".into() },
                "empty text {empty:?} on a fresh create must discard"
            );
        }
        // An *existing* shape emptied out commits the empty text instead —
        // the user is clearing a shape they own, not abandoning a creation.
        let existing = TextEditState {
            shape: text_shape("t2", 0.0, "old"),
            fresh: false,
        };
        let doc = doc_with(vec![existing.shape.clone()]);
        assert!(matches!(
            commit_outcome(&existing, Some(&doc), ""),
            TextEditOutcome::Commit { .. }
        ));
    }

    #[test]
    fn an_unchanged_text_on_an_existing_shape_commits_nothing() {
        let edit = TextEditState {
            shape: text_shape("t1", 0.0, "same"),
            fresh: false,
        };
        let doc = doc_with(vec![edit.shape.clone()]);
        assert_eq!(
            commit_outcome(&edit, Some(&doc), "same"),
            TextEditOutcome::Keep
        );
    }

    #[test]
    fn a_text_change_bases_on_the_live_shape_not_the_snapshot() {
        // The shape moved (broadcast) while the user typed: the commit must
        // keep the live geometry and only swap the text.
        let edit = TextEditState {
            shape: text_shape("t1", 0.0, "old"),
            fresh: false,
        };
        let doc = doc_with(vec![text_shape("t1", 99.0, "old")]);
        let TextEditOutcome::Commit { redo, undo } = commit_outcome(&edit, Some(&doc), "new")
        else {
            panic!("changed text must commit");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("commit upserts");
        };
        assert_eq!(shape.common().x, 99.0, "live geometry survives the edit");
        assert_eq!(text_of(shape), Some("new"));
        assert_eq!(
            undo,
            vec![CanvasOp::UpsertShape {
                shape: text_shape("t1", 99.0, "old")
            }],
            "undo restores the live shape, not the stale snapshot"
        );
    }

    #[test]
    fn a_vanished_shape_commits_from_the_snapshot() {
        // Deleted (or conflict-wiped) mid-edit: the typed text must not be
        // lost — the commit recreates the shape from the snapshot.
        let edit = TextEditState {
            shape: text_shape("t1", 5.0, "old"),
            fresh: false,
        };
        let doc = doc_with(vec![]);
        let TextEditOutcome::Commit { redo, .. } = commit_outcome(&edit, Some(&doc), "kept") else {
            panic!("must commit from the snapshot");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("commit upserts");
        };
        assert_eq!(shape.common().x, 5.0);
        assert_eq!(text_of(shape), Some("kept"));
    }

    #[test]
    fn cancel_keeps_existing_shapes_and_discards_only_fresh_ones() {
        let fresh = TextEditState {
            shape: text_shape("t1", 0.0, ""),
            fresh: true,
        };
        assert_eq!(
            cancel_outcome(&fresh),
            TextEditOutcome::Discard { id: "t1".into() }
        );
        let existing = TextEditState {
            shape: text_shape("t2", 0.0, "body"),
            fresh: false,
        };
        assert_eq!(cancel_outcome(&existing), TextEditOutcome::Keep);
    }

    #[test]
    fn with_text_writes_the_three_editable_variants_and_passes_others_through() {
        let geo = Shape::Geo {
            common: common("g", 0.0),
            form: aleph_protocol::canvas::GeoForm::Rect,
            style: ShapeStyle::default(),
            text: String::new(),
        };
        let note = Shape::Note {
            common: common("n", 0.0),
            style: ShapeStyle::default(),
            text: String::new(),
        };
        for s in [&geo, &note, &text_shape("t", 0.0, "")] {
            assert_eq!(text_of(&with_text(s, "x")), Some("x"));
        }
        let frame = Shape::Frame {
            common: common("f", 0.0),
            title: "title".to_string(),
            aspect_locked: false,
        };
        assert_eq!(text_of(&frame), None, "frame titles are not body text");
        assert_eq!(
            with_text(&frame, "x"),
            frame,
            "non-text variants pass through"
        );
    }
}

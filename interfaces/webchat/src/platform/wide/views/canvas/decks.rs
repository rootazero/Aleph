//! Slide decks over the whiteboard — the drawer that groups
//! [`Shape::Frame`]s into ordered [`Deck`]s, plus the pure ops math behind
//! every drawer action (create / reorder / delete).
//!
//! A deck is document data, not UI state: it travels as `UpsertDeck` /
//! `DeleteDeck` ops through the editor's optimistic commit funnel (the
//! `on_commit` prop — the same channel as a drag), so deck edits are
//! undoable, broadcast to every client, and revision-checked like any other
//! edit. `deck.frame_ids` itself is the slide order — no fractional index,
//! reordering rewrites the whole (small) list.
//!
//! The drawer lives in the editor's screen-space action cluster (top-right),
//! which already stops pointer propagation; playing hands the deck id to the
//! editor, which mounts `present::PresentOverlay` (`on_play`).

use aleph_protocol::canvas::{CanvasDoc, CanvasOp, Deck, Shape};
use leptos::prelude::*;

use super::{id_mint, ops, present};
use crate::i18n::{t, t_string, use_i18n};
use crate::state::canvas::CanvasState;

// ---------------------------------------------------------------------------
// Pure deck math — unit-tested, zero DOM.
// ---------------------------------------------------------------------------

/// The selected [`Shape::Frame`]s in slide reading order: center-x ascending
/// (slides lay out left-to-right — both the Frame tool's row habit and the
/// model's "insert N 16:9 frames" flow), center-y as the tie-break for
/// vertical stacks, id last so equal geometry still orders stably.
///
/// Selection order is deliberately NOT used: a marquee reports document
/// order, which is creation order, not what the user sees. The drawer's drag
/// reorder is the correction path for layouts this heuristic misreads.
#[must_use]
pub(super) fn selected_frames(shapes: &[Shape], selection: &[String]) -> Vec<Shape> {
    let mut frames: Vec<&Shape> = shapes
        .iter()
        .filter(|s| matches!(s, Shape::Frame { .. }) && selection.iter().any(|id| id == s.id()))
        .collect();
    frames.sort_by(|a, b| {
        let (ac, bc) = (a.common(), b.common());
        (ac.x + ac.w / 2.0)
            .total_cmp(&(bc.x + bc.w / 2.0))
            .then_with(|| (ac.y + ac.h / 2.0).total_cmp(&(bc.y + bc.h / 2.0)))
            .then_with(|| a.id().cmp(b.id()))
    });
    frames.into_iter().cloned().collect()
}

/// Ops for "create a deck from the selected frames": one `UpsertDeck` whose
/// `frame_ids` are [`selected_frames`] order. `None` when the selection
/// holds no frame — the button is disabled then, and a keyboard race must
/// not mint an empty deck.
#[must_use]
pub(super) fn create_deck_ops(
    doc: &CanvasDoc,
    selection: &[String],
    deck_id: String,
    title: String,
) -> Option<(Vec<CanvasOp>, Vec<CanvasOp>, String)> {
    let frames = selected_frames(&doc.shapes, selection);
    if frames.is_empty() {
        return None;
    }
    let deck = Deck {
        id: deck_id.clone(),
        title,
        frame_ids: frames.iter().map(|f| f.id().to_string()).collect(),
    };
    let redo = vec![CanvasOp::UpsertDeck { deck }];
    let undo = ops::invert(doc, &redo);
    Some((redo, undo, deck_id))
}

/// Move the entry at `from` so it ends up at index `to` in the result — the
/// drop-onto-a-row semantics (the dragged slide takes the target row's
/// visual position, in both directions). `None` for a no-op (`from == to`)
/// or an out-of-range index: a stale drop must produce no op at all rather
/// than a corrupted order.
#[must_use]
pub(super) fn reorder_frame_ids(ids: &[String], from: usize, to: usize) -> Option<Vec<String>> {
    if from == to || from >= ids.len() || to >= ids.len() {
        return None;
    }
    let mut v = ids.to_vec();
    let item = v.remove(from);
    // `to` indexes the shortened list, which is exactly "the final index of
    // the dragged entry" in both directions (the test pins this).
    v.insert(to, item);
    Some(v)
}

/// [`reorder_frame_ids`] lifted to ops against the live document: an
/// `UpsertDeck` carrying the new order, with the inverse restoring the old.
#[must_use]
pub(super) fn reorder_deck_ops(
    doc: &CanvasDoc,
    deck_id: &str,
    from: usize,
    to: usize,
) -> Option<(Vec<CanvasOp>, Vec<CanvasOp>)> {
    let deck = doc.decks.iter().find(|d| d.id == deck_id)?;
    let frame_ids = reorder_frame_ids(&deck.frame_ids, from, to)?;
    let redo = vec![CanvasOp::UpsertDeck {
        deck: Deck {
            id: deck.id.clone(),
            title: deck.title.clone(),
            frame_ids,
        },
    }];
    let undo = ops::invert(doc, &redo);
    Some((redo, undo))
}

/// Delete a deck (the frames stay — a deck only references them). `None`
/// for an id the document does not hold.
#[must_use]
pub(super) fn delete_deck_ops(
    doc: &CanvasDoc,
    deck_id: &str,
) -> Option<(Vec<CanvasOp>, Vec<CanvasOp>)> {
    if !doc.decks.iter().any(|d| d.id == deck_id) {
        return None;
    }
    let redo = vec![CanvasOp::DeleteDeck {
        id: deck_id.to_string(),
    }];
    let undo = ops::invert(doc, &redo);
    Some((redo, undo))
}

/// The title of the live [`Shape::Frame`] with this id — `None` when the id
/// resolves to nothing or to a non-frame shape, which the drawer renders as
/// a "missing frame" row (kept in the list so drag indices keep matching
/// `deck.frame_ids`, never silently dropped).
#[must_use]
pub(super) fn frame_title(shapes: &[Shape], frame_id: &str) -> Option<String> {
    shapes.iter().find_map(|s| match s {
        Shape::Frame { common, title, .. } if common.id == frame_id => Some(title.clone()),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Drawer component.
// ---------------------------------------------------------------------------

/// Precomputed per-deck render data — one `doc.with` per list render, so the
/// row markup below works on plain clones instead of nesting doc reads.
struct DeckRowData {
    deck: Deck,
    /// How many `frame_ids` still resolve to live frames — the Play gate
    /// (playback and this count share [`present::slide_frames`], so the
    /// button can never enable a show that would fit zero slides).
    live: usize,
    /// Per-entry labels aligned with `deck.frame_ids`; `None` = missing.
    labels: Vec<Option<String>>,
}

/// An in-flight slide-row drag: which deck's list it started in and from
/// which index. Indices refer to `deck.frame_ids` — the list renders missing
/// entries too, precisely so these indices cannot drift from the data.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SlideDrag {
    deck_id: String,
    from: usize,
}

/// The decks drawer: a chip in the editor's action cluster that expands into
/// the deck list — create-from-selection, per-deck play/delete, and drag
/// reorder of the slides inside an expanded deck.
#[component]
pub(super) fn DecksDrawer(
    /// The editor's optimistic commit funnel — same channel as every gesture.
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
    /// Starts fullscreen playback of the given deck id.
    on_play: Callback<String>,
) -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let expanded: RwSignal<Option<String>> = RwSignal::new(None);
    let dragging: RwSignal<Option<SlideDrag>> = RwSignal::new(None);
    let drag_over: RwSignal<Option<(String, usize)>> = RwSignal::new(None);

    let deck_count = Memo::new(move |_| {
        canvas
            .doc
            .with(|d| d.as_ref().map(|d| d.decks.len()).unwrap_or(0))
    });
    let selected_frame_count = Memo::new(move |_| {
        let sel = canvas.selection.get();
        canvas.doc.with(|d| {
            d.as_ref()
                .map(|d| selected_frames(&d.shapes, &sel).len())
                .unwrap_or(0)
        })
    });

    let on_create = move |_| {
        let sel = canvas.selection.get_untracked();
        let minted = canvas.doc.with_untracked(|d| {
            d.as_ref().and_then(|d| {
                let title = format!(
                    "{} {}",
                    t_string!(i18n, canvas.deck_default_title),
                    d.decks.len() + 1
                );
                create_deck_ops(d, &sel, format!("deck-{}", id_mint::mint_shape_id()), title)
            })
        });
        if let Some((redo, undo, new_id)) = minted {
            on_commit.run((redo, undo));
            expanded.set(Some(new_id));
        }
    };

    view! {
        {move || if !open.get() {
            view! {
                <button
                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border \
                           bg-surface-raised text-xs font-medium text-text-secondary \
                           hover:text-text-primary hover:border-primary/50 shadow-sm transition-colors"
                    on:click=move |_| open.set(true)
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="5" width="14" height="11" rx="1.5" />
                        <path d="M20 8v9a2 2 0 0 1-2 2H8" />
                    </svg>
                    {t!(i18n, canvas.decks_button)}
                    {move || {
                        let n = deck_count.get();
                        (n > 0).then(|| view! {
                            <span class="px-1.5 rounded-full bg-primary/15 text-primary text-[10px] \
                                         font-semibold">
                                {n.to_string()}
                            </span>
                        })
                    }}
                </button>
            }
            .into_any()
        } else {
            view! {
                <div class="w-72 max-h-[65vh] p-3 rounded-xl border border-border bg-surface-raised \
                            shadow-lg flex flex-col gap-2 overflow-y-auto">
                    <div class="flex items-center justify-between">
                        <div class="text-xs font-semibold text-text-primary">
                            {t!(i18n, canvas.decks_title)}
                        </div>
                        <button
                            class="p-1 rounded-md text-text-tertiary hover:text-text-primary \
                                   hover:bg-surface-sunken transition-colors"
                            on:click=move |_| open.set(false)
                        >
                            <svg width="13" height="13" viewBox="0 0 24 24" fill="none"
                                 stroke="currentColor" stroke-width="2" stroke-linecap="round">
                                <line x1="18" y1="6" x2="6" y2="18" />
                                <line x1="6" y1="6" x2="18" y2="18" />
                            </svg>
                        </button>
                    </div>

                    <button
                        class="px-3 py-1.5 rounded-lg bg-primary hover:bg-primary-hover text-white \
                               text-xs font-medium transition-colors disabled:opacity-50"
                        prop:disabled=move || selected_frame_count.get() == 0
                        on:click=on_create
                    >
                        {t!(i18n, canvas.deck_create)}
                        {move || {
                            let n = selected_frame_count.get();
                            (n > 0).then(|| format!(" ({n})"))
                        }}
                    </button>
                    {move || (selected_frame_count.get() == 0).then(|| view! {
                        <div class="text-xs text-text-tertiary">
                            {t!(i18n, canvas.deck_create_hint)}
                        </div>
                    })}

                    {move || {
                        let rows: Vec<DeckRowData> = canvas.doc.with(|d| {
                            d.as_ref()
                                .map(|d| {
                                    d.decks
                                        .iter()
                                        .map(|deck| DeckRowData {
                                            deck: deck.clone(),
                                            live: present::slide_frames(&d.shapes, deck).len(),
                                            labels: deck
                                                .frame_ids
                                                .iter()
                                                .map(|fid| frame_title(&d.shapes, fid))
                                                .collect(),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        });
                        if rows.is_empty() {
                            return view! {
                                <div class="py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, canvas.decks_empty)}
                                </div>
                            }
                            .into_any();
                        }
                        rows.into_iter()
                            .map(|row| deck_row(
                                row, canvas, i18n, expanded, dragging, drag_over, on_commit,
                                on_play,
                            ))
                            .collect_view()
                            .into_any()
                    }}
                </div>
            }
            .into_any()
        }}
    }
}

/// One deck's card in the drawer: header (expand toggle, title, count, play,
/// delete) plus — when expanded — the draggable slide rows.
///
/// A plain function, not a `#[component]`: it renders inside the list
/// closure above, which already re-runs per document change; a component
/// boundary here would only add prop plumbing for the same rebuild.
#[allow(clippy::too_many_arguments)]
fn deck_row(
    row: DeckRowData,
    canvas: CanvasState,
    i18n: crate::i18n::I18nCtx,
    expanded: RwSignal<Option<String>>,
    dragging: RwSignal<Option<SlideDrag>>,
    drag_over: RwSignal<Option<(String, usize)>>,
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
    on_play: Callback<String>,
) -> impl IntoView {
    let DeckRowData { deck, live, labels } = row;
    let deck_id = deck.id.clone();
    let is_expanded = expanded.get().as_deref() == Some(deck_id.as_str());
    let frame_count = deck.frame_ids.len();

    let toggle_id = deck_id.clone();
    let on_toggle = move |_| {
        expanded.update(|e| {
            *e = if e.as_deref() == Some(toggle_id.as_str()) {
                None
            } else {
                Some(toggle_id.clone())
            };
        });
    };

    let play_id = deck_id.clone();
    let on_play_click = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        on_play.run(play_id.clone());
    };

    let delete_id = deck_id.clone();
    let on_delete = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let pair = canvas
            .doc
            .with_untracked(|d| d.as_ref().and_then(|d| delete_deck_ops(d, &delete_id)));
        if let Some((redo, undo)) = pair {
            on_commit.run((redo, undo));
            if expanded.get_untracked().as_deref() == Some(delete_id.as_str()) {
                expanded.set(None);
            }
        }
    };

    view! {
        <div class="rounded-lg border border-border bg-surface">
            <div
                class="flex items-center gap-2 px-2.5 py-2 cursor-pointer group"
                on:click=on_toggle
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                     class=if is_expanded {
                         "text-text-tertiary rotate-90 transition-transform"
                     } else {
                         "text-text-tertiary transition-transform"
                     }>
                    <polyline points="9 18 15 12 9 6" />
                </svg>
                <div class="flex-1 min-w-0">
                    <div class="text-xs font-medium text-text-primary truncate">
                        {deck.title.clone()}
                    </div>
                    <div class="text-[11px] text-text-tertiary">
                        {frame_count.to_string()} " " {t!(i18n, canvas.deck_frames)}
                    </div>
                </div>
                <button
                    class="p-1.5 rounded-md text-text-secondary hover:text-primary \
                           hover:bg-primary/10 transition-colors disabled:opacity-40 \
                           disabled:hover:bg-transparent disabled:hover:text-text-secondary"
                    title=move || t_string!(i18n, canvas.deck_play).to_string()
                    prop:disabled=live == 0
                    on:click=on_play_click
                >
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor"
                         stroke="none">
                        <polygon points="6 4 20 12 6 20" />
                    </svg>
                </button>
                <button
                    class="opacity-0 group-hover:opacity-100 p-1.5 rounded-md text-text-tertiary \
                           hover:text-danger hover:bg-danger/10 transition-all"
                    title=move || t_string!(i18n, common.delete).to_string()
                    on:click=on_delete
                >
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="3 6 5 6 21 6" />
                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                    </svg>
                </button>
            </div>
            {is_expanded.then(|| view! {
                <div class="px-2 pb-2 flex flex-col gap-0.5">
                    {labels
                        .into_iter()
                        .enumerate()
                        .map(|(idx, label)| {
                            slide_row(
                                deck_id.clone(), idx, label, canvas, i18n, dragging, drag_over,
                                on_commit,
                            )
                        })
                        .collect_view()}
                </div>
            })}
        </div>
    }
}

/// One draggable slide row inside an expanded deck. Dropping onto a row
/// lands the dragged slide at that row's index ([`reorder_frame_ids`]).
#[allow(clippy::too_many_arguments)]
fn slide_row(
    deck_id: String,
    idx: usize,
    label: Option<String>,
    canvas: CanvasState,
    i18n: crate::i18n::I18nCtx,
    dragging: RwSignal<Option<SlideDrag>>,
    drag_over: RwSignal<Option<(String, usize)>>,
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
) -> impl IntoView {
    let start_deck = deck_id.clone();
    let on_dragstart = move |ev: web_sys::DragEvent| {
        if let Some(dt) = ev.data_transfer() {
            dt.set_effect_allowed("move");
            let _ = dt.set_data("text/plain", &format!("{start_deck}:{idx}"));
        }
        dragging.set(Some(SlideDrag {
            deck_id: start_deck.clone(),
            from: idx,
        }));
    };
    let on_dragend = move |_ev: web_sys::DragEvent| {
        dragging.set(None);
        drag_over.set(None);
    };

    let over_deck = deck_id.clone();
    let on_dragover = move |ev: web_sys::DragEvent| {
        let same_deck = dragging
            .get_untracked()
            .is_some_and(|d| d.deck_id == over_deck && d.from != idx);
        if !same_deck {
            return; // a cross-deck (or self) drop is not a move
        }
        ev.prevent_default(); // mandatory for `drop` to fire
        if let Some(dt) = ev.data_transfer() {
            dt.set_drop_effect("move");
        }
        if drag_over.get_untracked() != Some((over_deck.clone(), idx)) {
            drag_over.set(Some((over_deck.clone(), idx)));
        }
    };
    let leave_deck = deck_id.clone();
    let on_dragleave = move |_ev: web_sys::DragEvent| {
        if drag_over.get_untracked() == Some((leave_deck.clone(), idx)) {
            drag_over.set(None);
        }
    };

    let drop_deck = deck_id.clone();
    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        drag_over.set(None);
        let Some(drag) = dragging.get_untracked() else {
            return;
        };
        dragging.set(None);
        if drag.deck_id != drop_deck {
            return;
        }
        let pair = canvas.doc.with_untracked(|d| {
            d.as_ref()
                .and_then(|d| reorder_deck_ops(d, &drop_deck, drag.from, idx))
        });
        if let Some((redo, undo)) = pair {
            on_commit.run((redo, undo));
        }
    };

    let hl_deck = deck_id.clone();
    let row_class = move || {
        let base = "flex items-center gap-2 px-2 py-1.5 rounded-md text-xs cursor-grab \
                    hover:bg-surface-sunken transition-colors";
        if drag_over.get() == Some((hl_deck.clone(), idx)) {
            format!("{base} ring-1 ring-primary bg-primary/10")
        } else {
            base.to_string()
        }
    };

    view! {
        <div
            class=row_class
            draggable="true"
            on:dragstart=on_dragstart
            on:dragend=on_dragend
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" stroke="none"
                 class="text-text-tertiary flex-shrink-0">
                <circle cx="8" cy="6" r="1.6" /><circle cx="16" cy="6" r="1.6" />
                <circle cx="8" cy="12" r="1.6" /><circle cx="16" cy="12" r="1.6" />
                <circle cx="8" cy="18" r="1.6" /><circle cx="16" cy="18" r="1.6" />
            </svg>
            <span class="text-text-tertiary tabular-nums">{(idx + 1).to_string()}</span>
            {match label {
                Some(title) if !title.is_empty() => view! {
                    <span class="text-text-primary truncate">{title}</span>
                }
                .into_any(),
                Some(_) => view! {
                    <span class="text-text-secondary truncate">
                        {t!(i18n, canvas.tool_frame)} " " {(idx + 1).to_string()}
                    </span>
                }
                .into_any(),
                None => view! {
                    <span class="text-warning italic truncate">
                        {t!(i18n, canvas.deck_missing_frame)}
                    </span>
                }
                .into_any(),
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon, ShapeStyle};

    fn common(id: &str, x: f64, y: f64, w: f64, h: f64) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x,
            y,
            w,
            h,
            z: FracIndex::first(),
            parent_id: None,
        }
    }

    fn frame(id: &str, x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape::Frame {
            common: common(id, x, y, w, h),
            title: format!("T-{id}"),
            aspect_locked: false,
        }
    }

    fn note(id: &str) -> Shape {
        Shape::Note {
            common: common(id, 0.0, 0.0, 100.0, 100.0),
            style: ShapeStyle::default(),
            text: String::new(),
        }
    }

    fn doc_with(shapes: Vec<Shape>, decks: Vec<Deck>) -> CanvasDoc {
        CanvasDoc {
            id: "cv-1".to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes,
            decks,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn ids(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    /// Only frames survive the filter, and they come out left-to-right by
    /// center-x — not in selection order, not in document order.
    #[test]
    fn selected_frames_keeps_only_frames_in_reading_order() {
        let shapes = vec![
            frame("f-right", 300.0, 0.0, 160.0, 90.0),
            note("n1"),
            frame("f-left", 0.0, 5.0, 160.0, 90.0),
            frame("f-unselected", 600.0, 0.0, 160.0, 90.0),
        ];
        let selection = vec![
            "n1".to_string(),
            "f-right".to_string(),
            "f-left".to_string(),
        ];
        let got = selected_frames(&shapes, &selection);
        let got_ids: Vec<&str> = got.iter().map(Shape::id).collect();
        assert_eq!(
            got_ids,
            vec!["f-left", "f-right"],
            "frames only, ordered by center-x"
        );
    }

    /// Equal center-x falls back to center-y (top first), then to id, so a
    /// vertical stack orders top-down and identical geometry stays stable.
    #[test]
    fn selected_frames_ties_break_on_center_y_then_id() {
        let shapes = vec![
            frame("low", 0.0, 200.0, 160.0, 90.0),
            frame("high", 0.0, 0.0, 160.0, 90.0),
            frame("b", 500.0, 0.0, 160.0, 90.0),
            frame("a", 500.0, 0.0, 160.0, 90.0),
        ];
        let selection: Vec<String> = ["low", "high", "b", "a"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let got = selected_frames(&shapes, &selection);
        let got_ids: Vec<&str> = got.iter().map(Shape::id).collect();
        assert_eq!(got_ids, vec!["high", "low", "a", "b"]);
    }

    /// Create = one `UpsertDeck` with the frames in reading order; the
    /// inverse of a deck that did not exist is `DeleteDeck`.
    #[test]
    fn create_deck_ops_builds_upsert_with_reading_order_and_delete_undo() {
        let doc = doc_with(
            vec![
                frame("f2", 400.0, 0.0, 160.0, 90.0),
                frame("f1", 0.0, 0.0, 160.0, 90.0),
                note("n1"),
            ],
            Vec::new(),
        );
        let selection: Vec<String> = ["f2", "f1", "n1"].iter().map(ToString::to_string).collect();
        let (redo, undo, new_id) = create_deck_ops(
            &doc,
            &selection,
            "deck-1".to_string(),
            "Slides 1".to_string(),
        )
        .expect("frames are selected");
        assert_eq!(new_id, "deck-1");
        assert_eq!(
            redo,
            vec![CanvasOp::UpsertDeck {
                deck: Deck {
                    id: "deck-1".to_string(),
                    title: "Slides 1".to_string(),
                    frame_ids: vec!["f1".to_string(), "f2".to_string()],
                }
            }]
        );
        assert_eq!(
            undo,
            vec![CanvasOp::DeleteDeck {
                id: "deck-1".to_string()
            }],
            "the deck did not exist before, so undo removes it"
        );
    }

    /// No frame in the selection ⇒ no ops at all — never an empty deck.
    #[test]
    fn create_deck_ops_without_a_selected_frame_is_none() {
        let doc = doc_with(
            vec![frame("f1", 0.0, 0.0, 160.0, 90.0), note("n1")],
            Vec::new(),
        );
        assert!(create_deck_ops(&doc, &["n1".to_string()], "d".into(), "t".into()).is_none());
        assert!(create_deck_ops(&doc, &[], "d".into(), "t".into()).is_none());
    }

    /// The dragged entry lands at the target index — in both directions.
    #[test]
    fn reorder_lands_the_dragged_entry_at_the_target_index() {
        let v: Vec<String> = ["a", "b", "c"].iter().map(ToString::to_string).collect();
        assert_eq!(
            ids(&reorder_frame_ids(&v, 0, 2).expect("valid")),
            vec!["b", "c", "a"],
            "forward: a ends at index 2"
        );
        assert_eq!(
            ids(&reorder_frame_ids(&v, 2, 0).expect("valid")),
            vec!["c", "a", "b"],
            "backward: c ends at index 0"
        );
    }

    /// A no-op or out-of-range drop yields nothing — no op goes on the wire.
    #[test]
    fn reorder_rejects_noop_and_out_of_bounds() {
        let v: Vec<String> = ["a", "b"].iter().map(ToString::to_string).collect();
        assert!(reorder_frame_ids(&v, 1, 1).is_none(), "no-op");
        assert!(reorder_frame_ids(&v, 2, 0).is_none(), "from out of range");
        assert!(reorder_frame_ids(&v, 0, 2).is_none(), "to out of range");
        assert!(reorder_frame_ids(&[], 0, 0).is_none(), "empty list");
    }

    /// Reorder ops carry the new order and invert to the old one.
    #[test]
    fn reorder_deck_ops_upserts_new_order_and_inverts_to_the_old() {
        let deck = Deck {
            id: "d1".to_string(),
            title: "T".to_string(),
            frame_ids: ["a", "b", "c"].iter().map(ToString::to_string).collect(),
        };
        let doc = doc_with(Vec::new(), vec![deck.clone()]);
        let (redo, undo) = reorder_deck_ops(&doc, "d1", 0, 2).expect("valid reorder");
        assert_eq!(
            redo,
            vec![CanvasOp::UpsertDeck {
                deck: Deck {
                    id: "d1".to_string(),
                    title: "T".to_string(),
                    frame_ids: ["b", "c", "a"].iter().map(ToString::to_string).collect(),
                }
            }]
        );
        assert_eq!(
            undo,
            vec![CanvasOp::UpsertDeck { deck }],
            "undo restores the pre-reorder deck verbatim"
        );
        assert!(reorder_deck_ops(&doc, "unknown", 0, 1).is_none());
    }

    /// Delete inverts to a verbatim restore; an unknown id is `None`.
    #[test]
    fn delete_deck_ops_inverts_to_restore() {
        let deck = Deck {
            id: "d1".to_string(),
            title: "T".to_string(),
            frame_ids: vec!["a".to_string()],
        };
        let doc = doc_with(Vec::new(), vec![deck.clone()]);
        let (redo, undo) = delete_deck_ops(&doc, "d1").expect("deck exists");
        assert_eq!(
            redo,
            vec![CanvasOp::DeleteDeck {
                id: "d1".to_string()
            }]
        );
        assert_eq!(undo, vec![CanvasOp::UpsertDeck { deck }]);
        assert!(delete_deck_ops(&doc, "unknown").is_none());
    }

    /// Only a live `Frame` answers with its title; a foreign shape or a
    /// deleted id answers `None` — the drawer's "missing frame" row.
    #[test]
    fn frame_title_answers_only_for_live_frames() {
        let shapes = vec![frame("f1", 0.0, 0.0, 160.0, 90.0), note("n1")];
        assert_eq!(frame_title(&shapes, "f1").as_deref(), Some("T-f1"));
        assert_eq!(frame_title(&shapes, "n1"), None, "a note is not a slide");
        assert_eq!(frame_title(&shapes, "gone"), None);
    }
}

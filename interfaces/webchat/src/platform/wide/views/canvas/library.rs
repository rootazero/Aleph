//! The canvas library — the left-column gallery and every action over it.
//!
//! # Why the gallery is a sidebar
//!
//! It used to be a full-page list in the main area, mutually exclusive with
//! the editor: opening a canvas replaced the library, and reaching a second
//! canvas cost three steps (back → scan → open) and threw away the editor's
//! camera and undo stack on the way. A left-column list makes navigation and
//! work coexist — one click switches, and the list is the answer to "what
//! else is there" without leaving what you are doing.
//!
//! # Who loads the rows
//!
//! **Not this module.** [`super::CanvasView`] owns the three liveness wires
//! (load/reconnect Effect, topic subscription, frame consumption) because it
//! is a keep-alive container mounted once for the life of the app;
//! [`CanvasSidebar`] mounts and unmounts every time the user switches
//! sections. A loader here would refetch the library on every visit and
//! churn the topic subscription — and, worse, be a second answer to "what is
//! in the library". The sidebar is a pure consumer of
//! [`CanvasState::rows`](crate::state::canvas::CanvasState::rows) plus a
//! writer of `open_canvas`; the write actions below refetch because they
//! changed something, which is a different reason.
//!
//! # One function per verb, however many surfaces
//!
//! [`rename_canvas`] is called from the sidebar row and from the editor's
//! title — one function, because the parts a copy would drop are the base
//! revision resolution and the conflict retry, and a rename that silently
//! did nothing on the second surface is exactly the failure this codebase
//! keeps paying for.

use aleph_protocol::canvas::{check_title, CanvasDoc, CanvasOp, CanvasRow, TitleRejection};
use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::HtmlInputElement;

use crate::api::canvas::{CanvasApi, CanvasApplyError};
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};
use crate::state::canvas::CanvasState;

/// Epoch milliseconds → local `YYYY-MM-DD HH:MM` label (browser timezone,
/// same idiom as `tasks.rs::format_timestamp_ms`).
fn updated_label(ts_ms: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time(ts_ms as f64);
    let y = date.get_full_year();
    let mo = date.get_month() + 1;
    let d = date.get_date();
    let hh = date.get_hours();
    let mm = date.get_minutes();
    format!("{y}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

/// Client-side title filter over the library rows (R4: pure I/O in the
/// interface layer — there is no server-side canvas search and this list is
/// small enough that adding one would be a second source of "what matches").
///
/// Case-insensitive substring over the title, and over the id as well: ids
/// are what the model reports back in chat ("I drew on cv-3f9…"), so pasting
/// one should find its canvas. A blank query keeps the server's order, which
/// is already most-recently-updated first.
pub(super) fn filter_rows(rows: &[CanvasRow], query: &str) -> Vec<CanvasRow> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|r| {
            r.title.to_lowercase().contains(&needle) || r.id.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect()
}

/// The `(revision, title)` this client believes `id` currently has.
///
/// The open document wins over the library row whenever it *is* this canvas:
/// `doc` is reconciled by every `canvas.updated` frame, while `rows` is
/// refreshed by a separate `canvas.list` round trip that can lag a revision
/// or two behind. Picking the staler of the two would turn every rename of
/// the canvas you are actively drawing on into a conflict, and would compare
/// a new title against a title that is no longer current.
///
/// Both facts come from one lookup so the precedence rule is stated once —
/// two functions with the same `doc`-beats-`rows` preamble is how one of them
/// eventually stops agreeing with the other.
///
/// `None` means this client has no idea what state that canvas is in — the
/// caller must fetch rather than guess, because a guessed base either
/// conflicts (harmless) or, if it happened to be right, applies against a
/// document the user never saw.
pub(super) fn pick_known_state(
    id: &str,
    doc: Option<&CanvasDoc>,
    rows: &[CanvasRow],
) -> Option<(u64, String)> {
    doc.filter(|d| d.id == id)
        .map(|d| (d.revision, d.title.clone()))
        .or_else(|| {
            rows.iter()
                .find(|r| r.id == id)
                .map(|r| (r.revision, r.title.clone()))
        })
}

/// Refetch the library rows. Errors are ignored here — this runs on event
/// frames and after writes, where the load `Effect` (which does classify
/// errors) remains the surface that reports a broken connection.
pub(super) fn refresh_rows(state: DashboardState, canvas: CanvasState) {
    spawn_local(async move {
        if let Ok(list) = CanvasApi::list(&state).await {
            canvas.rows.set(list);
        }
    });
}

/// Fetch one canvas into the shared state, keeping the answer only if that
/// canvas is still the open one by the time it arrives.
///
/// One function rather than four inlined copies (row click, create, frame
/// refresh, editor reopen), because the staleness check is the part a copy
/// would drop.
pub(super) fn fetch_open_doc(
    state: DashboardState,
    canvas: CanvasState,
    i18n: I18nCtx,
    id: String,
) {
    spawn_local(async move {
        let fetched = CanvasApi::get(&state, &id).await;
        // Re-check *after* the await: the user may have closed or switched
        // canvases while the fetch was in flight, and landing a stale
        // envelope would reopen the one they just left.
        let Some(open_now) = canvas.open_canvas.try_get_untracked() else {
            return;
        };
        if open_now.as_deref() != Some(id.as_str()) {
            return;
        }
        match fetched {
            Ok(envelope) => canvas.adopt_envelope(envelope),
            Err(e) => {
                canvas
                    .load_error
                    .set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                        format!("Failed to open canvas: {e}")
                    })));
            }
        }
    });
}

/// Open a canvas: mark it open, drop the previous document, fetch.
///
/// `doc` is cleared *before* the fetch so the editor unmounts instead of
/// rendering the previous canvas's shapes under the new one's title for a
/// round trip.
pub(super) fn open_canvas(state: DashboardState, canvas: CanvasState, i18n: I18nCtx, id: String) {
    canvas.open_canvas.set(Some(id.clone()));
    canvas.doc.set(None);
    fetch_open_doc(state, canvas, i18n, id);
}

/// Create a canvas and open it.
pub(super) fn create_canvas(
    state: DashboardState,
    canvas: CanvasState,
    i18n: I18nCtx,
    creating: RwSignal<bool>,
) {
    if creating.get_untracked() {
        return;
    }
    creating.set(true);
    spawn_local(async move {
        match CanvasApi::create(&state, None, None).await {
            Ok(doc) => {
                refresh_rows(state, canvas);
                // The create envelope carries no asset_base (the server mints
                // one only on `canvas.get`), so opening goes through the same
                // fetch as a row click.
                open_canvas(state, canvas, i18n, doc.id);
            }
            Err(e) => {
                canvas
                    .load_error
                    .set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to create canvas: {e}")
                    })));
            }
        }
        creating.set(false);
    });
}

/// Delete a canvas (the owner-only verb) and refresh the library.
///
/// Deleting the canvas you have open closes it too — leaving it open would
/// leave an editor bound to a document the server no longer has, whose every
/// apply answers not-found.
pub(super) fn delete_canvas(state: DashboardState, canvas: CanvasState, i18n: I18nCtx, id: String) {
    spawn_local(async move {
        match CanvasApi::delete(&state, &id).await {
            Ok(()) => {
                if canvas.open_canvas.try_get_untracked().flatten().as_deref() == Some(id.as_str())
                {
                    canvas.close_canvas();
                }
                refresh_rows(state, canvas);
            }
            Err(e) => {
                canvas
                    .load_error
                    .set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to delete canvas: {e}")
                    })));
            }
        }
    });
}

/// What a title edit should do, decided from the draft and the current title
/// alone.
///
/// Pure, and shared by both editing surfaces (the sidebar row and the editor
/// header), because the three answers below are exactly the part that a
/// second hand-written copy gets wrong: it validates but forgets that a
/// rename to the same string still burns a revision and broadcasts a frame to
/// every connected client, or it skips the no-op but stops validating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TitleEdit {
    /// Admissible and different — send it.
    Send(String),
    /// Same title as now: close the editor, touch nothing.
    Unchanged,
    /// Inadmissible; the reason is for whoever typed it.
    Refused(TitleRejection),
}

/// Localized rendering of a title refusal.
///
/// An exhaustive `match` on purpose, and the reason [`TitleRejection`] is an
/// enum rather than a sentence: a fourth refusal added in the contract is a
/// compile error right here, instead of a blank line where a message should
/// have been.
pub(super) fn rejection_label(i18n: I18nCtx, why: TitleRejection) -> String {
    match why {
        TitleRejection::Empty => t_string!(i18n, canvas.title_empty).to_string(),
        TitleRejection::TooLong => t_string!(i18n, canvas.title_too_long).to_string(),
        TitleRejection::ControlCharacter => t_string!(i18n, canvas.title_control).to_string(),
    }
}

/// Decide a title edit. `draft` is taken as typed and trimmed here — trimming
/// at the input edge is deliberate, because the shared contract gate
/// ([`check_title`]) refuses rather than rewrites, so the one place allowed to
/// normalize is the place a human types.
pub(super) fn decide_title_edit(draft: &str, current: &str) -> TitleEdit {
    let title = draft.trim();
    if let Err(why) = check_title(title) {
        return TitleEdit::Refused(why);
    }
    if title == current {
        return TitleEdit::Unchanged;
    }
    TitleEdit::Send(title.to_string())
}

/// Run a title edit against the canvas `id`: decide, then send if there is
/// anything to send. `Err` carries the refusal reason for display.
///
/// The current title comes from [`pick_known_state`], so "did this actually
/// change" is answered against the same precedence (`doc` over `rows`) that
/// picks the base revision — comparing against the stale one would send a
/// pointless revision every time the model had just renamed the canvas.
pub(super) fn submit_title(
    state: DashboardState,
    canvas: CanvasState,
    i18n: I18nCtx,
    id: &str,
    draft: &str,
) -> Result<(), TitleRejection> {
    let current = canvas
        .rows
        .with_untracked(|rows| {
            canvas
                .doc
                .with_untracked(|doc| pick_known_state(id, doc.as_ref(), rows))
        })
        .map(|(_, title)| title)
        .unwrap_or_default();
    match decide_title_edit(draft, &current) {
        TitleEdit::Refused(why) => Err(why),
        TitleEdit::Unchanged => Ok(()),
        TitleEdit::Send(title) => {
            rename_canvas(state, canvas, i18n, id.to_string(), title);
            Ok(())
        }
    }
}

/// Rename a canvas — the first human producer of [`CanvasOp::SetDocMeta`].
///
/// The op, the server's applier and the model's `canvas` tool all shipped
/// with the subsystem; what was missing was any way for a person to reach it,
/// so every canvas a human created was called "Untitled" forever. That was
/// tolerable while the library was a page of cards with shape counts and
/// timestamps; it is not tolerable now that the title *is* the navigation.
///
/// # Conflict handling mirrors the tool face
///
/// The base revision comes from [`pick_known_state`], and a
/// [`CanvasApplyError::Conflict`] is retried **once** against a freshly read
/// revision — the same discipline `builtin_tools::canvas` applies for the
/// same reason: a rename is not a shape edit, it has nothing to rebase, so
/// replaying it verbatim against the current revision is exactly right. A
/// second conflict surfaces rather than looping: at that point something is
/// writing continuously and the honest answer is to say so.
///
/// The retry reads the canvas but deliberately **does not adopt** the
/// envelope: if this canvas is open, the editor's document is the live one
/// (mid-drag, with an optimistic batch of its own), and replacing it to learn
/// one integer would discard work the user can see. Only the revision is
/// taken. The renamed title reaches the open document the same way any other
/// client's edit does — through the `canvas.updated` frame and
/// `reconcile.rs`.
pub(super) fn rename_canvas(
    state: DashboardState,
    canvas: CanvasState,
    i18n: I18nCtx,
    id: String,
    title: String,
) {
    spawn_local(async move {
        let base = {
            let doc = canvas.doc.try_get_untracked().flatten();
            let rows = canvas.rows.try_get_untracked().unwrap_or_default();
            pick_known_state(&id, doc.as_ref(), &rows).map(|(revision, _)| revision)
        };
        let Some(base) = base else {
            return;
        };
        let op = CanvasOp::SetDocMeta {
            title: title.clone(),
        };
        let first = CanvasApi::apply(&state, &id, base, vec![op.clone()]).await;
        let outcome = match first {
            Err(CanvasApplyError::Conflict) => {
                let Ok(envelope) = CanvasApi::get(&state, &id).await else {
                    return;
                };
                CanvasApi::apply(&state, &id, envelope.canvas.revision, vec![op]).await
            }
            other => other,
        };
        match outcome {
            Ok(_) => refresh_rows(state, canvas),
            // A conflict that survived the retry has no message of its own
            // worth showing — it is not a refusal, it is "someone else is
            // writing right now". Say that, in the one sentence this surface
            // has for failures.
            Err(CanvasApplyError::Conflict) => {
                canvas
                    .load_error
                    .set(Some(t_string!(i18n, canvas.rename_busy).to_string()));
            }
            Err(CanvasApplyError::Other(e)) => {
                canvas
                    .load_error
                    .set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to rename canvas: {e}")
                    })));
            }
        }
    });
}

/// The left-column gallery: create, filter, and the title list.
///
/// Rendered by `ModeSidebar` for `PanelMode::Canvas`, alongside
/// `MemorySidebar` / `TeamsSidebar` / `ProjectsSidebar` — a wide view module
/// exporting its own secondary menu is the established shape here, so this
/// introduces no new one.
///
/// Every piece of state below is component-local on purpose: a filter query,
/// an armed delete and an open rename are all *this visit's* interaction
/// state, and resetting them when the user navigates away is the safe
/// direction (an armed delete that outlived a section switch would fire on a
/// click the user made in a different context).
#[component]
#[must_use]
pub fn CanvasSidebar() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    let query = RwSignal::new(String::new());
    let creating = RwSignal::new(false);
    // Row id awaiting delete confirmation. Inline, not a modal: the second
    // click lands where the first one did.
    let pending_delete = RwSignal::new(Option::<String>::None);
    // Row id currently being renamed, and its draft text. At most one row
    // edits at a time, so one input ref serves them all (chat_sidebar idiom).
    let renaming = RwSignal::new(Option::<String>::None);
    let rename_text = RwSignal::new(String::new());
    // Why a `TitleRejection` and not an `Option<String>`: this signal holds
    // the contract gate's verdict, which structurally cannot be a server
    // error — the type says so, which is a stronger statement than an entry
    // on an allowlist would be (see `admin_refusal`'s
    // `no_error_signal_is_fed_an_unclassified_error`) — and it keeps the
    // wording out of the signal, so the message is rendered in the reader's
    // language at the point it is displayed.
    let rename_error = RwSignal::new(Option::<TitleRejection>::None);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // The filtered slice, memoized: `PartialEq` dedupe means a `canvas.list`
    // refresh that changed nothing at all (the common case — most frames touch
    // one canvas's revision, and most of those are the open one) notifies no
    // reader and diffs no DOM.
    let visible = Memo::new(move |_| canvas.rows.with(|rows| filter_rows(rows, &query.get())));

    // Focus and select the rename input when a row enters edit mode.
    //
    // Two details, both borrowed from `chat_sidebar`'s twin of this effect:
    // the read is deferred a tick because the input is created inside a
    // nested reactive closure and the `NodeRef` is not bound yet when this
    // effect first runs; and the effect keys on `renaming` **alone**, so the
    // frames that stream in while the open canvas is being drawn on — which
    // update this list several times a second — never re-grab a caret the
    // user is already using.
    Effect::new(move |_| {
        if renaming.get().is_none() {
            return;
        }
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(10).await;
            if let Some(el) = input_ref.get() {
                let input: &HtmlInputElement = &el;
                let _ = input.focus();
                input.select();
            }
        });
    });

    // `keep_open` distinguishes the two ways an edit ends. Enter means "I
    // mean it", so a refusal keeps the input up with the reason; blur means
    // "I am leaving", and trapping someone in a red input they navigated away
    // from is worse than dropping a title the gate would not have taken.
    let commit_rename = Callback::new(move |keep_open_on_refusal: bool| {
        let Some(id) = renaming.get_untracked() else {
            return;
        };
        let draft = rename_text.get_untracked();
        match submit_title(state, canvas, i18n, &id, &draft) {
            Err(why) if keep_open_on_refusal => {
                rename_error.set(Some(why));
                return;
            }
            _ => {}
        }
        renaming.set(None);
        rename_error.set(None);
    });

    view! {
        <div class="flex flex-col h-full">
            <div class="p-3 border-b border-border space-y-2">
                <button
                    class="w-full px-3 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover transition-colors text-sm font-medium disabled:opacity-50"
                    prop:disabled=move || creating.get()
                    on:click=move |_| create_canvas(state, canvas, i18n, creating)
                >
                    {t!(i18n, canvas.new_canvas)}
                </button>
                <input
                    type="text"
                    placeholder=move || t_string!(i18n, canvas.search_placeholder).to_string()
                    prop:value=move || query.get()
                    on:input=move |ev| query.set(event_target_value(&ev))
                    class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-md text-xs text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-1 focus:ring-primary/30"
                />
            </div>

            <div class="flex-1 min-h-0 overflow-y-auto p-2 space-y-0.5">
                // `Show`, not an `if` inside one closure over the rows: a
                // closure that reads `rows` rebuilds everything it returns on
                // every frame, and what it returns here is the `For` — which
                // would make the keying below decorative. `Show` memoizes its
                // condition, so the `For` is built once and thereafter
                // diffs.
                <Show
                    when=move || !visible.with(Vec::is_empty)
                    fallback=move || view! {
                        <div class="px-3 py-6 text-center text-xs text-text-tertiary">
                            // Three states, not two: "still asking", "you
                            // have none" and "none match this filter" are
                            // different sentences, and answering the first
                            // with the second tells the user their library is
                            // empty before the question has been asked. (The
                            // phone screen already drew this distinction; the
                            // wide surface did not.)
                            {move || if !canvas.rows_loaded.get() {
                                t!(i18n, common.loading).into_any()
                            } else if canvas.rows.with(Vec::is_empty) {
                                t!(i18n, canvas.empty).into_any()
                            } else {
                                t!(i18n, canvas.no_match).into_any()
                            }}
                        </div>
                    }
                >
                    <For
                        // Keyed by id alone. Folding `revision` into the key
                        // (as the old page-sized library did) remounts the row
                        // on every applied batch — so the row of the canvas
                        // you are drawing on rebuilds many times a second, and
                        // an open rename input on it loses its caret on every
                        // stroke. The row reads its own data through memos
                        // instead, so it updates in place.
                        each=move || visible.get()
                        key=|row: &CanvasRow| row.id.clone()
                        children=move |row: CanvasRow| {
                            view! {
                                <LibraryRow
                                    id=row.id
                                    pending_delete=pending_delete
                                    renaming=renaming
                                    rename_text=rename_text
                                    rename_error=rename_error
                                    input_ref=input_ref
                                    commit_rename=commit_rename
                                />
                            }
                        }
                    />
                </Show>
            </div>
        </div>
    }
}

/// One library row: title, meta line, and the rename / delete affordances.
///
/// # Why this takes an id and reads its own row
///
/// With a `For` keyed by id the child is built once and never rebuilt while
/// the row survives, so a by-value `CanvasRow` would freeze at whatever the
/// list held when the row first appeared — the title would not change after a
/// rename, and the shape count never at all. Folding `revision` into the key
/// instead (what the old page-sized library did) fixes the staleness by
/// remounting the row on every applied batch, which means the row of the
/// canvas you are drawing on rebuilds several times a second.
///
/// So: keyed by id, data read through memos. And the memos are read at the
/// **leaves** — no closure wraps the whole row — because a closure over the
/// row would rebuild the subtree it returns, and that subtree contains the
/// rename input. Renaming the canvas the model happens to be drawing on would
/// lose the caret on every batch. Only `is_renaming` (a `bool` memo) swaps a
/// subtree, and only when the mode actually flips.
///
/// The id lives in a `StoredValue` for a mechanical reason worth stating
/// once: a `move` closure capturing a `String` would move it out of the
/// component's environment, and several handlers need it. `StoredValue` and
/// `Memo` are `Copy`; nothing is moved and no handler needs a defensive
/// `.clone()` that exists only to appease capture rules.
#[component]
fn LibraryRow(
    id: String,
    pending_delete: RwSignal<Option<String>>,
    renaming: RwSignal<Option<String>>,
    rename_text: RwSignal<String>,
    rename_error: RwSignal<Option<TitleRejection>>,
    input_ref: NodeRef<leptos::html::Input>,
    commit_rename: Callback<bool>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    let row_id = StoredValue::new(id);

    let row = Memo::new(move |_| {
        row_id.with_value(|id| {
            canvas
                .rows
                .with(|rows| rows.iter().find(|r| &r.id == id).cloned())
        })
    });
    // Leaf projections. A row that has just left the library renders blank
    // for the one frame before `For` drops it — cheaper, and far less
    // startling, than tearing the subtree down from inside itself.
    let title = Memo::new(move |_| row.get().map(|r| r.title).unwrap_or_default());
    let shape_count = Memo::new(move |_| {
        row.get()
            .map(|r| r.shape_count.to_string())
            .unwrap_or_default()
    });
    let updated = Memo::new(move |_| {
        row.get()
            .map(|r| updated_label(r.updated_at_ms))
            .unwrap_or_default()
    });

    let is_open = Memo::new(move |_| {
        row_id.with_value(|id| {
            canvas
                .open_canvas
                .with(|o| o.as_deref() == Some(id.as_str()))
        })
    });
    let is_armed = Memo::new(move |_| {
        row_id.with_value(|id| pending_delete.with(|p| p.as_deref() == Some(id.as_str())))
    });
    let is_renaming = Memo::new(move |_| {
        row_id.with_value(|id| renaming.with(|r| r.as_deref() == Some(id.as_str())))
    });

    view! {
        <div
            class=move || if is_open.get() {
                "nav-tile-active group rounded-lg px-2.5 py-2 cursor-pointer"
            } else {
                "nav-tile group rounded-lg px-2.5 py-2 cursor-pointer"
            }
            on:click=move |_| {
                // A click anywhere on the row opens it — except while some row
                // is being renamed, where the click belongs to the text field
                // (or is the click that dismisses it) and must not navigate.
                if renaming.get_untracked().is_none() {
                    open_canvas(state, canvas, i18n, row_id.get_value());
                }
            }
        >
            {move || if is_renaming.get() {
                view! {
                    <input
                        node_ref=input_ref
                        type="text"
                        prop:value=move || rename_text.get()
                        on:click=|ev| ev.stop_propagation()
                        on:input=move |ev| {
                            rename_text.set(event_target_value(&ev));
                            rename_error.set(None);
                        }
                        on:blur=move |_| commit_rename.run(false)
                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                            match ev.key().as_str() {
                                "Enter" => commit_rename.run(true),
                                "Escape" => {
                                    renaming.set(None);
                                    rename_error.set(None);
                                }
                                _ => {}
                            }
                        }
                        class="w-full px-1.5 py-0.5 bg-surface-sunken border border-primary/60 rounded text-sm text-text-primary focus:outline-none"
                    />
                }
                .into_any()
            } else {
                view! {
                    <div class="flex items-center gap-2">
                        <span
                            class="text-sm font-medium truncate flex-1 min-w-0"
                            title=move || title.get()
                        >
                            {move || title.get()}
                        </span>
                        <span class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0">
                            <button
                                class="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-sunken"
                                title=move || t_string!(i18n, canvas.rename).to_string()
                                on:click=move |ev: leptos::ev::MouseEvent| {
                                    ev.stop_propagation();
                                    pending_delete.set(None);
                                    rename_error.set(None);
                                    rename_text.set(title.get_untracked());
                                    renaming.set(Some(row_id.get_value()));
                                }
                            >
                                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12 20h9" />
                                    <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" />
                                </svg>
                            </button>
                            <button
                                class="p-1 rounded text-text-tertiary hover:text-danger hover:bg-danger/10"
                                title=move || t_string!(i18n, common.delete).to_string()
                                on:click=move |ev: leptos::ev::MouseEvent| {
                                    ev.stop_propagation();
                                    renaming.set(None);
                                    pending_delete.set(Some(row_id.get_value()));
                                }
                            >
                                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <polyline points="3 6 5 6 21 6" />
                                    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                                </svg>
                            </button>
                        </span>
                    </div>
                }
                .into_any()
            }}

            <div class="mt-0.5 text-[11px] text-text-tertiary truncate">
                {move || shape_count.get()}
                " " {t!(i18n, canvas.shapes)}
                " \u{b7} " {move || updated.get()}
            </div>

            {move || (is_renaming.get() && rename_error.get().is_some()).then(|| view! {
                <div class="mt-1 text-[11px] text-danger">
                    {move || rename_error.get().map(|why| rejection_label(i18n, why))}
                </div>
            })}

            {move || is_armed.get().then(|| view! {
                <div class="mt-1.5 flex items-center gap-1.5"
                     on:click=|ev: leptos::ev::MouseEvent| ev.stop_propagation()>
                    <button
                        class="px-2 py-0.5 rounded bg-danger text-white text-[11px] font-medium"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.stop_propagation();
                            delete_canvas(state, canvas, i18n, row_id.get_value());
                            pending_delete.set(None);
                        }
                    >
                        {t!(i18n, common.delete)}
                    </button>
                    <button
                        class="px-2 py-0.5 rounded border border-border text-[11px] text-text-secondary hover:bg-surface-sunken"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.stop_propagation();
                            pending_delete.set(None);
                        }
                    >
                        {t!(i18n, common.cancel)}
                    </button>
                </div>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::CanvasRow;

    fn row(id: &str, title: &str, revision: u64) -> CanvasRow {
        CanvasRow {
            id: id.to_string(),
            title: title.to_string(),
            revision,
            shape_count: 0,
            project_id: None,
            updated_at_ms: 0,
        }
    }

    fn doc(id: &str, revision: u64) -> CanvasDoc {
        CanvasDoc {
            id: id.to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision,
            shapes: Vec::new(),
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    /// A blank query keeps the server's order verbatim — the list arrives
    /// most-recently-updated first and the filter has no business resorting
    /// it.
    #[test]
    fn a_blank_query_keeps_every_row_in_the_servers_order() {
        let rows = vec![row("cv-a", "Zeta", 1), row("cv-b", "Alpha", 1)];
        let out = filter_rows(&rows, "   ");
        assert_eq!(
            out.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["cv-a", "cv-b"]
        );
    }

    /// Case-insensitive over the title, and over the id too: the id is what
    /// the model quotes back in chat, so pasting one must find its canvas.
    #[test]
    fn the_filter_matches_titles_and_ids_case_insensitively() {
        let rows = vec![row("cv-abc", "Roadmap", 1), row("cv-xyz", "Sketches", 1)];
        assert_eq!(filter_rows(&rows, "ROAD").len(), 1);
        assert_eq!(filter_rows(&rows, "cv-XY")[0].id, "cv-xyz");
        assert!(filter_rows(&rows, "nothing").is_empty());
    }

    /// The open document outranks the library row for the canvas it *is*:
    /// frames reconcile `doc` continuously while `rows` waits on a separate
    /// list round trip, so taking the row's number would base every rename of
    /// the canvas being drawn on against a stale revision — and compare the
    /// new title against a title that is no longer current.
    #[test]
    fn the_open_document_outranks_the_library_row_for_its_own_canvas() {
        let rows = vec![row("cv-1", "stale name", 4)];
        let mut open = doc("cv-1", 9);
        open.title = "live name".to_string();
        assert_eq!(
            pick_known_state("cv-1", Some(&open), &rows),
            Some((9, "live name".to_string()))
        );
    }

    /// …but only for its own canvas: acting on a *different* row while one is
    /// open must read that row, never the open document's state.
    #[test]
    fn a_row_that_is_not_the_open_document_reads_its_own_state() {
        let rows = vec![row("cv-1", "one", 4), row("cv-2", "two", 7)];
        let open = doc("cv-1", 9);
        assert_eq!(
            pick_known_state("cv-2", Some(&open), &rows),
            Some((7, "two".to_string()))
        );
    }

    /// Unknown canvas → nothing at all. Returning a default (revision 0, or
    /// the open document's) would let a write land against a revision this
    /// client has never seen.
    #[test]
    fn an_unknown_canvas_has_no_known_state_rather_than_a_default() {
        assert_eq!(pick_known_state("cv-ghost", None, &[]), None);
    }

    /// A title that only gained whitespace is not a rename. Sending it would
    /// burn a revision and broadcast a frame to every connected client for a
    /// change nobody can see — and, because the Panel trims at the input edge
    /// while the server refuses rather than rewrites, the trimmed form IS the
    /// current title.
    #[test]
    fn a_draft_that_trims_back_to_the_current_title_is_not_a_rename() {
        assert_eq!(
            decide_title_edit("  Roadmap  ", "Roadmap"),
            TitleEdit::Unchanged
        );
    }

    /// An admissible, different title is sent trimmed.
    #[test]
    fn an_admissible_change_is_sent_trimmed() {
        assert_eq!(
            decide_title_edit(" Q3 plan ", "Roadmap"),
            TitleEdit::Send("Q3 plan".to_string())
        );
    }

    /// Refusal comes from the shared contract gate, not from a client-side
    /// approximation of it: the Panel and the server say no to the same
    /// strings for the same reasons, and the reason is carried so the person
    /// who typed it learns what to change.
    #[test]
    fn an_inadmissible_title_is_refused_with_the_contract_gates_reason() {
        assert_eq!(
            decide_title_edit("   ", "Roadmap"),
            TitleEdit::Refused(TitleRejection::Empty),
            "the verdict is the contract gate's own, not a local re-derivation"
        );
        assert_eq!(
            decide_title_edit(&"x".repeat(1000), "Roadmap"),
            TitleEdit::Refused(TitleRejection::TooLong)
        );
    }
}

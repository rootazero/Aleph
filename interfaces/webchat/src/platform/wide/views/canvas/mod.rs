//! Whiteboard canvas — `/canvas`: the editor surface and the three liveness
//! wires that feed it.
//!
//! The **library** is no longer here: it moved to the left column as
//! [`library::CanvasSidebar`], so navigating between canvases no longer
//! means leaving the one you are in. What stays in the main area is the
//! open document — or, when none is open, a welcome pane pointing at the
//! list. The wires below stay here rather than moving with the list
//! because this view is mounted once for the life of the app while the
//! sidebar remounts on every section switch; see `library.rs`'s module
//! doc for the full argument.
//!
//! Not the memory galaxy: that renderer moved to `views/memory/galaxy/` when
//! the whiteboard claimed the `canvas` name. This module is the Panel half of
//! the `canvas.*` RPC family (`api/canvas.rs`) and of the `canvas.updated`
//! event topic.
//!
//! # Liveness
//!
//! Three wires, all mounted here because this view is a keep-alive container
//! (`MainContent` hides it with CSS instead of unmounting):
//!
//! 1. **Load / reconnect** — the `WorkspacesView` idiom: an `Effect` gated on
//!    `is_connected`, so the first load waits for the socket instead of
//!    failing against a connecting one, and a reconnect refetches.
//! 2. **Topic subscription** — `subscribe_topic` per mount (NOT
//!    `BASE_TOPICS`): the ledger in `context.rs` replays it across
//!    reconnects, and a topic only this section consumes has no business on
//!    every socket.
//! 3. **Frame consumption** — a `canvas.updated` frame refreshes the library
//!    rows and reconciles the open document through `reconcile.rs`: the next
//!    revision's ops apply in place, our own optimistic echo is dropped
//!    (matched against `CanvasState.inflight` by base revision + ops), and a
//!    revision gap falls back to a whole-doc refetch.

mod ai;
mod asset_ingest;
mod decks;
mod editor;
mod export;
mod freehand;
mod id_mint;
mod interaction;
mod library;
mod ops;
mod present;
mod reconcile;
mod shape_view;
mod text_edit;
mod toolbar;
mod viewport;

pub use library::CanvasSidebar;

use aleph_protocol::canvas as canvas_proto;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::canvas::CanvasState;

use library::{create_canvas, fetch_open_doc, refresh_rows};

#[component]
#[must_use]
pub fn CanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    // (1) Load + reconnect — the WorkspacesView idiom, and for the same
    // reason: a bare `spawn_local` on mount races a socket that is usually
    // still connecting, fails with "Not connected", and never retries.
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            match CanvasApi::list(&state).await {
                Ok(list) => {
                    canvas.rows.set(list);
                    canvas.load_error.set(None);
                }
                Err(e) => {
                    // The rows are NOT cleared on failure: a refusal says
                    // nothing about what is there.
                    canvas
                        .load_error
                        .set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                            format!("Failed to load canvases: {e}")
                        })));
                }
            }
            // Both arms: the question has now been asked. A failure is
            // reported by `load_error`; leaving `rows_loaded` false would
            // make the list say "still loading" forever instead.
            canvas.rows_loaded.set(true);
        });
    });

    // (2) Topic subscription — gated on `is_connected` like the loader; a
    // subscription that failed against a connecting socket is silent forever.
    Effect::new(move |_| {
        if !state.is_connected.get() {
            return;
        }
        let dash = state;
        spawn_local(async move {
            let _ = dash.subscribe_topic(canvas_proto::TOPIC).await;
        });
    });

    // (3) Frame consumption — refresh the library, reconcile the open doc.
    let sub_id = state.subscribe_events(move |evt| {
        if evt.topic != canvas_proto::TOPIC {
            return;
        }
        let Ok(frame) = serde_json::from_value::<canvas_proto::CanvasUpdated>(evt.data.clone())
        else {
            return;
        };
        refresh_rows(state, canvas);
        let open = canvas.open_canvas.get_untracked();
        if open.as_deref() != Some(frame.canvas_id.as_str()) {
            return;
        }
        let held = canvas.doc.with_untracked(|d| {
            d.as_ref()
                .filter(|d| d.id == frame.canvas_id)
                .map(|d| d.revision)
        });
        let Some(local_rev) = held else {
            // Open but still loading (or the doc signal holds the previous
            // canvas): the in-flight open fetch may have been answered
            // before this batch committed, so refetch — `fetch_open_doc`'s
            // staleness check arbitrates whichever answer lands last.
            fetch_open_doc(state, canvas, i18n, frame.canvas_id);
            return;
        };
        let decision = canvas
            .inflight
            .with_untracked(|inflight| reconcile::reconcile(local_rev, &frame, inflight.as_ref()));
        match decision {
            reconcile::Reconcile::ApplyOps => {
                canvas.doc.update(|d| {
                    let Some(d) = d.as_mut() else { return };
                    if d.id == frame.canvas_id {
                        ops::apply_local(d, &frame.ops);
                        // The frame IS server truth — same authority as an
                        // apply ack (ops.rs module doc), never optimistic.
                        d.revision = frame.revision;
                    }
                });
            }
            reconcile::Reconcile::Refetch => {
                fetch_open_doc(state, canvas, i18n, frame.canvas_id);
            }
            reconcile::Reconcile::DropEcho => {}
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    view! {
        {move || match canvas.open_canvas.get() {
            Some(_) => view! { <OpenCanvasPane /> }.into_any(),
            None => view! { <WelcomePane /> }.into_any(),
        }}
    }
}

/// Shown while no canvas is open: what this section is, and the one action
/// that gets you started.
///
/// The list that used to fill this space is now the left column, so this pane
/// deliberately does **not** repeat it — two lists of the same rows in one
/// viewport would be two answers to "which canvases are there", and the
/// second one would be the stale one.
#[component]
fn WelcomePane() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();
    let creating = RwSignal::new(false);

    view! {
        <div class="flex flex-col h-full">
            {move || {
                canvas.load_error.get().map(|msg| view! {
                    <div class="mx-6 mt-4 px-4 py-3 rounded-lg bg-warning-subtle border border-warning text-sm text-text-primary">
                        {msg}
                    </div>
                })
            }}
            <div class="flex-1 flex flex-col items-center justify-center gap-3 px-8 text-center">
                <svg width="44" height="44" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"
                     class="text-text-tertiary">
                    <rect x="3" y="3" width="18" height="18" rx="2" />
                    <path d="M7 14c1.5-4 3-4 4.5-1s3 3 5.5-3" />
                </svg>
                <h1 class="text-lg font-semibold text-text-primary">
                    {t!(i18n, canvas.title)}
                </h1>
                <p class="max-w-sm text-sm text-text-secondary">
                    {t!(i18n, canvas.subtitle)}
                </p>
                <p class="text-xs text-text-tertiary">
                    {t!(i18n, canvas.select_hint)}
                </p>
                <button
                    class="mt-2 px-3.5 py-2 rounded-lg bg-primary hover:bg-primary-hover text-white text-sm font-medium transition-colors disabled:opacity-50"
                    prop:disabled=move || creating.get()
                    on:click=move |_| create_canvas(state, canvas, i18n, creating)
                >
                    {t!(i18n, canvas.new_canvas)}
                </button>
            </div>
        </div>
    }
}

/// The open document: header (way back, editable title, shape count) over the
/// editor surface. The editor mounts only once the fetch has landed — its
/// camera gestures and key listeners have no business existing for a spinner.
///
/// The title here is the **second** rename surface, and it goes through
/// `library::submit_title` exactly like the sidebar row does: same gate, same
/// no-op skip, same base-revision precedence, same one-retry conflict
/// handling. Two surfaces, one function — a hand-written second copy is how
/// one of them quietly stops working.
#[component]
fn OpenCanvasPane() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    // Title editing, local to this pane: an edit in progress is this visit's
    // interaction state and must not survive closing the canvas.
    let editing_title = RwSignal::new(false);
    let title_draft = RwSignal::new(String::new());
    // A `TitleRejection`, not an `Option<String>`: the type is what proves
    // this can never carry an unclassified server error, and it keeps the
    // wording out of the signal so the message renders in the reader's
    // language where it is displayed.
    let title_error = RwSignal::new(Option::<aleph_protocol::canvas::TitleRejection>::None);
    let title_input = NodeRef::<leptos::html::Input>::new();

    let title_now = move || {
        canvas
            .doc
            .with(|d| d.as_ref().map(|d| d.title.clone()))
            .unwrap_or_default()
    };

    // Deferred a tick for the same reason as the sidebar's twin: the input is
    // created inside a nested reactive closure, so the `NodeRef` is not bound
    // when this effect first runs.
    Effect::new(move |_| {
        if !editing_title.get() {
            return;
        }
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(10).await;
            if let Some(el) = title_input.get() {
                let input: &web_sys::HtmlInputElement = &el;
                let _ = input.focus();
                input.select();
            }
        });
    });

    // Same two endings as the sidebar row: Enter keeps a refusal visible,
    // blur drops it rather than trapping the user in a red input.
    let commit_title = move |keep_open_on_refusal: bool| {
        let Some(id) = canvas.open_canvas.get_untracked() else {
            editing_title.set(false);
            return;
        };
        let draft = title_draft.get_untracked();
        if let Err(why) = library::submit_title(state, canvas, i18n, &id, &draft) {
            if keep_open_on_refusal {
                title_error.set(Some(why));
                return;
            }
        }
        editing_title.set(false);
        title_error.set(None);
    };

    // Memoized on purpose: the raw `doc.with(|d| d.is_none())` closure would
    // re-run — and rebuild the editor, discarding its drag/undo/queue state —
    // on EVERY doc mutation, including each optimistic preview frame of a
    // drag. The memo's `PartialEq` dedupe means the editor mounts once per
    // open and unmounts once per close, nothing in between.
    let doc_missing = Memo::new(move |_| canvas.doc.with(|d| d.is_none()));

    view! {
        <div class="flex flex-col h-full">
            <div class="px-6 py-4 border-b border-border aleph-content-top flex items-center gap-3">
                <button
                    class="flex items-center gap-1.5 text-sm text-text-secondary hover:text-text-primary transition-colors"
                    on:click=move |_| canvas.close_canvas()
                >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                    {t!(i18n, canvas.back_to_library)}
                </button>
                {move || if editing_title.get() {
                    view! {
                        <input
                            node_ref=title_input
                            type="text"
                            prop:value=move || title_draft.get()
                            on:input=move |ev| {
                                title_draft.set(event_target_value(&ev));
                                title_error.set(None);
                            }
                            on:blur=move |_| commit_title(false)
                            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                match ev.key().as_str() {
                                    "Enter" => commit_title(true),
                                    "Escape" => {
                                        editing_title.set(false);
                                        title_error.set(None);
                                    }
                                    _ => {}
                                }
                            }
                            class="min-w-0 flex-1 max-w-md px-2 py-1 bg-surface-sunken border border-primary/60 rounded text-base font-semibold text-text-primary focus:outline-none"
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <h2
                            class="text-base font-semibold text-text-primary truncate cursor-text hover:text-primary transition-colors"
                            title=move || t_string!(i18n, canvas.rename).to_string()
                            on:click=move |_| {
                                title_draft.set(title_now());
                                title_error.set(None);
                                editing_title.set(true);
                            }
                        >
                            {title_now}
                        </h2>
                    }
                    .into_any()
                }}
                {move || title_error.get().map(|why| view! {
                    <span class="text-xs text-danger">{library::rejection_label(i18n, why)}</span>
                })}
                <span class="text-xs text-text-tertiary">
                    {move || canvas.doc.with(|d| d.as_ref().map(|d| d.shapes.len().to_string())).unwrap_or_default()}
                    " " {t!(i18n, canvas.shapes)}
                </span>
            </div>
            {move || {
                canvas.load_error.get().map(|msg| view! {
                    <div class="mx-6 mt-4 px-4 py-3 rounded-lg bg-warning-subtle border border-warning text-sm text-text-primary">
                        {msg}
                    </div>
                })
            }}
            {move || if doc_missing.get() {
                view! {
                    <div class="flex-1 flex items-center justify-center text-sm text-text-tertiary">
                        {t!(i18n, common.loading)}
                    </div>
                }
                .into_any()
            } else {
                view! { <editor::CanvasEditor /> }.into_any()
            }}
        </div>
    }
}

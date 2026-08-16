//! Whiteboard canvas — `/canvas`: the library page, and (from Task 12) the
//! editor.
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
//!    rows, and refetches the open document when someone else moved it.
//!    Incremental ops application (echo suppression, gap detection) is the
//!    Task 17 reconciler; until then a whole-doc refetch is the correct,
//!    dumber answer.

use aleph_protocol::canvas as canvas_proto;
use aleph_protocol::canvas::CanvasRow;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
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

/// Fetch one canvas into the shared state, keeping the answer only if that
/// canvas is still the open one by the time it arrives.
///
/// One function rather than three inlined copies (open-click, create, frame
/// refresh), because the staleness check is the part a copy would drop.
fn fetch_open_doc(
    state: DashboardState,
    canvas: CanvasState,
    i18n: crate::i18n::I18nCtx,
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

/// Refetch the library rows. Errors are ignored here — this runs on event
/// frames and after writes, where the load `Effect` (which does classify
/// errors) remains the surface that reports a broken connection.
fn refresh_rows(state: DashboardState, canvas: CanvasState) {
    spawn_local(async move {
        if let Ok(list) = CanvasApi::list(&state).await {
            canvas.rows.set(list);
        }
    });
}

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

    // (3) Frame consumption — refresh the library, refetch a moved open doc.
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
        let held = canvas
            .doc
            .with_untracked(|d| d.as_ref().map(|d| d.revision));
        if held != Some(frame.revision) {
            fetch_open_doc(state, canvas, i18n, frame.canvas_id);
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    view! {
        {move || match canvas.open_canvas.get() {
            Some(_) => view! { <OpenCanvasPane /> }.into_any(),
            None => view! { <LibraryPane /> }.into_any(),
        }}
    }
}

/// The library: every canvas the caller can see, a create button, and a
/// per-row delete with inline confirmation.
#[component]
fn LibraryPane() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    // Row id awaiting delete confirmation. Inline (not a modal): the second
    // click lands where the first one did.
    let pending_delete = RwSignal::new(Option::<String>::None);
    let creating = RwSignal::new(false);

    let on_create = move |_| {
        if creating.get_untracked() {
            return;
        }
        creating.set(true);
        spawn_local(async move {
            match CanvasApi::create(&state, None, None).await {
                Ok(doc) => {
                    refresh_rows(state, canvas);
                    // The create envelope carries no asset_base (the server
                    // mints one only on `canvas.get`), so opening goes
                    // through the same fetch as a row click.
                    canvas.open_canvas.set(Some(doc.id.clone()));
                    canvas.doc.set(None);
                    fetch_open_doc(state, canvas, i18n, doc.id);
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
    };

    view! {
        <div class="flex flex-col h-full">
            <div class="p-6 border-b border-border aleph-content-top flex items-start justify-between gap-4">
                <div>
                    <h1 class="text-2xl font-bold text-text-primary">
                        {t!(i18n, canvas.title)}
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        {t!(i18n, canvas.subtitle)}
                    </p>
                </div>
                <button
                    class="px-3.5 py-2 rounded-lg bg-primary hover:bg-primary-hover text-white text-sm font-medium transition-colors disabled:opacity-50 flex-shrink-0"
                    prop:disabled=move || creating.get()
                    on:click=on_create
                >
                    {t!(i18n, canvas.new_canvas)}
                </button>
            </div>

            {move || {
                canvas.load_error.get().map(|msg| view! {
                    <div class="mx-6 mt-4 px-4 py-3 rounded-lg bg-warning-subtle border border-warning text-sm text-text-primary">
                        {msg}
                    </div>
                })
            }}

            <div class="flex-1 overflow-y-auto p-6">
                {move || {
                    let rows = canvas.rows.get();
                    if rows.is_empty() {
                        return view! {
                            <div class="h-full flex items-center justify-center text-sm text-text-tertiary">
                                {t!(i18n, canvas.empty)}
                            </div>
                        }
                        .into_any();
                    }
                    view! {
                        <div class="space-y-2 max-w-3xl">
                            <For
                                each=move || canvas.rows.get()
                                key=|row| (row.id.clone(), row.revision)
                                children=move |row: CanvasRow| {
                                    view! { <LibraryRow row=row pending_delete=pending_delete /> }
                                }
                            />
                        </div>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn LibraryRow(row: CanvasRow, pending_delete: RwSignal<Option<String>>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    let id = row.id.clone();
    let open_id = row.id.clone();
    let arm_id = row.id.clone();
    let confirm_id = row.id.clone();
    let is_armed = move || pending_delete.get().as_deref() == Some(id.as_str());

    let on_open = move |_| {
        canvas.open_canvas.set(Some(open_id.clone()));
        canvas.doc.set(None);
        fetch_open_doc(state, canvas, i18n, open_id.clone());
    };

    let on_confirm_delete = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let id = confirm_id.clone();
        spawn_local(async move {
            match CanvasApi::delete(&state, &id).await {
                Ok(()) => refresh_rows(state, canvas),
                Err(e) => {
                    canvas
                        .load_error
                        .set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to delete canvas: {e}")
                        })));
                }
            }
            pending_delete.set(None);
        });
    };

    view! {
        <div
            class="group flex items-center gap-3 px-4 py-3 rounded-xl border border-border bg-surface-raised hover:border-primary/50 cursor-pointer transition-colors"
            on:click=on_open
        >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"
                 class="text-text-tertiary flex-shrink-0">
                <rect x="3" y="3" width="18" height="18" rx="2" />
                <path d="M7 14c1.5-4 3-4 4.5-1s3 3 5.5-3" />
            </svg>
            <div class="flex-1 min-w-0">
                <div class="text-sm font-medium text-text-primary truncate">{row.title.clone()}</div>
                <div class="text-xs text-text-tertiary mt-0.5">
                    {row.shape_count.to_string()}
                    " " {t!(i18n, canvas.shapes)}
                    " · " {updated_label(row.updated_at_ms)}
                </div>
            </div>
            {move || if is_armed() {
                view! {
                    <div class="flex items-center gap-2" on:click=|ev| ev.stop_propagation()>
                        <span class="text-xs text-text-secondary">
                            {t!(i18n, common.confirm_delete)}
                        </span>
                        <button
                            class="px-2.5 py-1 rounded-md bg-danger text-white text-xs font-medium"
                            on:click=on_confirm_delete.clone()
                        >
                            {t!(i18n, common.delete)}
                        </button>
                        <button
                            class="px-2.5 py-1 rounded-md border border-border text-xs text-text-secondary hover:bg-surface-sunken"
                            on:click=move |ev| { ev.stop_propagation(); pending_delete.set(None); }
                        >
                            {t!(i18n, common.cancel)}
                        </button>
                    </div>
                }.into_any()
            } else {
                let arm = arm_id.clone();
                view! {
                    <button
                        class="opacity-0 group-hover:opacity-100 p-1.5 rounded-md text-text-tertiary hover:text-danger hover:bg-danger/10 transition-all"
                        title=move || t_string!(i18n, common.delete).to_string()
                        on:click=move |ev| { ev.stop_propagation(); pending_delete.set(Some(arm.clone())); }
                    >
                        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="3 6 5 6 21 6" />
                            <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                        </svg>
                    </button>
                }.into_any()
            }}
        </div>
    }
}

/// The open document — a placeholder shell until the editor lands (Task 12).
/// Shows the fetched document is real (title, shape count) and offers the way
/// back; the SVG surface replaces the placeholder body, not this frame.
#[component]
fn OpenCanvasPane() -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

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
                <h2 class="text-base font-semibold text-text-primary truncate">
                    {move || canvas.doc.with(|d| d.as_ref().map(|d| d.title.clone())).unwrap_or_default()}
                </h2>
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
            <div class="flex-1 flex items-center justify-center text-sm text-text-tertiary">
                {move || if canvas.doc.with(|d| d.is_none()) {
                    t!(i18n, common.loading).into_any()
                } else {
                    t!(i18n, canvas.editor_pending).into_any()
                }}
            </div>
        </div>
    }
}

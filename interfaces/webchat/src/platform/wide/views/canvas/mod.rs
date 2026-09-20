//! Whiteboard canvas — the **canvas body of the workspace pane**: a header
//! strip, the editor surface, and the three liveness wires that feed it.
//!
//! It used to be a section of its own — its own route, its own panel mode,
//! the library as the left column. It is now one of the bodies
//! `components/workspace_panel.rs` can show beside the chat, because a
//! whiteboard the model draws on while you talk to it is worth more next to
//! the conversation than in a section you have to leave the conversation to
//! reach. The library came along as [`library::CanvasPicker`], a popover on
//! the header strip below; the welcome pane inlines the same list.
//!
//! Not the memory galaxy: that renderer moved to `views/memory/galaxy/` when
//! the whiteboard claimed the `canvas` name. This module is the Panel half of
//! the `canvas.*` RPC family (`api/canvas.rs`) and of the `canvas.updated`
//! event topic.
//!
//! # Liveness
//!
//! Three wires, all mounted here because this view is a keep-alive container
//! — `WorkspacePanel` hides the canvas body with `style:display` instead of
//! unmounting it, exactly as `MainContent` used to:
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
mod arrow_geom;
mod asset_ingest;
mod decks;
mod editor;
mod export;
mod freehand;
mod geo_path;
mod id_mint;
mod interaction;
mod library;
mod ops;
mod present;
mod reconcile;
mod reveal;
mod shape_view;
mod sketch;
mod snap;
mod text_edit;
mod toolbar;
mod viewport;

pub(crate) use library::open_canvas;

use aleph_protocol::canvas as canvas_proto;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::canvas::CanvasState;

use library::{create_canvas, fetch_open_doc, refresh_rows};

/// The registry name of the model's whiteboard tool.
///
/// One spelling for every Panel-side consumer of it: `ai.rs`'s message
/// templates (prose that instructs the model to call it), the chat stream's
/// auto-reveal, and the transcript card's "open in canvas". Prose naming a
/// tool is a second copy of that tool's name (CANVAS.md §6 / §4.11 round-12);
/// this constant is the one copy, and `tests/canvas_wire.rs` resolves it
/// against the real tool table server-side.
pub(crate) const CANVAS_TOOL: &str = "canvas";

/// The canvas a `canvas` tool result touched, if any.
///
/// Every action that reads or writes a document reports `canvas_id` at the
/// top level of its output; `list`, a refusal and an error do not, and those
/// must reveal nothing. Two wire shapes are accepted because the harness
/// serializes a tool's structured output as a **JSON-encoded string** inside
/// `Success.output` (the same text the model sees) while the in-process and
/// replayed forms carry the object itself — a reader that knew only one of
/// them would be silently dead for the production one, which is exactly how
/// the plan-snapshot projection in `views/chat/events.rs` was broken once
/// (`aleph_protocol::plan::snapshot_value_from_tool_output`).
pub(crate) fn canvas_id_from_tool_result(result: &serde_json::Value) -> Option<String> {
    let output = result
        .get("Success")
        .and_then(|s| s.get("output"))
        .unwrap_or(result);
    let decoded;
    let object = match output {
        serde_json::Value::String(text) => {
            decoded = serde_json::from_str::<serde_json::Value>(text).ok()?;
            &decoded
        }
        other => other,
    };
    object
        .get("canvas_id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
        .map(std::string::ToString::to_string)
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
        <div class="flex flex-col h-full min-h-0">
            <CanvasHeader />
            {move || match canvas.open_canvas.get() {
                Some(_) => view! { <OpenCanvasPane /> }.into_any(),
                None => view! { <WelcomePane /> }.into_any(),
            }}
        </div>
    }
}

/// Shown while no canvas is open: what this body is, and the list.
///
/// The list is [`library::CanvasList`] — the same component the picker
/// popover hosts, not a second rendering of it. Inlining it here rather than
/// pointing at the picker is the difference between "there is nothing here"
/// and "open a menu to find out there is nothing here"; and because both
/// hosts are one component, there is still exactly one answer to "which
/// canvases are there".
#[component]
fn WelcomePane() -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    view! {
        <div class="flex-1 min-h-0 flex flex-col">
            {move || {
                canvas.load_error.get().map(|msg| view! {
                    <div class="mx-3 mt-3 px-3 py-2 rounded-lg bg-warning-subtle border border-warning text-xs text-text-primary">
                        {msg}
                    </div>
                })
            }}
            <div class="flex flex-col items-center gap-1.5 px-6 pt-6 pb-3 text-center shrink-0">
                <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"
                     class="text-text-tertiary">
                    <rect x="3" y="3" width="18" height="18" rx="2" />
                    <path d="M7 14c1.5-4 3-4 4.5-1s3 3 5.5-3" />
                </svg>
                <h1 class="text-base font-semibold text-text-primary">
                    {t!(i18n, canvas.title)}
                </h1>
                <p class="max-w-sm text-xs text-text-secondary">
                    {t!(i18n, canvas.subtitle)}
                </p>
            </div>
            <div class="flex-1 min-h-0 border-t border-border">
                // Nothing to dismiss: this host IS the pane.
                <library::CanvasList on_pick=Callback::new(|()| {}) />
            </div>
        </div>
    }
}

/// The canvas body's header strip: library picker, the open document's title
/// (rename in place), its shape count, and the close affordance.
///
/// Above BOTH panes, not inside the open one, so the picker is reachable from
/// the empty state too — that is the whole point of a picker.
///
/// The title here is the **second** rename surface, and it goes through
/// `library::submit_title` exactly like the list row does: same gate, same
/// no-op skip, same base-revision precedence, same one-retry conflict
/// handling — and through `library::end_rename` for the ending, so the edit
/// state is the commit gate here too. Two surfaces, one function — a
/// hand-written second copy is how one of them quietly stops working, and
/// the copy this strip used to carry gated on "is a canvas open" and let the
/// blur of the unmounting input commit on Escape and resend on Enter.
#[component]
fn CanvasHeader() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();
    let creating = RwSignal::new(false);

    // Title editing, local to this strip: an edit in progress is this visit's
    // interaction state and must not survive closing the canvas. The id of
    // the canvas being renamed, like the list row's `renaming` — the same
    // shape so the same ending can gate on it.
    let editing_title = RwSignal::new(Option::<String>::None);
    let title_draft = RwSignal::new(String::new());
    // A `TitleRejection`, not an `Option<String>`: the type is what proves
    // this can never carry an unclassified server error, and it keeps the
    // wording out of the signal so the message renders in the reader's
    // language where it is displayed.
    let title_error = RwSignal::new(Option::<aleph_protocol::canvas::TitleRejection>::None);
    let title_input = NodeRef::<leptos::html::Input>::new();

    let is_open = Memo::new(move |_| canvas.open_canvas.with(Option::is_some));
    let title_now = move || {
        canvas
            .doc
            .with(|d| d.as_ref().map(|d| d.title.clone()))
            .unwrap_or_default()
    };

    // Deferred a tick for the same reason as the list row's twin: the input is
    // created inside a nested reactive closure, so the `NodeRef` is not bound
    // when this effect first runs.
    Effect::new(move |_| {
        if editing_title.get().is_none() {
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

    // The list row's ending, verbatim: gate on the edit, not on the open
    // canvas (`library::end_rename` says why).
    let commit_title = move |keep_open_on_refusal: bool| {
        library::end_rename(
            editing_title,
            title_draft,
            title_error,
            keep_open_on_refusal,
            |id, draft| library::submit_title(state, canvas, i18n, id, draft),
        );
    };

    view! {
        <div class="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
            <library::CanvasPicker />
            <Show when=move || is_open.get()>
                {move || if editing_title.get().is_some() {
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
                                        editing_title.set(None);
                                        title_error.set(None);
                                    }
                                    _ => {}
                                }
                            }
                            class="min-w-0 flex-1 px-2 py-0.5 bg-surface-sunken border border-primary/60 rounded text-sm font-semibold text-text-primary focus:outline-none"
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <h2
                            class="min-w-0 flex-1 text-sm font-semibold text-text-primary truncate cursor-text hover:text-primary transition-colors"
                            title=move || t_string!(i18n, canvas.rename).to_string()
                            on:click=move |_| {
                                title_draft.set(title_now());
                                title_error.set(None);
                                editing_title.set(canvas.open_canvas.get_untracked());
                            }
                        >
                            {title_now}
                        </h2>
                    }
                    .into_any()
                }}
                {move || title_error.get().map(|why| view! {
                    <span class="text-[11px] text-danger shrink-0">{library::rejection_label(i18n, why)}</span>
                })}
                <span class="text-[11px] text-text-tertiary shrink-0">
                    {move || canvas.doc.with(|d| d.as_ref().map(|d| d.shapes.len().to_string())).unwrap_or_default()}
                    " " {t!(i18n, canvas.shapes)}
                </span>
                <button
                    class="shrink-0 p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-sunken"
                    title=move || t_string!(i18n, canvas.close_canvas).to_string()
                    on:click=move |_| canvas.close_canvas()
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <line x1="18" y1="6" x2="6" y2="18" />
                        <line x1="6" y1="6" x2="18" y2="18" />
                    </svg>
                </button>
            </Show>
            <Show when=move || !is_open.get()>
                <span class="flex-1" />
                <button
                    class="shrink-0 px-2.5 py-1 rounded-md bg-primary hover:bg-primary-hover text-white text-xs font-medium transition-colors disabled:opacity-50"
                    prop:disabled=move || creating.get()
                    on:click=move |_| create_canvas(state, canvas, i18n, creating)
                >
                    {t!(i18n, canvas.new_canvas)}
                </button>
            </Show>
        </div>
    }
}

/// The open document: the editor surface, under the shared header strip.
/// The editor mounts only once the fetch has landed — its camera gestures and
/// key listeners have no business existing for a spinner.
#[component]
fn OpenCanvasPane() -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    // Memoized on purpose: the raw `doc.with(|d| d.is_none())` closure would
    // re-run — and rebuild the editor, discarding its drag/undo/queue state —
    // on EVERY doc mutation, including each optimistic preview frame of a
    // drag. The memo's `PartialEq` dedupe means the editor mounts once per
    // open and unmounts once per close, nothing in between.
    let doc_missing = Memo::new(move |_| canvas.doc.with(|d| d.is_none()));

    view! {
        <div class="flex-1 min-h-0 flex flex-col">
            {move || {
                canvas.load_error.get().map(|msg| view! {
                    <div class="mx-3 mt-3 px-3 py-2 rounded-lg bg-warning-subtle border border-warning text-xs text-text-primary">
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The production wire shape: the harness JSON-encodes the tool's output
    /// into a **string** inside `Success.output`. A reader that only knew the
    /// object form would be permanently dead here and nothing would say so.
    #[test]
    fn the_canvas_id_is_read_out_of_the_json_encoded_string_the_harness_emits() {
        let result = json!({
            "Success": {
                "output": json!({"canvas_id": "cv-9", "revision": 4}).to_string()
            }
        });
        assert_eq!(
            canvas_id_from_tool_result(&result).as_deref(),
            Some("cv-9")
        );
    }

    /// …and the object form, which the in-process and replayed paths carry.
    #[test]
    fn the_object_form_of_the_same_output_reads_the_same() {
        let result = json!({"Success": {"output": {"canvas_id": "cv-9"}}});
        assert_eq!(
            canvas_id_from_tool_result(&result).as_deref(),
            Some("cv-9")
        );
        // A bare payload (no envelope) is accepted too — the envelope is the
        // harness's, not the tool's.
        assert_eq!(
            canvas_id_from_tool_result(&json!({"canvas_id": "cv-1"})).as_deref(),
            Some("cv-1")
        );
    }

    /// Nothing to reveal is `None`, never a placeholder: a refusal, an error,
    /// a `list` and an empty id all mean "there is no canvas to show", and
    /// turning any of them into an id would open a canvas that does not exist.
    #[test]
    fn a_result_that_names_no_canvas_reveals_nothing() {
        assert_eq!(
            canvas_id_from_tool_result(&json!({"Error": {"error": "denied"}})),
            None
        );
        assert_eq!(
            canvas_id_from_tool_result(&json!({"Success": {"output": {"canvases": []}}})),
            None
        );
        assert_eq!(
            canvas_id_from_tool_result(&json!({"Success": {"output": "not json"}})),
            None
        );
        assert_eq!(
            canvas_id_from_tool_result(&json!({"Success": {"output": {"canvas_id": ""}}})),
            None
        );
        assert_eq!(
            canvas_id_from_tool_result(&json!({"Success": {"output": {"canvas_id": 7}}})),
            None
        );
    }
}

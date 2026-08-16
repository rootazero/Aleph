//! Canvas status overlay: the difference between "no notes", "still loading"
//! and "the server said no".
//!
//! Before this existed all three collapsed into the same featureless black
//! rectangle — a zero-note agent, a failed `graph.query` and an in-flight
//! round-trip were literally indistinguishable, which made the first thing a
//! new user ever sees look like a crash. This component is pure presentation
//! (R4): it renders the status the host derives and emits a retry intent.

use leptos::callback::Callback;
use leptos::prelude::*;

use crate::i18n::{t, use_i18n};

/// What the canvas is currently showing, in priority order (a GL failure beats
/// a load failure beats a pending round-trip beats an empty graph).
///
/// The two failure variants differ in whether the user can do anything about it,
/// and the UI must not pretend otherwise:
///
/// - [`CanvasStatus::Error`] — the `graph.query` round-trip failed. Retrying is
///   exactly the right move, so the card offers a Retry button.
/// - [`CanvasStatus::Fatal`] — WebGL2 init failed, or the GPU context was lost.
///   The scene is built once, on mount; nothing the host can re-run will bring it
///   back, so offering Retry would be a button that provably does nothing. The
///   card asks for a page reload instead.
///
/// Both carry the raw detail, rendered under the translated headline.
#[derive(Clone, PartialEq, Eq)]
pub enum CanvasStatus {
    Loading,
    Empty,
    Error(String),
    Fatal(String),
    Ready,
}

/// Absolutely-positioned status layer over the galaxy.
///
/// Click-through everywhere except the Retry button and the notice strip, so
/// the canvas keeps every pointer gesture while a notice is up.
#[component]
#[must_use]
pub fn CanvasOverlay(
    /// Derived by the host from (gl_error, load_error, loading, node count).
    status: Signal<CanvasStatus>,
    /// Transient one-line message ("no matches", "that note isn't in the loaded
    /// galaxy"). Cleared by the host on the next selection / rebuild, and by a
    /// click on the strip itself.
    notice: RwSignal<Option<String>>,
    /// Re-runs the galaxy fetch.
    on_retry: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();

    let card = "flex flex-col items-center gap-2 max-w-xs text-center px-5 py-4 rounded-lg \
                bg-black/50 border border-white/10 backdrop-blur-sm text-white/80 text-xs \
                leading-relaxed";

    view! {
        <div class="absolute inset-0 pointer-events-none">
            <div class="absolute inset-0 flex items-center justify-center">
                {move || match status.get() {
                    CanvasStatus::Ready => ().into_any(),
                    CanvasStatus::Loading => view! {
                        <div class=card>
                            <span class="text-white/60 italic">{t!(i18n, memory.canvas_loading)}</span>
                        </div>
                    }
                    .into_any(),
                    CanvasStatus::Empty => view! {
                        <div class=card>
                            <span class="text-2xl select-none">"✦"</span>
                            <span>{t!(i18n, memory.graph_empty)}</span>
                        </div>
                    }
                    .into_any(),
                    CanvasStatus::Error(detail) => view! {
                        <div class=card>
                            <span class="text-white">{t!(i18n, memory.graph_error)}</span>
                            <span class="text-white/50 break-words">{detail}</span>
                            <button
                                class="pointer-events-auto mt-1 px-2 py-0.5 rounded border \
                                       border-white/20 hover:bg-white/10 transition-colors"
                                on:click=move |_| on_retry.run(())
                            >
                                {t!(i18n, memory.retry)}
                            </button>
                        </div>
                    }
                    .into_any(),
                    // No Retry here: the GL context is built once on mount, so a
                    // re-fetch cannot revive it. Ask for the reload that actually works.
                    CanvasStatus::Fatal(detail) => view! {
                        <div class=card>
                            <span class="text-white">{t!(i18n, memory.graph_gl_fatal)}</span>
                            <span class="text-white/50 break-words">{detail}</span>
                        </div>
                    }
                    .into_any(),
                }}
            </div>
            {move || notice.get().map(|msg| view! {
                <div
                    class="pointer-events-auto absolute top-2 left-1/2 -translate-x-1/2 cursor-pointer
                           text-[11px] text-amber-200/90 bg-black/50 rounded px-2 py-0.5 select-none"
                    on:click=move |_| notice.set(None)
                >
                    {msg}
                </div>
            })}
        </div>
    }
}

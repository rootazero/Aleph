//! Canvas surface — STUB SEAM (P2). Placeholder only; unreachable until an
//! emitter produces [`crate::state::inspector::InspectorTarget::Canvas`].
//!
//! FUTURE DROP-IN (do not build now):
//! - Preferred: a JSON-Canvas 1.0 document rendered as DOM `<div>` nodes +
//!   SVG `<path>` edges — no new crate, LLM emits the canvas as tool output,
//!   this surface stays a pure renderer (R4). Put the wire struct (serde /
//!   schemars) under `crate::canvas_engine`.
//! - Heavyweight alt: reuse the existing WebGL2 galaxy `Scene`
//!   (`platform/wide/views/canvas/gl/`) only if a surface ever needs 500+ live
//!   nodes — importing tldraw / infinite-canvas is ruled out (R3/R10 bloat).
//!
//! When graduating: add the emitter (a canvas-producing tool's output field),
//! then swap this component's body for the renderer. The router arm and the
//! `Canvas { doc_id }` variant already exist, so nothing else changes.

use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// Placeholder for the future UI-canvas surface.
#[component]
pub fn CanvasStub(doc_id: String) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col items-center justify-center
                    text-center gap-2 py-12 px-6 text-text-tertiary">
            <span class="text-2xl">"🎨"</span>
            <p class="text-sm text-text-secondary">{t!(i18n, common.inspector_canvas_soon)}</p>
            <p class="text-[11px] font-mono opacity-60 break-all">{doc_id}</p>
        </div>
    }
}

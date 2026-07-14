//! Browser surface — STUB SEAM (P3). Placeholder only; unreachable until an
//! emitter produces [`crate::state::inspector::InspectorTarget::Browser`].
//!
//! FUTURE DROP-IN (do not build now):
//! - Preferred: a screenshot-stream shell. Per R1 the real browser is a limb —
//!   a server/native browser driver frames the page and pushes them over the
//!   existing WS/JSON-RPC (config already at `crate::api::browser`); this
//!   surface renders frames + forwards clicks as RPC. No cross-origin limit.
//! - Degenerate alt: a sandboxed `<iframe>` for agent-generated / localhost
//!   pages only (real external sites are blocked by X-Frame-Options).
//! - Real EXTERNAL URLs a user wants to open go to OS hand-off
//!   (`desktop/shell/src/external_link.rs`), never the panel's lone webview (R5).
//!
//! When graduating: add the emitter (a `web_fetch`/`browse` tool-output URL),
//! then swap this component's body. The router arm and `Browser { url }`
//! variant already exist.

use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// Placeholder for the future built-in-browser surface.
#[component]
pub fn BrowserStub(url: String) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col items-center justify-center
                    text-center gap-2 py-12 px-6 text-text-tertiary">
            <span class="text-2xl">"🌐"</span>
            <p class="text-sm text-text-secondary">{t!(i18n, common.inspector_browser_soon)}</p>
            <p class="text-[11px] font-mono opacity-60 break-all">{url}</p>
        </div>
    }
}

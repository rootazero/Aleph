//! Phone canvas screen (`/canvas`): a **read-only** library list plus an
//! "edit on desktop" note — v1 ships no phone editor (spec §1: wide only).
//!
//! Reached through the ••• More menu, so `PanelMode::Canvas.under_more()` is
//! true and the ••• tab keeps its highlight here (see `more.rs`).
//!
//! State is local signals, not [`crate::state::canvas::CanvasState`]: that
//! context is the wide editor's working set (camera, tool, open document),
//! and this screen only lists rows. Reading it would be harmless; the point
//! is that nothing here may *provide* it — the `context_ownership` guard's
//! rule that a phone screen never provides a type the desktop tree reads.

use aleph_protocol::canvas::CanvasRow;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::platform::phone::shell::PhoneShell;

#[component]
#[must_use]
pub fn PhoneCanvas() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let rows = RwSignal::new(Vec::<CanvasRow>::new());
    let load_error = RwSignal::new(Option::<String>::None);
    let loaded = RwSignal::new(false);

    // Load + reconnect — the same `is_connected`-gated Effect as the wide
    // library (WorkspacesView idiom): the first load waits for the socket,
    // a reconnect refetches.
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            match CanvasApi::list(&state).await {
                Ok(list) => {
                    rows.set(list);
                    load_error.set(None);
                }
                Err(e) => {
                    load_error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                        format!("Failed to load canvases: {e}")
                    })));
                }
            }
            loaded.set(true);
        });
    });

    view! {
        <PhoneShell title=t_string!(i18n, nav.canvas).to_string() back="/more" back_label="More">
            <div style="padding: 12px 16px 4px; font-size: 0.8rem; color: var(--color-text-tertiary);">
                {t!(i18n, canvas.phone_read_only)}
            </div>
            {move || {
                load_error.get().map(|msg| view! {
                    <div style="margin: 8px 16px; padding: 10px 12px; border-radius: 10px; border: 1px solid var(--color-warning); background: var(--color-warning-subtle); font-size: 0.85rem; color: var(--color-text-primary);">
                        {msg}
                    </div>
                })
            }}
            <div class="list">
                {move || {
                    let list = rows.get();
                    if list.is_empty() {
                        let empty_key = if loaded.get() {
                            t!(i18n, canvas.empty).into_any()
                        } else {
                            t!(i18n, common.loading).into_any()
                        };
                        return view! {
                            <div style="padding: 24px 16px; text-align: center; font-size: 0.85rem; color: var(--color-text-tertiary);">
                                {empty_key}
                            </div>
                        }
                        .into_any();
                    }
                    list.into_iter()
                        .map(|row| {
                            view! {
                                <div class="cell">
                                    <span class="cell-leading">
                                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                            <rect x="3" y="3" width="18" height="18" rx="2"></rect>
                                            <path d="M7 14c1.5-4 3-4 4.5-1s3 3 5.5-3"></path>
                                        </svg>
                                    </span>
                                    <div class="cell-body">
                                        <div class="cell-title">{row.title}</div>
                                    </div>
                                    <span class="cell-value">{row.shape_count.to_string()}</span>
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </div>
        </PhoneShell>
    }
}

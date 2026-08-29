//! Login wall — full-screen credential box shown when a remote Panel connected
//! without a valid credential (`DashboardState::needs_token`). Mirrors a browser
//! hitting the core's LAN IP and being asked to authorize. Loopback never sees
//! this.
//!
//! The box accepts any of the three credentials the server understands —
//! `DashboardState::submit_token` routes by prefix. In particular a **pairing
//! code** (`aleph-bt-…`) typed by hand is as good as the QR that encodes it,
//! which is the only path available when a phone cannot scan.

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn TokenWall() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();
    let token = RwSignal::new(String::new());

    let submit = move || {
        let t = token.get();
        if !t.trim().is_empty() {
            state.submit_token(t);
        }
    };

    view! {
        <Show when=move || state.needs_token.get()>
            <div class="fixed inset-0 z-[100] flex items-center justify-center bg-surface/95 backdrop-blur-sm p-6">
                <div class="max-w-md w-full bg-surface-raised border border-border rounded-2xl p-8 shadow-xl">
                    <h2 class="text-2xl font-bold text-text-primary mb-2">
                        {t!(i18n, common.token_wall_title)}
                    </h2>
                    <p class="text-sm text-text-secondary mb-6">
                        {move || {
                            if state.token_was_rejected.get() {
                                t_string!(i18n, common.token_wall_instruction_rejected).to_string()
                            } else {
                                t_string!(i18n, common.token_wall_instruction).to_string()
                            }
                        }}
                    </p>
                    <input
                        type="password"
                        class="w-full px-4 py-3 rounded-xl bg-surface-sunken border border-border text-text-primary font-mono text-sm mb-4 focus:outline-none focus:border-primary"
                        placeholder=move || t_string!(i18n, common.token_wall_placeholder).to_string()
                        prop:value=move || token.get()
                        on:input=move |ev| token.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                submit();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="w-full py-3 bg-primary hover:bg-primary/90 text-white rounded-xl transition-colors font-semibold disabled:opacity-50"
                        disabled=move || token.get().trim().is_empty()
                        on:click=move |_| submit()
                    >
                        {t!(i18n, common.token_wall_submit)}
                    </button>
                </div>
            </div>
        </Show>
    }
}

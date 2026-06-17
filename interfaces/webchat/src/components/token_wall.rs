//! Login wall — full-screen token box shown when a remote Panel connected
//! without a valid Gateway token (`DashboardState::needs_token`). Mirrors a
//! browser hitting the core's LAN IP and being asked to authorize: paste the
//! shared Gateway token (or open a `?token=` link / scan the QR) to unlock the
//! full app with the same authority as the local App. Loopback never sees this.

use crate::context::DashboardState;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn TokenWall() -> impl IntoView {
    let state = expect_context::<DashboardState>();
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
                        "Authorize this device"
                    </h2>
                    <p class="text-sm text-text-secondary mb-6">
                        "Enter the Gateway token to connect to this Aleph core. Get it from the \
                         core's Settings → Security, or run `aleph-server bootstrap-token` on the \
                         core machine. Once authorized, this device has the same access as the local app."
                    </p>
                    <input
                        type="password"
                        class="w-full px-4 py-3 rounded-xl bg-surface-sunken border border-border text-text-primary font-mono text-sm mb-4 focus:outline-none focus:border-primary"
                        placeholder="aleph-…"
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
                        "Authorize"
                    </button>
                </div>
            </div>
        </Show>
    }
}

//! Gateway login wall and the desktop's explicit pairing action.

use crate::api::native_credentials::native_expected;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

fn is_loopback_gateway(gateway_url: &str) -> bool {
    let Ok(url) = url::Url::parse(gateway_url) else {
        return false;
    };
    if !matches!(url.scheme(), "ws" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[component]
#[must_use]
pub fn TokenWall() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    view! {
        // The Gateway decides if this connection needs a credential. A
        // credentialless loopback authorized by Core opens the Panel directly.
        <Show when=move || state.needs_token.get()>
            <TokenPrompt />
        </Show>
    }
}

/// Pairing remains available to users who explicitly want to attach a device
/// identity to an already-authorized local desktop connection.
#[component]
#[must_use]
pub fn PairThisDevice() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let native = native_expected();
    let open = RwSignal::new(false);

    view! {
        <Show when=move || {
            native && state.is_connected.get() && !state.needs_token.get()
                && is_loopback_gateway(&state.gateway_url.get())
                && state.protected_user.get().is_none()
        }>
            <button
                data-testid="pair-this-device"
                type="button"
                class="px-3 py-2 rounded-lg border border-border text-text-primary hover:bg-surface-sunken"
                on:click=move |_| {
                    state.token_was_rejected.set(false);
                    open.set(true);
                }
            >
                {move || t_string!(i18n, common.native_pairing_submit).to_string()}
            </button>
            <Show when=move || open.get()>
                <TokenPrompt cancel=open />
            </Show>
        </Show>
    }
}

#[component]
fn TokenPrompt(#[prop(optional)] cancel: Option<RwSignal<bool>>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let native = native_expected();
    let token = RwSignal::new(String::new());

    // A new endpoint must never inherit an unsubmitted pairing code.
    Effect::new(move |_| {
        let _ = state.gateway_url.get();
        token.set(String::new());
    });

    let submit = move || {
        let value = token.get_untracked();
        if !value.trim().is_empty() {
            token.set(String::new());
            state.submit_token(value);
            if let Some(open) = cancel.filter(|_| state.needs_token.get_untracked()) {
                open.set(false);
            }
        }
    };

    view! {
        <div
            data-testid="token-wall"
            class="fixed inset-0 z-[100] flex items-center justify-center bg-surface/95 backdrop-blur-sm p-6"
            role="dialog"
            aria-modal="true"
            aria-labelledby="token-wall-title"
        >
            <div class="max-w-md w-full bg-surface-raised border border-border rounded-2xl p-8 shadow-xl">
                <h2 id="token-wall-title" class="text-2xl font-bold text-text-primary mb-2">
                    {move || if cancel.is_some() {
                        t_string!(i18n, common.native_pairing_submit).to_string()
                    } else {
                        t_string!(i18n, common.token_wall_title).to_string()
                    }}
                </h2>
                <p
                    data-testid=move || {
                        if state.token_was_rejected.get() {
                            "token-wall-instruction-rejected"
                        } else {
                            "token-wall-instruction"
                        }
                    }
                    class="text-sm text-text-secondary mb-6"
                >
                    {move || match (native, state.token_was_rejected.get()) {
                        (true, true) => t_string!(i18n, common.native_pairing_rejected).to_string(),
                        (true, false) => t_string!(i18n, common.native_pairing_instruction).to_string(),
                        (false, true) => t_string!(i18n, common.token_wall_instruction_rejected).to_string(),
                        (false, false) => t_string!(i18n, common.token_wall_instruction).to_string(),
                    }}
                </p>
                <form on:submit=move |ev| {
                    ev.prevent_default();
                    submit();
                }>
                    <label for="token-wall-input" class="sr-only">
                        {move || if native {
                            t_string!(i18n, common.native_pairing_label).to_string()
                        } else {
                            t_string!(i18n, common.token_wall_placeholder).to_string()
                        }}
                    </label>
                    <input
                        id="token-wall-input"
                        data-testid="token-wall-input"
                        type="password"
                        autocomplete="off"
                        class="w-full px-4 py-3 rounded-xl bg-surface-sunken border border-border text-text-primary font-mono text-sm mb-4 focus:outline-none focus:border-primary"
                        placeholder=move || if native {
                            t_string!(i18n, common.native_pairing_placeholder).to_string()
                        } else {
                            t_string!(i18n, common.token_wall_placeholder).to_string()
                        }
                        prop:value=move || token.get()
                        on:input=move |ev| token.set(event_target_value(&ev))
                    />
                    <button
                        data-testid="token-wall-submit"
                        type="submit"
                        class="w-full py-3 bg-primary hover:bg-primary/90 text-white rounded-xl transition-colors font-semibold disabled:opacity-50"
                        disabled=move || token.get().trim().is_empty()
                    >
                        {move || if native {
                            t_string!(i18n, common.native_pairing_submit).to_string()
                        } else {
                            t_string!(i18n, common.token_wall_submit).to_string()
                        }}
                    </button>
                </form>
                {cancel.map(|open| view! {
                    <button
                        type="button"
                        class="w-full mt-3 py-2 rounded-xl border border-border text-text-primary hover:bg-surface-sunken"
                        on:click=move |_| open.set(false)
                    >
                        {move || t_string!(i18n, common.cancel).to_string()}
                    </button>
                })}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_gateway;

    #[test]
    fn manual_pairing_is_available_only_for_loopback_gateway_urls() {
        for url in [
            "ws://127.0.0.1:18790/ws",
            "wss://localhost/ws",
            "ws://[::1]:18790/ws",
        ] {
            assert!(is_loopback_gateway(url), "{url}");
        }
        for url in [
            "ws://localhost@remote.example/ws",
            "ws://user:secret@localhost/ws",
            "wss://remote.example/ws",
            "ws://localhost.evil/ws",
            "https://localhost/",
            "",
        ] {
            assert!(!is_loopback_gateway(url), "{url}");
        }
    }
}

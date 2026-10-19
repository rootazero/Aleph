//! Native first-connection welcome and the existing remote login wall.
//!
//! A successful credentialless native connection may show a one-time welcome.
//! Dismissing it acknowledges an already-authorized connection; it never bypasses
//! Gateway authentication. Only the nonsecret endpoint acknowledgement is stored
//! in localStorage. Device credentials continue to use the native keyring.

use crate::api::native_credentials::native_expected;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

const WELCOME_KEY: &str = "aleph.native-welcome.v1";

/// Scope the acknowledgement to a local Gateway, without retaining URL secrets.
fn welcome_endpoint(gateway_url: &str) -> Option<String> {
    let url = url::Url::parse(gateway_url).ok()?;
    if !matches!(url.scheme(), "ws" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let local = match url.host()? {
        url::Host::Domain(host) => host == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    };
    local.then(|| url.origin().ascii_serialization())
}

fn welcome_visible(
    native: bool,
    connected: bool,
    protected: bool,
    acknowledged: bool,
    local_endpoint: bool,
) -> bool {
    native && connected && !protected && !acknowledged && local_endpoint
}

fn read_acknowledgements() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(WELCOME_KEY).ok().flatten())
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

fn persist_acknowledgements(endpoints: &[String]) {
    #[cfg(target_arch = "wasm32")]
    if let (Some(storage), Ok(value)) = (
        web_sys::window().and_then(|w| w.local_storage().ok().flatten()),
        serde_json::to_string(endpoints),
    ) {
        let _ = storage.set_item(WELCOME_KEY, &value);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (WELCOME_KEY, endpoints);
}

#[component]
#[must_use]
pub fn TokenWall() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();
    let native = native_expected();
    let token = RwSignal::new(String::new());
    // Keep the in-session acknowledgement even when localStorage is unavailable.
    let acknowledged = RwSignal::new(read_acknowledgements());
    let endpoint = Memo::new(move |_| welcome_endpoint(&state.gateway_url.get()));
    let welcome = Memo::new(move |_| {
        let endpoint = endpoint.get();
        welcome_visible(
            native,
            state.is_connected.get(),
            state.protected_user.get().is_some(),
            endpoint
                .as_ref()
                .is_some_and(|key| acknowledged.with(|keys| keys.contains(key))),
            endpoint.is_some(),
        )
    });

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
        }
    };

    view! {
        <Show when=move || state.needs_token.get() || welcome.get()>
            <div
                data-testid="token-wall"
                class="fixed inset-0 z-[100] flex items-center justify-center bg-surface/95 backdrop-blur-sm p-6"
                role="dialog"
                aria-modal="true"
                aria-labelledby="token-wall-title"
            >
                <div class="max-w-md w-full bg-surface-raised border border-border rounded-2xl p-8 shadow-xl">
                    <h2 id="token-wall-title" class="text-2xl font-bold text-text-primary mb-2">
                        {move || if welcome.get() && !state.needs_token.get() {
                            t_string!(i18n, common.native_welcome_title).to_string()
                        } else {
                            t_string!(i18n, common.token_wall_title).to_string()
                        }}
                    </h2>
                    <Show when=move || welcome.get() && !state.needs_token.get()>
                        <p data-testid="native-welcome" class="text-sm text-text-secondary mb-4">
                            {move || t_string!(i18n, common.native_welcome_body).to_string()}
                        </p>
                        <button
                            data-testid="native-welcome-continue"
                            type="button"
                            class="w-full py-3 mb-6 rounded-xl border border-border text-text-primary hover:bg-surface-sunken transition-colors font-semibold"
                            on:click=move |_| {
                                // Recheck at the click boundary: this action only dismisses
                                // a welcome for an already-authorized local connection.
                                if !welcome.get_untracked() || state.needs_token.get_untracked() {
                                    return;
                                }
                                if let Some(key) = endpoint.get_untracked() {
                                    acknowledged.update(|keys| {
                                        if !keys.contains(&key) {
                                            keys.push(key);
                                        }
                                    });
                                    acknowledged.with_untracked(|keys| persist_acknowledgements(keys));
                                }
                            }
                        >
                            {move || t_string!(i18n, common.native_welcome_continue).to_string()}
                        </button>
                    </Show>
                    <p
                        data-testid=move || {
                            if state.token_was_rejected.get() {
                                "token-wall-instruction-rejected"
                            } else {
                                "token-wall-instruction"
                            }
                        }
                        class="text-sm text-text-secondary mb-6">
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
                </div>
            </div>
        </Show>
    }.into_any()
}

#[cfg(test)]
mod tests {
    use super::{welcome_endpoint, welcome_visible};

    #[test]
    fn welcome_requires_a_successful_unpaired_native_local_connection() {
        assert!(welcome_visible(true, true, false, false, true));
        assert!(!welcome_visible(false, true, false, false, true));
        assert!(!welcome_visible(true, false, false, false, true));
        assert!(!welcome_visible(true, true, true, false, true));
        assert!(!welcome_visible(true, true, false, true, true));
        assert!(!welcome_visible(true, true, false, false, false));
    }

    #[test]
    fn acknowledgement_is_endpoint_scoped_and_contains_no_url_secrets() {
        assert_eq!(
            welcome_endpoint("ws://localhost:18790/ws?bt=secret#secret"),
            Some("ws://localhost:18790".into()),
        );
        assert_ne!(
            welcome_endpoint("ws://localhost:18790/ws"),
            welcome_endpoint("ws://localhost:18791/ws"),
        );
        for url in [
            "ws://127.0.0.1:18790/ws",
            "wss://localhost/ws",
            "ws://[::1]:18790/ws",
        ] {
            assert!(welcome_endpoint(url).is_some(), "{url}");
        }
        for url in [
            "ws://localhost@remote.example/ws",
            "ws://user:secret@localhost/ws",
            "wss://remote.example/ws",
            "ws://localhost.evil/ws",
            "https://localhost/",
            "",
        ] {
            assert!(welcome_endpoint(url).is_none(), "{url}");
        }
    }
}

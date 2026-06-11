//! Unified provider API-key field with stable-echo semantics
//!
//! The editable input ALWAYS starts empty (callers guarantee this). The stored
//! secret is never echoed back into the field. An empty value on save means
//! "keep the existing key". `has_api_key` drives the placeholder and a status
//! line so the user can tell whether a key is already on file.

use crate::components::ui::secret_input::SecretInput;
use crate::i18n::{t_string, use_i18n, I18nLocaleTrait};
use leptos::prelude::*;

/// A provider API-key input that never pre-fills the stored secret.
///
/// - `value` always starts empty; empty on save = keep existing key.
/// - `has_api_key` drives placeholder + status line.
/// - `hint` is an optional vendor placeholder shown when no key is set.
#[component]
#[must_use]
pub fn ProviderKeyField(
    /// Editable value (callers guarantee it starts empty).
    value: RwSignal<String>,
    /// Whether a key is already stored on the server.
    has_api_key: Signal<bool>,
    /// Optional vendor placeholder shown when unset.
    #[prop(optional)]
    hint: Option<String>,
) -> impl IntoView {
    let i18n = use_i18n();

    // SecretInput's placeholder prop is a plain String (non-reactive), so we
    // render one of two SecretInputs reactively based on `has_api_key`.
    let configured_placeholder =
        t_string!(i18n, settings.providers.key_configured_hint).to_string();
    let unset_placeholder =
        hint.unwrap_or_else(|| t_string!(i18n, settings.providers.key_unset_hint).to_string());

    view! {
        <div>
            {move || {
                if has_api_key.get() {
                    view! {
                        <SecretInput
                            value=value.into()
                            on_change=move |v| value.set(v)
                            placeholder=configured_placeholder.clone()
                            monospace=true
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <SecretInput
                            value=value.into()
                            on_change=move |v| value.set(v)
                            placeholder=unset_placeholder.clone()
                            monospace=true
                        />
                    }
                    .into_any()
                }
            }}
            <p class="mt-1 text-xs text-text-tertiary">
                {move || {
                    if has_api_key.get() {
                        t_string!(i18n, settings.providers.key_status_configured).to_string()
                    } else {
                        t_string!(i18n, settings.providers.key_status_unset).to_string()
                    }
                }}
            </p>
        </div>
    }
}

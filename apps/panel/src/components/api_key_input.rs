//! API key input with debounced change callback
//!
//! Wraps the existing SecretInput with debounced on-change behavior.

use leptos::prelude::*;
use super::ui::secret_input::SecretInput;

/// API key input with debounced change callback
#[component]
pub fn ApiKeyInput(
    /// Current API key value
    value: RwSignal<String>,
    /// Placeholder text
    #[prop(default = "Enter API key...")]
    placeholder: &'static str,
    /// Called when key changes (after debounce)
    #[prop(optional)]
    on_key_change: Option<Callback<String>>,
    /// Debounce delay in milliseconds
    #[prop(default = 500u32)]
    debounce_ms: u32,
) -> impl IntoView {
    // Hold the pending timeout handle in local (non-Send) storage.
    // Dropping the handle cancels the timer, which implements debouncing.
    let timer_handle: StoredValue<Option<gloo_timers::callback::Timeout>, leptos::prelude::LocalStorage> =
        StoredValue::new_local(None);

    let on_change = {
        let on_key_change = on_key_change.clone();
        move |val: String| {
            value.set(val.clone());

            // Cancel previous pending timer by dropping the handle
            timer_handle.set_value(None);

            // Schedule a new debounce timer if the value is non-empty
            if !val.is_empty() {
                if let Some(ref cb) = on_key_change {
                    let cb = cb.clone();
                    let handle = gloo_timers::callback::Timeout::new(debounce_ms, move || {
                        cb.run(val);
                    });
                    timer_handle.set_value(Some(handle));
                }
            }
        }
    };

    // Convert RwSignal to Signal for SecretInput
    let value_signal = Signal::derive(move || value.get());

    view! {
        <div class="api-key-input">
            <SecretInput
                value=value_signal
                on_change=on_change
                placeholder=placeholder
            />
        </div>
    }
}

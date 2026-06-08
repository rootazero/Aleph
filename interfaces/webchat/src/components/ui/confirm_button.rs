//! Inline two-step confirm button for destructive actions.
//!
//! Renders only the *armed* (confirm) state: a danger-filled button that fires
//! `on_confirm` when clicked and auto-reverts (clears `confirming`) when it
//! loses focus — i.e. the user clicked elsewhere.
//!
//! The idle button lives at the call site and flips `confirming` to `true` on
//! click. This keeps every idle button's own look/icon while rendering one
//! unified confirm state across the whole panel (provider/channel delete
//! buttons + the regenerate-token button).

use leptos::prelude::*;

use crate::i18n::*;

#[component]
pub fn ConfirmButton<F>(
    /// Drives the idle/armed toggle; owned by the call site so its idle button
    /// can set it `true`. This component sets it back to `false` on confirm or
    /// blur.
    confirming: RwSignal<bool>,
    /// Fired when the user clicks the armed confirm button.
    on_confirm: F,
    /// Armed-state label. Defaults to `common.confirm_delete` when `None`.
    #[prop(optional)]
    label: Option<Signal<String>>,
    /// Width utility class to match the idle button it replaces
    /// (e.g. `"w-full"`, `"flex-1"`, or `""` for natural width).
    #[prop(into, optional)]
    width_class: String,
) -> impl IntoView
where
    F: Fn() + 'static,
{
    let i18n = use_i18n();
    let btn_ref = NodeRef::<leptos::html::Button>::new();

    // Auto-focus the armed button so `on:blur` fires when the user clicks
    // elsewhere, reverting to the idle button.
    Effect::new(move |_| {
        if let Some(el) = btn_ref.get() {
            let _ = el.focus();
        }
    });

    let label_text = move || match label {
        Some(s) => s.get(),
        None => t_string!(i18n, common.confirm_delete).to_string(),
    };

    view! {
        <button
            node_ref=btn_ref
            on:click=move |_| {
                confirming.set(false);
                on_confirm();
            }
            on:blur=move |_| confirming.set(false)
            class=format!(
                "{width_class} px-4 py-2.5 bg-danger text-text-inverse rounded-lg \
                 hover:brightness-95 disabled:opacity-50 transition-colors font-medium text-sm"
            )
        >
            {label_text}
        </button>
    }
}

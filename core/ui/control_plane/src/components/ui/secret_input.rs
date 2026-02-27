//! Secret input component with visibility toggle
//!
//! A password/secret input field with an eye/eye-off toggle button
//! that switches between masked and plaintext display.

use leptos::prelude::*;

/// A styled secret input field with show/hide toggle
///
/// Renders an `<input>` that toggles between `type="password"` and `type="text"`,
/// with an eye icon button on the right side for toggling visibility.
///
/// # Example
/// ```rust
/// let (api_key, set_api_key) = signal(String::new());
/// view! {
///     <SecretInput
///         value=api_key
///         on_change=move |v| set_api_key.set(v)
///         placeholder="sk-..."
///         monospace=true
///     />
/// }
/// ```
#[component]
pub fn SecretInput(
    /// Current value
    value: Signal<String>,
    /// Change handler
    on_change: impl Fn(String) + 'static,
    /// Optional placeholder
    #[prop(optional)]
    placeholder: Option<&'static str>,
    /// Use monospace font
    #[prop(optional)]
    monospace: bool,
) -> impl IntoView {
    let visible = RwSignal::new(false);
    let font_class = if monospace { "font-mono" } else { "" };

    view! {
        <div class="relative">
            <input
                type=move || if visible.get() { "text" } else { "password" }
                value=move || value.get()
                on:input=move |ev| on_change(event_target_value(&ev))
                placeholder=placeholder.unwrap_or("")
                class=format!(
                    "w-full px-3 py-2 pr-10 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary {}",
                    font_class
                )
            />
            <button
                type="button"
                on:click=move |_| visible.update(|v| *v = !*v)
                class="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-text-tertiary hover:text-text-secondary transition-colors"
                aria-label=move || if visible.get() { "Hide secret" } else { "Show secret" }
            >
                {move || if visible.get() {
                    // Eye-off icon (secret is visible, click to hide)
                    view! {
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/>
                            <path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/>
                            <path d="M14.12 14.12a3 3 0 1 1-4.24-4.24"/>
                            <line x1="1" y1="1" x2="23" y2="23"/>
                        </svg>
                    }.into_any()
                } else {
                    // Eye icon (secret is hidden, click to show)
                    view! {
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                            <circle cx="12" cy="12" r="3"/>
                        </svg>
                    }.into_any()
                }}
            </button>
        </div>
    }
}

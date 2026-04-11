use crate::canvas_engine::types::ViewMode;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

#[component]
pub fn CanvasToolbar(
    view_mode: ReadSignal<ViewMode>,
    search_query: RwSignal<String>,
    on_toggle_mode: impl Fn() + 'static + Copy,
    on_search: impl Fn(String) + 'static + Copy,
) -> impl IntoView {
    let input_value = RwSignal::new(String::new());

    let on_input = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlInputElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
        if let Some(input) = target {
            input_value.set(input.value());
        }
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" {
            let val = input_value.get();
            search_query.set(val.clone());
            on_search(val);
        }
    };

    let mode_label = move || match view_mode.get() {
        ViewMode::Global { .. } => "Global",
        ViewMode::Local { .. } => "Local",
    };

    let is_global = move || matches!(view_mode.get(), ViewMode::Global { .. });

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-surface-raised border-b border-border">
            // Agent label
            <div class="flex items-center gap-2 text-sm font-medium text-text-secondary">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                    stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="10"/>
                    <line x1="12" y1="8" x2="12" y2="16"/>
                    <line x1="8" y1="12" x2="16" y2="12"/>
                </svg>
                "Knowledge Graph"
            </div>

            // Spacer
            <div class="flex-1" />

            // Search input
            <div class="relative">
                <input
                    type="text"
                    placeholder="Search entities..."
                    class="w-48 px-3 py-1.5 text-sm bg-surface-sunken border border-border rounded-lg
                           text-text-primary placeholder-text-tertiary focus:outline-none focus:border-primary/50"
                    on:input=on_input
                    on:keydown=on_keydown
                />
            </div>

            // Global/Local toggle
            <button
                on:click=move |_| on_toggle_mode()
                class=move || {
                    if is_global() {
                        "px-3 py-1.5 text-xs font-medium rounded-lg bg-primary/20 text-primary border border-primary/30"
                    } else {
                        "px-3 py-1.5 text-xs font-medium rounded-lg bg-accent/20 text-accent border border-accent/30"
                    }
                }
            >
                {mode_label}
            </button>
        </div>
    }
}

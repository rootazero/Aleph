use leptos::prelude::*;
use wasm_bindgen::JsCast;

#[component]
pub fn CanvasToolbar(
    search_query: RwSignal<String>,
    on_search: impl Fn(String) + 'static + Copy,
    fold_threshold: ReadSignal<usize>,
    set_fold_threshold: WriteSignal<usize>,
    /// (visible 1-hop count, total 1-hop count) for the "(K of N)" hint
    visible_counts: ReadSignal<(usize, usize)>,
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

    let on_slider_input = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlInputElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
        if let Some(input) = target {
            let v: usize = input.value().parse().unwrap_or(12);
            set_fold_threshold.set(v);
        }
    };

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-surface-raised border-b border-border">
            <div class="flex items-center gap-2 text-sm font-medium text-text-secondary">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                    stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="10"/>
                    <line x1="12" y1="8" x2="12" y2="16"/>
                    <line x1="8" y1="12" x2="16" y2="12"/>
                </svg>
                "Knowledge Graph"
            </div>

            <div class="flex-1" />

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

            <div class="flex items-center gap-1.5 text-xs text-text-secondary">
                <span>"Detail"</span>
                <input
                    type="range"
                    min="4"
                    max="30"
                    step="1"
                    class="w-24 accent-primary"
                    prop:value=move || fold_threshold.get().to_string()
                    on:input=on_slider_input
                />
                <span class="w-6 text-center">{move || fold_threshold.get()}</span>
                <span class="text-text-tertiary">
                    {move || {
                        let (k, n) = visible_counts.get();
                        format!("({k} of {n})")
                    }}
                </span>
            </div>
        </div>
    }
}

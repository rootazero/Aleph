//! Search bar component for session full-text search

use leptos::ev::SubmitEvent;
use leptos::prelude::*;

#[component]
pub fn SearchBar(
    /// Current search query
    query: RwSignal<String>,
    /// Callback when search is submitted
    on_search: impl Fn(String) + Send + Sync + 'static + Copy,
) -> impl IntoView {
    let input_ref = NodeRef::<leptos::html::Input>::new();

    let handle_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let q = query.get().trim().to_string();
        if !q.is_empty() {
            on_search(q);
        }
    };

    let handle_clear = move |_| {
        query.set(String::new());
        on_search(String::new());
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    };

    view! {
        <form
            on:submit=handle_submit
            class="flex items-center gap-2 p-4 bg-surface-raised border-b border-border"
        >
            <div class="flex-1 relative">
                <input
                    type="text"
                    node_ref=input_ref
                    prop:value=move || query.get()
                    on:input=move |ev| {
                        query.set(event_target_value(&ev));
                    }
                    placeholder="Search session transcripts..."
                    class="w-full px-4 py-2 pr-10 bg-surface-base border border-border rounded-lg text-text-primary placeholder-text-tertiary focus:outline-none focus:ring-2 focus:ring-primary focus:border-transparent"
                />
                <Show when=move || !query.get().is_empty()>
                    <button
                        type="button"
                        on:click=handle_clear
                        class="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-text-tertiary hover:text-text-primary transition-colors"
                        aria-label="Clear search"
                    >
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                    </button>
                </Show>
            </div>

            <button
                type="submit"
                prop:disabled=move || query.get().trim().is_empty()
                class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors font-medium"
            >
                "Search"
            </button>
        </form>
    }
}

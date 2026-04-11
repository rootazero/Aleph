use crate::canvas_engine::types::BreadcrumbEntry;
use leptos::prelude::*;

#[component]
pub fn Breadcrumb(
    entries: ReadSignal<Vec<BreadcrumbEntry>>,
    on_navigate: impl Fn(Option<String>) + 'static + Copy + Send,
) -> impl IntoView {
    move || {
        let items = entries.get();
        if items.is_empty() {
            return None;
        }

        Some(view! {
            <div class="flex items-center gap-1 px-4 py-1.5 bg-surface-sunken border-b border-border text-xs text-text-tertiary">
                // Global root
                <button
                    on:click=move |_| on_navigate(None)
                    class="hover:text-text-primary transition-colors cursor-pointer"
                >
                    "Global"
                </button>

                {items.into_iter().map(|entry| {
                    let node_id = entry.node_id.clone();
                    let node_name = entry.node_name.clone();
                    view! {
                        <span class="text-text-tertiary/50">" / "</span>
                        <button
                            on:click=move |_| on_navigate(Some(node_id.clone()))
                            class="hover:text-text-primary transition-colors cursor-pointer truncate max-w-[120px]"
                        >
                            {node_name}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
    }
}

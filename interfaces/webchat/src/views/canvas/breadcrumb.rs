use crate::canvas_engine::types::BreadcrumbEntry;
use leptos::prelude::*;

// Maximum items to show before collapsing the middle with an ellipsis.
const DISPLAY_MAX: usize = 8;
// Number of tail items to keep when collapsing.
const TAIL_KEEP: usize = 6;

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

        // Build display list — collapse middle entries when list is long.
        // Each display item: (Option<node_id>, display_name, is_ellipsis)
        let display: Vec<(Option<String>, String, bool)> = if items.len() > DISPLAY_MAX {
            let head = items.first().unwrap();
            let tail = &items[items.len() - TAIL_KEEP..];
            let mut out: Vec<(Option<String>, String, bool)> = vec![
                (Some(head.node_id.clone()), head.node_name.clone(), false),
                (None, "…".to_string(), true),
            ];
            out.extend(tail.iter().map(|e| (Some(e.node_id.clone()), e.node_name.clone(), false)));
            out
        } else {
            items
                .iter()
                .map(|e| (Some(e.node_id.clone()), e.node_name.clone(), false))
                .collect()
        };

        let total = display.len();

        Some(view! {
            <nav
                class="flex items-center gap-1 px-4 py-1.5 bg-surface-sunken border-b border-border text-xs text-text-tertiary"
                aria-label="Navigation history"
            >
                // Global root — always navigates back to the global view (None target).
                <button
                    on:click=move |_| on_navigate(None)
                    class="hover:text-text-primary transition-colors cursor-pointer"
                >
                    "Global"
                </button>

                {display.into_iter().enumerate().map(|(idx, (node_id_opt, name, is_ellipsis))| {
                    let is_last = idx == total - 1;
                    view! {
                        <span class="text-text-tertiary/50">" / "</span>
                        {if is_ellipsis {
                            view! {
                                <span class="text-text-tertiary/40 select-none">{name}</span>
                            }.into_any()
                        } else if is_last {
                            view! {
                                <span class="text-text-primary font-medium truncate max-w-[120px]">
                                    {name}
                                </span>
                            }.into_any()
                        } else {
                            let id = node_id_opt.unwrap_or_default();
                            view! {
                                <button
                                    on:click=move |_| on_navigate(Some(id.clone()))
                                    class="hover:text-text-primary transition-colors cursor-pointer truncate max-w-[120px]"
                                >
                                    {name}
                                </button>
                            }.into_any()
                        }}
                    }
                }).collect::<Vec<_>>()}
            </nav>
        })
    }
}

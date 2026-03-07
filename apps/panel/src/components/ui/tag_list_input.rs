//! Chip-based tag list editor component
//!
//! Provides a tag input with add/remove functionality, duplicate prevention,
//! and Enter key support. Tags render as colored chips with remove buttons.

use leptos::prelude::*;

/// A chip-based tag list editor with add/remove functionality.
///
/// Displays existing tags as colored chips with an X button to remove each.
/// Includes a text input + "Add" button for adding new tags.
/// Pressing Enter in the input also adds the tag.
/// Prevents duplicate tags (case-sensitive).
///
/// # Example
/// ```rust
/// let tags = RwSignal::new(vec!["tag1".to_string(), "tag2".to_string()]);
/// view! {
///     <TagListInput
///         tags=tags.into()
///         on_change=move |new_tags| tags.set(new_tags)
///         placeholder="Enter a tag..."
///         hint="Press Enter or click Add to add tags"
///     />
/// }
/// ```
#[component]
pub fn TagListInput(
    /// Current list of tags
    tags: Signal<Vec<String>>,
    /// Called with the full new tag list on any change
    on_change: impl Fn(Vec<String>) + Send + 'static + Copy,
    /// Placeholder text for the input field
    #[prop(optional)]
    placeholder: Option<&'static str>,
    /// Hint text displayed below the component
    #[prop(optional)]
    hint: Option<&'static str>,
) -> impl IntoView {
    let input_value = RwSignal::new(String::new());

    let add_tag = move || {
        let trimmed = input_value.get().trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let current = tags.get();
        if current.contains(&trimmed) {
            return;
        }
        let mut new_tags = current;
        new_tags.push(trimmed);
        on_change(new_tags);
        input_value.set(String::new());
    };

    let remove_tag = move |index: usize| {
        let mut current = tags.get();
        if index < current.len() {
            current.remove(index);
            on_change(current);
        }
    };

    view! {
        <div class="space-y-2">
            // Tag chips area
            <div class="flex flex-wrap gap-2 min-h-[32px]">
                {move || {
                    let current_tags = tags.get();
                    if current_tags.is_empty() {
                        view! {
                            <span class="text-sm text-text-tertiary italic">"No items added"</span>
                        }.into_any()
                    } else {
                        current_tags
                            .into_iter()
                            .enumerate()
                            .map(|(i, tag)| {
                                view! {
                                    <span class="inline-flex items-center gap-1 px-2 py-1 bg-primary-subtle text-primary text-xs rounded-md border border-primary/20">
                                        {tag}
                                        <button
                                            type="button"
                                            on:click=move |_| remove_tag(i)
                                            class="text-primary hover:text-danger transition-colors cursor-pointer"
                                            aria-label="Remove tag"
                                        >
                                            <svg
                                                xmlns="http://www.w3.org/2000/svg"
                                                width="14"
                                                height="14"
                                                viewBox="0 0 24 24"
                                                fill="none"
                                                stroke="currentColor"
                                                stroke-width="2"
                                                stroke-linecap="round"
                                                stroke-linejoin="round"
                                            >
                                                <line x1="18" y1="6" x2="6" y2="18" />
                                                <line x1="6" y1="6" x2="18" y2="18" />
                                            </svg>
                                        </button>
                                    </span>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>

            // Input row: text input + Add button
            <div class="flex gap-2">
                <input
                    type="text"
                    prop:value=move || input_value.get()
                    on:input=move |ev| input_value.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            add_tag();
                        }
                    }
                    placeholder=placeholder.unwrap_or("")
                    class="flex-1 px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                />
                <button
                    type="button"
                    on:click=move |_| add_tag()
                    class="px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-raised text-sm transition-colors cursor-pointer"
                >
                    "Add"
                </button>
            </div>

            // Optional hint text
            {hint.map(|h| view! {
                <p class="text-xs text-text-tertiary">{h}</p>
            })}
        </div>
    }
}

use crate::canvas_engine::adapter::NoteDetailResponse;
use crate::canvas_engine::types::NOTE_COLOR;
use leptos::prelude::*;

#[component]
pub fn DetailPanel(detail: ReadSignal<Option<NoteDetailResponse>>) -> impl IntoView {
    move || {
        let resp = detail.get()?;

        let node = resp.node;
        let content = resp.content;
        let backlinks = resp.backlinks;

        let color_css = NOTE_COLOR.to_css();

        // Render markdown content to HTML
        let content_html = if !content.is_empty() {
            let parser = pulldown_cmark::Parser::new(&content);
            let mut html_output = String::new();
            pulldown_cmark::html::push_html(&mut html_output, parser);
            Some(html_output)
        } else {
            None
        };

        Some(
            view! {
                <div class="w-80 bg-surface-raised border-l border-border overflow-y-auto flex-shrink-0">
                    // Header
                    <div class="p-4 border-b border-border">
                        <div class="flex items-center gap-2 mb-2">
                            <h3 class="text-lg font-semibold text-text-primary truncate">
                                {node.name.clone()}
                            </h3>
                        </div>
                        <div class="flex items-center gap-2 text-xs text-text-tertiary">
                            <span
                                class="px-2 py-0.5 rounded-full text-white text-[10px] font-medium"
                                style:background-color=color_css.clone()
                            >
                                {node.category.clone()}
                            </span>
                            <span>"Links: " {node.link_count.to_string()}</span>
                        </div>
                        {(!node.tags.is_empty()).then(|| {
                            let tags_str = node.tags.join(", ");
                            view! {
                                <div class="mt-2 text-xs text-text-tertiary">
                                    <span class="font-medium">"Tags: "</span>
                                    {tags_str}
                                </div>
                            }
                        })}
                    </div>

                    // Content section
                    {content_html.map(|html| {
                        view! {
                            <div class="p-4 border-b border-border">
                                <h4 class="text-sm font-semibold text-text-secondary mb-2">"Content"</h4>
                                <div
                                    class="prose prose-sm prose-invert max-w-none text-text-secondary
                                           [&_h1]:text-base [&_h2]:text-sm [&_h3]:text-sm
                                           [&_p]:text-xs [&_li]:text-xs [&_a]:text-primary"
                                    inner_html=html
                                />
                            </div>
                        }
                    })}

                    // Backlinks section
                    {(!backlinks.is_empty()).then(|| {
                        let backlinks_clone = backlinks.clone();
                        view! {
                            <div class="p-4">
                                <h4 class="text-sm font-semibold text-text-secondary mb-2">
                                    "Backlinks (" {backlinks_clone.len().to_string()} ")"
                                </h4>
                                <div class="space-y-1">
                                    {backlinks_clone.into_iter().map(|bl| {
                                        view! {
                                            <div class="p-2 bg-surface-sunken rounded-lg text-xs text-text-secondary">
                                                {bl}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        }
                    })}
                </div>
            }
            .into_any(),
        )
    }
}

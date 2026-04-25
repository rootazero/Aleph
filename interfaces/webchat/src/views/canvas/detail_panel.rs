use crate::canvas_engine::adapter::NoteDetailResponse;
use crate::canvas_engine::types::NOTE_COLOR;
use leptos::prelude::*;

/// Content state for the detail panel.
#[derive(Debug, Clone)]
pub enum DetailContent {
    /// Panel is hidden.
    Closed,
    /// A single graph node is selected — show its wiki content and backlinks.
    Node { detail: NoteDetailResponse },
    /// A cluster is selected — show member list with jump buttons.
    Cluster {
        kind: String,
        members: Vec<(String, String)>, // (id, name)
    },
}

#[component]
pub fn DetailPanel(
    content: ReadSignal<DetailContent>,
    on_jump_to: Callback<String>,
) -> impl IntoView {
    move || match content.get() {
        DetailContent::Closed => None,

        DetailContent::Node { detail } => {
            let node = detail.node;
            let wiki_content = detail.content;
            let backlinks = detail.backlinks;

            let color_css = NOTE_COLOR.to_css();

            // Render markdown content to HTML
            let content_html = if !wiki_content.is_empty() {
                let parser = pulldown_cmark::Parser::new(&wiki_content);
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

        DetailContent::Cluster { kind, members } => {
            let count = members.len();
            Some(
                view! {
                    <div class="w-80 bg-surface-raised border-l border-border overflow-y-auto flex-shrink-0">
                        <div class="p-4 border-b border-border">
                            <h3 class="text-lg font-semibold text-text-primary">
                                {format!("{} 群组（共 {} 个）", kind, count)}
                            </h3>
                        </div>
                        <ul class="p-4 space-y-1">
                            {members.into_iter().map(|(id, name)| {
                                let id_clone = id.clone();
                                let cb = on_jump_to.clone();
                                view! {
                                    <li class="flex items-center justify-between p-2 bg-surface-sunken rounded-lg">
                                        <span class="text-xs text-text-secondary truncate">{name}</span>
                                        <button
                                            class="ml-2 text-xs text-primary hover:underline flex-shrink-0"
                                            on:click=move |_| cb.run(id_clone.clone())
                                        >
                                            "→ 跳转"
                                        </button>
                                    </li>
                                }
                            }).collect::<Vec<_>>()}
                        </ul>
                    </div>
                }
                .into_any(),
            )
        }
    }
}

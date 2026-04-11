use crate::canvas_engine::adapter::{FactDto, NodeDetailResponse};
use crate::canvas_engine::types::{kind_color, kind_icon};
use leptos::prelude::*;

#[component]
pub fn DetailPanel(detail: ReadSignal<Option<NodeDetailResponse>>) -> impl IntoView {
    move || {
        let resp = detail.get()?;

        let node = resp.node;
        let wiki = resp.wiki;
        let facts = resp.facts;

        let color = kind_color(&node.kind);
        let color_css = color.to_css();

        // Render wiki markdown to HTML
        let wiki_html = wiki.map(|w| {
            let parser = pulldown_cmark::Parser::new(&w.content);
            let mut html_output = String::new();
            pulldown_cmark::html::push_html(&mut html_output, parser);
            html_output
        });

        Some(
            view! {
                <div class="w-80 bg-surface-raised border-l border-border overflow-y-auto flex-shrink-0">
                    // Header
                    <div class="p-4 border-b border-border">
                        <div class="flex items-center gap-2 mb-2">
                            <span class="text-lg">{kind_icon(&node.kind).to_string()}</span>
                            <h3 class="text-lg font-semibold text-text-primary truncate">
                                {node.name.clone()}
                            </h3>
                        </div>
                        <div class="flex items-center gap-2 text-xs text-text-tertiary">
                            <span
                                class="px-2 py-0.5 rounded-full text-white text-[10px] font-medium"
                                style:background-color=color_css.clone()
                            >
                                {node.kind.clone()}
                            </span>
                            <span>"Decay: " {format!("{:.2}", node.decay_score)}</span>
                            <span>"Edges: " {node.edge_count.to_string()}</span>
                        </div>
                        {(!node.aliases.is_empty()).then(|| {
                            let aliases_str = node.aliases.join(", ");
                            view! {
                                <div class="mt-2 text-xs text-text-tertiary">
                                    <span class="font-medium">"Aliases: "</span>
                                    {aliases_str}
                                </div>
                            }
                        })}
                    </div>

                    // Wiki section
                    {wiki_html.map(|html| {
                        view! {
                            <div class="p-4 border-b border-border">
                                <h4 class="text-sm font-semibold text-text-secondary mb-2">"Wiki"</h4>
                                <div
                                    class="prose prose-sm prose-invert max-w-none text-text-secondary
                                           [&_h1]:text-base [&_h2]:text-sm [&_h3]:text-sm
                                           [&_p]:text-xs [&_li]:text-xs [&_a]:text-primary"
                                    inner_html=html
                                />
                            </div>
                        }
                    })}

                    // Facts section
                    {(!facts.is_empty()).then(|| {
                        let facts_clone = facts.clone();
                        view! {
                            <div class="p-4">
                                <h4 class="text-sm font-semibold text-text-secondary mb-2">
                                    "Facts (" {facts_clone.len().to_string()} ")"
                                </h4>
                                <div class="space-y-2">
                                    {facts_clone.into_iter().map(|fact| {
                                        view! { <FactItem fact=fact /> }
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

#[component]
fn FactItem(fact: FactDto) -> impl IntoView {
    let confidence_pct = (fact.confidence * 100.0) as u8;
    let confidence_color = if fact.confidence >= 0.8 {
        "text-green-400"
    } else if fact.confidence >= 0.5 {
        "text-yellow-400"
    } else {
        "text-red-400"
    };

    view! {
        <div class="p-2 bg-surface-sunken rounded-lg text-xs">
            <div class="flex items-center justify-between mb-1">
                <span class="px-1.5 py-0.5 bg-surface rounded text-[10px] text-text-tertiary font-medium">
                    {fact.fact_type}
                </span>
                <span class={format!("text-[10px] font-medium {}", confidence_color)}>
                    {format!("{}%", confidence_pct)}
                </span>
            </div>
            <p class="text-text-secondary leading-relaxed">{fact.content}</p>
        </div>
    }
}

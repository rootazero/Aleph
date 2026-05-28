use leptos::prelude::*;
use std::collections::HashMap;

use crate::canvas_engine::category_color::category_color;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::i18n::*;
use crate::state::memory::MemoryState;

/// Pre-fetched body excerpt for a single node.
#[derive(Clone)]
pub struct NodeExcerpt {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body_markdown: String,
    pub breadcrumb: Vec<String>,
}

/// Sidebar variant of the node detail view. 240 px content width — the
/// shell sidebar (`w-64`) gives us 256 px and the surrounding padding
/// claims the rest. When no node is selected, falls back to a
/// "recently visited" list.
#[component]
pub fn NodeDetailPanel(
    /// Pre-fetched body excerpts keyed by node id.
    excerpts: RwSignal<HashMap<String, NodeExcerpt>>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();

    view! {
        <div class="flex-1 min-h-0 overflow-y-auto px-3 py-2">
            {move || {
                let selected = mem.selected_node.get();
                if let Some(id) = selected {
                    if let Some(ex) = excerpts.with(|m| m.get(&id).cloned()) {
                        view! { <DetailFor excerpt=ex /> }.into_any()
                    } else {
                        view! { <DetailLoading id=id /> }.into_any()
                    }
                } else {
                    view! { <RecentVisitedList /> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn DetailFor(excerpt: NodeExcerpt) -> impl IntoView {
    let stripe = category_color(&excerpt.category);
    let body_html = render_excerpt(&excerpt.body_markdown);
    let breadcrumb = excerpt.breadcrumb.clone();

    view! {
        <div>
            {(!breadcrumb.is_empty()).then(|| view! {
                <div style="font-size:10px;color:var(--text-meta);margin-bottom:6px">
                    {breadcrumb.join(" › ")}
                </div>
            })}
            <div style=format!("height:3px;background:{};border-radius:2px;margin-bottom:8px", stripe)></div>
            <h3 style="color:var(--text-title);font-size:14px;font-weight:600;line-height:1.3;margin:0 0 6px">
                {excerpt.name.clone()}
            </h3>
            <div style="color:var(--text-body);font-size:12px;line-height:1.55" inner_html=body_html></div>
            {(!excerpt.tags.is_empty()).then(|| {
                let t = excerpt.tags.clone();
                view! {
                    <div style="margin-top:10px;display:flex;flex-wrap:wrap;gap:4px">
                        {t.into_iter().map(|tag| view! {
                            <span style="font-size:10px;color:var(--cat-feedback);background:rgba(167,139,250,0.13);padding:1px 6px;border-radius:3px">
                                "#"{tag}
                            </span>
                        }).collect_view()}
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn DetailLoading(id: String) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div style="color:var(--text-meta);font-size:11px;font-style:italic">
            {t!(i18n, memory.loading_node)} " " {id} " …"
        </div>
    }
}

#[component]
fn RecentVisitedList() -> impl IntoView {
    let i18n = use_i18n();
    let mem = expect_context::<MemoryState>();

    view! {
        <div>
            <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:6px">
                {t!(i18n, memory.recently_visited)}
            </div>
            {move || {
                mem.recent_visited.with(|q| {
                    let top: Vec<String> = q.iter().take(5).cloned().collect();
                    if top.is_empty() {
                        view! {
                            <p style="color:var(--text-meta);font-size:11px;font-style:italic">
                                {t!(i18n, memory.click_node_hint)}
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:4px">
                                {top.into_iter().map(|id| {
                                    let id_for_click = id.clone();
                                    view! {
                                        <li
                                            style="font-size:11.5px;color:var(--text-body);padding:6px 8px;border-radius:5px;background:rgba(255,255,255,0.02);cursor:pointer"
                                            on:click=move |_| {
                                                mem.selected_node.set(Some(id_for_click.clone()));
                                            }
                                        >
                                            {id}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }
                })
            }}
        </div>
    }
}

//! Phone note detail (`/memory/note`): read-only full markdown + backlinks for
//! the note selected in the list. Fetches via `graph.node_detail` (R4). If no
//! note is selected (refresh on this route), redirects to `/memory`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::graph::GraphApi;
use crate::canvas_engine::category_color::category_color;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;

use super::PhoneMemoryState;

#[component]
#[must_use]
pub fn PhoneMemoryDetail() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let st = expect_context::<PhoneMemoryState>();
    let navigate = use_navigate();

    let body = RwSignal::new(None::<String>);
    let backlinks = RwSignal::new(Vec::<String>::new());
    let error = RwSignal::new(None::<String>);

    // Redirect when there is no selected note (deep-link / refresh on this route).
    {
        let navigate = navigate.clone();
        Effect::new(move || {
            if st.selected.get().is_none() {
                navigate("/memory", NavigateOptions::default());
            }
        });
    }

    // Fetch full markdown + backlinks once connected, for the selected note.
    Effect::new(move || {
        let Some(fact) = st.selected.get() else {
            return;
        };
        if !dashboard.is_connected.get() {
            return;
        }
        let agent = mem.agent_id.get_untracked();
        spawn_local(async move {
            match GraphApi::node_detail(&dashboard, &agent, &fact.path).await {
                Ok(d) => {
                    body.set(Some(d.content));
                    backlinks.set(d.backlinks);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    move || {
        let Some(fact) = st.selected.get() else {
            // The redirect Effect is navigating away; render an empty shell.
            return view! { <PhoneShell title="Note" back="/memory"><div></div></PhoneShell> }
                .into_any();
        };
        let stripe = category_color(&fact.category);
        let title = fact.content.clone();
        let path = fact.path.clone();
        view! {
            <PhoneShell title="Note" back="/memory">
            <div>
                <div style=format!("height:3px;background:{stripe};border-radius:2px;margin-bottom:10px")></div>
                <h3 style="font-size:16px; font-weight:600; color:var(--color-text-primary); margin:0 0 6px; word-break:break-word;">{title}</h3>
                <div class="mono" style="font-size:12px; color:var(--color-text-tertiary); margin-bottom:14px; word-break:break-all;">{path}</div>

                {move || match body.get() {
                    Some(md) => view! {
                        <div class="node-card-full__excerpt" style="font-size:14px; line-height:1.6; color:var(--color-text-secondary);" inner_html=render_excerpt(&md)></div>
                    }.into_any(),
                    None => view! {
                        <div style="font-size:13px; font-style:italic; color:var(--color-text-tertiary);">"Loading…"</div>
                    }.into_any(),
                }}

                {move || error.get().map(|e| view! {
                    <div style="color:var(--cat-error,#f44336); font-size:13px; margin-top:8px;">{e}</div>
                })}

                {move || {
                    let bl = backlinks.get();
                    (!bl.is_empty()).then(|| view! {
                        <div style="margin-top:18px;">
                            <div style="font-size:10px; text-transform:uppercase; letter-spacing:0.12em; color:var(--color-text-tertiary); margin-bottom:6px;">"Backlinks"</div>
                            <div class="list">
                                {bl.into_iter().map(|b| view! {
                                    <div class="cell"><div class="cell-body"><div class="cell-sub mono" style="word-break:break-all;">{b}</div></div></div>
                                }).collect_view()}
                            </div>
                        </div>
                    })
                }}
            </div>
            </PhoneShell>
        }.into_any()
    }
}

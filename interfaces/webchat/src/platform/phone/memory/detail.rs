//! Phone note detail (`/memory/note`): read-only full markdown + backlinks for
//! the note selected in the list. Fetches via `graph.node_detail` (R4). If no
//! note is selected (refresh on this route), redirects to `/memory`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::graph::GraphApi;
use crate::api::CompressedFact;
use crate::memory_graph::category_color::category_color;
use crate::memory_graph::markdown_excerpt::{render_excerpt, wikilink_click_target};
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;

use super::PhoneMemoryState;

#[component]
#[must_use]
pub fn PhoneMemoryDetail() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
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
                navigate("/memory/list", NavigateOptions::default());
            }
        });
    }

    // Fetch full markdown + backlinks once connected, for the selected note.
    Effect::new(move || {
        let Some(fact) = st.selected.get() else {
            return;
        };
        // Reset to the fresh-mount state on every selection change: same-screen
        // wikilink/backlink navigation swaps `st.selected` without a remount, so
        // the previous note's body/backlinks — or a stale error from an earlier
        // failed load — must not linger under the new title while the fetch is
        // in flight.
        body.set(None);
        backlinks.set(Vec::new());
        error.set(None);
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
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    });

    move || {
        let Some(fact) = st.selected.get() else {
            // The redirect Effect is navigating away; render an empty shell.
            return view! { <PhoneShell title="Note" back="/memory/list" back_label="List"><div></div></PhoneShell> }
                .into_any();
        };
        let stripe = category_color(&fact.category);
        let title = fact.content.clone();
        let path = fact.path.clone();
        view! {
            <PhoneShell title="Note" back="/memory/list" back_label="List">
            <div>
                <div style=format!("height:3px;background:{stripe};border-radius:2px;margin-bottom:10px")></div>
                <h3 style="font-size:16px; font-weight:600; color:var(--color-text-primary); margin:0 0 6px; word-break:break-word;">{title}</h3>
                <div class="mono" style="font-size:12px; color:var(--color-text-tertiary); margin-bottom:14px; word-break:break-all;">{path}</div>

                {move || match body.get() {
                    Some(md) => view! {
                        <div
                            class="node-card-full__excerpt"
                            style="font-size:14px; line-height:1.6; color:var(--color-text-secondary);"
                            inner_html=render_excerpt(&md)
                            on:click=move |ev| {
                                if let Some(t) = wikilink_click_target(&ev) {
                                    navigate_phone(&dashboard, &mem, st, t);
                                }
                            }
                        ></div>
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
                                {bl.into_iter().map(|b| {
                                    let b_click = b.clone();
                                    view! {
                                        <div
                                            class="cell"
                                            style="cursor:pointer;"
                                            on:click=move |_| navigate_phone(&dashboard, &mem, st, b_click.clone())
                                        >
                                            <div class="cell-body"><div class="cell-sub mono" style="word-break:break-all;">{b}</div></div>
                                        </div>
                                    }
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

/// Resolve a clicked wikilink target to a node id and load it into the
/// detail screen. Path-form targets navigate directly; bare names resolve
/// via graph.search (first hit — mirrors `navigate_wl` in the canvas detail
/// panel). Setting `st.selected` re-triggers the fetch Effect above, which
/// reloads the body + backlinks for the new note without leaving the route.
fn navigate_phone(
    dashboard: &DashboardState,
    mem: &MemoryState,
    st: PhoneMemoryState,
    target: String,
) {
    let dashboard = *dashboard;
    let mem = *mem;
    spawn_local(async move {
        let id = if target.contains('/') {
            Some(target)
        } else {
            let agent = mem.agent_id.get_untracked();
            GraphApi::search(&dashboard, &agent, &target, 1)
                .await
                .ok()
                .and_then(|r| r.results.first().map(|f| f.id.clone()))
        };
        if let Some(id) = id {
            st.selected.set(Some(CompressedFact::stub_from_path(&id)));
        }
    });
}

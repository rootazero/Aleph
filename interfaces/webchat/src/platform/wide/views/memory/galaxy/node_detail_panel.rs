use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::graph::GraphApi;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::memory_graph::adapter::OutgoingLinkDto;
use crate::memory_graph::category_color::category_color;
use crate::memory_graph::markdown_excerpt::render_excerpt;
use crate::state::memory::{MemoryState, MemoryView};

/// Pre-fetched body excerpt for a single node.
#[derive(Clone)]
pub struct NodeExcerpt {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body_markdown: String,
    pub breadcrumb: Vec<String>,
    pub backlinks: Vec<String>,
    pub outgoing: Vec<OutgoingLinkDto>,
}

/// Turn a note path like `a/b/c.md` into breadcrumb dir segments `["a","b"]`.
/// Drops the final filename component, empty segments, and `.` segments.
fn breadcrumb_from_path(path: &str) -> Vec<String> {
    let mut segs: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    segs.pop(); // drop the filename (last component)
    segs.into_iter().map(str::to_owned).collect()
}

/// Sidebar variant of the node detail view. 240 px content width — the
/// shell sidebar (`w-64`) gives us 256 px and the surrounding padding
/// claims the rest. When no node is selected, falls back to a
/// "recently visited" list.
///
/// Owns a fetch Effect: when the selected node changes it loads the raw
/// markdown via `graph.node_detail` and caches a [`NodeExcerpt`] in
/// `excerpts`. The detail body can be edited inline and persisted via
/// `graph.update_note` (see [`DetailFor`]). Pure I/O — all persistence is
/// JSON-RPC to the server (R4).
#[component]
pub fn NodeDetailPanel(
    /// Body-excerpt cache keyed by node id. Populated by this component's
    /// fetch Effect and updated optimistically on save.
    excerpts: RwSignal<HashMap<String, NodeExcerpt>>,
    /// Galaxy-invalidation pulse: bumped after every successful note mutation
    /// so the canvas host re-fetches the graph. Without it a deleted note's
    /// star (and its edges) keeps rendering, and a renamed note's new id is
    /// absent from the galaxy index — both silent, until the next agent switch.
    graph_nonce: RwSignal<u32>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let state = expect_context::<DashboardState>();

    // Fetch detail (raw markdown) when the selection changes; skip if already
    // cached. Mirrors the existing `node_detail` RPC shape used by the canvas.
    Effect::new(move |_| {
        let Some(id) = mem.selected_node.get() else {
            return;
        };
        if excerpts.with(|m| m.contains_key(&id)) {
            return;
        }
        let agent = mem.agent_id.get_untracked();
        spawn_local(async move {
            match GraphApi::node_detail(&state, &agent, &id).await {
                Ok(detail) => {
                    let ex = NodeExcerpt {
                        id: detail.node.id.clone(),
                        name: detail.node.name,
                        category: detail.node.category,
                        tags: detail.node.tags,
                        body_markdown: detail.content,
                        breadcrumb: breadcrumb_from_path(&detail.node.path),
                        backlinks: detail.backlinks,
                        outgoing: detail.outgoing,
                    };
                    excerpts.update(|m| {
                        m.insert(id, ex);
                    });
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("NodeDetailPanel: node_detail fetch failed for {id}: {e}").into(),
                    );
                }
            }
        });
    });

    view! {
        <div class="flex-1 min-h-0 overflow-y-auto px-3 py-2">
            {move || {
                let selected = mem.selected_node.get();
                if let Some(id) = selected {
                    if let Some(ex) = excerpts.with(|m| m.get(&id).cloned()) {
                        view! { <DetailFor excerpt=ex excerpts=excerpts graph_nonce=graph_nonce /> }
                            .into_any()
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
fn DetailFor(
    excerpt: NodeExcerpt,
    /// Shared cache — updated optimistically when an edit is saved so the
    /// read view re-renders with the new content (no extra round-trip).
    excerpts: RwSignal<HashMap<String, NodeExcerpt>>,
    /// Bumped in every mutation's Ok arm — the galaxy is rebuilt from the
    /// server's new topology (link edges change on every body edit, and a
    /// delete removes a star outright).
    graph_nonce: RwSignal<u32>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let stripe = category_color(&excerpt.category);
    let body_html = render_excerpt(&excerpt.body_markdown);
    let breadcrumb = excerpt.breadcrumb.clone();
    let tags = excerpt.tags.clone();
    let backlinks = excerpt.backlinks.clone();
    let outgoing = excerpt.outgoing.clone();
    let node_id = excerpt.id.clone();
    let node_id_for_list = excerpt.id.clone();
    let title = excerpt.name.clone();
    let body_markdown = excerpt.body_markdown;

    // Edit state — local to this node's detail card.
    let is_editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let is_saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    // Rename state
    let is_renaming = RwSignal::new(false);
    let rename_draft = RwSignal::new(title.clone());

    // Delete state
    let confirm_delete = RwSignal::new(false);

    let start_edit = {
        let body_markdown = body_markdown;
        move |_| {
            draft.set(body_markdown.clone());
            error.set(None);
            confirm_delete.set(false);
            is_renaming.set(false);
            is_editing.set(true);
        }
    };

    let cancel_edit = move |_| {
        is_editing.set(false);
        error.set(None);
    };

    let save_edit = {
        let node_id = node_id.clone();
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let content = draft.get_untracked();
            let id = node_id.clone();
            let agent = mem.agent_id.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::update_note(&state, &agent, &id, &content).await {
                    Ok(()) => {
                        // Optimistic: write_note_raw persists verbatim, so the
                        // cached body equals what the server now holds. Updating
                        // the cache remounts DetailFor in read mode with the new
                        // content — no second fetch needed.
                        excerpts.update(|m| {
                            if let Some(ex) = m.get_mut(&id) {
                                ex.body_markdown = content;
                            }
                        });
                        // Body edits change wikilinks — rebuild the galaxy.
                        graph_nonce.update(|n| *n += 1);
                        is_saving.set(false);
                        is_editing.set(false);
                    }
                    Err(e) => {
                        is_saving.set(false);
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                e.to_string()
                            }),
                        ));
                    }
                }
            });
        }
    };

    let do_rename = {
        let node_id = node_id.clone();
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let new_title = rename_draft.get_untracked();
            if new_title.is_empty() {
                error.set(Some("Title cannot be empty".to_string()));
                return;
            }
            let id = node_id.clone();
            let agent = mem.agent_id.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::rename_note(&state, &agent, &id, &new_title).await {
                    Ok(new_id) => {
                        // Clear old excerpt cache, update selected node to new_id
                        excerpts.update(|m| {
                            m.remove(&id);
                        });
                        // The old id is gone from the store — rebuild the galaxy
                        // so the new id is in its index and fly-to can find it.
                        graph_nonce.update(|n| *n += 1);
                        mem.selected_node.set(Some(new_id));
                        is_saving.set(false);
                        is_renaming.set(false);
                    }
                    Err(e) => {
                        is_saving.set(false);
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                e.to_string()
                            }),
                        ));
                    }
                }
            });
        }
    };

    let do_delete = {
        let node_id = node_id;
        move |_| {
            if is_saving.get_untracked() || !confirm_delete.get_untracked() {
                // First tap: arm the confirm
                confirm_delete.set(true);
                return;
            }
            // Second tap: execute delete
            let id = node_id.clone();
            let agent = mem.agent_id.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::delete_note(&state, &agent, &id).await {
                    Ok(()) => {
                        // Purge the stale cache entry so re-selecting this id
                        // (recent-visited, backlinks) can't show deleted content.
                        excerpts.update(|m| {
                            m.remove(&id);
                        });
                        // The star and its edges must stop rendering.
                        graph_nonce.update(|n| *n += 1);
                        mem.selected_node.set(None);
                        is_saving.set(false);
                    }
                    Err(e) => {
                        is_saving.set(false);
                        confirm_delete.set(false); // reset on error
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                e.to_string()
                            }),
                        ));
                    }
                }
            });
        }
    };

    view! {
        <div>
            {(!breadcrumb.is_empty()).then(|| view! {
                <div style="font-size:10px;color:var(--text-meta);margin-bottom:6px">
                    {breadcrumb.join(" › ")}
                </div>
            })}
            <div style=format!("height:3px;background:{};border-radius:2px;margin-bottom:8px", stripe)></div>
            <h3 style="color:var(--text-title);font-size:14px;font-weight:600;line-height:1.3;margin:0 0 6px">
                {title}
            </h3>
            <button
                class="node-detail-btn"
                style="margin:0 0 8px"
                on:click=move |_| {
                    mem.highlight_note_id.set(Some(node_id_for_list.clone()));
                    mem.memory_view.set(MemoryView::Table);
                }
            >
                {t!(i18n, memory.view_in_list)}
            </button>
            {move || if is_editing.get() {
                let save = save_edit.clone();
                let cancel = cancel_edit;
                view! {
                    <div>
                        <textarea
                            class="node-detail-editor"
                            prop:value=move || draft.get()
                            on:input=move |ev| draft.set(event_target_value(&ev))
                        ></textarea>
                        {move || error.get().map(|e| view! {
                            <div style="color:var(--cat-error,#f44336);font-size:11px;margin-top:4px">{e}</div>
                        })}
                        <div style="display:flex;gap:6px;margin-top:6px">
                            <button
                                class="node-detail-btn node-detail-btn--save"
                                prop:disabled=move || is_saving.get()
                                on:click=save
                            >
                                {move || if is_saving.get() {
                                    view! { {t!(i18n, memory.saving)} }.into_any()
                                } else {
                                    view! { {t!(i18n, memory.save)} }.into_any()
                                }}
                            </button>
                            <button
                                class="node-detail-btn"
                                prop:disabled=move || is_saving.get()
                                on:click=cancel
                            >
                                {t!(i18n, memory.cancel)}
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
                let html = body_html.clone();
                let edit = start_edit.clone();
                let rename = do_rename.clone();
                let delete = do_delete.clone();
                view! {
                    <div>
                        {move || if is_renaming.get() {
                            let rename = rename.clone();
                            view! {
                                <div>
                                    <input
                                        type="text"
                                        style="width:100%;padding:6px;border:1px solid var(--border-subtle);border-radius:4px;background:var(--surface-sunken);color:var(--text-primary);font-size:12px;box-sizing:border-box"
                                        prop:value=move || rename_draft.get()
                                        on:input=move |ev| rename_draft.set(event_target_value(&ev))
                                    />
                                    <div style="display:flex;gap:6px;margin-top:6px">
                                        <button
                                            class="node-detail-btn node-detail-btn--save"
                                            prop:disabled=move || is_saving.get()
                                            on:click=rename
                                        >
                                            {move || if is_saving.get() {
                                                view! { {t!(i18n, memory.saving)} }.into_any()
                                            } else {
                                                view! { "Confirm" }.into_any()
                                            }}
                                        </button>
                                        <button
                                            class="node-detail-btn"
                                            prop:disabled=move || is_saving.get()
                                            on:click=move |_| is_renaming.set(false)
                                        >
                                            {t!(i18n, memory.cancel)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            let html = html.clone();
                            let edit = edit.clone();
                            let delete = delete.clone();
                            view! {
                                <div>
                                    <div
                                        class="node-card-full__excerpt"
                                        style="color:var(--text-body);font-size:12px;line-height:1.55"
                                        inner_html=html
                                        on:click=move |ev| {
                                            if let Some(t) = crate::memory_graph::markdown_excerpt::wikilink_click_target(&ev) {
                                                navigate_wl(&state, &mem, t);
                                            }
                                        }
                                    ></div>
                                    <div style="display:flex;gap:6px;margin-top:8px">
                                        <button class="node-detail-btn" on:click=edit>
                                            {t!(i18n, memory.edit)}
                                        </button>
                                        <button class="node-detail-btn" on:click=move |_| is_renaming.set(true)>
                                            "Rename"
                                        </button>
                                        <button
                                            class="node-detail-btn"
                                            style=move || {
                                                if confirm_delete.get() {
                                                    "color:white;background:var(--cat-error,#f44336)".to_string()
                                                } else {
                                                    String::new()
                                                }
                                            }
                                            on:click=delete
                                        >
                                            {move || if confirm_delete.get() {
                                                "Confirm delete?"
                                            } else {
                                                "Delete"
                                            }}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }}
                    </div>
                }.into_any()
            }}
            {(!tags.is_empty()).then(|| {
                let t = tags.clone();
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
            {(!backlinks.is_empty()).then(|| {
                let bl = backlinks.clone();
                view! {
                    <div style="margin-top:10px">
                        <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:4px">
                            "Backlinks"
                        </div>
                        <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:3px">
                            {bl.into_iter().map(|id| {
                                let id_click = id.clone();
                                view! {
                                    <li
                                        style="font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer"
                                        on:click=move |_| mem.selected_node.set(Some(id_click.clone()))
                                    >
                                        {id}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                }
            })}
            {(!outgoing.is_empty()).then(|| {
                let ol = outgoing.clone();
                view! {
                    <div style="margin-top:10px">
                        <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:4px">
                            "Links"
                        </div>
                        <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:3px">
                            {ol.into_iter().map(|l| {
                                let is_active = l.status == "active";
                                let target = l.to.clone();
                                let display = l.label.clone().unwrap_or_else(|| l.raw.clone());
                                let badge = l.relation.clone().map(|r| format!(" · {r}")).unwrap_or_default();
                                let meta = match l.status.as_str() {
                                    "dangling" => " · dangling".to_string(),
                                    "tombstone" => " · 🪦 deleted".to_string(),
                                    // Provenance badge: which resolver tier made
                                    // this edge (exact_path / exact_filename /
                                    // alias / normalized).
                                    _ => match l.resolved_by.as_deref() {
                                        Some(tier) => {
                                            format!(" · {:.0}% · {tier}", l.confidence * 100.0)
                                        }
                                        None => format!(" · {:.0}%", l.confidence * 100.0),
                                    },
                                };
                                let style = match l.status.as_str() {
                                    "dangling" => "font-size:11px;color:var(--text-meta);font-style:italic;padding:3px 6px",
                                    "tombstone" => "font-size:11px;color:var(--text-meta);text-decoration:line-through;padding:3px 6px",
                                    _ => "font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer",
                                };
                                view! {
                                    <li
                                        style=style
                                        on:click=move |_| {
                                            if is_active {
                                                mem.selected_node.set(Some(target.clone()));
                                            }
                                        }
                                    >
                                        {display}{badge}{meta}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
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

/// Resolve a clicked wikilink target to a node id and select it.
/// Path-form targets navigate directly; bare names resolve via graph.search
/// (first hit — same resolution surface the sidebar search uses).
fn navigate_wl(state: &DashboardState, mem: &MemoryState, target: String) {
    let state = *state;
    let mem = *mem;
    spawn_local(async move {
        let id = if target.contains('/') {
            Some(target)
        } else {
            let agent = mem.agent_id.get_untracked();
            GraphApi::search(&state, &agent, &target, 1)
                .await
                .ok()
                .and_then(|r| r.results.first().map(|f| f.id.clone()))
        };
        if let Some(id) = id {
            mem.push_recent(id.clone());
            mem.selected_node.set(Some(id));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::breadcrumb_from_path;

    #[test]
    fn breadcrumb_strips_filename_and_splits_dirs() {
        assert_eq!(
            breadcrumb_from_path("project/aleph/notes/foo.md"),
            vec!["project", "aleph", "notes"]
        );
    }

    #[test]
    fn breadcrumb_empty_for_bare_filename() {
        assert_eq!(breadcrumb_from_path("foo.md"), Vec::<String>::new());
        assert_eq!(breadcrumb_from_path(""), Vec::<String>::new());
    }

    #[test]
    fn breadcrumb_ignores_empty_and_dot_segments() {
        assert_eq!(breadcrumb_from_path("/a//b/c.md"), vec!["a", "b"]);
    }
}

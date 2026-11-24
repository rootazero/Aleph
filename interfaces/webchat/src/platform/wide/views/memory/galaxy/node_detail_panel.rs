use aleph_protocol::memory_graph::NoteAddress;
use leptos::prelude::*;

use crate::platform::wide::views::memory::note_links::OutgoingLinks;
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
    /// Excerpts are keyed by the exact partition and note ID.
    excerpts: RwSignal<HashMap<NoteAddress, NodeExcerpt>>,
    /// Galaxy-invalidation pulse: bumped after every successful note mutation
    /// so the canvas host re-fetches the graph. Without it a deleted note's
    /// star (and its edges) keeps rendering, and a renamed note's new id is
    /// absent from the galaxy index — both silent, until the next agent switch.
    graph_nonce: RwSignal<u32>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let state = expect_context::<DashboardState>();

    let version = RwSignal::new(0u32);
    let detail_error = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let request = version.get_untracked().wrapping_add(1);
        version.set(request);
        detail_error.set(None);
        let selector = mem.agent_id.get();
        let epoch = state.connection_epoch.get();
        graph_nonce.get();
        let selected = mem.selected_node.get();
        if !state.is_connected.get() {
            return;
        }
        let Some(address) = selected else {
            return;
        };
        if excerpts.with_untracked(|m| m.contains_key(&address)) {
            return;
        }
        spawn_local(async move {
            let reply = GraphApi::node_detail(&state, &address.agent_id, &address.node_id).await;
            if version.try_get_untracked() != Some(request)
                || state.connection_epoch.get_untracked() != epoch
                || mem.agent_id.get_untracked() != selector
                || mem
                    .selected_node
                    .with_untracked(|node| node.as_ref() != Some(&address))
                || !state.is_connected.get_untracked()
            {
                return;
            }
            match reply {
                Ok(detail) => {
                    let ex = NodeExcerpt {
                        name: detail.node.name,
                        category: detail.node.category,
                        tags: detail.node.tags,
                        body_markdown: detail.content,
                        breadcrumb: breadcrumb_from_path(&detail.node.path),
                        backlinks: detail.backlinks,
                        outgoing: detail.outgoing,
                    };
                    excerpts.update(|m| {
                        m.insert(address, ex);
                    });
                }
                Err(e) => {
                    detail_error.set(Some(e.to_string()));
                }
            }
        });
    });

    view! {
        <div class="flex-1 min-h-0 overflow-y-auto px-3 py-2">
            {move || {
                let selected = mem.selected_node.get();
                if let Some(address) = selected {
                    if let Some(ex) = excerpts.with(|m| m.get(&address).cloned()) {
                        view! { <DetailFor address=address excerpt=ex excerpts=excerpts graph_nonce=graph_nonce /> }
                            .into_any()
                    } else if let Some(error) = detail_error.get() {
                        view! { <p role="alert">{error}</p> }.into_any()
                    } else {
                        view! { <DetailLoading id=address.node_id /> }.into_any()
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
    address: NoteAddress,
    excerpt: NodeExcerpt,
    excerpts: RwSignal<HashMap<NoteAddress, NodeExcerpt>>,
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
    let outgoing = StoredValue::new(excerpt.outgoing.clone());
    let address_for_list = address.clone();
    let partition = StoredValue::new(address.agent_id.clone());
    let address = StoredValue::new(address);
    let same_selection = move |target: &NoteAddress| {
        mem.selected_node
            .with_untracked(|node| node.as_ref() == Some(target))
    };
    let same_scope = move |selector: &str, epoch| {
        graph_nonce.try_get_untracked().is_some()
            && state.is_connected.get_untracked()
            && state.connection_epoch.get_untracked() == epoch
            && mem.agent_id.get_untracked() == selector
    };
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
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let content = draft.get_untracked();
            let target = address.get_value();
            let selector = mem.agent_id.get_untracked();
            let epoch = state.connection_epoch.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                let result =
                    GraphApi::update_note(&state, &target.agent_id, &target.node_id, &content)
                        .await;
                if !same_scope(&selector, epoch) {
                    return;
                }
                match result {
                    Ok(()) => {
                        // Reload the body and resolved links together.
                        excerpts.update(|m| {
                            m.remove(&target);
                        });
                        graph_nonce.update(|n| *n += 1);
                        if is_editing.try_get_untracked().is_some() {
                            is_saving.set(false);
                            is_editing.set(false);
                        }
                    }
                    Err(e) => {
                        if same_selection(&target) && is_saving.try_get_untracked().is_some() {
                            is_saving.set(false);
                            error.set(Some(
                                crate::components::admin_refusal::settings_write_error(
                                    i18n,
                                    &e,
                                    |e| e.to_string(),
                                ),
                            ));
                        }
                    }
                }
            });
        }
    };

    let do_rename = {
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let new_title = rename_draft.get_untracked();
            if new_title.is_empty() {
                error.set(Some("Title cannot be empty".to_string()));
                return;
            }
            let target = address.get_value();
            let selector = mem.agent_id.get_untracked();
            let epoch = state.connection_epoch.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                let result =
                    GraphApi::rename_note(&state, &target.agent_id, &target.node_id, &new_title)
                        .await;
                if !same_scope(&selector, epoch) {
                    return;
                }
                match result {
                    Ok(new_id) => {
                        let renamed = NoteAddress {
                            agent_id: target.agent_id.clone(),
                            node_id: new_id,
                        };
                        excerpts.update(|m| {
                            m.remove(&target);
                            m.remove(&renamed);
                        });
                        graph_nonce.update(|n| *n += 1);
                        if same_selection(&target) && is_renaming.try_get_untracked().is_some() {
                            mem.selected_node.set(Some(renamed));
                        }
                    }
                    Err(e) => {
                        if same_selection(&target) && is_saving.try_get_untracked().is_some() {
                            is_saving.set(false);
                            error.set(Some(
                                crate::components::admin_refusal::settings_write_error(
                                    i18n,
                                    &e,
                                    |e| e.to_string(),
                                ),
                            ));
                        }
                    }
                }
            });
        }
    };

    let do_delete = {
        move |_| {
            if is_saving.get_untracked() || !confirm_delete.get_untracked() {
                // First tap: arm the confirm
                confirm_delete.set(true);
                return;
            }
            // Second tap: execute delete
            let target = address.get_value();
            let selector = mem.agent_id.get_untracked();
            let epoch = state.connection_epoch.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                let result = GraphApi::delete_note(&state, &target.agent_id, &target.node_id).await;
                if !same_scope(&selector, epoch) {
                    return;
                }
                match result {
                    Ok(()) => {
                        // Purge the stale cache entry so re-selecting this id
                        // (recent-visited, backlinks) can't show deleted content.
                        excerpts.update(|m| {
                            m.remove(&target);
                        });
                        graph_nonce.update(|n| *n += 1);
                        if same_selection(&target) && confirm_delete.try_get_untracked().is_some() {
                            mem.selected_node.set(None);
                        }
                    }
                    Err(e) => {
                        if !same_selection(&target) || is_saving.try_get_untracked().is_none() {
                            return;
                        }
                        is_saving.set(false);
                        confirm_delete.set(false);
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
                    mem.highlight_note.set(Some(address_for_list.clone()));
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
                                                let source = address.get_value();
                                                let target = outgoing.with_value(|links| wikilink_address(&source, links, &t));
                                                if mem.selected_node.get_untracked().as_ref() == Some(&source) {
                                                    if let Some(target) = target {
                                                        mem.push_recent(target.clone());
                                                        mem.selected_node.set(Some(target));
                                                    }
                                                }
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
                                        on:click=move |_| mem.selected_node.set(Some(NoteAddress { agent_id: partition.get_value(), node_id: id_click.clone() }))
                                    >
                                        {id}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                }
            })}
            <OutgoingLinks
                links=Signal::derive(move || outgoing.get_value())
                on_navigate=Callback::new(move |id: String| mem.selected_node.set(Some(NoteAddress { agent_id: partition.get_value(), node_id: id })))
            />
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
                    let top: Vec<NoteAddress> = q.iter().take(5).cloned().collect();
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
                                            {id.node_id}
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

/// Follow the Core-resolved link in the source partition, never a search hit.
fn wikilink_address(
    source: &NoteAddress,
    links: &[OutgoingLinkDto],
    target: &str,
) -> Option<NoteAddress> {
    let link = links
        .iter()
        .find(|link| link.raw == target && link.status == "active")?;
    Some(NoteAddress {
        agent_id: source.agent_id.clone(),
        node_id: link.to.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{breadcrumb_from_path, wikilink_address, NoteAddress, OutgoingLinkDto};

    #[test]
    fn bare_wikilink_follows_an_active_resolution_in_the_source_partition() {
        let source = NoteAddress {
            agent_id: "main".into(),
            node_id: "concept/Source".into(),
        };
        let mut outgoing = OutgoingLinkDto {
            to: "references/Target".into(),
            raw: "Target".into(),
            relation: None,
            label: None,
            confidence: 1.0,
            resolved_by: None,
            status: "active".into(),
        };
        assert_eq!(
            wikilink_address(&source, &[outgoing.clone()], "Target"),
            Some(NoteAddress {
                agent_id: "main".into(),
                node_id: "references/Target".into(),
            })
        );
        outgoing.status = "dangling".into();
        assert_eq!(wikilink_address(&source, &[outgoing], "Target"), None);
    }

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

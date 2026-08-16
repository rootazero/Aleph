//! Right-side detail drawer for the memory console. Note rows fetch full
//! markdown + backlinks via `graph.node_detail` and can be edited inline via
//! `graph.update_note` (mirrors the canvas node detail panel). Raw rows show
//! their stored Q/A. Pure I/O — all persistence is JSON-RPC (R4).

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::provenance::ProvenanceSection;
use super::toast::{push_toast, ToastKind, ToastSlot};
use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, RawMemory, TraceKind};
use crate::memory_graph::category_color::category_color;
use crate::memory_graph::markdown_excerpt::{render_excerpt, wikilink_click_target};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::memory::MemoryState;

/// What the drawer is currently showing.
#[derive(Clone)]
pub enum DrawerTarget {
    /// A note, stamped with the agent it was opened under.
    ///
    /// Note paths (`category/filename`) collide readily across agents, and
    /// every mutation this drawer issues is agent-scoped. Resolving the agent
    /// at click time instead — from the live signal — means a drawer that
    /// outlives an agent switch by even one frame sends `graph.update_note` /
    /// `rename_note` / `delete_note` for a path resolved under the old agent
    /// to the new agent's store. Carrying the agent with the target makes the
    /// drawer act on what it is actually displaying.
    Note { agent: String, fact: CompressedFact },
    /// A raw conversation row. Read-only here, so no stamp is needed: nothing
    /// in `RawDetail` mutates, and `memory.delete` is not agent-scoped.
    Raw(RawMemory),
}

impl DrawerTarget {
    /// Open a note by path under `agent`, without a loaded row to show yet.
    fn note_stub(agent: &str, path: &str) -> Self {
        Self::Note {
            agent: agent.to_string(),
            fact: CompressedFact::stub_from_path(path),
        }
    }
}

#[component]
pub fn DetailDrawer(
    target: RwSignal<Option<DrawerTarget>>,
    toast_slot: ToastSlot,
    #[prop(into)] on_mutated: Callback<()>,
) -> impl IntoView {
    view! {
        {move || match target.get() {
            None => view! { <div></div> }.into_any(),
            Some(DrawerTarget::Note { agent, fact }) => view! {
                <DrawerShell target=target>
                    <NoteDetail agent=agent fact=fact target=target toast_slot=toast_slot on_mutated=on_mutated />
                </DrawerShell>
            }
            .into_any(),
            Some(DrawerTarget::Raw(raw)) => view! {
                <DrawerShell target=target>
                    <RawDetail raw=raw />
                </DrawerShell>
            }
            .into_any(),
        }}
    }
}

#[component]
fn DrawerShell(target: RwSignal<Option<DrawerTarget>>, children: Children) -> impl IntoView {
    view! {
        <div class="fixed inset-0 z-40 flex justify-end">
            // Scrim: blurs + dims the content behind the drawer (readability) and
            // closes the drawer on any outside click (mirrors teams task_drawer).
            <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=move |_| target.set(None)></div>
            <aside class="relative h-full w-[380px] max-w-[90vw] bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)] border-l border-border shadow-2xl flex flex-col">
                <div class="flex items-center justify-end p-3 border-b border-border-subtle">
                    <button
                        class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-sunken transition-colors"
                        on:click=move |_| target.set(None)
                    >
                        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <line x1="18" y1="6" x2="6" y2="18" />
                            <line x1="6" y1="6" x2="18" y2="18" />
                        </svg>
                    </button>
                </div>
                <div class="flex-1 min-h-0 overflow-y-auto p-4">{children()}</div>
            </aside>
        </div>
    }
}

#[component]
fn NoteDetail(
    /// The agent this note was opened under. Every RPC below uses it rather
    /// than re-reading the live agent signal — see [`DrawerTarget::Note`].
    agent: String,
    fact: CompressedFact,
    target: RwSignal<Option<DrawerTarget>>,
    toast_slot: ToastSlot,
    #[prop(into)] on_mutated: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();

    let path = fact.path.clone();
    let stripe = category_color(&fact.category);
    let title = fact.content.clone();

    let body = RwSignal::new(None::<String>);
    let backlinks = RwSignal::new(Vec::<String>::new());
    let is_editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let is_saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    // Rename state
    let is_renaming = RwSignal::new(false);
    let rename_draft = RwSignal::new(title.clone());

    // Delete state
    let confirm_delete = RwSignal::new(false);

    // Fetch full content + backlinks once on mount.
    {
        let path = path.clone();
        let agent = agent.clone();
        Effect::new(move |_| {
            let path = path.clone();
            let agent = agent.clone();
            spawn_local(async move {
                match GraphApi::node_detail(&state, &agent, &path).await {
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
    }

    let save = {
        let path = path.clone();
        let agent = agent.clone();
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let content = draft.get_untracked();
            let path = path.clone();
            let agent = agent.clone();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::update_note(&state, &agent, &path, &content).await {
                    Ok(()) => {
                        body.set(Some(content));
                        is_saving.set(false);
                        is_editing.set(false);
                        push_toast(
                            toast_slot,
                            t_string!(i18n, memory.toast_saved).to_string(),
                            ToastKind::Success,
                        );
                        // The card list's `updated_at` would otherwise sit stale
                        // until an unrelated refresh; a save changes it for real.
                        on_mutated.run(());
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
        let path = path.clone();
        let agent = agent.clone();
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let new_title = rename_draft.get_untracked();
            if new_title.is_empty() {
                error.set(Some("Title cannot be empty".to_string()));
                return;
            }
            let path = path.clone();
            let agent = agent.clone();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::rename_note(&state, &agent, &path, &new_title).await {
                    Ok(new_id) => {
                        target.set(Some(DrawerTarget::note_stub(&agent, &new_id)));
                        is_saving.set(false);
                        is_renaming.set(false);
                        push_toast(
                            toast_slot,
                            t_string!(i18n, memory.toast_renamed).to_string(),
                            ToastKind::Success,
                        );
                        // Without this, the card list still keys off the old
                        // path: it stays clickable (opening a drawer whose
                        // `graph.node_detail` fails against the renamed path)
                        // and selectable (its delete would target a path that
                        // no longer exists) until an unrelated refresh happens.
                        on_mutated.run(());
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
        let path = path.clone();
        let agent = agent.clone();
        move |_| {
            if is_saving.get_untracked() || !confirm_delete.get_untracked() {
                // First tap: arm the confirm
                confirm_delete.set(true);
                return;
            }
            // Second tap: execute delete
            let path = path.clone();
            let agent = agent.clone();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::delete_note(&state, &agent, &path).await {
                    Ok(()) => {
                        target.set(None);
                        is_saving.set(false);
                        mem.highlight_note_id.set(None);
                        push_toast(
                            toast_slot,
                            t_string!(i18n, memory.toast_deleted).to_string(),
                            ToastKind::Success,
                        );
                        on_mutated.run(());
                    }
                    Err(e) => {
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

    // The view's stamp lives in a `StoredValue` because a bare `String` is not
    // `Copy`: moving it into a nested `on:click` handler would consume it from
    // the enclosing reactive closure, leaving that closure `FnOnce` where the
    // renderer needs `FnMut`. (The pre-stamp code read `mem`, which is `Copy`,
    // so the question never arose.)
    let agent_v = StoredValue::new(agent);

    view! {
        <div>
            <div style=format!("height:3px;background:{stripe};border-radius:2px;margin-bottom:8px")></div>
            <h3 class="text-sm font-semibold text-text-primary mb-1 break-words">{title}</h3>
            <div class="text-xs text-text-tertiary font-mono mb-3 break-all">{path.clone()}</div>

            {move || if is_editing.get() {
                let save = save.clone();
                view! {
                    <div>
                        <textarea
                            class="w-full h-64 p-2 text-xs font-mono bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary/50"
                            prop:value=move || draft.get()
                            on:input=move |ev| draft.set(event_target_value(&ev))
                        ></textarea>
                        <div class="flex gap-2 mt-2">
                            <button
                                class="px-3 py-1 text-xs rounded-lg bg-primary text-white disabled:opacity-50"
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
                                class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                                prop:disabled=move || is_saving.get()
                                on:click=move |_| is_editing.set(false)
                            >
                                {t!(i18n, memory.cancel)}
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
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
                                        class="w-full p-2 text-xs bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary/50 box-border"
                                        prop:value=move || rename_draft.get()
                                        on:input=move |ev| rename_draft.set(event_target_value(&ev))
                                    />
                                    <div class="flex gap-2 mt-2">
                                        <button
                                            class="px-3 py-1 text-xs rounded-lg bg-primary text-white disabled:opacity-50"
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
                                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                                            prop:disabled=move || is_saving.get()
                                            on:click=move |_| is_renaming.set(false)
                                        >
                                            {t!(i18n, memory.cancel)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            let delete = delete.clone();
                            view! {
                                <div>
                                    {move || match body.get() {
                                        Some(md) => view! {
                                            <div
                                                class="node-card-full__excerpt text-xs leading-relaxed text-text-secondary"
                                                inner_html=render_excerpt(&md)
                                                on:click=move |ev| {
                                                    if let Some(t) = wikilink_click_target(&ev) {
                                                        navigate_drawer(&state, &agent_v.get_value(), target, t);
                                                    }
                                                }
                                            ></div>
                                        }.into_any(),
                                        None => view! {
                                            <div class="text-xs italic text-text-tertiary">{t!(i18n, common.loading)}</div>
                                        }.into_any(),
                                    }}
                                    <div class="flex gap-2 mt-3">
                                        <button
                                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                                            on:click=move |_| {
                                                draft.set(body.get_untracked().unwrap_or_default());
                                                error.set(None);
                                                confirm_delete.set(false);
                                                is_renaming.set(false);
                                                is_editing.set(true);
                                            }
                                        >
                                            {t!(i18n, memory.edit)}
                                        </button>
                                        <button
                                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                                            on:click=move |_| is_renaming.set(true)
                                        >
                                            "Rename"
                                        </button>
                                        <button
                                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                                            style=move || {
                                                if confirm_delete.get() {
                                                    "border-color:var(--cat-error,#f44336);color:white;background:var(--cat-error,#f44336)".to_string()
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

            {move || error.get().map(|e| view! {
                <div style="color:var(--cat-error,#f44336)" class="text-xs mt-2">{e}</div>
            })}

            {move || {
                let bl = backlinks.get();
                (!bl.is_empty()).then(|| view! {
                    <div class="mt-4">
                        <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1">
                            {t!(i18n, memory.detail_backlinks)}
                        </div>
                        <ul class="space-y-1">
                            {bl.into_iter().map(|b| {
                                let b_click = b.clone();
                                view! {
                                    <li
                                        style="font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer;word-break:break-all;font-family:monospace"
                                        on:click=move |_| navigate_drawer(&state, &agent_v.get_value(), target, b_click.clone())
                                    >
                                        {b}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                })
            }}

            <ProvenanceSection
                agent=Signal::derive(move || agent_v.get_value())
                target=path.clone()
                kind=TraceKind::Note
            />

            <div class="mt-4 pt-3 border-t border-border-subtle text-[11px] italic text-text-tertiary">
                {t!(i18n, memory.note_lifecycle_managed)}
            </div>
        </div>
    }
}

/// Resolve a clicked wikilink target to a node id and load it into the
/// drawer. Path-form targets navigate directly; bare names resolve via
/// graph.search (first hit — mirrors `navigate_wl` in the canvas detail
/// panel). Setting `target_signal` remounts `NoteDetail`, which fetches the
/// new note on its own.
fn navigate_drawer(
    state: &DashboardState,
    agent: &str,
    target_signal: RwSignal<Option<DrawerTarget>>,
    wl: String,
) {
    let state = *state;
    // Following a wikilink stays inside the agent whose note contained it —
    // both the `graph.search` resolution and the new target keep the stamp.
    let agent = agent.to_string();
    spawn_local(async move {
        let id = if wl.contains('/') {
            Some(wl)
        } else {
            GraphApi::search(&state, &agent, &wl, 1)
                .await
                .ok()
                .and_then(|r| r.results.first().map(|f| f.id.clone()))
        };
        if let Some(id) = id {
            target_signal.set(Some(DrawerTarget::note_stub(&agent, &id)));
        }
    });
}

#[component]
fn RawDetail(raw: RawMemory) -> impl IntoView {
    let i18n = use_i18n();
    let raw_id = raw.id.clone();
    // A raw row states its own owner (`memory.list` always fills `agent_id`
    // from the stored row), so the trace is scoped to the row on screen rather
    // than to whichever agent happens to be selected when the drawer is read.
    let raw_agent = StoredValue::new(raw.agent_id.clone());
    view! {
        <div>
            <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-2">
                {t!(i18n, memory.facet_raw)}
            </div>
            <pre class="whitespace-pre-wrap break-words text-xs leading-relaxed text-text-secondary font-sans">
                {raw.display_text()}
            </pre>
            <ProvenanceSection
                agent=Signal::derive(move || raw_agent.get_value())
                target=raw_id.clone()
                kind=TraceKind::Raw
            />
        </div>
    }
}

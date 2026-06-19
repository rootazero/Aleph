//! Right-side detail drawer for the memory console. Note rows fetch full
//! markdown + backlinks via `graph.node_detail` and can be edited inline via
//! `graph.update_note` (mirrors the canvas node detail panel). Raw rows show
//! their stored Q/A and, when from a search, the similarity score. Pure I/O —
//! all persistence is JSON-RPC (R4).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, RawMemory};
use crate::canvas_engine::category_color::category_color;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::state::memory::MemoryState;

/// What the drawer is currently showing.
#[derive(Clone)]
pub enum DrawerTarget {
    Note(CompressedFact),
    Raw(RawMemory),
}

#[component]
pub fn DetailDrawer(target: RwSignal<Option<DrawerTarget>>) -> impl IntoView {
    view! {
        {move || match target.get() {
            None => ().into_any(),
            Some(DrawerTarget::Note(fact)) => view! {
                <DrawerShell target=target>
                    <NoteDetail fact=fact />
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
        <aside class="fixed right-0 top-0 h-full w-[380px] max-w-[90vw] bg-surface-raised border-l border-border shadow-2xl z-40 flex flex-col">
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
    }
}

#[component]
fn NoteDetail(fact: CompressedFact) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();

    let body = RwSignal::new(None::<String>);
    let backlinks = RwSignal::new(Vec::<String>::new());
    let is_editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let is_saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let path = fact.path.clone();
    let stripe = category_color(&fact.category);
    let title = fact.content.clone();

    // Fetch full content + backlinks once on mount.
    {
        let path = path.clone();
        Effect::new(move |_| {
            let path = path.clone();
            let agent = mem.agent_id.get_untracked();
            spawn_local(async move {
                match GraphApi::node_detail(&state, &agent, &path).await {
                    Ok(d) => {
                        body.set(Some(d.content));
                        backlinks.set(d.backlinks);
                    }
                    Err(e) => error.set(Some(e)),
                }
            });
        });
    }

    let save = {
        let path = path.clone();
        move |_| {
            if is_saving.get_untracked() {
                return;
            }
            let content = draft.get_untracked();
            let path = path.clone();
            let agent = mem.agent_id.get_untracked();
            is_saving.set(true);
            error.set(None);
            spawn_local(async move {
                match GraphApi::update_note(&state, &agent, &path, &content).await {
                    Ok(()) => {
                        body.set(Some(content));
                        is_saving.set(false);
                        is_editing.set(false);
                    }
                    Err(e) => {
                        is_saving.set(false);
                        error.set(Some(e));
                    }
                }
            });
        }
    };

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
                view! {
                    <div>
                        {move || match body.get() {
                            Some(md) => view! {
                                <div
                                    class="node-card-full__excerpt text-xs leading-relaxed text-text-secondary"
                                    inner_html=render_excerpt(&md)
                                ></div>
                            }.into_any(),
                            None => view! {
                                <div class="text-xs italic text-text-tertiary">{t!(i18n, common.loading)}</div>
                            }.into_any(),
                        }}
                        <button
                            class="mt-3 px-3 py-1 text-xs rounded-lg border border-border text-text-secondary hover:text-text-primary"
                            on:click=move |_| {
                                draft.set(body.get_untracked().unwrap_or_default());
                                is_editing.set(true);
                            }
                        >
                            {t!(i18n, memory.edit)}
                        </button>
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
                            {bl.into_iter().map(|b| view! {
                                <li class="text-xs font-mono text-text-secondary break-all">{b}</li>
                            }).collect_view()}
                        </ul>
                    </div>
                })
            }}

            <div class="mt-4 pt-3 border-t border-border-subtle text-[11px] italic text-text-tertiary">
                {t!(i18n, memory.note_lifecycle_managed)}
            </div>
        </div>
    }
}

#[component]
fn RawDetail(raw: RawMemory) -> impl IntoView {
    let i18n = use_i18n();
    let sim = raw.similarity;
    view! {
        <div>
            <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-2">
                {t!(i18n, memory.facet_raw)}
            </div>
            {sim.map(|s| view! {
                <div class="mb-3 text-xs">
                    <span class="text-text-tertiary">{t!(i18n, memory.similarity)}": "</span>
                    <span class="font-mono text-primary">{format!("{s:.3}")}</span>
                </div>
            })}
            <pre class="whitespace-pre-wrap break-words text-xs leading-relaxed text-text-secondary font-sans">
                {raw.content}
            </pre>
        </div>
    }
}

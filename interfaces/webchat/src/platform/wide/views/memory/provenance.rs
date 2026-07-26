//! Evidence-chain section for the memory detail drawer.
//!
//! `memory.trace` has been registered and reachable by the LLM (`memory_trace`
//! tool) since 2026-06-27, but the panel never called it: the drawer showed a
//! note's body and backlinks with no way to ask "what conversation is this
//! claim actually based on?".
//!
//! Both directions are wired. From a note we walk DOWN to the raw rows it was
//! distilled from; from a raw row we walk UP to the notes citing it.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use super::TRACE_MAX_RESULTS;
use crate::api::{EvidenceItem, MemoryApi, TraceKind, TraceResult};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
#[must_use]
pub fn ProvenanceSection(agent: Signal<String>, target: String, kind: TraceKind) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let trace = RwSignal::new(Loadable::<TraceResult>::Loading);
    let expanded = RwSignal::new(None::<String>);

    {
        let target = target.clone();
        Effect::new(move |_| {
            let target = target.clone();
            let agent = agent.get_untracked();
            trace.set(Loadable::Loading);
            spawn_local(async move {
                let res = MemoryApi::trace(&state, &agent, &target, kind, TRACE_MAX_RESULTS).await;
                trace.set(Loadable::from_rpc(res));
            });
        });
    }

    view! {
        <div class="mt-4">
            <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1.5">
                {t!(i18n, memory.provenance)}
            </div>

            {move || match trace.get() {
                Loadable::Loading => view! {
                    <div class="h-12 rounded-lg bg-surface-sunken animate-pulse"></div>
                }.into_any(),

                // A trace failure is not "no evidence" — say which it is.
                Loadable::Failed(e) => view! {
                    <div class="text-xs" style="color:var(--cat-error,#f44336)">
                        {t!(i18n, memory.provenance_failed)}" "<span class="font-mono">{e}</span>
                    </div>
                }.into_any(),

                Loadable::Ready(res) => {
                    // The server also answers "which notes did the walk visit"
                    // (`notes_citing`, for `TraceKind::Raw`) separately from the
                    // evidence rows themselves — render it too rather than
                    // dropping a populated response field. Not filtered/deduped:
                    // whatever the server sent is what shows up here.
                    let capped = res.evidence.len() >= TRACE_MAX_RESULTS;
                    let notes = res.notes;
                    let evidence = res.evidence;
                    let has_notes = !notes.is_empty();
                    let has_evidence = !evidence.is_empty();
                    view! {
                        <div>
                            {has_notes.then(|| view! {
                                <div class="mb-2">
                                    <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1">
                                        {t!(i18n, memory.provenance_notes)}
                                    </div>
                                    <ul class="space-y-1">
                                        {notes.into_iter().map(|n| view! {
                                            <li class="text-[11px] font-mono text-text-secondary break-all">{n}</li>
                                        }).collect_view()}
                                    </ul>
                                </div>
                            })}
                            {if has_evidence {
                                view! {
                                    <div>
                                        <ul class="space-y-1.5">
                                            {evidence.into_iter().map(|item| view! {
                                                <EvidenceRow item=item expanded=expanded />
                                            }).collect_view()}
                                        </ul>
                                        // No silent caps.
                                        {capped.then(|| view! {
                                            <p class="text-[10px] italic text-text-tertiary mt-1">
                                                {t!(i18n, memory.provenance_capped)}
                                            </p>
                                        })}
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <p class="text-[11px] italic text-text-tertiary">{t!(i18n, memory.provenance_empty)}</p>
                                }.into_any()
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn EvidenceRow(item: EvidenceItem, expanded: RwSignal<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let raw_id = item.raw_id.clone();
    let id_for_click = raw_id.clone();
    let id_for_open = raw_id.clone();
    let is_open = Signal::derive(move || expanded.get().as_deref() == Some(id_for_open.as_str()));
    let has_body = item.content.is_some();
    let content = item.content.clone();
    let via_note = item.via_note.clone();
    let via_session = item.via_session.clone();
    let pruned = item.pruned;

    view! {
        <li class="rounded-lg border border-border-subtle bg-surface-sunken px-2.5 py-2">
            <button
                class="w-full text-left"
                prop:disabled=!has_body
                on:click=move |_| {
                    if !has_body { return; }
                    expanded.update(|e| {
                        *e = if e.as_deref() == Some(id_for_click.as_str()) {
                            None
                        } else {
                            Some(id_for_click.clone())
                        };
                    });
                }
            >
                <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-[11px] font-mono text-text-secondary break-all">{raw_id}</span>
                    {via_session.map(|s| view! {
                        <span class="text-[10px] text-text-tertiary font-mono">
                            {move || t_string!(i18n, memory.session).to_string()}" "{s}
                        </span>
                    })}
                    {via_note.map(|n| view! {
                        <span class="text-[10px] text-text-tertiary font-mono break-all">
                            {move || t_string!(i18n, memory.provenance_via).to_string()}" "{n}
                        </span>
                    })}
                    // A cited row whose source is gone is a real state, not an
                    // absence of evidence. Label it rather than hiding it.
                    {pruned.then(|| view! {
                        <span class="px-1.5 py-0.5 rounded text-[10px] bg-warning-subtle text-warning border border-warning/20">
                            {move || t_string!(i18n, memory.provenance_pruned).to_string()}
                        </span>
                    })}
                </div>
            </button>
            {move || (is_open.get() && has_body).then(|| view! {
                <pre class="mt-1.5 whitespace-pre-wrap break-words text-[11px] leading-relaxed \
                            text-text-secondary font-sans">
                    {content.clone().unwrap_or_default()}
                </pre>
            })}
        </li>
    }
}

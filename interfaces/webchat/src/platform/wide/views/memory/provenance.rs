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

/// Whether the "notes" section of a trace result should render.
///
/// Only for `TraceKind::Raw`: `memory_trace.rs` fills `notes` with
/// `notes_citing(raw_id)` in that direction (a real relationship — which
/// notes cite this raw row), but with exactly `[target]` for `TraceKind::Note`
/// (the note tracing itself). Rendering that under a "cited by" header would
/// show every note its own path, not evidence about it.
#[must_use]
fn show_notes_section(kind: TraceKind, notes_len: usize) -> bool {
    matches!(kind, TraceKind::Raw) && notes_len > 0
}

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
                    // For `TraceKind::Raw`, `notes` is `notes_citing(raw_id)` —
                    // the notes that cite this raw row, a real relationship the
                    // user has no other way to see. For `TraceKind::Note` it is
                    // always exactly `[target]` (the note tracing itself — see
                    // `memory_trace.rs`'s `TraceKind::Note` arm): rendering that
                    // verbatim would show every note's own path under this
                    // header, which is not "which notes cite this" but "what did
                    // I just ask about". Gate on the direction, not just
                    // emptiness — mirrors the Delete-verb split elsewhere in this
                    // console (notes vs. raw use different server calls; this is
                    // the same "the two directions carry different meaning"
                    // reasoning applied to a display field).
                    let capped = res.evidence.len() >= TRACE_MAX_RESULTS;
                    let notes = res.notes;
                    let evidence = res.evidence;
                    let has_notes = show_notes_section(kind, notes.len());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_notes_for_raw_direction_when_the_server_returned_some() {
        // `TraceKind::Raw` is the direction where `notes` carries a real
        // relationship (`notes_citing`) rather than the target echoed back.
        assert!(show_notes_section(TraceKind::Raw, 1));
        assert!(show_notes_section(TraceKind::Raw, 3));
    }

    #[test]
    fn hides_notes_for_raw_direction_when_empty() {
        assert!(!show_notes_section(TraceKind::Raw, 0));
    }

    #[test]
    fn hides_notes_for_note_direction_even_when_populated() {
        // For `TraceKind::Note`, `memory_trace.rs` fills `notes` with exactly
        // `[target]` -- always non-empty, and always the note tracing itself.
        // A non-empty count here must not flip this on.
        assert!(!show_notes_section(TraceKind::Note, 1));
    }
}

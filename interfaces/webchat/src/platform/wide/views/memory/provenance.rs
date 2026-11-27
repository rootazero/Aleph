//! Displays recorded evidence for the note or raw row open in the drawer.

use aleph_protocol::memory_trace::{FactOrigin, NoteSource, NoteSourceStatus};
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

fn trace_is_empty(kind: TraceKind, trace: &TraceResult) -> bool {
    !show_notes_section(kind, trace.notes.len())
        && trace.note_sources.is_empty()
        && trace.facts.is_empty()
        && trace.evidence.is_empty()
        && !trace.truncated
        && !trace.incomplete
}

#[derive(Clone, PartialEq, Eq)]
struct EvidenceKey {
    raw_id: String,
    via_note: Option<String>,
}

impl From<&EvidenceItem> for EvidenceKey {
    fn from(item: &EvidenceItem) -> Self {
        Self {
            raw_id: item.raw_id.clone(),
            via_note: item.via_note.clone(),
        }
    }
}

#[component]
#[must_use]
pub fn ProvenanceSection(agent: Signal<String>, target: String, kind: TraceKind) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let trace = RwSignal::new(Loadable::<TraceResult>::Loading);
    let expanded = RwSignal::new(None::<EvidenceKey>);

    {
        let target = target.clone();
        Effect::new(move |_| {
            let target = target.clone();
            let agent = agent.get_untracked();
            trace.set(Loadable::Loading);
            spawn_local(async move {
                let res =
                    MemoryApi::trace(&state, &agent, &target, kind, TRACE_MAX_RESULTS, true).await;
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
                    let empty = trace_is_empty(kind, &res);
                    let has_notes = show_notes_section(kind, res.notes.len());
                    let TraceResult { notes, evidence, note_sources, facts, truncated, incomplete, .. } = res;
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
                            {(!note_sources.is_empty()).then(|| view! {
                                <div class="mb-2">
                                    <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1">
                                        {t!(i18n, memory.provenance_source_notes)}
                                    </div>
                                    <ul class="space-y-1">
                                        {note_sources.into_iter().map(|edge| view! { <SourceNoteRow edge=edge /> }).collect_view()}
                                    </ul>
                                </div>
                            })}
                            {(!facts.is_empty()).then(|| view! {
                                <div class="mb-2">
                                    <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1">
                                        {t!(i18n, memory.provenance_facts)}
                                    </div>
                                    <ul class="space-y-1">
                                        {facts.into_iter().map(|fact| view! { <FactOriginRow fact=fact /> }).collect_view()}
                                    </ul>
                                </div>
                            })}
                            {(!evidence.is_empty()).then(|| view! {
                                <ul class="space-y-1.5">
                                    {evidence.into_iter().map(|item| view! {
                                        <EvidenceRow item=item expanded=expanded />
                                    }).collect_view()}
                                </ul>
                            })}
                            {empty.then(|| view! {
                                <p class="text-[11px] italic text-text-tertiary">{t!(i18n, memory.provenance_empty)}</p>
                            })}
                            {truncated.then(|| view! {
                                <p class="text-[10px] italic text-text-tertiary mt-1">{t!(i18n, memory.provenance_capped)}</p>
                            })}
                            {incomplete.then(|| view! {
                                <p class="text-[10px] italic text-text-tertiary mt-1">{t!(i18n, memory.provenance_incomplete)}</p>
                            })}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SourceNoteRow(edge: NoteSource) -> impl IntoView {
    let i18n = use_i18n();
    let label = move || match edge.status {
        NoteSourceStatus::Available => None,
        NoteSourceStatus::Unavailable => {
            Some(t_string!(i18n, memory.provenance_unavailable).to_string())
        }
        NoteSourceStatus::NotVisited => {
            Some(t_string!(i18n, memory.provenance_not_visited).to_string())
        }
    };
    view! {
        <li class="text-[11px] break-all">
            <span class="font-mono text-text-secondary">{edge.note_path}" → "{edge.source_path}</span>
            <span class="ml-2 text-text-tertiary">{label}</span>
        </li>
    }
}

#[component]
fn FactOriginRow(fact: FactOrigin) -> impl IntoView {
    let i18n = use_i18n();
    let FactOrigin {
        text,
        origin,
        inferred,
        source_id,
        ..
    } = fact;
    let show_inferred = inferred && origin != "inferred";
    let label = move || match origin.as_str() {
        "raw_source" => t_string!(i18n, memory.provenance_origin_raw).to_string(),
        "prior_note" => t_string!(i18n, memory.provenance_origin_note).to_string(),
        "inferred" => t_string!(i18n, memory.provenance_origin_inferred).to_string(),
        "system" => t_string!(i18n, memory.provenance_origin_system).to_string(),
        "legacy" => t_string!(i18n, memory.provenance_origin_legacy).to_string(),
        _ => origin.clone(),
    };
    view! {
        <li class="text-[11px] text-text-secondary break-words">
            <div class="whitespace-pre-wrap">{text}</div>
            <div class="flex flex-wrap gap-x-2 text-[10px] text-text-tertiary">
                <span>{label}</span>
                {show_inferred.then(|| view! { <span>{t!(i18n, memory.provenance_inferred_flag)}</span> })}
                {source_id.map(|id| view! { <span class="font-mono break-all">{id}</span> })}
            </div>
        </li>
    }
}

#[component]
fn EvidenceRow(item: EvidenceItem, expanded: RwSignal<Option<EvidenceKey>>) -> impl IntoView {
    let i18n = use_i18n();
    let key_for_click = EvidenceKey::from(&item);
    let key_for_open = key_for_click.clone();
    let is_open =
        Signal::derive(move || expanded.with(|open| open.as_ref() == Some(&key_for_open)));
    let EvidenceItem {
        raw_id,
        via_note,
        via_session,
        content,
        pruned,
    } = item;
    let has_body = content.is_some();

    view! {
        <li class="rounded-lg border border-border-subtle bg-surface-sunken px-2.5 py-2">
            <button
                class="w-full text-left"
                prop:disabled=!has_body
                aria-expanded=move || if is_open.get() { "true" } else { "false" }
                on:click=move |_| {
                    if !has_body { return; }
                    expanded.update(|e| {
                        *e = if e.as_ref() == Some(&key_for_click) {
                            None
                        } else {
                            Some(key_for_click.clone())
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
    fn preserves_fact_only_wire_data_and_evidence_states() {
        let mut trace: TraceResult = serde_json::from_str(
            r#"{"target":"synthesis/topic","notes":["synthesis/topic"],"evidence":[],
                "facts":[{"note_path":"synthesis/topic","fact_idx":0,"text":"Runs daily",
                "origin":"prior_note","inferred":false,"source_id":"habits/run"}]}"#,
        )
        .unwrap();
        assert_eq!(trace.facts[0].source_id.as_deref(), Some("habits/run"));
        assert!(!show_notes_section(TraceKind::Note, trace.notes.len()));
        assert!(!trace_is_empty(TraceKind::Note, &trace));
        trace.facts.clear();
        assert!(trace_is_empty(TraceKind::Note, &trace));
        trace.note_sources.push(NoteSource {
            note_path: "synthesis/topic".into(),
            source_path: "habits/run".into(),
            status: NoteSourceStatus::Unavailable,
        });
        assert!(!trace_is_empty(TraceKind::Note, &trace));
        trace.note_sources.clear();
        trace.incomplete = true;
        assert!(!trace_is_empty(TraceKind::Note, &trace));
        trace.truncated = true;
        assert!(!trace_is_empty(TraceKind::Note, &trace));
        trace.incomplete = false;
        assert!(!trace_is_empty(TraceKind::Note, &trace));
    }

    #[test]
    fn same_raw_rows_via_different_notes_have_independent_expansion_keys() {
        let first = EvidenceItem {
            raw_id: "raw-1".into(),
            via_note: Some("habits/run".into()),
            via_session: None,
            content: Some("Runs daily".into()),
            pruned: false,
        };
        let mut second = first.clone();
        second.via_note = Some("synthesis/topic".into());
        let opened = Some(EvidenceKey::from(&first));
        assert!(opened.as_ref() == Some(&EvidenceKey::from(&first)));
        assert!(opened.as_ref() != Some(&EvidenceKey::from(&second)));
    }

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

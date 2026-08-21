//! The correction queue: what the user told the agent it got wrong, and
//! whether the dream daemon has distilled it into the feedback tier yet.
//!
//! `memory.list_corrections` shipped complete — handler, partition gate,
//! watermark-derived status, and a client wrapper — with **zero callers**
//! anywhere in the Panel. A capability with no surface is not delivered, and
//! this is the one that closes the correction→feedback loop for the person who
//! raised the correction: without it, "I told it that was wrong" and "it has
//! actually learned that" look identical from every client.
//!
//! It sits above the Feedback facet because that is where the distilled end
//! of the same pipeline lands — pending rows here become notes there.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use crate::api::{CorrectionRow, MemoryApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// How many corrections to fetch. This is a governance read ("did my last
/// few corrections land?"), not an archive — the section lives under a fold
/// and an unbounded fetch would grow with the agent's whole lifetime.
const CORRECTIONS_LIMIT: usize = 25;

/// Collapsible correction queue, rendered above the Feedback facet's list.
#[component]
pub fn CorrectionQueue(
    state: DashboardState,
    agent: Signal<String>,
    refresh_nonce: RwSignal<u32>,
) -> impl IntoView {
    let i18n = use_i18n();
    let rows = RwSignal::new(Loadable::<Vec<CorrectionRow>>::Loading);
    let open = RwSignal::new(false);

    Effect::new(move || {
        refresh_nonce.get();
        if !open.get() {
            return;
        }
        let agent = agent.get();
        if !state.is_connected.get() {
            return;
        }
        rows.set(Loadable::Loading);
        spawn_local(async move {
            let res = MemoryApi::list_corrections(&state, &agent, CORRECTIONS_LIMIT).await;
            rows.set(Loadable::from_rpc(res));
        });
    });

    view! {
        <div class="rounded-lg border border-border-subtle bg-surface-raised px-3 py-2">
            <button
                class="flex items-center gap-1.5 text-xs text-text-secondary hover:text-text-primary w-full"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <span class="font-mono">{move || if open.get() { "▾" } else { "▸" }}</span>
                {t!(i18n, memory.corrections)}
            </button>
            <Show when=move || open.get()>
                <p class="text-[11px] text-text-tertiary pt-1 pb-2">
                    {t!(i18n, memory.corrections_hint)}
                </p>
                {move || match rows.get() {
                    Loadable::Loading => view! {
                        <div class="h-8 rounded bg-surface-sunken animate-pulse"></div>
                    }.into_any(),
                    // A failed read says so. Folding it into "no corrections"
                    // would tell the user their corrections were never
                    // recorded — the most alarming possible false statement
                    // for this particular list.
                    Loadable::Failed(e) => view! {
                        <p class="text-xs font-mono text-error break-words">{e}</p>
                    }.into_any(),
                    Loadable::Ready(list) if list.is_empty() => view! {
                        <p class="text-xs text-text-tertiary">{t!(i18n, memory.corrections_empty)}</p>
                    }.into_any(),
                    Loadable::Ready(list) => view! {
                        <ul class="space-y-1.5">
                            {list.into_iter().map(|row| {
                                let distilled = row.status == "distilled";
                                let severity = row.severity.clone();
                                view! {
                                    <li class="flex items-start gap-2 text-xs">
                                        <span
                                            class=if distilled {
                                                "px-1.5 py-0.5 rounded text-[10px] bg-success-subtle text-success flex-shrink-0"
                                            } else {
                                                "px-1.5 py-0.5 rounded text-[10px] bg-warning-subtle text-warning flex-shrink-0"
                                            }
                                        >
                                            {move || if distilled {
                                                t_string!(i18n, memory.correction_distilled).to_string()
                                            } else {
                                                t_string!(i18n, memory.correction_pending).to_string()
                                            }}
                                        </span>
                                        <span class="text-[10px] uppercase font-mono text-text-tertiary flex-shrink-0 w-12">
                                            {severity}
                                        </span>
                                        <span class="text-text-secondary break-words">{row.content}</span>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any(),
                }}
            </Show>
        </div>
    }
}

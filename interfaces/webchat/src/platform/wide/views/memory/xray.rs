//! Retrieval x-ray: what the agent's recall pipeline did with a query.
//!
//! The console can already show what is *stored*. This shows what is
//! *retrieved* — which candidates entered each stage, how many each one
//! dropped, and what survived into the prompt. Recall failures ("it should
//! have remembered that") are otherwise unfalsifiable from any client: the
//! note is right there in the list, and nothing says the retriever never
//! reached it.
//!
//! The server has answered `memory.retrieve_with_trace` with exactly this
//! telemetry all along; its only consumer was the Settings debug panel, which
//! could not even name an agent (it always probed the default one).
//!
//! ## Why in→out per stage, and not a score breakdown
//!
//! `StageTrace` carries `{name, duration_ms, input_count, output_count}`. The
//! drop count is `input - output`, so the funnel is derivable and honest. It
//! deliberately does NOT invent a reason for each drop: the server does not
//! send one, and labelling "rerank dropped 40" as "below threshold" would be
//! the panel making up a vocabulary the retriever does not have.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use crate::api::{MemoryConfigApi, RetrieveWithTraceResponse};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};

/// How many results to ask the retriever for. Matches the injection-time
/// working set closely enough that the funnel describes a realistic run
/// rather than an artificially wide one.
const XRAY_LIMIT: usize = 10;

/// Collapsible x-ray panel, opened from the console toolbar.
#[component]
pub fn RetrievalXray(
    state: DashboardState,
    agent: Signal<String>,
    /// The console's search box content — the x-ray probes the same query the
    /// user is already asking about, instead of asking them to type it twice.
    query: Signal<String>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let result = RwSignal::new(None::<Loadable<RetrieveWithTraceResponse>>);

    let run = move || {
        let q = query.get_untracked();
        if q.trim().is_empty() {
            return;
        }
        let agent = agent.get_untracked();
        result.set(Some(Loadable::Loading));
        spawn_local(async move {
            // Scoped to the agent the console is showing. The Settings twin
            // sends no agent at all and therefore always x-rays the default
            // one — a trace of a different population than the rows on screen.
            let res = MemoryConfigApi::retrieve_with_trace(&state, Some(&agent), &q, XRAY_LIMIT)
                .await;
            let _ = result.try_set(Some(Loadable::from_rpc(res)));
        });
    };

    view! {
        <Show when=move || open.get()>
            <div class="rounded-lg border border-border-subtle bg-surface-raised p-3 space-y-2">
                <div class="flex items-center gap-3">
                    <span class="text-xs font-medium text-text-secondary">
                        {t!(i18n, memory.xray)}
                    </span>
                    <span class="flex-1"></span>
                    <button
                        class="text-xs text-primary hover:underline disabled:opacity-50"
                        prop:disabled=move || query.get().trim().is_empty()
                        on:click=move |_| run()
                    >
                        {t!(i18n, memory.xray_run)}
                    </button>
                </div>
                <p class="text-[11px] text-text-tertiary">{t!(i18n, memory.xray_hint)}</p>

                {move || match result.get() {
                    // Never run: say so rather than render an empty funnel,
                    // which would read as "the retriever found nothing".
                    None => view! {
                        <p class="text-xs text-text-tertiary">{t!(i18n, memory.xray_idle)}</p>
                    }.into_any(),
                    Some(Loadable::Loading) => view! {
                        <div class="h-10 rounded bg-surface-sunken animate-pulse"></div>
                    }.into_any(),
                    Some(Loadable::Failed(e)) => view! {
                        <p class="text-xs font-mono text-error break-words">{e}</p>
                    }.into_any(),
                    Some(Loadable::Ready(resp)) => {
                        let stages = resp.trace.stages.clone();
                        let results = resp.results.clone();
                        view! {
                            <Funnel stages=stages />
                            <div class="pt-1 space-y-1">
                                {if results.is_empty() {
                                    view! {
                                        <p class="text-xs text-text-tertiary">
                                            {t!(i18n, memory.xray_no_results)}
                                        </p>
                                    }.into_any()
                                } else {
                                    view! {
                                        <ul class="space-y-1">
                                            {results.into_iter().map(|r| view! {
                                                <li class="flex items-baseline gap-2 text-xs">
                                                    <span class="font-mono text-[10px] text-primary tabular-nums flex-shrink-0">
                                                        {format!("{:.3}", r.score)}
                                                    </span>
                                                    <span class="font-mono text-[10px] text-text-tertiary flex-shrink-0 truncate max-w-[12rem]">
                                                        {r.id}
                                                    </span>
                                                    <span class="text-text-secondary truncate">{r.content}</span>
                                                </li>
                                            }).collect_view()}
                                        </ul>
                                    }.into_any()
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </Show>
    }
}

/// The widest working set any stage touched, used as the bar scale.
///
/// Taken across BOTH counts of every stage rather than assumed to be the first
/// stage's input: a stage that *adds* candidates (a union with the org tier, a
/// query expansion) is legal, and scaling to the first input would push its bar
/// past 100%. Never zero, so the caller's division is always safe.
#[must_use]
fn funnel_scale(stages: &[crate::api::TraceStage]) -> usize {
    stages
        .iter()
        .flat_map(|s| [s.input_count, s.output_count])
        .max()
        .unwrap_or(1)
        .max(1)
}

/// The stage funnel: one row per stage, each showing what entered, what left,
/// and what that cost.
#[component]
fn Funnel(stages: Vec<crate::api::TraceStage>) -> impl IntoView {
    let i18n = use_i18n();
    if stages.is_empty() {
        // A traced run with no stages means the retriever short-circuited
        // (no embedder, empty corpus). "No stages" is the finding, not an
        // empty section.
        return view! {
            <p class="text-xs text-text-tertiary">{t!(i18n, memory.xray_no_stages)}</p>
        }
        .into_any();
    }
    let widest = funnel_scale(&stages);

    view! {
        <ul class="space-y-1">
            {stages.into_iter().map(|stage| {
                let dropped = stage.input_count.saturating_sub(stage.output_count);
                let width = (stage.output_count * 100 / widest).max(1);
                view! {
                    <li class="space-y-0.5">
                        <div class="flex items-baseline gap-2 text-[11px]">
                            <span class="font-mono text-text-secondary w-28 flex-shrink-0 truncate">
                                {stage.name}
                            </span>
                            <span class="font-mono text-text-tertiary tabular-nums">
                                {stage.input_count}" → "{stage.output_count}
                            </span>
                            {(dropped > 0).then(|| view! {
                                <span class="font-mono text-warning tabular-nums">
                                    {format!("-{dropped}")}
                                </span>
                            })}
                            <span class="flex-1"></span>
                            <span class="font-mono text-text-tertiary tabular-nums">
                                {stage.duration_ms}"ms"
                            </span>
                        </div>
                        <div class="h-1 w-full rounded-full bg-surface-sunken overflow-hidden">
                            <div
                                class="h-full bg-primary/60"
                                style=format!("width: {width}%")
                            ></div>
                        </div>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::funnel_scale;
    use crate::api::TraceStage;

    fn stage(input: usize, output: usize) -> TraceStage {
        TraceStage {
            name: "s".into(),
            duration_ms: 0,
            input_count: input,
            output_count: output,
            scores: Vec::new(),
        }
    }

    #[test]
    fn scale_is_the_widest_working_set_across_all_stages() {
        let stages = [stage(100, 40), stage(40, 5)];
        assert_eq!(funnel_scale(&stages), 100);
    }

    #[test]
    fn a_stage_that_adds_candidates_does_not_overflow_the_bar() {
        // Scaling to the first stage's input (12) would make the expansion's
        // 80 render as a 666%-wide bar. Widening for it is the whole reason
        // the scale looks at every count.
        let stages = [stage(12, 12), stage(12, 80), stage(80, 6)];
        assert_eq!(funnel_scale(&stages), 80);
        for s in &stages {
            assert!(s.output_count * 100 / funnel_scale(&stages) <= 100);
        }
    }

    #[test]
    fn an_all_zero_trace_still_yields_a_usable_divisor() {
        // A retriever that saw nothing must not divide by zero on the way to
        // saying so.
        assert_eq!(funnel_scale(&[stage(0, 0)]), 1);
        assert_eq!(funnel_scale(&[]), 1);
    }
}

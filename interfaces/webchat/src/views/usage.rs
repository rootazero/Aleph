//! `views/usage.rs` — combined Lanes + per-team token usage panel.
//!
//! Consumes two existing read-only RPCs that previously had no UI:
//! - `gateway.metrics.lanes` (saturation gauge, 4 lanes)
//! - `teams.usage`           (F5 per-team token aggregation)
//!
//! Pure I/O per R4: zero business logic. Polls on a 5 s wall-clock tick
//! (lane gauge is eventually consistent so a slow tick is safe and avoids
//! event-bus chatter for a low-priority dashboard).

use crate::api::system::{LaneOccupancy, SystemApi};
use crate::api::teams::{TeamSummary, TeamUsageDto, TeamsApi};
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let first = bytes.len() % 3;
    for (i, b) in bytes.iter().enumerate() {
        if (i != 0 && (i.saturating_sub(first)) % 3 == 0 && i >= first && first != 0)
            || (first == 0 && i != 0 && i % 3 == 0)
        {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[component]
pub fn UsageView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Lane gauge state — `None` until the first poll completes.
    let lanes = RwSignal::new(None::<Vec<LaneOccupancy>>);

    // Teams list + selected team + that team's usage rollup.
    let teams = RwSignal::new(Vec::<TeamSummary>::new());
    let selected_team = RwSignal::new(None::<String>);
    let team_usage = RwSignal::new(None::<TeamUsageDto>);
    let usage_error = RwSignal::new(None::<String>);

    // Initial fetch (and refetch on (re)connection).
    Effect::new(move || {
        if !state.is_connected.get() {
            lanes.set(None);
            teams.set(Vec::new());
            team_usage.set(None);
            usage_error.set(None);
            selected_team.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(v) = SystemApi::lane_metrics(&state).await {
                lanes.set(Some(v));
            }
            if let Ok(list) = TeamsApi::list(&state).await {
                // Default-select the first team so the per-team panel has
                // something to render without an extra click.
                if selected_team.get_untracked().is_none() {
                    selected_team.set(list.first().map(|t| t.id.clone()));
                }
                teams.set(list);
            }
        });
    });

    // Whenever `selected_team` changes, refetch its usage rollup.
    Effect::new(move || {
        let Some(tid) = selected_team.get() else {
            team_usage.set(None);
            return;
        };
        if !state.is_connected.get_untracked() {
            return;
        }
        let tid_async = tid.clone();
        leptos::task::spawn_local(async move {
            match TeamsApi::usage(&state, &tid_async, None, None).await {
                Ok(u) => {
                    team_usage.set(Some(u));
                    usage_error.set(None);
                }
                Err(e) => {
                    team_usage.set(None);
                    usage_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-10">
            <header>
                <h2 class="text-3xl font-bold tracking-tight mb-2">{t!(i18n, usage.header_title)}</h2>
                <p class="text-text-secondary">
                    {t!(i18n, usage.header_description)}
                </p>
            </header>

            // ── Lane gauge ────────────────────────────────────────────────────
            <section class="space-y-4">
                <h3 class="text-xl font-semibold text-text-secondary px-1">{t!(i18n, usage.lane_saturation)}</h3>
                {move || {
                    if !state.is_connected.get() {
                        view! {
                            <Card class="p-6">
                                <p class="text-sm text-text-tertiary">{t!(i18n, usage.connect_to_view_lanes)}</p>
                            </Card>
                        }.into_any()
                    } else if let Some(rows) = lanes.get() {
                        if rows.is_empty() {
                            view! {
                                <Card class="p-6">
                                    <p class="text-sm text-text-tertiary">{t!(i18n, usage.no_lane_data)}</p>
                                </Card>
                            }.into_any()
                        } else {
                            view! {
                                <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
                                    {rows.into_iter().map(|row| view! { <LaneCard row=row /> }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    } else {
                        view! {
                            <Card class="p-6">
                                <p class="text-sm text-text-tertiary">{t!(i18n, usage.loading_lane_metrics)}</p>
                            </Card>
                        }.into_any()
                    }
                }}
            </section>

            // ── Per-team token usage ──────────────────────────────────────────
            <section class="space-y-4">
                <div class="flex items-end justify-between gap-4 flex-wrap">
                    <div>
                        <h3 class="text-xl font-semibold text-text-secondary px-1">{t!(i18n, usage.per_team_token_usage)}</h3>
                        <p class="text-xs text-text-tertiary px-1 mt-1">{t!(i18n, usage.per_team_hint)}</p>
                    </div>
                    <select
                        class="bg-surface-raised border border-border rounded-lg px-3 py-2 text-sm"
                        on:change=move |ev| {
                            let v = event_target_value(&ev);
                            selected_team.set(if v.is_empty() { None } else { Some(v) });
                        }
                        prop:value=move || selected_team.get().unwrap_or_default()
                    >
                        <option value="" disabled=true>{t!(i18n, usage.select_a_team)}</option>
                        {move || teams.get().into_iter().map(|t| {
                            view! {
                                <option value=t.id.clone()>{t.name}</option>
                            }
                        }).collect_view()}
                    </select>
                </div>

                {move || {
                    if !state.is_connected.get() {
                        view! { <Card class="p-6"><p class="text-sm text-text-tertiary">{t!(i18n, usage.connect_to_view_usage)}</p></Card> }.into_any()
                    } else if let Some(err) = usage_error.get() {
                        view! {
                            <Card class="p-6">
                                <p class="text-sm text-danger">{format!("{}{}", t_string!(i18n, usage.error_prefix), err)}</p>
                            </Card>
                        }.into_any()
                    } else if let Some(usage) = team_usage.get() {
                        view! { <UsagePanel usage=usage /> }.into_any()
                    } else if selected_team.get().is_some() {
                        view! { <Card class="p-6"><p class="text-sm text-text-tertiary">{t!(i18n, usage.loading)}</p></Card> }.into_any()
                    } else {
                        view! { <Card class="p-6"><p class="text-sm text-text-tertiary">{t!(i18n, usage.pick_a_team)}</p></Card> }.into_any()
                    }
                }}
            </section>
        </div>
    }
}

#[component]
fn LaneCard(row: LaneOccupancy) -> impl IntoView {
    let i18n = use_i18n();
    let shared_used = row.shared_total.saturating_sub(row.shared_available);
    let shared_pct = (shared_used * 100)
        .checked_div(row.shared_total)
        .unwrap_or(0)
        .min(100);
    let desktop_split = match (row.desktop_total, row.desktop_available) {
        (Some(total), Some(avail)) => Some((total, total.saturating_sub(avail))),
        _ => None,
    };
    let band_color = if shared_pct >= 90 {
        "bg-danger"
    } else if shared_pct >= 70 {
        "bg-warning"
    } else {
        "bg-success"
    };
    view! {
        <Card class="p-5 space-y-3">
            <div class="flex items-center justify-between">
                <span class="font-semibold text-text-primary text-sm">{row.lane}</span>
                <span class="text-xs font-mono text-text-tertiary">{format!("{}/{}", shared_used, row.shared_total)}</span>
            </div>
            <div class="w-full h-2 bg-border rounded-full overflow-hidden">
                <div class=format!("h-full {} transition-all duration-500", band_color) style=format!("width: {}%", shared_pct)></div>
            </div>
            {move || match desktop_split {
                Some((dtotal, dused)) => view! {
                    <div class="text-[10px] text-text-tertiary uppercase font-bold tracking-wider mt-2">
                        {format!("{} {}/{}", t_string!(i18n, usage.desktop_reserve), dused, dtotal)}
                    </div>
                }.into_any(),
                None => view! { <div></div> }.into_any(),
            }}
        </Card>
    }
}

#[component]
fn UsagePanel(usage: TeamUsageDto) -> impl IntoView {
    let i18n = use_i18n();
    let total = usage.total.clone();
    let per_agent = usage.per_agent.clone();
    let member_count = usage.member_count;
    let grand_total = total
        .input_tokens
        .saturating_add(total.output_tokens)
        .saturating_add(total.cache_read_tokens)
        .saturating_add(total.cache_creation_tokens)
        .saturating_add(total.reasoning_tokens);

    view! {
        <Card class="p-6 space-y-6">
            // Top stat row
            <div class="grid grid-cols-2 md:grid-cols-5 gap-6">
                <StatTile label={t_string!(i18n, usage.stat_calls).to_string()} value={format_thousands(total.call_count)} />
                <StatTile label={t_string!(i18n, usage.stat_input_tokens).to_string()} value={format_thousands(total.input_tokens)} />
                <StatTile label={t_string!(i18n, usage.stat_output_tokens).to_string()} value={format_thousands(total.output_tokens)} />
                <StatTile label={t_string!(i18n, usage.stat_members).to_string()} value={format_thousands(member_count)} />
                <StatTile
                    label={t_string!(i18n, usage.stat_cache_hit).to_string()}
                    value={total.cache_hit_ratio.map(|r| format!("{:.0}%", r * 100.0)).unwrap_or_else(|| "\u{2014}".to_string())}
                />
            </div>

            // Detail breakdown
            <div class="space-y-3">
                <h4 class="text-sm font-medium text-text-secondary">{t!(i18n, usage.token_breakdown)}</h4>
                <UsageBar label={t_string!(i18n, usage.bar_input).to_string()} value=total.input_tokens whole=grand_total color="bg-primary" />
                <UsageBar label={t_string!(i18n, usage.bar_output).to_string()} value=total.output_tokens whole=grand_total color="bg-success" />
                <UsageBar label={t_string!(i18n, usage.bar_cache_read).to_string()} value=total.cache_read_tokens whole=grand_total color="bg-info" />
                <UsageBar label={t_string!(i18n, usage.bar_cache_creation).to_string()} value=total.cache_creation_tokens whole=grand_total color="bg-warning" />
                <UsageBar label={t_string!(i18n, usage.bar_reasoning).to_string()} value=total.reasoning_tokens whole=grand_total color="bg-primary/60" />
            </div>

            // Per-agent rows
            {if per_agent.is_empty() {
                view! {
                    <div class="text-sm text-text-tertiary border-t border-border pt-4">
                        {t!(i18n, usage.no_per_agent_rows)}
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="border-t border-border pt-4">
                        <h4 class="text-sm font-medium text-text-secondary mb-3">{t!(i18n, usage.per_agent_breakdown)}</h4>
                        <div class="overflow-x-auto">
                            <table class="w-full text-sm">
                                <thead class="text-[10px] uppercase tracking-wider text-text-tertiary border-b border-border">
                                    <tr>
                                        <th class="text-left py-2 px-2 font-medium">{t!(i18n, usage.th_agent)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_calls)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_in)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_out)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_cache_r)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_cache_w)}</th>
                                        <th class="text-right py-2 px-2 font-medium">{t!(i18n, usage.th_reasoning)}</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {per_agent.into_iter().map(|row| view! {
                                        <tr class="border-b border-border/40 hover:bg-surface-sunken">
                                            <td class="py-2 px-2 font-mono text-xs text-text-primary">{row.agent_id}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.call_count)}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.input_tokens)}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.output_tokens)}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.cache_read_tokens)}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.cache_creation_tokens)}</td>
                                            <td class="py-2 px-2 text-right font-mono text-xs">{format_thousands(row.reasoning_tokens)}</td>
                                        </tr>
                                    }).collect_view()}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }.into_any()
            }}
        </Card>
    }
}

#[component]
fn StatTile(label: String, value: String) -> impl IntoView {
    view! {
        <div>
            <div class="text-[10px] uppercase tracking-widest text-text-tertiary font-bold mb-1">{label}</div>
            <div class="text-2xl font-bold tracking-tight font-mono">{value}</div>
        </div>
    }
}

#[component]
fn UsageBar(label: String, value: u64, whole: u64, color: &'static str) -> impl IntoView {
    let pct = if whole > 0 {
        ((value as f64 / whole as f64) * 100.0).round() as u32
    } else {
        0
    };
    view! {
        <div class="space-y-1">
            <div class="flex items-center justify-between text-xs">
                <span class="text-text-secondary">{label}</span>
                <span class="text-text-tertiary font-mono">{format!("{} ({}%)", format_thousands(value), pct)}</span>
            </div>
            <div class="w-full h-1.5 bg-border rounded-full overflow-hidden">
                <div class=format!("h-full {} transition-all", color) style=format!("width: {}%", pct.min(100))></div>
            </div>
        </div>
    }
}

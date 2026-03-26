// Teams Tab — read-only display of teams an agent belongs to

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::teams::TeamsApi;
use crate::i18n::*;

#[component]
pub fn TeamsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let agent_id = StoredValue::new(agent_id);
    let teams = RwSignal::new(Vec::new());
    let is_loading = RwSignal::new(true);

    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = agent_id.get_value();
        spawn_local(async move {
            match TeamsApi::agent_teams(&dash, &id).await {
                Ok(result) => {
                    teams.set(result);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load agent teams: {e}").into());
                }
            }
            is_loading.set(false);
        });
    });

    view! {
        <div class="space-y-6">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">{t!(i18n, common.loading)}</div>
                    }.into_any();
                }

                let team_list = teams.get();

                if team_list.is_empty() {
                    return view! {
                        <div class="bg-surface-raised border border-border rounded-xl p-8 text-center">
                            <p class="text-text-secondary text-sm">{t!(i18n, agents.teams.empty_hint)}</p>
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-3">
                        {team_list.into_iter().map(|team| {
                            let is_active = team.status == "active";
                            view! {
                                <div class="bg-surface-raised border border-border rounded-xl p-4 flex items-center justify-between gap-4">
                                    <div class="flex-1 min-w-0">
                                        <div class="flex items-center gap-2 mb-1">
                                            <span class="font-medium text-text-primary truncate">{team.name.clone()}</span>
                                            {if is_active {
                                                view! {
                                                    <span class="px-2 py-0.5 rounded-full text-xs font-medium bg-success/20 text-success">{t!(i18n, agents.teams.status_active)}</span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <span class="px-2 py-0.5 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary">{t!(i18n, agents.teams.status_disbanded)}</span>
                                                }.into_any()
                                            }}
                                        </div>
                                        <div class="flex items-center gap-4 text-xs text-text-secondary">
                                            <span>{t!(i18n, agents.teams.leader)}{": "}{team.leader_id.clone()}</span>
                                            <span>{team.member_count.to_string()}{" "}{t!(i18n, agents.teams.members)}</span>
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

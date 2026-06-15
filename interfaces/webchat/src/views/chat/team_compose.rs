//! Team compose popover: leader = current active agent, multi-select members
//! from existing agents, "开始" → TeamsApi::create → enter team chat mode.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::agents::AgentsApi;
use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use super::state::{ChatState, MemberStatus, TeamMemberView};

#[component]
#[must_use]
pub fn TeamCompose(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let team_name = RwSignal::new(String::new());
    let selected: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    // Vec<(id, display_name)> — loaded from agents.list on mount
    let agents: RwSignal<Vec<(String, String)>> = RwSignal::new(Vec::new());

    // Load selectable agents on mount (wait until connected)
    let dash = dashboard;
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&dash).await {
                let list = resp
                    .agents
                    .into_iter()
                    .map(|a| {
                        let display = a.name.clone().unwrap_or_else(|| a.id.clone());
                        (a.id, display)
                    })
                    .collect::<Vec<_>>();
                agents.set(list);
            }
        });
    });

    let start = move |_| {
        let leader = chat.agent_id.get_untracked().unwrap_or_default();
        let name = team_name.get_untracked();
        if leader.is_empty() || name.trim().is_empty() {
            return;
        }
        let members: Vec<(String, String)> = selected
            .get_untracked()
            .into_iter()
            .map(|id| (id, "member".to_string()))
            .collect();

        let dash = dashboard;
        spawn_local(async move {
            match TeamsApi::create(&dash, &name, "", &leader, &members).await {
                Ok(team_id) => {
                    chat.clear_session();
                    chat.team_id.set(Some(team_id));
                    // Seed the roster with leader + selected members
                    let mut roster = vec![TeamMemberView {
                        agent_id: leader.clone(),
                        name: leader.clone(),
                        role: "leader".into(),
                        is_leader: true,
                        status: MemberStatus::Idle,
                    }];
                    for (id, role) in &members {
                        roster.push(TeamMemberView {
                            agent_id: id.clone(),
                            name: id.clone(),
                            role: role.clone(),
                            is_leader: false,
                            status: MemberStatus::Idle,
                        });
                    }
                    chat.team_members.set(roster);
                    on_close.run(());
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("teams.create failed: {e}").into(),
                    );
                }
            }
        });
    };

    view! {
        <div class="aleph-team-compose p-3 space-y-2">
            <h3 class="text-sm font-semibold">"新建团队群聊"</h3>
            <p class="text-xs text-text-tertiary">
                "Leader（当前 agent）= "
                {move || chat.agent_id.get().unwrap_or_default()}
            </p>
            <input
                class="w-full px-2 py-1 rounded bg-surface-sunken border border-border text-sm"
                placeholder="团队名称"
                on:input=move |e| team_name.set(event_target_value(&e))
            />
            // Member checklist — excludes the leader (already pre-filled above)
            <div class="max-h-48 overflow-y-auto space-y-1">
                {move || {
                    // Tracked read: re-filters the list if the active agent
                    // (leader) changes while the popover is open.
                    let leader = chat.agent_id.get().unwrap_or_default();
                    agents
                        .get()
                        .into_iter()
                        .filter(|(id, _)| *id != leader)
                        .map(|(id, label)| {
                            let id_for_toggle = id.clone();
                            let id_for_change = id.clone();
                            let checked = move || selected.get().contains(&id_for_toggle);
                            view! {
                                <label class="flex items-center gap-2 text-sm cursor-pointer">
                                    <input
                                        type="checkbox"
                                        prop:checked=checked
                                        on:change=move |_| selected.update(|s| {
                                            if let Some(pos) = s.iter().position(|x| x == &id_for_change) {
                                                s.remove(pos);
                                            } else {
                                                s.push(id_for_change.clone());
                                            }
                                        })
                                    />
                                    {label}
                                </label>
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>
            <button
                class="w-full px-3 py-1.5 rounded bg-primary text-white text-sm hover:bg-primary/90 transition-colors"
                on:click=start
            >
                "开始"
            </button>
        </div>
    }
}

//! Team compose popover: leader = current active agent, multi-select members
//! from existing agents, "Start" → TeamsApi::create → enter team chat mode.

use super::state::{ChatState, MemberStatus, TeamMemberView};
use crate::api::agents::AgentsApi;
use crate::api::teams::TeamsApi;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Validation outcome for the team-compose form. Modeled as semantic variants
/// (not localized strings) so this pure check stays host-testable while the
/// view layer owns i18n rendering.
#[derive(Debug, PartialEq, Eq)]
enum TeamComposeError {
    /// No active agent could act as leader.
    EmptyLeader,
    /// No members were selected.
    NoMembers,
}

/// Validate the team-compose form and resolve the chosen team name.
///
/// Pure + host-testable so the "Start button does nothing" silent-failure
/// class cannot regress: a blank team name used to trip an `is_empty()` guard
/// that returned with no feedback. Returns `Ok(Some(name))` for an explicit
/// (trimmed) name, `Ok(None)` when the name is blank and the caller should
/// auto-generate a default, or `Err` for a validation failure to surface.
fn resolve_team_compose(
    leader: &str,
    raw_name: &str,
    member_count: usize,
) -> Result<Option<String>, TeamComposeError> {
    if leader.trim().is_empty() {
        return Err(TeamComposeError::EmptyLeader);
    }
    if member_count == 0 {
        return Err(TeamComposeError::NoMembers);
    }
    let name = raw_name.trim();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name.to_string()))
    }
}

#[component]
#[must_use]
pub fn TeamCompose(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let team_name = RwSignal::new(String::new());
    let selected: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    // Vec<(id, display_name, emoji)> — loaded from agents.list on mount
    let agents: RwSignal<Vec<(String, String, Option<String>)>> = RwSignal::new(Vec::new());
    // Inline validation / failure message (replaces the old silent `return`).
    let error = RwSignal::new(None::<String>);

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
                        (a.id, display, a.emoji)
                    })
                    .collect::<Vec<_>>();
                agents.set(list);
            }
        });
    });

    let start = move |_| {
        let leader = chat.agent_id.get_untracked().unwrap_or_default();
        let members: Vec<(String, String)> = selected
            .get_untracked()
            .into_iter()
            .map(|id| (id, "member".to_string()))
            .collect();
        // Blank → provisional "新群聊" + auto_name=true so the backend replaces
        // it with an LLM topic on the first message (mirrors single chat).
        let (name, auto_name) =
            match resolve_team_compose(&leader, &team_name.get_untracked(), members.len()) {
                Ok(Some(n)) => (n, false),
                Ok(None) => (t_string!(i18n, chat.new_group_chat).to_string(), true),
                Err(e) => {
                    let msg = match e {
                        TeamComposeError::EmptyLeader => t_string!(i18n, chat.team_err_no_leader),
                        TeamComposeError::NoMembers => t_string!(i18n, chat.team_err_no_member),
                    };
                    error.set(Some(msg.to_string()));
                    return;
                }
            };
        error.set(None);

        let dash = dashboard;
        spawn_local(async move {
            match TeamsApi::create(&dash, &name, "", &leader, &members, auto_name).await {
                Ok(team_id) => {
                    chat.clear_session();
                    chat.team_id.set(Some(team_id));
                    // Resolve each agent's display name + emoji from the fetched
                    // agents list so a freshly-composed team's roster reads the
                    // same as a re-opened one (`chat_sidebar::on_open_group`
                    // resolves both). Seeding `name` from the id showed raw
                    // handles like `risk_analyst` in the pills and avatars until
                    // the user left the group and came back.
                    // Post-`.await`: the compose sheet closes on success, and a
                    // navigation during the create RPC disposes it first (see
                    // `crate::disposed_reads`). The team exists either way; only
                    // the local roster seeding is skipped.
                    let Some(agent_list) = agents.try_get_untracked() else {
                        return;
                    };
                    let identity_of = |id: &str| -> (String, Option<String>) {
                        agent_list
                            .iter()
                            .find(|(aid, _, _)| aid == id)
                            .map(|(_, name, emoji)| (name.clone(), emoji.clone()))
                            .unwrap_or_else(|| (id.to_string(), None))
                    };
                    // Seed the roster with leader + selected members
                    let (leader_name, leader_emoji) = identity_of(&leader);
                    let mut roster = vec![TeamMemberView {
                        agent_id: leader.clone(),
                        name: leader_name,
                        emoji: leader_emoji,
                        role: "leader".into(),
                        is_leader: true,
                        status: MemberStatus::Idle,
                    }];
                    for (id, role) in &members {
                        let (name, emoji) = identity_of(id);
                        roster.push(TeamMemberView {
                            agent_id: id.clone(),
                            name,
                            emoji,
                            role: role.clone(),
                            is_leader: false,
                            status: MemberStatus::Idle,
                        });
                    }
                    chat.team_members.set(roster);
                    on_close.run(());
                }
                Err(e) => {
                    error.set(Some(format!(
                        "{}{e}",
                        t_string!(i18n, chat.team_create_failed)
                    )));
                    web_sys::console::error_1(&format!("teams.create failed: {e}").into());
                }
            }
        });
    };

    view! {
        <div class="aleph-team-compose relative z-50 p-3 space-y-2">
            <h3 class="text-sm font-semibold">
                {move || t_string!(i18n, chat.team_new_title).to_string()}
            </h3>
            <p class="text-xs text-text-tertiary">
                {move || format!(
                    "{} = {}",
                    t_string!(i18n, chat.team_leader_label),
                    chat.agent_id.get().unwrap_or_default(),
                )}
            </p>
            <input
                class="w-full px-2 py-1 rounded bg-surface-sunken border border-border text-sm"
                placeholder=move || t_string!(i18n, chat.team_name_placeholder).to_string()
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
                        .filter(|(id, _, _)| *id != leader)
                        .map(|(id, label, _)| {
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
            <div class="flex items-center gap-2">
                <button
                    class="px-3 py-1.5 rounded bg-surface-sunken text-text-secondary text-sm
                           hover:bg-surface-raised transition-colors"
                    on:click=move |_| on_close.run(())
                >
                    {move || t_string!(i18n, common.cancel).to_string()}
                </button>
                <button
                    class="flex-1 px-3 py-1.5 rounded bg-primary text-white text-sm
                           hover:bg-primary/90 transition-colors"
                    on:click=start
                >
                    {move || t_string!(i18n, chat.team_start).to_string()}
                </button>
            </div>
            {move || error.get().map(|msg| view! {
                <p class="text-xs text-red-400">{msg}</p>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_team_compose, TeamComposeError};

    #[test]
    fn blank_name_defers_to_caller_default_instead_of_silently_blocking() {
        // Regression for "select agents, click Start, nothing happens": a blank
        // team name used to trip an is_empty() guard and return with no
        // feedback. Now it yields Ok(None) so the caller auto-generates a name.
        assert_eq!(resolve_team_compose("default", "   ", 2), Ok(None));
    }

    #[test]
    fn explicit_name_is_trimmed_and_kept() {
        assert_eq!(
            resolve_team_compose("default", "  研究小组 ", 1),
            Ok(Some("研究小组".to_string()))
        );
    }

    #[test]
    fn empty_leader_returns_error() {
        assert_eq!(
            resolve_team_compose("", "x", 1),
            Err(TeamComposeError::EmptyLeader)
        );
    }

    #[test]
    fn zero_members_returns_error() {
        assert_eq!(
            resolve_team_compose("default", "x", 0),
            Err(TeamComposeError::NoMembers)
        );
    }
}

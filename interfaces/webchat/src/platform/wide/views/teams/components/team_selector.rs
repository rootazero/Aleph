//! `TeamSelector` — pill + popover bound to `TeamsTabState.selected_team_id`,
//! mirroring the chat composer's `ModelPicker` (`components/model_picker.rs`)
//! so switching between the chat and teams tabs feels consistent.

use crate::api::teams::TeamSummary;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::teams::TeamsTabState;
use leptos::prelude::*;

/// Pill label: the selected team's name, or `placeholder` when nothing is
/// selected or the selected id is no longer present in `teams`.
pub fn current_team_label(
    teams: &[TeamSummary],
    selected_id: Option<&str>,
    placeholder: &str,
) -> String {
    selected_id
        .and_then(|id| teams.iter().find(|t| t.id == id))
        .map(|t| t.name.clone())
        .unwrap_or_else(|| placeholder.to_string())
}

/// Order-preserving, case-insensitive substring filter over team names.
/// An empty/whitespace query returns every team (clone of the input).
pub fn filter_teams(teams: &[TeamSummary], query: &str) -> Vec<TeamSummary> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return teams.to_vec();
    }
    teams
        .iter()
        .filter(|t| t.name.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

#[component]
#[must_use]
pub fn TeamSelector() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();
    let open = RwSignal::new(false);
    let search = RwSignal::new(String::new());

    // Reset the filter whenever the popover closes (mirrors ModelPicker).
    Effect::new(move |_| {
        if !open.get() {
            search.set(String::new());
        }
    });

    let trigger_label = move || -> String {
        let placeholder = t_string!(i18n, teams.kanban.selector_placeholder).to_string();
        current_team_label(
            &state.teams.get(),
            state.selected_team_id.get().as_deref(),
            &placeholder,
        )
    };

    let select_team = move |id: String| {
        state.selected_team_id.set(Some(id));
        open.set(false);
    };

    view! {
        <div class="relative">
            <button
                on:click=move |_| open.update(|v| *v = !*v)
                class="w-full flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono \
                       text-text-secondary border border-border \
                       bg-surface-raised hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, teams.kanban.selector_label).to_string()
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                </svg>
                <span class="flex-1 text-left truncate">{trigger_label}</span>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            <Show when=move || open.get()>
                <div class="absolute top-full mt-2 left-0 z-50 w-full max-h-80 overflow-y-auto \
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl \
                            p-2 space-y-1"
                    on:mouseleave=move |_| open.set(false)>

                    {move || (!state.teams.get().is_empty()).then(|| view! {
                        <input
                            type="text"
                            placeholder=move || {
                                t_string!(i18n, teams.kanban.filter_placeholder).to_string()
                            }
                            class="w-full px-2.5 py-1.5 mb-1 rounded-md text-xs bg-surface-sunken \
                                   text-text-primary placeholder:text-text-tertiary outline-none \
                                   border border-border focus:border-primary/40"
                            on:input=move |ev| search.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Escape" {
                                    open.set(false);
                                }
                            }
                            prop:value=move || search.get()
                        />
                    })}

                    {move || {
                        let teams = state.teams.get();
                        if teams.is_empty() {
                            return view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, teams.kanban.empty_teams)}
                                </div>
                            }.into_any();
                        }
                        if filter_teams(&teams, &search.get()).is_empty() {
                            return view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, teams.kanban.no_match)}
                                </div>
                            }.into_any();
                        }
                        view! {
                            <For
                                each=move || filter_teams(&state.teams.get(), &search.get())
                                key=|t: &TeamSummary| t.id.clone()
                                children=move |team: TeamSummary| {
                                    let id = team.id.clone();
                                    let id_active = id.clone();
                                    let name = team.name.clone();
                                    let is_active_team = team.status == "active";
                                    let is_selected = move || {
                                        state.selected_team_id.get().as_deref()
                                            == Some(id_active.as_str())
                                    };
                                    view! {
                                        <button
                                            on:mousedown=move |ev| ev.prevent_default()
                                            on:click=move |_| select_team(id.clone())
                                            class=move || {
                                                let base = "w-full text-left px-2.5 py-1.5 rounded-md \
                                                            text-xs transition-colors flex items-center \
                                                            gap-2 border";
                                                if is_selected() {
                                                    format!("{base} bg-primary/10 text-primary border-primary/30")
                                                } else {
                                                    format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                                }
                                            }
                                        >
                                            <span class=move || {
                                                if is_active_team {
                                                    "w-2 h-2 rounded-full bg-success shrink-0"
                                                } else {
                                                    "w-2 h-2 rounded-full bg-text-tertiary shrink-0"
                                                }
                                            } />
                                            <span class="flex-1 truncate">{name}</span>
                                        </button>
                                    }
                                }
                            />
                        }.into_any()
                    }}
                </div>
            </Show>
        </div>
    }
}

fn event_target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|s| s.value())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::teams::TeamSummary;

    fn team(id: &str, name: &str, status: &str) -> TeamSummary {
        TeamSummary {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            leader_id: "main".into(),
            status: status.into(),
            member_count: 0,
            task_count: 0,
            created_at: 0,
            disbanded_at: None,
            members_preview: Vec::new(),
            last_message: None,
            last_message_at: None,
            scope_id: None,
        }
    }

    #[test]
    fn label_returns_selected_team_name() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(current_team_label(&teams, Some("b"), "Choose"), "Beta");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_none_selected() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, None, "Choose"), "Choose");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_id_missing() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, Some("gone"), "Choose"), "Choose");
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(filter_teams(&teams, "   ").len(), 2);
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "disbanded")];
        let got = filter_teams(&teams, "ET");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "b");
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let teams = vec![team("a", "Alpha", "active")];
        assert!(filter_teams(&teams, "zzz").is_empty());
    }
}

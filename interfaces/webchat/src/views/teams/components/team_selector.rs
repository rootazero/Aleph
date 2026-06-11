//! `TeamSelector` — small dropdown bound to `TeamsTabState.selected_team_id`.

use crate::i18n::{t_string, use_i18n, I18nLocaleTrait};
use crate::views::teams::TeamsTabState;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn TeamSelector() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let on_change = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        if value.is_empty() {
            state.selected_team_id.set(None);
        } else {
            state.selected_team_id.set(Some(value));
        }
    };

    view! {
        <label class="block text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
            {move || t_string!(i18n, teams.kanban.selector_label).to_string()}
        </label>
        <select
            on:change=on_change
            class="w-full px-2 py-1.5 rounded bg-surface border border-border text-sm text-text-primary"
        >
            <option value="" selected=move || state.selected_team_id.get().is_none()>
                {move || t_string!(i18n, teams.kanban.selector_placeholder).to_string()}
            </option>
            {move || {
                state.teams.get().into_iter().map(|t| {
                    let selected = state.selected_team_id.get() == Some(t.id.clone());
                    let id = t.id.clone();
                    let name = t.name;
                    view! {
                        <option value=id selected=selected>{name}</option>
                    }
                }).collect_view()
            }}
        </select>
    }
}

fn event_target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
        .map(|s| s.value())
        .unwrap_or_default()
}

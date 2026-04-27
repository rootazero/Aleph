use crate::api::agents::AgentSummary;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Format one dropdown option label.
///
/// Rules:
/// - emoji + name + " (id)" when both present, e.g. "🤖 Main (main)"
/// - id-only fallback when name is missing, e.g. "worker-3 (worker-3)"
/// - no leading emoji when emoji is missing
/// - default agent gets a trailing " ★"
pub fn format_agent_option(agent: &AgentSummary, is_default: bool) -> String {
    let name = agent.name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&agent.id);
    let star = if is_default { " ★" } else { "" };
    match agent.emoji.as_deref().filter(|s| !s.is_empty()) {
        Some(emoji) => format!("{emoji} {name} ({}){star}", agent.id),
        None => format!("{name} ({}){star}", agent.id),
    }
}

#[component]
pub fn AgentSelectorBar(
    agent_id: RwSignal<String>,
    agents: RwSignal<Vec<AgentSummary>>,
    default_agent_id: RwSignal<String>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refresh: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let on_change = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlSelectElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
        if let Some(select) = target {
            agent_id.set(select.value());
        }
    };

    let on_refresh_click = move |_| on_refresh();

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-surface-raised border-b border-border">
            <div class="text-sm font-medium text-text-secondary">"Agent"</div>

            {move || {
                if loading.get() {
                    view! {
                        <span class="text-sm text-text-tertiary">"Loading agents…"</span>
                    }.into_any()
                } else if let Some(msg) = error.get() {
                    view! {
                        <span class="text-sm text-error">
                            {format!("Failed to load agents: {msg} · ")}
                            <button class="underline" on:click=on_refresh_click>"Retry"</button>
                        </span>
                    }.into_any()
                } else {
                    view! {
                        <select
                            class="px-3 py-1.5 text-sm bg-surface-sunken border border-border rounded-lg
                                   text-text-primary focus:outline-none focus:border-primary/50"
                            on:change=on_change
                            prop:value=move || agent_id.get()
                        >
                            {move || {
                                let current = agent_id.get();
                                let default = default_agent_id.get();
                                agents.get().iter().map(|a| {
                                    let label = format_agent_option(a, a.id == default);
                                    let id = a.id.clone();
                                    let selected = id == current;
                                    view! {
                                        <option value=id.clone() selected=selected>{label}</option>
                                    }
                                }).collect_view()
                            }}
                        </select>
                    }.into_any()
                }
            }}

            <button
                class="px-2 py-1 text-sm text-text-secondary hover:text-text-primary"
                title="Refresh agent list"
                on:click=on_refresh_click
            >
                "↻"
            </button>

            <div class="flex-1" />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::agents::AgentSummary;

    fn agent(id: &str, name: Option<&str>, emoji: Option<&str>) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: name.map(String::from),
            emoji: emoji.map(String::from),
            description: None,
            model: None,
            is_default: false,
        }
    }

    #[test]
    fn formats_emoji_name_id_when_all_present() {
        let a = agent("main", Some("Main"), Some("🤖"));
        assert_eq!(format_agent_option(&a, false), "🤖 Main (main)");
    }

    #[test]
    fn falls_back_to_id_when_name_missing() {
        let a = agent("worker-3", None, None);
        assert_eq!(format_agent_option(&a, false), "worker-3 (worker-3)");
    }

    #[test]
    fn omits_emoji_prefix_when_emoji_missing() {
        let a = agent("fox", Some("fox-research"), None);
        assert_eq!(format_agent_option(&a, false), "fox-research (fox)");
    }

    #[test]
    fn appends_star_for_default_agent() {
        let a = agent("main", Some("Main"), Some("🤖"));
        assert_eq!(format_agent_option(&a, true), "🤖 Main (main) ★");
    }

    #[test]
    fn empty_string_emoji_is_treated_as_missing() {
        let a = agent("main", Some("Main"), Some(""));
        assert_eq!(format_agent_option(&a, false), "Main (main)");
    }

    #[test]
    fn empty_string_name_is_treated_as_missing() {
        let a = agent("main", Some(""), None);
        assert_eq!(format_agent_option(&a, false), "main (main)");
    }
}

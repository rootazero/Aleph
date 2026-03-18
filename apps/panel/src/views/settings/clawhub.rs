use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::DashboardState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClawHubSkill {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub stars: u64,
    #[serde(default)]
    pub owner_handle: String,
}

/// Format a download count for display (e.g. 1234 -> "1.2k")
fn format_count(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Load browse results from ClawHub
fn load_browse(
    state: DashboardState,
    skills: RwSignal<Vec<ClawHubSkill>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    cursor: RwSignal<Option<String>>,
    has_more: RwSignal<bool>,
    append: bool,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        let mut params = json!({ "sort": "downloads", "limit": 20 });
        if append {
            if let Some(c) = cursor.get_untracked() {
                params.as_object_mut().unwrap().insert("cursor".into(), serde_json::Value::String(c));
            }
        }
        match state.rpc_call("clawhub.browse", params).await {
            Ok(result) => {
                if let Some(list) = result.get("skills") {
                    if let Ok(parsed) = serde_json::from_value::<Vec<ClawHubSkill>>(list.clone()) {
                        if append {
                            skills.update(|current| current.extend(parsed));
                        } else {
                            skills.set(parsed);
                        }
                    }
                }
                cursor.set(result.get("cursor").and_then(|v| v.as_str()).map(String::from));
                has_more.set(result.get("hasMore").and_then(|v| v.as_bool()).unwrap_or(false));
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load skills: {}", e)));
                loading.set(false);
            }
        }
    });
}

/// Load search results from ClawHub
fn load_search(
    state: DashboardState,
    skills: RwSignal<Vec<ClawHubSkill>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    query: String,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match state.rpc_call("clawhub.search", json!({ "query": query })).await {
            Ok(result) => {
                if let Some(list) = result.get("skills") {
                    if let Ok(parsed) = serde_json::from_value::<Vec<ClawHubSkill>>(list.clone()) {
                        skills.set(parsed);
                    }
                }
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Search failed: {}", e)));
                loading.set(false);
            }
        }
    });
}

/// Load installed skill IDs from the local skills.list
fn load_installed_slugs(state: DashboardState, installed_slugs: RwSignal<Vec<String>>) {
    spawn_local(async move {
        if let Ok(result) = state.rpc_call("skills.list", json!({})).await {
            if let Some(list) = result.get("skills") {
                if let Ok(parsed) = serde_json::from_value::<Vec<serde_json::Value>>(list.clone()) {
                    let ids: Vec<String> = parsed
                        .into_iter()
                        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from))
                        .collect();
                    installed_slugs.set(ids);
                }
            }
        }
    });
}

#[component]
pub fn ClawHubView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let skills = RwSignal::new(Vec::<ClawHubSkill>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let search_query = RwSignal::new(String::new());
    let cursor = RwSignal::new(Option::<String>::None);
    let has_more = RwSignal::new(false);
    let installed_slugs = RwSignal::new(Vec::<String>::new());
    let is_searching = RwSignal::new(false);

    // Load initial data when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_browse(state, skills, loading, error, cursor, has_more, false);
            load_installed_slugs(state, installed_slugs);
        } else {
            loading.set(false);
        }
    });

    // Search handler
    let on_search_input = move |ev: leptos::ev::Event| {
        let query = event_target_value(&ev);
        search_query.set(query.clone());
        let query = query.trim().to_string();
        if query.is_empty() {
            is_searching.set(false);
            cursor.set(None);
            has_more.set(false);
            load_browse(state, skills, loading, error, cursor, has_more, false);
        } else {
            is_searching.set(true);
            load_search(state, skills, loading, error, query);
        }
    };

    // Load more handler
    let load_more = move |_| {
        load_browse(state, skills, loading, error, cursor, has_more, true);
    };

    // Refresh handler
    let refresh = move |_| {
        search_query.set(String::new());
        is_searching.set(false);
        cursor.set(None);
        has_more.set(false);
        load_browse(state, skills, loading, error, cursor, has_more, false);
        load_installed_slugs(state, installed_slugs);
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div>
                    <h1 class="text-2xl font-semibold text-text-primary mb-1">
                        "ClawHub Skill Marketplace"
                    </h1>
                    <p class="text-sm text-text-secondary">
                        "Browse and install community skills from ClawHub"
                    </p>
                </div>

                // Search bar + Refresh
                <div class="flex items-center gap-2">
                    <input
                        type="text"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                        placeholder="Search skills..."
                        prop:value=move || search_query.get()
                        on:input=on_search_input
                    />
                    <button
                        class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm flex-shrink-0"
                        on:click=refresh
                    >
                        "Refresh"
                    </button>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Loading state
                {move || {
                    if loading.get() && skills.get().is_empty() {
                        Some(view! {
                            <div class="flex items-center justify-center py-12">
                                <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                // Section header
                <Show when=move || !loading.get() || !skills.get().is_empty()>
                    <div class="flex items-center gap-2">
                        <h2 class="text-lg font-medium text-text-primary">
                            {move || if is_searching.get() { "Search Results" } else { "Hot Skills" }}
                        </h2>
                        <span class="px-2 py-0.5 rounded-full text-xs font-medium bg-primary-subtle text-primary">
                            {move || format!("{}", skills.get().len())}
                        </span>
                    </div>
                </Show>

                // Skills grid
                <Show when=move || !skills.get().is_empty()>
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <For
                            each=move || skills.get()
                            key=|skill| skill.slug.clone()
                            children=move |skill| {
                                view! {
                                    <ClawHubSkillCard
                                        skill=skill
                                        installed_slugs=installed_slugs
                                    />
                                }
                            }
                        />
                    </div>
                </Show>

                // Empty state
                <Show when=move || !loading.get() && skills.get().is_empty()>
                    <div class="text-center py-6 border border-dashed border-border rounded">
                        <p class="text-sm text-text-secondary">"No skills found"</p>
                        <p class="text-xs text-text-tertiary mt-1">
                            {move || if is_searching.get() { "Try a different search query" } else { "Check your connection and try again" }}
                        </p>
                    </div>
                </Show>

                // Load more button
                <Show when=move || has_more.get() && !is_searching.get() && !loading.get()>
                    <div class="flex justify-center">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=load_more
                        >
                            "Load more..."
                        </button>
                    </div>
                </Show>

                // Loading indicator for load-more
                <Show when=move || loading.get() && !skills.get().is_empty()>
                    <div class="flex items-center justify-center py-4">
                        <div class="animate-spin rounded-full h-5 w-5 border-b-2 border-primary"></div>
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn ClawHubSkillCard(
    skill: ClawHubSkill,
    installed_slugs: RwSignal<Vec<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let installing = RwSignal::new(false);
    let install_error = RwSignal::new(Option::<String>::None);
    let slug = StoredValue::new(skill.slug.clone());
    let slug_for_check = skill.slug.clone();

    let is_installed = Memo::new(move |_| {
        installed_slugs.get().contains(&slug_for_check)
    });

    let handle_install = move |_| {
        installing.set(true);
        install_error.set(None);
        let install_slug = slug.get_value();
        spawn_local(async move {
            match state.rpc_call("clawhub.install", json!({ "slug": install_slug })).await {
                Ok(result) => {
                    installing.set(false);
                    // Add to installed list
                    if let Some(installed) = result.get("installedSlug").and_then(|v| v.as_str()) {
                        installed_slugs.update(|list| {
                            if !list.contains(&installed.to_string()) {
                                list.push(installed.to_string());
                            }
                        });
                    } else {
                        // Fallback: add the slug we installed
                        let s = slug.get_value();
                        installed_slugs.update(|list| {
                            if !list.contains(&s) {
                                list.push(s);
                            }
                        });
                    }
                }
                Err(e) => {
                    install_error.set(Some(format!("Install failed: {}", e)));
                    installing.set(false);
                }
            }
        });
    };

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex flex-col gap-2">
                <div class="flex items-start justify-between">
                    <div>
                        <p class="text-sm font-medium text-text-primary">{skill.name.clone()}</p>
                        {(!skill.owner_handle.is_empty()).then(|| view! {
                            <p class="text-xs text-text-tertiary">{format!("by {}", skill.owner_handle)}</p>
                        })}
                    </div>
                    <div>
                        {move || {
                            if is_installed.get() {
                                view! {
                                    <button
                                        class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded text-sm disabled:opacity-50"
                                        disabled=true
                                    >
                                        "Installed"
                                    </button>
                                }.into_any()
                            } else if installing.get() {
                                view! {
                                    <button
                                        class="px-3 py-1.5 bg-primary text-white rounded text-sm disabled:opacity-50"
                                        disabled=true
                                    >
                                        "Installing..."
                                    </button>
                                }.into_any()
                            } else {
                                view! {
                                    <button
                                        class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                                        on:click=handle_install
                                    >
                                        "Install"
                                    </button>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>

                <p class="text-xs text-text-secondary line-clamp-2">{skill.summary.clone()}</p>

                // Tags
                {(!skill.tags.is_empty()).then(|| {
                    let tags = skill.tags.clone();
                    view! {
                        <div class="flex flex-wrap gap-1">
                            {tags.into_iter().map(|tag| view! {
                                <span class="px-1.5 py-0.5 bg-surface-sunken text-text-tertiary rounded text-xs">
                                    {tag}
                                </span>
                            }).collect::<Vec<_>>()}
                        </div>
                    }
                })}

                // Stats + error
                <div class="flex items-center gap-3">
                    <span class="text-xs text-text-tertiary">
                        {format!("↓{}", format_count(skill.downloads))}
                    </span>
                    <span class="text-xs text-text-tertiary">
                        {format!("★{}", format_count(skill.stars))}
                    </span>
                </div>

                {move || install_error.get().map(|err| view! {
                    <p class="text-xs text-danger">{err}</p>
                })}
            </div>
        </div>
    }
}

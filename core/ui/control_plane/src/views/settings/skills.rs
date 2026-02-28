use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::DashboardState;

const OFFICIAL_SKILLS_URL: &str = "https://github.com/rootazero/AlephSkills";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default = "default_ecosystem")]
    pub ecosystem: String,
}

fn default_ecosystem() -> String {
    "aleph".to_string()
}

/// Load skills list from Gateway (with retry for WebSocket timing)
fn load_skills(state: DashboardState, skills: RwSignal<Vec<SkillInfo>>, loading: RwSignal<bool>, error: RwSignal<Option<String>>) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        // Retry up to 3 times with short delays for WebSocket connection timing
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                gloo_timers::future::sleep(std::time::Duration::from_millis(500)).await;
            }
            match state.rpc_call("skills.list", json!({})).await {
                Ok(result) => {
                    if let Some(list) = result.get("skills") {
                        if let Ok(parsed) = serde_json::from_value::<Vec<SkillInfo>>(list.clone()) {
                            skills.set(parsed);
                        }
                    }
                    loading.set(false);
                    return;
                }
                Err(e) => {
                    last_err = e;
                }
            }
        }
        error.set(Some(format!("Failed to load skills: {}", last_err)));
        loading.set(false);
    });
}

#[component]
pub fn SkillsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let skills = RwSignal::new(Vec::<SkillInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_install_dialog = RwSignal::new(false);
    let installing_official = RwSignal::new(false);

    // Derived signals for filtering by ecosystem
    let aleph_skills = Memo::new(move |_| {
        skills.get().into_iter().filter(|s| s.ecosystem == "aleph").collect::<Vec<_>>()
    });
    let claude_skills = Memo::new(move |_| {
        skills.get().into_iter().filter(|s| s.ecosystem == "claude").collect::<Vec<_>>()
    });

    // Load skills when connected
    Effect::new(move || {
        if state.is_connected.get() {
            load_skills(state, skills, loading, error);
        } else {
            loading.set(false);
        }
    });

    // Install official skills handler
    let install_official = move |_| {
        installing_official.set(true);
        error.set(None);
        spawn_local(async move {
            match state.rpc_call("markdown_skills.install", json!({ "url": OFFICIAL_SKILLS_URL, "flatten": true })).await {
                Ok(_) => {
                    installing_official.set(false);
                    load_skills(state, skills, loading, error);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to install official skills: {}", e)));
                    installing_official.set(false);
                }
            }
        });
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary mb-1">
                            "Skills"
                        </h1>
                        <p class="text-sm text-text-secondary">
                            "Install and manage AI skills"
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| {
                                load_skills(state, skills, loading, error);
                            }
                        >
                            "Refresh"
                        </button>
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm disabled:opacity-50"
                            disabled=move || installing_official.get()
                            on:click=install_official
                        >
                            {move || if installing_official.get() { "Installing..." } else { "Install Official Skills" }}
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| show_install_dialog.set(true)
                        >
                            "+ Install Skill"
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Loading state
                {move || {
                    if loading.get() {
                        Some(view! {
                            <div class="flex items-center justify-center py-12">
                                <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                // Aleph Official Skills Section
                <Show when=move || !loading.get()>
                    <SkillSection
                        title="Aleph Skills"
                        icon="A"
                        icon_bg="bg-primary-subtle"
                        icon_color="text-primary"
                        badge_bg="bg-primary-subtle"
                        badge_text="text-primary"
                        description="Native Aleph skills from ~/.aleph/skills"
                        skills_list=aleph_skills
                        all_skills=skills
                        loading=loading
                        error=error
                        empty_text="No Aleph skills installed"
                        empty_hint="Click 'Install Official Skills' to get started"
                    />
                </Show>

                // Claude Compatible Skills Section
                <Show when=move || !loading.get()>
                    <SkillSection
                        title="Claude Skills"
                        icon="C"
                        icon_bg="bg-[#da7756]/10"
                        icon_color="text-[#da7756]"
                        badge_bg="bg-[#da7756]/10"
                        badge_text="text-[#da7756]"
                        description="Claude Code compatible skills from ~/.claude/skills"
                        skills_list=claude_skills
                        all_skills=skills
                        loading=loading
                        error=error
                        empty_text="No Claude skills installed"
                        empty_hint="Add SKILL.md files to ~/.claude/skills/"
                    />
                </Show>

                // Info Box
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-info text-sm">"ℹ️"</span>
                        <div class="text-sm text-info space-y-1">
                            <p>"Skills extend the AI with specialized capabilities."</p>
                            <p>"Aleph skills: ~/.aleph/skills/ | Claude skills: ~/.claude/skills/"</p>
                        </div>
                    </div>
                </div>
            </div>

            // Install Dialog
            <Show when=move || show_install_dialog.get()>
                <InstallSkillDialog
                    on_close=move || show_install_dialog.set(false)
                    skills=skills
                    loading=loading
                    error=error
                />
            </Show>
        </div>
    }
}

#[component]
fn SkillSection(
    title: &'static str,
    icon: &'static str,
    icon_bg: &'static str,
    icon_color: &'static str,
    badge_bg: &'static str,
    badge_text: &'static str,
    description: &'static str,
    skills_list: Memo<Vec<SkillInfo>>,
    all_skills: RwSignal<Vec<SkillInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    empty_text: &'static str,
    empty_hint: &'static str,
) -> impl IntoView {
    view! {
        <div class="space-y-3">
            <div class="flex items-center gap-3">
                <div class={format!("w-7 h-7 rounded flex items-center justify-center flex-shrink-0 font-bold text-sm {} {}", icon_bg, icon_color)}>
                    {icon}
                </div>
                <div class="flex items-center gap-2">
                    <h2 class="text-lg font-medium text-text-primary">{title}</h2>
                    <span class={format!("px-2 py-0.5 rounded-full text-xs font-medium {} {}", badge_bg, badge_text)}>
                        {move || format!("{}", skills_list.get().len())}
                    </span>
                </div>
                <span class="text-xs text-text-tertiary">{description}</span>
            </div>

            {move || {
                let list = skills_list.get();
                if list.is_empty() {
                    view! {
                        <div class="text-center py-6 border border-dashed border-border rounded">
                            <p class="text-sm text-text-secondary">{empty_text}</p>
                            <p class="text-xs text-text-tertiary mt-1">{empty_hint}</p>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-2">
                            <For
                                each=move || skills_list.get()
                                key=|skill| skill.id.clone()
                                children=move |skill| {
                                    view! {
                                        <SkillCard
                                            skill=skill
                                            skills=all_skills
                                            loading=loading
                                            error=error
                                        />
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SkillCard(
    skill: SkillInfo,
    skills: RwSignal<Vec<SkillInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let deleting = RwSignal::new(false);
    let skill_id = StoredValue::new(skill.id.clone());

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-primary-subtle flex items-center justify-center flex-shrink-0">
                        <span class="text-primary">"⚡"</span>
                    </div>
                    <div>
                        <p class="text-sm font-medium text-text-primary">{skill.name.clone()}</p>
                        <p class="text-xs text-text-secondary mt-1">
                            {skill.description.clone().unwrap_or_else(|| "No description".to_string())}
                        </p>
                    </div>
                </div>

                <div class="flex items-center gap-2 flex-shrink-0 ml-4">
                    {move || {
                        if deleting.get() {
                            view! {
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-text-secondary"></div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    class="p-1.5 text-danger hover:bg-danger-subtle rounded"
                                    title="Delete"
                                    on:click=move |_| {
                                        deleting.set(true);
                                        let id = skill_id.get_value();
                                        spawn_local(async move {
                                            match state.rpc_call("skills.delete", json!({ "id": id })).await {
                                                Ok(_) => {
                                                    load_skills(state, skills, loading, error);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to delete skill: {}", e)));
                                                    deleting.set(false);
                                                }
                                            }
                                        });
                                    }
                                >
                                    "🗑️"
                                </button>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn InstallSkillDialog(
    on_close: impl Fn() + 'static + Copy,
    skills: RwSignal<Vec<SkillInfo>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let source = RwSignal::new("git".to_string());
    let url = RwSignal::new(String::new());
    let installing = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);

    let handle_install = move |_| {
        if url.get().trim().is_empty() {
            return;
        }
        installing.set(true);
        dialog_error.set(None);
        let install_url = url.get().trim().to_string();
        spawn_local(async move {
            match state.rpc_call("markdown_skills.install", json!({ "url": install_url })).await {
                Ok(_) => {
                    installing.set(false);
                    load_skills(state, skills, loading, error);
                    on_close();
                }
                Err(e) => {
                    dialog_error.set(Some(format!("Failed to install: {}", e)));
                    installing.set(false);
                }
            }
        });
    };

    view! {
        <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="bg-surface border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">"Install Skill"</h2>
                <p class="text-sm text-text-secondary mb-4">
                    "Install skills from Git repository, ZIP archive, or local folder"
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Source"</label>
                        <select
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            on:change=move |ev| source.set(event_target_value(&ev))
                        >
                            <option value="git">"Git Repository"</option>
                            <option value="zip">"ZIP Archive"</option>
                            <option value="local">"Local Folder"</option>
                        </select>
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">
                            {move || match source.get().as_str() {
                                "git" => "Repository URL",
                                "zip" => "ZIP URL or Path",
                                _ => "Folder Path",
                            }}
                        </label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder=move || match source.get().as_str() {
                                "git" => "https://github.com/user/skills.git",
                                "zip" => "https://example.com/skills.zip",
                                _ => "/path/to/skills",
                            }
                            value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
                    </div>

                    {move || dialog_error.get().map(|err| view! {
                        <div class="flex items-center gap-2 text-danger text-sm">
                            <span>"⚠️"</span>
                            <span>{err}</span>
                        </div>
                    })}
                </div>

                <div class="flex gap-2 mt-6">
                    <button
                        class="flex-1 px-4 py-2 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                        on:click=move |_| on_close()
                    >
                        "Cancel"
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || url.get().trim().is_empty() || installing.get()
                        on:click=handle_install
                    >
                        {move || if installing.get() { "Installing..." } else { "Install" }}
                    </button>
                </div>
            </div>
        </div>
    }
}

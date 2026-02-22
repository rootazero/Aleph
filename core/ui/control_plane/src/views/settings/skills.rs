use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source: Option<String>,
}

#[component]
pub fn SkillsView() -> impl IntoView {
    let skills = RwSignal::new(Vec::<SkillInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let show_install_dialog = RwSignal::new(false);

    // TODO: Load skills from Gateway
    Effect::new(move || {
        loading.set(false);
    });

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
                                loading.set(true);
                                // TODO: Reload skills
                                loading.set(false);
                            }
                        >
                            "Refresh"
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

                // Skills List Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">
                        {move || format!("Installed Skills ({})", skills.get().len())}
                    </h2>

                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                                </div>
                            }.into_any()
                        } else if skills.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-border rounded">
                                    <div class="text-4xl mb-4">"✨"</div>
                                    <p class="text-text-secondary">"No skills installed"</p>
                                    <p class="text-xs text-text-tertiary mt-1">
                                        "Install skills to extend AI capabilities"
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    <For
                                        each=move || skills.get()
                                        key=|skill| skill.id.clone()
                                        children=move |skill| {
                                            view! {
                                                <SkillCard skill=skill />
                                            }
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                // Info Box
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                    <div class="flex items-start gap-2">
                        <span class="text-info text-sm">"ℹ️"</span>
                        <span class="text-sm text-info">
                            "Skills extend the AI with specialized capabilities. Install skills from Git repositories or local folders."
                        </span>
                    </div>
                </div>
            </div>

            // Install Dialog
            <Show when=move || show_install_dialog.get()>
                <InstallSkillDialog
                    on_close=move || show_install_dialog.set(false)
                />
            </Show>
        </div>
    }
}

#[component]
fn SkillCard(skill: SkillInfo) -> impl IntoView {
    let deleting = RwSignal::new(false);

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded">
            <div class="flex items-start justify-between">
                <div class="flex items-start gap-3">
                    <div class="w-10 h-10 rounded bg-primary-subtle flex items-center justify-center flex-shrink-0">
                        <span class="text-primary">"⚡"</span>
                    </div>
                    <div>
                        <p class="text-sm font-medium text-text-primary">{skill.name}</p>
                        <p class="text-xs text-text-secondary mt-1">
                            {skill.description.unwrap_or_else(|| "No description".to_string())}
                        </p>
                        {skill.source.map(|source| view! {
                            <div class="flex items-center gap-1 mt-2">
                                <span class="text-xs text-text-tertiary">"🏷️"</span>
                                <span class="px-2 py-0.5 bg-surface-sunken border border-border rounded text-xs text-text-secondary">
                                    {source}
                                </span>
                            </div>
                        })}
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
                                        // TODO: Call Gateway API
                                        deleting.set(false);
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
fn InstallSkillDialog(on_close: impl Fn() + 'static + Copy) -> impl IntoView {
    let url = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let handle_install = move |_| {
        if url.get().trim().is_empty() {
            return;
        }
        loading.set(true);
        error.set(None);
        // TODO: Call Gateway API
        loading.set(false);
        on_close();
    };

    view! {
        <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="bg-surface border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">"Install Skill"</h2>
                <p class="text-sm text-text-secondary mb-4">
                    "Install a skill from a URL"
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">"Skill URL"</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder="https://github.com/user/skill.git"
                            value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
                    </div>

                    {move || error.get().map(|err| view! {
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
                        disabled=move || url.get().trim().is_empty() || loading.get()
                        on:click=handle_install
                    >
                        {move || if loading.get() { "Installing..." } else { "Install" }}
                    </button>
                </div>
            </div>
        </div>
    }
}

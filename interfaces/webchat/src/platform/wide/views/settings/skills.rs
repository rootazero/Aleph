use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, Locale};

// ---------------------------------------------------------------------------
// Local types matching the JSON shape from `skills.status` RPC
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingRequirements {
    pub bins: Vec<String>,
    pub env: Vec<String>,
    pub config: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOption {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub bins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillStatusEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub emoji: Option<String>,
    pub source: serde_json::Value,
    /// Human-readable source label for grouping and display (e.g. "Bundled", "Global", "Plugin")
    #[serde(default)]
    pub source_label: String,
    #[serde(default)]
    pub homepage: Option<String>,
    pub eligible: bool,
    pub disabled: bool,
    #[serde(default)]
    pub missing: MissingRequirements,
    #[serde(default)]
    pub install_options: Vec<InstallOption>,
    #[serde(default)]
    pub primary_env: Option<String>,
    #[serde(default)]
    pub api_key_set: bool,
    pub scope: String,
    pub user_invocable: bool,
}

// ---------------------------------------------------------------------------
// Tab enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum SkillTab {
    All,
    Ready,
    NeedsSetup,
    Disabled,
}

// ---------------------------------------------------------------------------
// Helper: determine status label for a skill
// ---------------------------------------------------------------------------

const fn skill_status(skill: &SkillStatusEntry) -> &'static str {
    if skill.disabled {
        "Disabled"
    } else if !skill.eligible
        || !skill.missing.bins.is_empty()
        || !skill.missing.env.is_empty()
        || !skill.missing.config.is_empty()
    {
        "Needs Setup"
    } else {
        "Ready"
    }
}

/// A vendor link on a skill row, screened by the shared predicate.
///
/// The screen used to live here alone, which is how the provider detail panel
/// came to render its "Get a key" link with none at all — one question, two
/// files, and only one of them answering it.
fn safe_homepage_link(href: &str, label: impl IntoView) -> AnyView {
    crate::components::external_link::safe_external_link(
        href,
        "text-xs text-primary hover:underline",
        label,
    )
}

// ---------------------------------------------------------------------------
// Load function
// ---------------------------------------------------------------------------

fn load_skills(
    // Localised copy for a refused load — the caller already holds the
    // context; `I18nContext` is `Copy`, so it rides along like the signals do.
    i18n: I18nContext<Locale>,
    state: DashboardState,
    skills: RwSignal<Vec<SkillStatusEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        let mut last_err = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                gloo_timers::future::sleep(std::time::Duration::from_millis(500)).await;
            }
            match state.rpc_call("skills.status", json!({})).await {
                Ok(result) => {
                    if let Some(list) = result.get("skills") {
                        if let Ok(parsed) =
                            serde_json::from_value::<Vec<SkillStatusEntry>>(list.clone())
                        {
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
        error.set(Some(crate::components::admin_refusal::settings_load_error(
            i18n,
            &last_err,
            |last_err| format!("Failed to load skills: {last_err}"),
        )));
        loading.set(false);
    });
}

// ---------------------------------------------------------------------------
// Main view
// ---------------------------------------------------------------------------

#[component]
#[must_use]
pub fn SkillsView() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let skills = RwSignal::new(Vec::<SkillStatusEntry>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let active_tab = RwSignal::new(SkillTab::All);
    let selected_skill_id = RwSignal::new(Option::<String>::None);
    let show_add_dialog = RwSignal::new(false);

    Effect::new(move || {
        if state.is_connected.get() {
            load_skills(i18n, state, skills, loading, error);
        } else {
            loading.set(false);
        }
    });

    // Filtered list based on active tab
    let filtered_skills = Memo::new(move |_| {
        let tab = active_tab.get();
        skills
            .get()
            .into_iter()
            .filter(|s| match tab {
                SkillTab::All => true,
                SkillTab::Ready => skill_status(s) == "Ready",
                SkillTab::NeedsSetup => skill_status(s) == "Needs Setup",
                SkillTab::Disabled => skill_status(s) == "Disabled",
            })
            .collect::<Vec<_>>()
    });

    let selected_skill = Memo::new(move |_| {
        let id = selected_skill_id.get()?;
        skills.get().into_iter().find(|s| s.id == id)
    });

    let on_select = Callback::new(move |id: String| {
        selected_skill_id.set(Some(id));
    });

    let on_toggle = {
        Callback::new(move |(id, enabled): (String, bool)| {
            let state = state;
            spawn_local(async move {
                match state
                    .rpc_call(
                        "skills.update",
                        json!({ "skill_id": id, "enabled": enabled }),
                    )
                    .await
                {
                    Ok(_) => load_skills(i18n, state, skills, loading, error),
                    Err(e) => error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to toggle skill: {e}")
                        }),
                    )),
                }
            });
        })
    };

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top">
            <div class="max-w-3xl space-y-6">
                // Page Header
                <div class="flex items-center justify-between">
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary mb-1">{t!(i18n, settings.skills.title)}</h1>
                        <p class="text-sm text-text-secondary">
                            {t!(i18n, skills_page.header_desc)}
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                            on:click=move |_| load_skills(i18n, state, skills, loading, error)
                        >
                            {t!(i18n, skills_page.refresh)}
                        </button>
                        <button
                            class="px-3 py-1.5 bg-primary text-white rounded hover:bg-primary-hover text-sm"
                            on:click=move |_| show_add_dialog.set(true)
                        >
                            {t!(i18n, skills_page.add_skill)}
                        </button>
                    </div>
                </div>

                // Error Message
                {move || error.get().map(|err| view! {
                    <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                        {err}
                    </div>
                })}

                // Tab Bar
                <Show when=move || !loading.get()>
                    <SkillTabBar active_tab=active_tab skills=skills />
                </Show>

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

                // Skill List grouped by source
                <Show when=move || !loading.get()>
                    {move || {
                        let list = filtered_skills.get();
                        if list.is_empty() {
                            view! {
                                <div class="text-center py-6 border border-dashed border-border rounded">
                                    <p class="text-sm text-text-secondary">{t!(i18n, settings.skills.no_skills_found)}</p>
                                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.skills.no_skills_hint)}</p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <SkillList
                                    skills=list
                                    on_select=on_select
                                    on_toggle=on_toggle
                                />
                            }.into_any()
                        }
                    }}
                </Show>
            </div>

            // Detail Dialog
            {move || selected_skill.get().map(|skill| {
                view! {
                    <SkillDetailDialog
                        skill=skill
                        on_close=move || selected_skill_id.set(None)
                        on_refresh=move || load_skills(i18n, state, skills, loading, error)
                    />
                }
            })}

            // Add Skill Dialog
            <Show when=move || show_add_dialog.get()>
                <AddSkillDialog
                    on_close=move || show_add_dialog.set(false)
                    on_success=move || {
                        show_add_dialog.set(false);
                        load_skills(i18n, state, skills, loading, error);
                    }
                />
            </Show>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tab Bar
// ---------------------------------------------------------------------------

#[component]
fn SkillTabBar(
    active_tab: RwSignal<SkillTab>,
    skills: RwSignal<Vec<SkillStatusEntry>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let count_all = Memo::new(move |_| skills.get().len());
    let count_ready = Memo::new(move |_| {
        skills
            .get()
            .iter()
            .filter(|s| skill_status(s) == "Ready")
            .count()
    });
    let count_needs = Memo::new(move |_| {
        skills
            .get()
            .iter()
            .filter(|s| skill_status(s) == "Needs Setup")
            .count()
    });
    let count_disabled = Memo::new(move |_| {
        skills
            .get()
            .iter()
            .filter(|s| skill_status(s) == "Disabled")
            .count()
    });

    let tab_class = |tab: SkillTab| {
        move || {
            if active_tab.get() == tab {
                "px-3 py-1.5 text-sm font-medium rounded text-primary bg-primary-subtle"
            } else {
                "px-3 py-1.5 text-sm font-medium rounded text-text-secondary hover:text-text-primary hover:bg-surface-raised"
            }
        }
    };

    view! {
        <div class="flex items-center gap-1 p-1 bg-surface-raised border border-border rounded">
            <button
                class=tab_class(SkillTab::All)
                on:click=move |_| active_tab.set(SkillTab::All)
            >
                {t!(i18n, settings.skills.tab_all, count = move || count_all.get())}
            </button>
            <button
                class=tab_class(SkillTab::Ready)
                on:click=move |_| active_tab.set(SkillTab::Ready)
            >
                {t!(i18n, settings.skills.tab_ready, count = move || count_ready.get())}
            </button>
            <button
                class=tab_class(SkillTab::NeedsSetup)
                on:click=move |_| active_tab.set(SkillTab::NeedsSetup)
            >
                {t!(i18n, settings.skills.tab_needs_setup, count = move || count_needs.get())}
            </button>
            <button
                class=tab_class(SkillTab::Disabled)
                on:click=move |_| active_tab.set(SkillTab::Disabled)
            >
                {t!(i18n, settings.skills.tab_disabled, count = move || count_disabled.get())}
            </button>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Skill Group (one source section)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
struct SkillGroup {
    source_label: String,
    skills: Vec<SkillStatusEntry>,
}

// ---------------------------------------------------------------------------
// Skill List (grouped by source)
// ---------------------------------------------------------------------------

#[component]
fn SkillList(
    skills: Vec<SkillStatusEntry>,
    on_select: Callback<String>,
    on_toggle: Callback<(String, bool)>,
) -> impl IntoView {
    // Group by source_label (pre-rendered display string from core)
    let mut groups: Vec<SkillGroup> = Vec::new();
    for skill in skills {
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g.source_label == skill.source_label)
        {
            group.skills.push(skill);
        } else {
            let label = skill.source_label.clone();
            groups.push(SkillGroup {
                source_label: label,
                skills: vec![skill],
            });
        }
    }

    let groups = StoredValue::new(groups);

    view! {
        <div class="space-y-6">
            <For
                each=move || groups.get_value()
                key=|g| g.source_label.clone()
                children=move |group| {
                    let group_skills = StoredValue::new(group.skills);
                    let source_label = group.source_label;
                    view! {
                        <div class="space-y-2">
                            <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider px-1">
                                {source_label}
                            </h3>
                            <div class="space-y-2">
                                <For
                                    each=move || group_skills.get_value()
                                    key=|s| s.id.clone()
                                    children=move |skill| {
                                        view! {
                                            <SkillCard
                                                skill=skill
                                                on_select=on_select
                                                on_toggle=on_toggle
                                            />
                                        }
                                    }
                                />
                            </div>
                        </div>
                    }
                }
            />
        </div>
    }
}

// ---------------------------------------------------------------------------
// Skill Card (list row)
// ---------------------------------------------------------------------------

#[component]
fn SkillCard(
    skill: SkillStatusEntry,
    on_select: Callback<String>,
    on_toggle: Callback<(String, bool)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let status = skill_status(&skill).to_string();
    let status_label = match status.as_str() {
        "Ready" => t_string!(i18n, skills_page.status_ready).to_string(),
        "Needs Setup" => t_string!(i18n, skills_page.status_needs_setup).to_string(),
        _ => t_string!(i18n, skills_page.status_disabled).to_string(),
    };
    let enabled = !skill.disabled;
    let skill_id = StoredValue::new(skill.id.clone());
    let skill_id2 = StoredValue::new(skill.id.clone());
    let emoji = skill.emoji.clone().unwrap_or_else(|| "⚡".to_string());

    let badge_class = match status.as_str() {
        "Ready" => "px-2 py-0.5 rounded-full text-xs font-medium bg-success-subtle text-success",
        "Needs Setup" => {
            "px-2 py-0.5 rounded-full text-xs font-medium bg-warning-subtle text-warning"
        }
        _ => "px-2 py-0.5 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary",
    };

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded flex items-center gap-3">
            // Emoji icon
            <div class="w-9 h-9 rounded bg-surface-sunken flex items-center justify-center flex-shrink-0 text-lg">
                {emoji}
            </div>

            // Name (clickable) + description
            <div class="flex-1 min-w-0">
                <button
                    class="text-sm font-medium text-text-primary hover:text-primary text-left truncate max-w-full"
                    on:click=move |_| {
                        let id = skill_id.get_value();
                        on_select.run(id);
                    }
                >
                    {skill.name.clone()}
                </button>
                <p class="text-xs text-text-secondary mt-0.5 truncate">
                    {skill.description}
                </p>
            </div>

            // Status badge
            <span class=badge_class>
                {status_label}
            </span>

            // Toggle switch
            <label class="relative inline-flex items-center cursor-pointer flex-shrink-0">
                <input
                    type="checkbox"
                    class="sr-only peer"
                    prop:checked=enabled
                    on:change=move |ev| {
                        let new_val = event_target_checked(&ev);
                        let id = skill_id2.get_value();
                        on_toggle.run((id, new_val));
                    }
                />
                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
            </label>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Skill Detail Dialog
// ---------------------------------------------------------------------------

#[component]
fn SkillDetailDialog(
    skill: SkillStatusEntry,
    on_close: impl Fn() + 'static + Copy + Send,
    on_refresh: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    let skill_id = StoredValue::new(skill.id.clone());
    let api_key_input = RwSignal::new(String::new());
    let saving_key = RwSignal::new(false);
    let key_error = RwSignal::new(Option::<String>::None);
    let installing_dep = RwSignal::new(Option::<String>::None);
    let dep_error = RwSignal::new(Option::<String>::None);
    let toggling = RwSignal::new(false);

    // Current toggle / scope state (local mirrors)
    let enabled = RwSignal::new(!skill.disabled);
    let scope = RwSignal::new(skill.scope.clone());

    let skill_for_header = skill.clone();
    let skill_for_reqs = skill.clone();
    let skill_for_api = skill.clone();
    let skill_for_settings = skill.clone();
    let skill_for_info = skill;

    view! {
        <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50"
            on:click=move |ev: web_sys::MouseEvent| {
                // Close on backdrop click
                use wasm_bindgen::JsCast;
                if let Some(target) = ev.target() {
                    if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                        if el.class_list().contains("fixed") {
                            on_close();
                        }
                    }
                }
            }
        >
            <div class="glass bg-surface-overlay/85 border border-border rounded-lg w-full max-w-lg mx-4 max-h-[85vh] flex flex-col overflow-hidden">
                // Header
                <div class="p-4 border-b border-border flex items-start justify-between gap-3 flex-shrink-0">
                    <div class="flex items-center gap-3">
                        <div class="w-10 h-10 rounded bg-surface-sunken flex items-center justify-center text-xl flex-shrink-0">
                            {skill_for_header.emoji.clone().unwrap_or_else(|| "⚡".to_string())}
                        </div>
                        <div>
                            <h2 class="text-base font-semibold text-text-primary">
                                {skill_for_header.name.clone()}
                            </h2>
                            <div class="flex items-center gap-2 mt-0.5 flex-wrap">
                                // Source badge
                                <span class="px-2 py-0.5 rounded-full text-xs bg-surface-sunken text-text-secondary">
                                    {skill_for_header.source_label.clone()}
                                </span>
                                // Scope badge
                                <span class="px-2 py-0.5 rounded-full text-xs bg-primary-subtle text-primary">
                                    {skill_for_header.scope.clone()}
                                </span>
                                // Eligible/disabled badge
                                {if skill_for_header.disabled {
                                    view! {
                                        <span class="px-2 py-0.5 rounded-full text-xs bg-surface-sunken text-text-tertiary">
                                            {t!(i18n, skills_page.status_disabled)}
                                        </span>
                                    }.into_any()
                                } else if skill_for_header.eligible {
                                    view! {
                                        <span class="px-2 py-0.5 rounded-full text-xs bg-success-subtle text-success">
                                            {t!(i18n, skills_page.eligible)}
                                        </span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <span class="px-2 py-0.5 rounded-full text-xs bg-warning-subtle text-warning">
                                            {t!(i18n, skills_page.not_eligible)}
                                        </span>
                                    }.into_any()
                                }}
                            </div>
                        </div>
                    </div>
                    <button
                        class="p-1 text-text-tertiary hover:text-text-primary rounded"
                        on:click=move |_| on_close()
                    >
                        "✕"
                    </button>
                </div>

                // Scrollable body
                <div class="p-4 overflow-y-auto space-y-5 flex-1">
                    // Description
                    <p class="text-sm text-text-secondary">{skill_for_header.description}</p>

                    // Requirements section
                    {
                        let missing = skill_for_reqs.missing.clone();
                        let install_opts = skill_for_reqs.install_options;
                        let has_missing = !missing.bins.is_empty()
                            || !missing.env.is_empty()
                            || !missing.config.is_empty();
                        view! {
                            <div class="space-y-2">
                                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                                    {t!(i18n, skills_page.requirements)}
                                </h3>
                                {if has_missing {
                                    view! {
                                        <div class="space-y-1.5">
                                            // Missing bins
                                            <For
                                                each=move || missing.bins.clone()
                                                key=|b| b.clone()
                                                children=move |bin| {
                                                    // Find install options for this bin
                                                    let opts: Vec<InstallOption> = install_opts
                                                        .iter()
                                                        .filter(|o| o.bins.contains(&bin))
                                                        .cloned()
                                                        .collect();
                                                    let bin_label = bin;
                                                    view! {
                                                        <div class="flex items-center gap-2">
                                                            <span class="text-warning text-sm">"⚠"</span>
                                                            <span class="text-sm text-text-primary font-mono">{bin_label}</span>
                                                            <span class="text-xs text-text-tertiary">{t!(i18n, settings.skills.binary_missing)}</span>
                                                            {if !opts.is_empty() {
                                                                let opt = opts[0].clone();
                                                                let opt_id = opt.id.clone();
                                                                let opt_label = opt.label;
                                                                let skill_id_v = skill_id;
                                                                view! {
                                                                    <button
                                                                        class="ml-auto px-2 py-0.5 bg-primary text-white rounded text-xs hover:bg-primary-hover disabled:opacity-50"
                                                                        disabled=move || installing_dep.get().is_some()
                                                                        on:click=move |_| {
                                                                            let id = skill_id_v.get_value();
                                                                            let spec = opt_id.clone();
                                                                            installing_dep.set(Some(spec.clone()));
                                                                            dep_error.set(None);
                                                                            spawn_local(async move {
                                                                                match state
                                                                                    .rpc_call("skills.install_dep", json!({ "skill_id": id, "spec_id": spec }))
                                                                                    .await
                                                                                {
                                                                                    Ok(_) => {
                                                                                        installing_dep.set(None);
                                                                                        on_refresh();
                                                                                        on_close();
                                                                                    }
                                                                                    Err(e) => {
                                                                                        dep_error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                                                            i18n,
                                                                                            &e,
                                                                                            |e| format!("Install failed: {e}"),
                                                                                        )));
                                                                                        installing_dep.set(None);
                                                                                    }
                                                                                }
                                                                            });
                                                                        }
                                                                    >
                                                                        {move || {
                                                                            if installing_dep.get().is_some() {
                                                                                t_string!(i18n, skills_page.installing).to_string()
                                                                            } else {
                                                                                opt_label.clone()
                                                                            }
                                                                        }}
                                                                    </button>
                                                                }.into_any()
                                                            } else {
                                                                view! { <span></span> }.into_any()
                                                            }}
                                                        </div>
                                                    }
                                                }
                                            />
                                            // Missing env
                                            <For
                                                each=move || missing.env.clone()
                                                key=|e| e.clone()
                                                children=move |env_var| {
                                                    view! {
                                                        <div class="flex items-center gap-2">
                                                            <span class="text-warning text-sm">"⚠"</span>
                                                            <span class="text-sm text-text-primary font-mono">{env_var}</span>
                                                            <span class="text-xs text-text-tertiary">{t!(i18n, settings.skills.env_var_missing)}</span>
                                                        </div>
                                                    }
                                                }
                                            />
                                            // Missing config
                                            <For
                                                each=move || missing.config.clone()
                                                key=|c| c.clone()
                                                children=move |cfg| {
                                                    view! {
                                                        <div class="flex items-center gap-2">
                                                            <span class="text-warning text-sm">"⚠"</span>
                                                            <span class="text-sm text-text-primary font-mono">{cfg}</span>
                                                            <span class="text-xs text-text-tertiary">{t!(i18n, settings.skills.config_missing)}</span>
                                                        </div>
                                                    }
                                                }
                                            />
                                            // Dep error
                                            {move || dep_error.get().map(|e| view! {
                                                <div class="p-2 bg-danger-subtle border border-border rounded text-danger text-xs">
                                                    {e}
                                                </div>
                                            })}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex items-center gap-2">
                                            <span class="text-success text-sm">"✓"</span>
                                            <span class="text-sm text-text-secondary">{t!(i18n, settings.skills.all_requirements_met)}</span>
                                        </div>
                                    }.into_any()
                                }}
                            </div>
                        }
                    }

                    // API Key section (only if primary_env set)
                    {
                        let primary_env = skill_for_api.primary_env.clone();
                        let api_key_set = skill_for_api.api_key_set;
                        let homepage = skill_for_api.homepage;
                        if let Some(env_name) = primary_env {
                            let env_label = env_name;
                            view! {
                                <div class="space-y-2">
                                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                                        {t!(i18n, skills_page.api_key)}
                                    </h3>
                                    <div class="space-y-2">
                                        <div class="flex items-center gap-2">
                                            <span class="text-xs text-text-secondary font-mono">
                                                {env_label}
                                            </span>
                                            {if api_key_set {
                                                view! {
                                                    <span class="px-2 py-0.5 rounded-full text-xs bg-success-subtle text-success">
                                                        {t!(i18n, skills_page.key_set)}
                                                    </span>
                                                }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }}
                                        </div>
                                        <div class="flex gap-2">
                                            <input
                                                type="password"
                                                class="flex-1 px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                                placeholder=move || t_string!(i18n, skills_page.api_key_placeholder).to_string()
                                                prop:value=move || api_key_input.get()
                                                on:input=move |ev| api_key_input.set(event_target_value(&ev))
                                            />
                                            <button
                                                class="px-3 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                                                disabled=move || api_key_input.get().trim().is_empty() || saving_key.get()
                                                on:click=move |_| {
                                                    let id = skill_id.get_value();
                                                    let key = api_key_input.get().trim().to_string();
                                                    saving_key.set(true);
                                                    key_error.set(None);
                                                    spawn_local(async move {
                                                        match state
                                                            .rpc_call("skills.update", json!({ "skill_id": id, "api_key": key }))
                                                            .await
                                                        {
                                                            Ok(_) => {
                                                                saving_key.set(false);
                                                                api_key_input.set(String::new());
                                                                on_refresh();
                                                                on_close();
                                                            }
                                                            Err(e) => {
                                                                key_error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                                    i18n,
                                                                    &e,
                                                                    |e| format!("Failed to save: {e}"),
                                                                )));
                                                                saving_key.set(false);
                                                            }
                                                        }
                                                    });
                                                }
                                            >
                                                {move || if saving_key.get() { t_string!(i18n, settings.skills.saving) } else { t_string!(i18n, settings.skills.save) }}
                                            </button>
                                        </div>
                                        {move || key_error.get().map(|e| view! {
                                            <div class="p-2 bg-danger-subtle border border-border rounded text-danger text-xs">
                                                {e}
                                            </div>
                                        })}
                                        {if let Some(hp) = homepage {
                                            safe_homepage_link(&hp, t!(i18n, skills_page.get_api_key))
                                        } else {
                                            view! { <span></span> }.into_any()
                                        }}
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }
                    }

                    // Settings section
                    <div class="space-y-3">
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                            {t!(i18n, skills_page.settings)}
                        </h3>

                        // Enabled toggle
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">{t!(i18n, settings.skills.enabled)}</div>
                                <div class="text-xs text-text-secondary">{t!(i18n, settings.skills.enabled_hint)}</div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    prop:checked=move || enabled.get()
                                    on:change=move |ev| {
                                        let new_val = event_target_checked(&ev);
                                        enabled.set(new_val);
                                        toggling.set(true);
                                        let id = skill_id.get_value();
                                        spawn_local(async move {
                                            match state
                                                .rpc_call("skills.update", json!({ "skill_id": id, "enabled": new_val }))
                                                .await
                                            {
                                                Ok(_) => {
                                                    toggling.set(false);
                                                    on_refresh();
                                                }
                                                Err(e) => {
                                                    key_error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                        i18n,
                                                        &e,
                                                        |e| format!("Failed to update: {e}"),
                                                    )));
                                                    enabled.set(!new_val);
                                                    toggling.set(false);
                                                }
                                            }
                                        });
                                    }
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>

                        // Scope dropdown
                        <div>
                            <label class="block text-sm font-medium text-text-primary mb-1">{t!(i18n, settings.skills.scope)}</label>
                            <select
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                on:change=move |ev| {
                                    let new_scope = event_target_value(&ev);
                                    scope.set(new_scope.clone());
                                    let id = skill_id.get_value();
                                    spawn_local(async move {
                                        if let Err(e) = state
                                            .rpc_call("skills.update", json!({ "skill_id": id, "scope": new_scope }))
                                            .await
                                        {
                                            key_error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                i18n,
                                                &e,
                                                |e| format!("Failed to update scope: {e}"),
                                            )));
                                        } else {
                                            on_refresh();
                                        }
                                    });
                                }
                            >
                                <option value="system" selected=move || scope.get() == "system">{t!(i18n, settings.skills.scope_system)}</option>
                                <option value="tool" selected=move || scope.get() == "tool">{t!(i18n, settings.skills.scope_tool)}</option>
                                <option value="standalone" selected=move || scope.get() == "standalone">{t!(i18n, settings.skills.scope_standalone)}</option>
                                <option value="disabled" selected=move || scope.get() == "disabled">{t!(i18n, settings.skills.scope_disabled)}</option>
                            </select>
                        </div>
                    </div>

                    // Info section
                    <div class="space-y-2">
                        <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.skills.info)}</h3>
                        <div class="space-y-1 text-xs text-text-secondary">
                            <div class="flex items-center gap-2">
                                <span class="text-text-tertiary w-16">{t!(i18n, settings.skills.source_label)}</span>
                                <span class="text-text-primary">{skill_for_info.source_label.clone()}</span>
                            </div>
                            <div class="flex items-center gap-2">
                                <span class="text-text-tertiary w-16">{t!(i18n, settings.skills.id_label)}</span>
                                <span class="text-text-primary font-mono">{skill_for_info.id.clone()}</span>
                            </div>
                            {if let Some(hp) = skill_for_info.homepage {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <span class="text-text-tertiary w-16">{t!(i18n, settings.skills.homepage)}</span>
                                        {safe_homepage_link(&hp, t!(i18n, skills_page.open))}
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }}
                        </div>
                    </div>

                    // Remove button (for non-bundled skills)
                    {if skill_for_settings.source_label != "Bundled" {
                        view! {
                            <div class="pt-2 border-t border-border">
                                <button
                                    class="px-3 py-1.5 text-danger hover:bg-danger-subtle rounded text-sm"
                                    on:click=move |_| {
                                        let id = skill_id.get_value();
                                        spawn_local(async move {
                                            match state
                                                .rpc_call("skills.remove", json!({ "skill_id": id }))
                                                .await
                                            {
                                                Ok(_) => {
                                                    on_refresh();
                                                    on_close();
                                                }
                                                Err(e) => {
                                                    key_error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                        i18n,
                                                        &e,
                                                        |e| format!("Failed to remove: {e}"),
                                                    )));
                                                }
                                            }
                                        });
                                    }
                                >
                                    {t!(i18n, skills_page.remove_skill)}
                                </button>
                            </div>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Add Skill Dialog
// ---------------------------------------------------------------------------

#[component]
fn AddSkillDialog(
    on_close: impl Fn() + 'static + Copy + Send,
    on_success: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let url = RwSignal::new(String::new());
    let adding = RwSignal::new(false);
    let dialog_error = RwSignal::new(Option::<String>::None);

    let handle_add = move |_| {
        let trimmed = url.get();
        let trimmed = trimmed.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        adding.set(true);
        dialog_error.set(None);
        spawn_local(async move {
            match state
                .rpc_call("skills.install", json!({ "url": trimmed }))
                .await
            {
                Ok(_) => {
                    adding.set(false);
                    on_success();
                }
                Err(e) => {
                    dialog_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to add skill: {e}")
                        }),
                    ));
                    adding.set(false);
                }
            }
        });
    };

    view! {
        <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div class="glass bg-surface-overlay/85 border border-border rounded-lg p-6 max-w-md w-full mx-4">
                <h2 class="text-lg font-semibold text-text-primary mb-2">{t!(i18n, skills_page.add_skill)}</h2>
                <p class="text-sm text-text-secondary mb-4">
                    {t!(i18n, skills_page.add_skill_help)}
                </p>

                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-2">{t!(i18n, settings.skills.url_label)}</label>
                        <input
                            type="text"
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                            placeholder=move || t_string!(i18n, skills_page.url_placeholder).to_string()
                            prop:value=move || url.get()
                            on:input=move |ev| url.set(event_target_value(&ev))
                        />
                    </div>

                    {move || dialog_error.get().map(|err| view! {
                        <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">
                            {err}
                        </div>
                    })}
                </div>

                <div class="flex gap-2 mt-6">
                    <button
                        class="flex-1 px-4 py-2 bg-surface-sunken text-text-secondary rounded hover:bg-surface-sunken text-sm"
                        on:click=move |_| on_close()
                    >
                        {t!(i18n, skills_page.cancel)}
                    </button>
                    <button
                        class="flex-1 px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm disabled:opacity-50"
                        disabled=move || url.get().trim().is_empty() || adding.get()
                        on:click=handle_add
                    >
                        {move || if adding.get() { t_string!(i18n, settings.skills.adding) } else { t_string!(i18n, settings.skills.add) }}
                    </button>
                </div>
            </div>
        </div>
    }
}

//! Right pane — job form editor, actions, and per-job run history.

use super::helpers::{build_schedule_kind_json, extract_schedule_from_kind, stale_agent_option};
use super::run_history::RunHistory;
use crate::api::agents::{AgentSummary, AgentsApi};
use crate::api::cron::{CreateCronJob, CronApi, CronJobInfo, JobRunInfo, UpdateCronJob};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// JobEditor — Right Pane (form + actions + history)
// ============================================================================

#[component]
pub(super) fn JobEditor(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_schedule_kind = RwSignal::new(String::from("cron"));
    let form_schedule = RwSignal::new(String::new());
    let form_agent_id = RwSignal::new(String::new());
    // Available agents for the selector + default id.
    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_agent_id = RwSignal::new(String::from("main"));
    let form_prompt = RwSignal::new(String::new());
    let form_timezone = RwSignal::new(String::new());
    let form_tags = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_anchor_ms = RwSignal::new(String::new());
    let form_stagger_ms = RwSignal::new(String::new());
    let form_session_target = RwSignal::new(String::new());
    // Failure alert sub-fields
    let form_alert_after = RwSignal::new(String::from("2"));
    let form_alert_cooldown = RwSignal::new(String::from("1h"));
    let form_alert_kind = RwSignal::new(String::from("announce"));
    let form_alert_channel = RwSignal::new(String::new());
    let form_alert_expanded = RwSignal::new(false);
    let form_channel = RwSignal::new(Option::<String>::None);

    let runs = RwSignal::new(Vec::<JobRunInfo>::new());
    let run_success = RwSignal::new(Option::<String>::None);

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Load available agents once connected, for the agent selector.
    {
        let dash = state;
        Effect::new(move || {
            if !dash.is_connected.get() {
                return;
            }
            spawn_local(async move {
                if let Ok(resp) = AgentsApi::list(&dash).await {
                    default_agent_id.set(resp.default_id.clone());
                    agents.set(resp.agents);
                }
            });
        });
    }

    // Populate form when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                // Reset form for new job
                form_name.set(String::new());
                form_schedule_kind.set(String::from("cron"));
                form_schedule.set(String::new());
                form_agent_id.set(default_agent_id.get_untracked());
                form_prompt.set(String::new());
                form_timezone.set(String::new());
                form_tags.set(String::new());
                form_enabled.set(true);
                form_anchor_ms.set(String::new());
                form_stagger_ms.set(String::new());
                form_session_target.set(String::new());
                form_alert_after.set("2".to_string());
                form_alert_cooldown.set("1h".to_string());
                form_alert_kind.set("announce".to_string());
                form_alert_channel.set(String::new());
                form_alert_expanded.set(false);
                form_channel.set(None);
                runs.set(Vec::new());
            } else {
                // Load existing job data
                if let Some(job) = jobs.get().get(idx) {
                    form_name.set(job.name.clone());

                    // Extract schedule type and value from schedule_kind JSON object
                    // Backend returns: {"kind":"cron","expr":"..."} or {"kind":"every","every_ms":...} etc.
                    let (sk_type, sk_val, sk_anchor, sk_stagger) =
                        extract_schedule_from_kind(&job.schedule_kind);
                    form_schedule_kind.set(sk_type);
                    form_schedule.set(sk_val);

                    form_agent_id.set(job.agent_id.clone());
                    form_channel.set(job.source_channel_id.clone());
                    form_prompt.set(job.prompt.clone());
                    form_timezone.set(job.timezone.clone().unwrap_or_default());
                    form_tags.set(job.tags.join(", "));
                    form_enabled.set(job.enabled);
                    form_anchor_ms.set(
                        sk_anchor
                            .or(job.anchor_ms.map(|v| v.to_string()))
                            .unwrap_or_default(),
                    );
                    form_stagger_ms.set(
                        sk_stagger
                            .or(job.stagger_ms.map(|v| v.to_string()))
                            .unwrap_or_default(),
                    );
                    form_session_target.set(job.session_target.clone().unwrap_or_default());

                    // Populate failure alert from JSON
                    if let Some(ref alert) = job.failure_alert {
                        form_alert_after.set(
                            alert
                                .get("after_n")
                                .and_then(|v| v.as_u64())
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "2".to_string()),
                        );
                        form_alert_cooldown.set(
                            alert
                                .get("cooldown")
                                .and_then(|v| v.as_str())
                                .unwrap_or("1h")
                                .to_string(),
                        );
                        form_alert_kind.set(
                            alert
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .unwrap_or("announce")
                                .to_string(),
                        );
                        form_alert_channel.set(
                            alert
                                .get("channel")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        );
                        form_alert_expanded.set(true);
                    } else {
                        form_alert_after.set("2".to_string());
                        form_alert_cooldown.set("1h".to_string());
                        form_alert_kind.set("announce".to_string());
                        form_alert_channel.set(String::new());
                        form_alert_expanded.set(false);
                    }

                    // Load run history
                    let job_id = job.id.clone();
                    spawn_local(async move {
                        match CronApi::runs(&state, &job_id, 20).await {
                            Ok(list) => runs.set(list),
                            Err(_) => runs.set(Vec::new()),
                        }
                    });
                }
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let name = form_name.get();
        if name.trim().is_empty() {
            error.set(Some(t_string!(i18n, cron.error_name_required).to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let schedule_kind = form_schedule_kind.get();
        let schedule = form_schedule.get();
        let agent_id = form_agent_id.get();
        let prompt = form_prompt.get();
        let timezone = {
            let tz = form_timezone.get();
            if tz.trim().is_empty() {
                None
            } else {
                Some(tz)
            }
        };
        let tags: Vec<String> = form_tags
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let enabled = form_enabled.get();

        // Build schedule_kind JSON object from form fields
        let schedule_kind_obj = build_schedule_kind_json(
            &schedule_kind,
            &schedule,
            &form_anchor_ms.get(),
            &form_stagger_ms.get(),
        );

        if schedule_kind_obj.is_none() {
            let hint = match schedule_kind.as_str() {
                "every" => "e.g. 5m, 2h, 30s, or 60000 (ms)",
                "at" => "a valid date and time",
                _ => "cron expression, e.g. 0 0 9 * * *",
            };
            error.set(Some(format!(
                "Invalid schedule value for type '{}'. Expected: {}",
                schedule_kind, hint
            )));
            saving.set(false);
            return;
        }

        let session_target = {
            let s = form_session_target.get();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        };

        // Build failure_alert JSON if channel is specified
        let failure_alert = {
            let ch = form_alert_channel.get();
            if ch.trim().is_empty() {
                None
            } else {
                Some(serde_json::json!({
                    "after_n": form_alert_after.get().parse::<u32>().unwrap_or(2),
                    "cooldown": form_alert_cooldown.get(),
                    "kind": form_alert_kind.get(),
                    "channel": ch,
                }))
            }
        };

        if is_new() {
            let create = CreateCronJob {
                name,
                schedule,
                schedule_kind: schedule_kind_obj,
                agent_id,
                prompt,
                enabled,
                timezone,
                tags,
                session_target,
                failure_alert,
            };

            spawn_local(async move {
                match CronApi::create(&state, create).await {
                    Ok(_) => {
                        error.set(None);
                        if let Ok(list) = CronApi::list(&state).await {
                            jobs.set(list);
                        }
                        selected.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to create job: {}", e)));
                    }
                }
                saving.set(false);
            });
        } else if let Some(idx) = selected.get() {
            if let Some(job) = jobs.get().get(idx).cloned() {
                let patch = UpdateCronJob {
                    job_id: job.id,
                    name: Some(name),
                    schedule_kind: schedule_kind_obj,
                    agent_id: Some(agent_id),
                    prompt: Some(prompt),
                    enabled: Some(enabled),
                    timezone,
                    tags: Some(tags),
                    session_target,
                    failure_alert,
                };

                spawn_local(async move {
                    match CronApi::update(&state, patch).await {
                        Ok(_) => {
                            error.set(None);
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to update job: {}", e)));
                        }
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Two-step delete confirmation
    let confirm_delete = RwSignal::new(false);

    // Reset confirmation when selection changes
    Effect::new(move || {
        let _ = selected.get();
        confirm_delete.set(false);
    });

    let on_delete = move |_| {
        if !confirm_delete.get() {
            // First click: show confirmation
            confirm_delete.set(true);
            return;
        }

        // Second click: actually delete
        confirm_delete.set(false);
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                return;
            }

            if let Some(job) = jobs.get().get(idx).cloned() {
                saving.set(true);
                error.set(None);

                let job_id = job.id;
                spawn_local(async move {
                    match CronApi::delete(&state, &job_id).await {
                        Ok(()) => {
                            error.set(None);
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to delete job: {}", e)));
                        }
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Handle run now
    let on_run_now = move |_| {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                return;
            }

            if let Some(job) = jobs.get().get(idx).cloned() {
                let job_id = job.id;
                let state_for_spawn = state;
                let runs_for_spawn = runs;
                let error_for_spawn = error;
                let success_for_spawn = run_success;
                spawn_local(async move {
                    match CronApi::run_now(&state_for_spawn, &job_id).await {
                        Ok(_) => {
                            error_for_spawn.set(None);
                            success_for_spawn
                                .set(Some(t_string!(i18n, cron.run_triggered).to_string()));
                            // Reload runs after triggering
                            if let Ok(list) = CronApi::runs(&state_for_spawn, &job_id, 20).await {
                                runs_for_spawn.set(list);
                            }
                            gloo_timers::future::sleep(std::time::Duration::from_secs(3)).await;
                            success_for_spawn.set(None);
                        }
                        Err(e) => {
                            error_for_spawn.set(Some(format!("Failed to trigger run: {}", e)));
                        }
                    }
                });
            }
        }
    };

    // Dynamic placeholder for schedule input
    let schedule_placeholder = move || match form_schedule_kind.get().as_str() {
        "every" => "5m, 2h, 30s",
        "at" => "1711944000",
        _ => "*/5 * * * *",
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    // Empty state — no selection
                    view! {
                        <div class="flex flex-col items-center justify-center h-full text-text-tertiary">
                            <svg class="w-16 h-16 mb-4 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <circle cx="12" cy="12" r="10" stroke-width="1.5"/>
                                <path d="M12 6v6l4 2" stroke-width="1.5" stroke-linecap="round"/>
                            </svg>
                            <span class="text-sm">{t!(i18n, cron.select_or_create)}</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { t_string!(i18n, cron.new_task_title) } else { t_string!(i18n, cron.edit_task_title) }}
                                </h2>
                                <p class="text-sm text-text-secondary">
                                    {t!(i18n, cron.form_description)}
                                </p>
                            </div>

                            // Error message
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            {move || {
                                if let Some(msg) = run_success.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                                            {msg}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="space-y-6">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_name)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder=move || t_string!(i18n, cron.placeholder_name).to_string()
                                    />
                                </div>

                                // Schedule Type + Schedule (grid 1/3 + 2/3)
                                <div class="grid grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_schedule_type)}
                                        </label>
                                        <select
                                            prop:value=move || form_schedule_kind.get()
                                            on:change=move |ev| form_schedule_kind.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        >
                                            <option value="cron">{t!(i18n, cron.schedule_type_cron)}</option>
                                            <option value="every">{t!(i18n, cron.schedule_type_every)}</option>
                                            <option value="at">{t!(i18n, cron.schedule_type_at)}</option>
                                        </select>
                                    </div>
                                    <div class="col-span-2">
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_schedule)}
                                        </label>
                                        {move || {
                                            if form_schedule_kind.get() == "at" {
                                                view! {
                                                    <input
                                                        type="datetime-local"
                                                        prop:value=move || form_schedule.get()
                                                        on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    />
                                                    <p class="mt-1.5 text-xs text-text-tertiary">
                                                        {t!(i18n, cron.at_hint)}
                                                    </p>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_schedule.get()
                                                        on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-primary"
                                                        placeholder=schedule_placeholder
                                                    />
                                                    <p class="mt-1.5 text-xs text-text-tertiary">
                                                        {move || match form_schedule_kind.get().as_str() {
                                                            "cron" => t_string!(i18n, cron.cron_hint),
                                                            "every" => t_string!(i18n, cron.every_hint),
                                                            _ => "",
                                                        }}
                                                    </p>
                                                }.into_any()
                                            }
                                        }}
                                    </div>
                                </div>

                                // Anchor / Stagger (conditional on schedule type)
                                {move || {
                                    let kind = form_schedule_kind.get();
                                    match kind.as_str() {
                                        "every" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    {t!(i18n, cron.field_anchor_ms)}
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_anchor_ms.get()
                                                    on:input=move |ev| form_anchor_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder=move || t_string!(i18n, cron.anchor_placeholder).to_string()
                                                />
                                            </div>
                                        }.into_any(),
                                        "cron" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    {t!(i18n, cron.field_stagger_ms)}
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_stagger_ms.get()
                                                    on:input=move |ev| form_stagger_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder=move || t_string!(i18n, cron.stagger_placeholder).to_string()
                                                />
                                            </div>
                                        }.into_any(),
                                        _ => view! { <div></div> }.into_any(),
                                    }
                                }}

                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_agent)}
                                    </label>
                                    <select
                                        on:change=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                    >
                                        {move || {
                                            let current = form_agent_id.get();
                                            let list = agents.get();
                                            let deleted_suffix =
                                                t_string!(i18n, cron.agent_deleted_suffix).to_string();
                                            let stale = stale_agent_option(&current, &list);
                                            let mut opts = list
                                                .into_iter()
                                                .map(|a| {
                                                    let id = a.id.clone();
                                                    let label = a.name.clone().unwrap_or_else(|| a.id.clone());
                                                    let sel = id == current;
                                                    view! {
                                                        <option value=id selected=sel>{label}</option>
                                                    }
                                                    .into_any()
                                                })
                                                .collect::<Vec<_>>();
                                            if let Some(stale_id) = stale {
                                                let label = format!("{stale_id}{deleted_suffix}");
                                                opts.push(view! {
                                                    <option value=stale_id selected=true disabled=true>
                                                        {label}
                                                    </option>
                                                }.into_any());
                                            }
                                            opts
                                        }}
                                    </select>
                                </div>

                                // Delivery channel (read-only)
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_channel)}
                                    </label>
                                    <div class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-tertiary text-sm">
                                        {move || match form_channel.get() {
                                            Some(ch) if !ch.is_empty() => ch,
                                            _ => t_string!(i18n, cron.channel_none).to_string(),
                                        }}
                                    </div>
                                </div>

                                // Prompt
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_prompt)}
                                    </label>
                                    <textarea
                                        prop:value=move || form_prompt.get()
                                        on:input=move |ev| form_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        rows="3"
                                        placeholder=move || t_string!(i18n, cron.placeholder_prompt).to_string()
                                    ></textarea>
                                </div>

                                // Timezone + Tags (grid 1/2 + 1/2)
                                <div class="grid grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_timezone)}
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_timezone.get()
                                            on:input=move |ev| form_timezone.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder=move || t_string!(i18n, cron.placeholder_timezone).to_string()
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_tags)}
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_tags.get()
                                            on:input=move |ev| form_tags.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder=move || t_string!(i18n, cron.placeholder_tags).to_string()
                                        />
                                    </div>
                                </div>

                                // Enabled toggle
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_status)}
                                    </label>
                                    <button
                                        on:click=move |_| form_enabled.set(!form_enabled.get())
                                        class=move || {
                                            if form_enabled.get() {
                                                "px-4 py-2 bg-success/20 border border-success text-success rounded-lg transition-colors text-sm font-medium"
                                            } else {
                                                "px-4 py-2 bg-surface-sunken border border-border text-text-tertiary rounded-lg transition-colors text-sm font-medium"
                                            }
                                        }
                                    >
                                        {move || if form_enabled.get() { t_string!(i18n, cron.enabled) } else { t_string!(i18n, cron.disabled) }}
                                    </button>
                                </div>

                                // Session Target
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_session_target)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_session_target.get()
                                        on:input=move |ev| form_session_target.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder=move || t_string!(i18n, cron.placeholder_session_target).to_string()
                                    />
                                </div>

                                // Failure Alert (collapsible)
                                <div class="border border-border rounded-lg">
                                    <button
                                        on:click=move |_| form_alert_expanded.set(!form_alert_expanded.get())
                                        class="w-full px-4 py-3 flex items-center gap-2 text-sm font-medium text-text-secondary hover:text-text-primary transition-colors"
                                    >
                                        <span>{move || if form_alert_expanded.get() { "\u{25BC}" } else { "\u{25B6}" }}</span>
                                        {t!(i18n, cron.field_failure_alert)}
                                    </button>
                                    {move || {
                                        if form_alert_expanded.get() {
                                            view! {
                                                <div class="px-4 pb-4 space-y-4 border-t border-border pt-3">
                                                    <div class="grid grid-cols-2 gap-4">
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_after_n)}
                                                            </label>
                                                            <input
                                                                type="number"
                                                                prop:value=move || form_alert_after.get()
                                                                on:input=move |ev| form_alert_after.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder="2"
                                                            />
                                                        </div>
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_cooldown)}
                                                            </label>
                                                            <input
                                                                type="text"
                                                                prop:value=move || form_alert_cooldown.get()
                                                                on:input=move |ev| form_alert_cooldown.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder="1h"
                                                            />
                                                        </div>
                                                    </div>
                                                    <div class="grid grid-cols-2 gap-4">
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_to)}
                                                            </label>
                                                            <select
                                                                prop:value=move || form_alert_kind.get()
                                                                on:change=move |ev| form_alert_kind.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                            >
                                                                <option value="announce">{t!(i18n, cron.alert_announce)}</option>
                                                                <option value="webhook">{t!(i18n, cron.alert_webhook)}</option>
                                                            </select>
                                                        </div>
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_channel_url)}
                                                            </label>
                                                            <input
                                                                type="text"
                                                                prop:value=move || form_alert_channel.get()
                                                                on:input=move |ev| form_alert_channel.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder=move || t_string!(i18n, cron.alert_channel_placeholder).to_string()
                                                            />
                                                        </div>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <div></div> }.into_any()
                                        }
                                    }}
                                </div>
                            </div>

                            // Actions
                            <div class="mt-8 flex items-center gap-3">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { t_string!(i18n, cron.btn_saving) } else { t_string!(i18n, cron.btn_save) }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_run_now
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-success/80 hover:bg-success disabled:bg-success/40 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                {t!(i18n, cron.btn_run_now)}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                prop:disabled=move || saving.get()
                                                class=move || {
                                                    if confirm_delete.get() {
                                                        "px-6 py-2 bg-danger hover:bg-danger/80 disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed ring-2 ring-danger/50 ring-offset-1 ring-offset-surface animate-pulse"
                                                    } else {
                                                        "px-6 py-2 bg-danger/80 hover:bg-danger disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                                    }
                                                }
                                            >
                                                {move || if confirm_delete.get() { t_string!(i18n, cron.btn_confirm_delete) } else { t_string!(i18n, cron.btn_delete) }}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-sunken text-text-primary rounded-lg transition-colors"
                                >
                                    {t!(i18n, cron.btn_cancel)}
                                </button>
                            </div>

                            // Run History (existing jobs only)
                            {move || {
                                if !is_new() {
                                    view! {
                                        <div class="mt-10">
                                            <RunHistory runs=runs />
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

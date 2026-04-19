use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{AcpApi, AcpHarnessConfig, AcpHarnessInfo, AcpPresetMeta};
use crate::context::DashboardState;
use crate::i18n::*;

const PRESET_ICON_COLORS: &[&str] = &[
    "#F97316", "#3B82F6", "#10B981", "#8B5CF6", "#EF4444",
    "#06B6D4", "#F59E0B", "#EC4899", "#6366F1", "#14B8A6",
    "#84CC16", "#D946EF", "#F43F5E", "#0EA5E9", "#A855F7",
    "#22C55E",
];

fn preset_icon_color(id: &str) -> &'static str {
    let hash = id.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as u32)
    });
    let idx = (hash as usize) % PRESET_ICON_COLORS.len();
    PRESET_ICON_COLORS[idx]
}

/// ACP Harnesses settings page — manages external CLI tools
#[component]
pub fn AcpHarnessesView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let harnesses = RwSignal::new(Vec::<AcpHarnessInfo>::new());
    let preset_metas = RwSignal::new(Vec::<AcpPresetMeta>::new());
    let loading = RwSignal::new(true);
    let selected_id = RwSignal::new(Option::<String>::None);
    let show_add_form = RwSignal::new(false);
    let acp_enabled = RwSignal::new(true);
    let toggling = RwSignal::new(false);

    // Load ACP enabled state + harness list + preset metadata on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                if let Ok(enabled) = AcpApi::get_acp_enabled(&state).await {
                    acp_enabled.set(enabled);
                }

                match AcpApi::presets_meta(&state).await {
                    Ok(metas) => preset_metas.set(metas),
                    Err(_) => preset_metas.set(vec![]),
                }

                loading.set(true);
                match AcpApi::list(&state).await {
                    Ok(list) => {
                        if selected_id.get_untracked().is_none() {
                            if let Some(first) = list.first() {
                                selected_id.set(Some(first.id.clone()));
                            }
                        }
                        harnesses.set(list);
                    }
                    Err(_) => {
                        harnesses.set(vec![]);
                    }
                }
                loading.set(false);
            });
        }
    });

    // Toggle ACP enabled state
    let on_toggle_acp = move |_| {
        let new_val = !acp_enabled.get_untracked();
        toggling.set(true);
        spawn_local(async move {
            match AcpApi::set_acp_enabled(&state, new_val).await {
                Ok(()) => {
                    acp_enabled.set(new_val);
                    // Note: ACP handler registration requires server restart.
                    // The toggle persists the config so next restart picks it up.
                }
                Err(_e) => {
                    // Revert on failure
                }
            }
            toggling.set(false);
        });
    };

    view! {
        <div class="flex h-full">
            // Left panel — harness list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
                <div class="px-6 py-4 border-b border-border">
                    <div class="flex items-center justify-between">
                        <div>
                            <h1 class="text-2xl font-semibold text-text-primary">
                                {t!(i18n, settings.acp.title)}
                            </h1>
                            <p class="mt-1 text-sm text-text-secondary">
                                {t!(i18n, settings.acp.description)}
                            </p>
                        </div>
                        <button
                            on:click=on_toggle_acp
                            disabled=move || toggling.get()
                            class="relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none"
                            class:bg-primary=move || acp_enabled.get()
                            class:bg-gray-500=move || !acp_enabled.get()
                            class:opacity-50=move || toggling.get()
                            title=t_string!(i18n, settings.acp.toggle_hint).to_string()
                        >
                            <span
                                class="pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out"
                                class:translate-x-5=move || acp_enabled.get()
                                class:translate-x-0=move || !acp_enabled.get()
                            />
                        </button>
                    </div>
                </div>

                <div class="flex-1 overflow-auto">
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">{t!(i18n, settings.acp.loading)}</div>
                                </div>
                            }.into_any()
                        } else {
                            let all = harnesses.get();
                            let metas = preset_metas.get();

                            let preset_harnesses: Vec<AcpHarnessInfo> = all.iter()
                                .filter(|h| h.preset.is_some())
                                .cloned()
                                .collect();
                            let custom_harnesses: Vec<AcpHarnessInfo> = all.iter()
                                .filter(|h| h.preset.is_none())
                                .cloned()
                                .collect();

                            view! {
                                <div class="p-6 space-y-4">
                                    // Preset CLI section
                                    <div>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                            {t!(i18n, settings.acp.preset_cli)}
                                        </h2>
                                        <div class="grid grid-cols-1 gap-2">
                                            {metas.into_iter().map(|preset| {
                                                let pid = preset.id.clone();
                                                let pname = preset.display_name.clone();
                                                let color = preset_icon_color(&preset.id).to_string();
                                                let first_char = pname.chars().next().unwrap_or('?').to_uppercase().to_string();

                                                // Find matching harness info
                                                let info = preset_harnesses.iter().find(|h| h.id == pid).cloned();
                                                let is_available = info.as_ref().map(|h| h.available).unwrap_or(false);
                                                let is_enabled = info.as_ref().map(|h| h.enabled).unwrap_or(false);
                                                let pid_click = pid.clone();
                                                let pid_check = pid.clone();

                                                view! {
                                                    <button
                                                        on:click=move |_| {
                                                            selected_id.set(Some(pid_click.clone()));
                                                            show_add_form.set(false);
                                                        }
                                                        class=move || {
                                                            let base = "text-left p-3 rounded-lg border transition-all w-full";
                                                            let is_sel = selected_id.get().as_deref() == Some(&pid_check) && !show_add_form.get();
                                                            if is_sel {
                                                                format!("{} bg-primary-subtle border-primary", base)
                                                            } else {
                                                                format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                                                            }
                                                        }
                                                    >
                                                        <div class="flex items-center gap-3">
                                                            <div
                                                                class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                                style=format!("background-color: {}", color)
                                                            >
                                                                {first_char}
                                                            </div>
                                                            <div class="min-w-0 flex-1">
                                                                <div class="flex items-center gap-2">
                                                                    <span class="font-medium text-text-primary text-sm truncate">
                                                                        {pname}
                                                                    </span>
                                                                    {if is_available {
                                                                        view! {
                                                                            <span class="px-1.5 py-0.5 bg-success/10 text-success text-xs rounded-full shrink-0">
                                                                                {t!(i18n, settings.acp.installed)}
                                                                            </span>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <span class="px-1.5 py-0.5 bg-text-tertiary/10 text-text-tertiary text-xs rounded-full shrink-0">
                                                                                {t!(i18n, settings.acp.not_installed)}
                                                                            </span>
                                                                        }.into_any()
                                                                    }}
                                                                </div>
                                                                <div class="text-xs text-text-tertiary truncate flex items-center gap-1.5 mt-0.5">
                                                                    {if is_enabled {
                                                                        view! {
                                                                            <span class="w-1.5 h-1.5 rounded-full bg-success inline-block"></span>
                                                                            <span>{t!(i18n, settings.acp.enabled)}</span>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <span class="w-1.5 h-1.5 rounded-full bg-text-tertiary/40 inline-block"></span>
                                                                            <span>{t!(i18n, settings.acp.disabled)}</span>
                                                                        }.into_any()
                                                                    }}
                                                                </div>
                                                            </div>
                                                        </div>
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>

                                    // Custom CLI section
                                    {if !custom_harnesses.is_empty() {
                                        let custom_list = custom_harnesses.clone();
                                        view! {
                                            <div>
                                                <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                    {t!(i18n, settings.acp.custom_cli)}
                                                </h2>
                                                <div class="grid grid-cols-1 gap-2">
                                                    {custom_list.into_iter().map(|h| {
                                                        let hid = h.id.clone();
                                                        let hname = h.display_name.clone();
                                                        let is_available = h.available;
                                                        let is_enabled = h.enabled;
                                                        let first_char = hname.chars().next().unwrap_or('?').to_uppercase().to_string();
                                                        let hid_click = hid.clone();
                                                        let hid_check = hid.clone();

                                                        view! {
                                                            <button
                                                                on:click=move |_| {
                                                                    selected_id.set(Some(hid_click.clone()));
                                                                    show_add_form.set(false);
                                                                }
                                                                class=move || {
                                                                    let base = "text-left p-3 rounded-lg border transition-all w-full";
                                                                    let is_sel = selected_id.get().as_deref() == Some(&hid_check) && !show_add_form.get();
                                                                    if is_sel {
                                                                        format!("{} bg-primary-subtle border-primary", base)
                                                                    } else {
                                                                        format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                                                                    }
                                                                }
                                                            >
                                                                <div class="flex items-center gap-3">
                                                                    <div class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0 bg-text-tertiary">
                                                                        {first_char}
                                                                    </div>
                                                                    <div class="min-w-0 flex-1">
                                                                        <div class="flex items-center gap-2">
                                                                            <span class="font-medium text-text-primary text-sm truncate">
                                                                                {hname}
                                                                            </span>
                                                                            {if is_available {
                                                                                view! {
                                                                                    <span class="px-1.5 py-0.5 bg-success/10 text-success text-xs rounded-full shrink-0">
                                                                                        {t!(i18n, settings.acp.installed)}
                                                                                    </span>
                                                                                }.into_any()
                                                                            } else {
                                                                                view! {
                                                                                    <span class="px-1.5 py-0.5 bg-text-tertiary/10 text-text-tertiary text-xs rounded-full shrink-0">
                                                                                        {t!(i18n, settings.acp.not_installed)}
                                                                                    </span>
                                                                                }.into_any()
                                                                            }}
                                                                        </div>
                                                                        <div class="text-xs text-text-tertiary truncate flex items-center gap-1.5 mt-0.5">
                                                                            {if is_enabled {
                                                                                view! {
                                                                                    <span class="w-1.5 h-1.5 rounded-full bg-success inline-block"></span>
                                                                                    <span>{t!(i18n, settings.acp.enabled)}</span>
                                                                                }.into_any()
                                                                            } else {
                                                                                view! {
                                                                                    <span class="w-1.5 h-1.5 rounded-full bg-text-tertiary/40 inline-block"></span>
                                                                                    <span>{t!(i18n, settings.acp.disabled)}</span>
                                                                                }.into_any()
                                                                            }}
                                                                        </div>
                                                                    </div>
                                                                </div>
                                                            </button>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }}

                                    // Add Custom CLI button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                show_add_form.set(true);
                                                selected_id.set(None);
                                            }
                                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                                        >
                                            {t!(i18n, settings.acp.add_custom_cli)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Right panel — detail / add form
            <div class="flex-1 flex flex-col overflow-hidden">
                {move || {
                    if loading.get() {
                        view! { <div></div> }.into_any()
                    } else if show_add_form.get() {
                        view! {
                            <AddHarnessPanel
                                harnesses=harnesses
                                show_add_form=show_add_form
                                selected_id=selected_id
                            />
                        }.into_any()
                    } else if let Some(sel_id) = selected_id.get() {
                        view! {
                            <HarnessDetailPanel
                                harness_id=sel_id
                                harnesses=harnesses
                                selected_id=selected_id
                                preset_metas=preset_metas
                            />
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary">
                                <div class="text-center">
                                    <p class="text-lg">{t!(i18n, settings.acp.select_harness)}</p>
                                    <p class="text-sm text-text-tertiary mt-1">{t!(i18n, settings.acp.add_harness_hint)}</p>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// Harness Detail Panel
// ============================================================================

#[component]
fn HarnessDetailPanel(
    harness_id: String,
    harnesses: RwSignal<Vec<AcpHarnessInfo>>,
    selected_id: RwSignal<Option<String>>,
    preset_metas: RwSignal<Vec<AcpPresetMeta>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let info = harnesses.get().into_iter().find(|h| h.id == harness_id);
    let Some(info) = info else {
        return view! {
            <div class="flex items-center justify-center h-full text-text-tertiary">
                {t!(i18n, settings.acp.harness_not_found)}
            </div>
        }
        .into_any();
    };

    let is_preset = info.preset.is_some();
    let display_name_label = info.display_name.clone();

    // Form state
    let executable = RwSignal::new(info.config.executable.clone().unwrap_or_default());
    // Use the top-level `mode` field (computed by server from actual harness instance),
    // not `config.mode` (which maps to `default_mode` serialized as `"default_mode"` key).
    let mode = RwSignal::new(info.mode.clone());
    let timeout_seconds = RwSignal::new(info.config.timeout_seconds);
    let args_text = RwSignal::new(info.config.args.join(", "));
    let output_format_type = RwSignal::new({
        if let Some(obj) = info.config.output_format.as_object() {
            if obj.contains_key("json") {
                "json".to_string()
            } else {
                "plain_text".to_string()
            }
        } else {
            "plain_text".to_string()
        }
    });
    let json_field_name = RwSignal::new({
        info.config
            .output_format
            .as_object()
            .and_then(|obj| obj.get("json"))
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get("field"))
            .and_then(|v| v.as_str())
            .unwrap_or("result")
            .to_string()
    });
    let env_pairs = RwSignal::new(
        info.config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<(String, String)>>(),
    );
    let cwd = RwSignal::new(info.config.cwd.clone().unwrap_or_default());
    let enabled = RwSignal::new(info.enabled);
    let show_advanced = RwSignal::new(false);

    // Action states
    let (testing, set_testing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (test_result, set_test_result) = signal(Option::<(bool, String)>::None);
    let (save_success, set_save_success) = signal(false);
    let (action_error, set_action_error) = signal(Option::<String>::None);
    let (deleting, set_deleting) = signal(false);

    let hid = harness_id.clone();
    let hid_for_preset = harness_id.clone();
    let display_name_str = info.display_name.clone();

    // Build config from form state
    let build_config = move || -> AcpHarnessConfig {
        let args: Vec<String> = args_text
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let output_format = if mode.get() == "oneshot" && output_format_type.get() == "json" {
            serde_json::json!({ "json": { "field": json_field_name.get() } })
        } else {
            serde_json::json!("plain_text")
        };

        let env: std::collections::HashMap<String, String> = env_pairs
            .get()
            .into_iter()
            .filter(|(k, _)| !k.is_empty())
            .collect();

        let cwd_val = {
            let c = cwd.get();
            if c.is_empty() {
                None
            } else {
                Some(c)
            }
        };

        let exe_val = {
            let e = executable.get();
            if e.is_empty() {
                None
            } else {
                Some(e)
            }
        };

        AcpHarnessConfig {
            display_name: display_name_str.clone(),
            executable: exe_val,
            args,
            default_mode: mode.get(),
            output_format,
            env,
            cwd: cwd_val,
            timeout_seconds: timeout_seconds.get(),
            enabled: enabled.get(),
            preset: if is_preset {
                Some(hid_for_preset.clone())
            } else {
                None
            },
        }
    };

    // Test handler
    let hid_test = hid.clone();
    let handle_test = move |_| {
        set_testing.set(true);
        set_test_result.set(None);
        set_action_error.set(None);

        let id = hid_test.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            match AcpApi::test(&state, &id).await {
                Ok(resp) => {
                    if resp.success {
                        set_test_result.set(Some((
                            true,
                            format!("Success! {} ({}ms)", resp.message, resp.duration_ms),
                        )));
                    } else {
                        set_test_result.set(Some((false, resp.message)));
                    }
                }
                Err(e) => {
                    set_test_result.set(Some((false, format!("RPC error: {}", e))));
                }
            }
            set_testing.set(false);
        });
    };

    // Save handler
    let build_for_save = build_config.clone();
    let hid_save = hid.clone();
    let handle_save = move |_| {
        set_action_error.set(None);
        set_save_success.set(false);
        set_saving.set(true);

        let config = build_for_save();
        let id = hid_save.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            match AcpApi::update(&state, &id, &config).await {
                Ok(updated) => {
                    // Update the harness in the list
                    harnesses.update(|list| {
                        if let Some(h) = list.iter_mut().find(|h| h.id == updated.id) {
                            *h = updated;
                        }
                    });
                    set_save_success.set(true);
                    set_timeout(
                        move || set_save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    set_action_error.set(Some(format!("Save failed: {}", e)));
                }
            }
            set_saving.set(false);
        });
    };

    // Toggle enabled handler
    let hid_toggle = hid.clone();
    let handle_toggle = move |_| {
        let new_val = !enabled.get();
        enabled.set(new_val);
        let id = hid_toggle.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            match AcpApi::set_enabled(&state, &id, new_val).await {
                Ok(_) => {
                    harnesses.update(|list| {
                        if let Some(h) = list.iter_mut().find(|h| h.id == id) {
                            h.enabled = new_val;
                        }
                    });
                }
                Err(e) => {
                    // Revert on error
                    enabled.set(!new_val);
                    set_action_error.set(Some(format!("Toggle failed: {}", e)));
                }
            }
        });
    };

    // Delete handler (custom only)
    let hid_delete = hid.clone();
    let handle_delete = move |_| {
        set_deleting.set(true);
        set_action_error.set(None);
        let id = hid_delete.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            match AcpApi::delete(&state, &id).await {
                Ok(_) => {
                    harnesses.update(|list| list.retain(|h| h.id != id));
                    selected_id.set(None);
                }
                Err(e) => {
                    set_action_error.set(Some(format!("Delete failed: {}", e)));
                }
            }
            set_deleting.set(false);
        });
    };

    let is_available = info.available;

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {display_name_label}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {t!(i18n, settings.acp.harness_config_desc)}
                        </p>
                    </div>
                    <div class="flex gap-2 items-center">
                        {if is_available {
                            view! {
                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-success/10 text-success">
                                    {t!(i18n, settings.acp.installed)}
                                </span>
                            }.into_any()
                        } else {
                            view! {
                                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-text-tertiary/10 text-text-tertiary">
                                    {t!(i18n, settings.acp.not_installed)}
                                </span>
                            }.into_any()
                        }}
                    </div>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.acp.config_section)}</h3>

                // Executable
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.executable_path)}
                    </label>
                    <input
                        type="text"
                        value=move || executable.get()
                        on:input=move |ev| executable.set(event_target_value(&ev))
                        placeholder="e.g. claude, codex, gemini"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.executable_hint)}</p>
                </div>

                // Mode
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.mode_label)}
                    </label>
                    <select
                        on:change=move |ev| mode.set(event_target_value(&ev))
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    >
                        <option value="native_acp" selected=move || mode.get() == "native_acp">{t!(i18n, settings.acp.mode_native_acp)}</option>
                        <option value="oneshot" selected=move || mode.get() == "oneshot">{t!(i18n, settings.acp.mode_oneshot)}</option>
                    </select>
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.mode_hint)}</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.timeout_label)}
                    </label>
                    <input
                        type="number"
                        min="1"
                        value=move || timeout_seconds.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                timeout_seconds.set(v);
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
            </div>

            // Advanced Settings card (collapsible)
            <div class="bg-surface-raised border border-border rounded-xl overflow-hidden">
                <button
                    on:click=move |_| show_advanced.update(|v| *v = !*v)
                    class="w-full px-4 py-3 flex items-center justify-between text-left hover:bg-surface-sunken/50 transition-colors"
                >
                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.acp.advanced_settings)}</h3>
                    <span class="text-text-tertiary text-sm">
                        {move || if show_advanced.get() { t_string!(i18n, settings.acp.hide_advanced).to_string() } else { t_string!(i18n, settings.acp.show_advanced).to_string() }}
                    </span>
                </button>

                {move || if show_advanced.get() {
                    view! {
                        <div class="px-4 pb-4 space-y-4 border-t border-border pt-4">

                        // Args
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                {t!(i18n, settings.acp.arguments_label)}
                            </label>
                            <input
                                type="text"
                                value=move || args_text.get()
                                on:input=move |ev| args_text.set(event_target_value(&ev))
                                placeholder="e.g. --print, --no-input"
                                class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                            />
                            <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.arguments_hint)}</p>
                        </div>

                        // Output Format (only for oneshot)
                        {move || if mode.get() == "oneshot" {
                            view! {
                                <div class="space-y-3">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-1">
                                            {t!(i18n, settings.acp.output_format)}
                                        </label>
                                        <select
                                            on:change=move |ev| output_format_type.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        >
                                            <option value="plain_text" selected=move || output_format_type.get() == "plain_text">{t!(i18n, settings.acp.output_plain_text)}</option>
                                            <option value="json" selected=move || output_format_type.get() == "json">{t!(i18n, settings.acp.output_json)}</option>
                                        </select>
                                    </div>
                                    {move || if output_format_type.get() == "json" {
                                        view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                                    {t!(i18n, settings.acp.json_field_name)}
                                                </label>
                                                <input
                                                    type="text"
                                                    value=move || json_field_name.get()
                                                    on:input=move |ev| json_field_name.set(event_target_value(&ev))
                                                    placeholder="result"
                                                    class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                />
                                                <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.json_field_hint)}</p>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }}
                                </div>
                            }.into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }}

                        // Environment Variables
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                {t!(i18n, settings.acp.env_vars_label)}
                            </label>
                            <div class="space-y-2">
                                {move || env_pairs.get().into_iter().enumerate().map(|(i, (k, v))| {
                                    let k_val = k.clone();
                                    let v_val = v.clone();
                                    view! {
                                        <div class="flex gap-2 items-center">
                                            <input
                                                type="text"
                                                value=k_val
                                                on:input=move |ev| {
                                                    env_pairs.update(|pairs| {
                                                        if let Some(pair) = pairs.get_mut(i) {
                                                            pair.0 = event_target_value(&ev);
                                                        }
                                                    });
                                                }
                                                placeholder="KEY"
                                                class="flex-1 px-3 py-2 border border-border rounded bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                            />
                                            <span class="text-text-tertiary">"="</span>
                                            <input
                                                type="text"
                                                value=v_val
                                                on:input=move |ev| {
                                                    env_pairs.update(|pairs| {
                                                        if let Some(pair) = pairs.get_mut(i) {
                                                            pair.1 = event_target_value(&ev);
                                                        }
                                                    });
                                                }
                                                placeholder="VALUE"
                                                class="flex-1 px-3 py-2 border border-border rounded bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                            />
                                            <button
                                                on:click=move |_| {
                                                    env_pairs.update(|pairs| { pairs.remove(i); });
                                                }
                                                class="px-2 py-2 text-danger hover:bg-danger/10 rounded transition-colors"
                                                title=t_string!(i18n, settings.acp.env_remove).to_string()
                                            >
                                                "x"
                                            </button>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                            <button
                                on:click=move |_| {
                                    env_pairs.update(|pairs| pairs.push((String::new(), String::new())));
                                }
                                class="mt-2 px-3 py-1.5 text-xs text-primary border border-primary/30 rounded-lg hover:bg-primary/5 transition-colors"
                            >
                                {t!(i18n, settings.acp.add_variable)}
                            </button>
                        </div>

                        // Working Directory
                        <div>
                            <label class="block text-sm font-medium text-text-secondary mb-1">
                                {t!(i18n, settings.acp.working_directory)}
                            </label>
                            <input
                                type="text"
                                value=move || cwd.get()
                                on:input=move |ev| cwd.set(event_target_value(&ev))
                                placeholder="Leave empty for default"
                                class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                            />
                        </div>

                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }}
            </div>

            // Test result
            {move || {
                if let Some((success, message)) = test_result.get() {
                    if success {
                        view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded-lg">
                                <p class="text-sm text-success">{message}</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg">
                                <p class="text-sm text-danger">{message}</p>
                            </div>
                        }.into_any()
                    }
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Save success
            {move || save_success.get().then(|| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{t!(i18n, settings.acp.saved)}</div>
            })}

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Actions
            <div class="flex flex-row gap-3 pt-2">
                <button
                    on:click=handle_test
                    disabled=move || testing.get()
                    class="flex-1 px-4 py-2.5 bg-info text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if testing.get() { t_string!(i18n, settings.acp.testing).to_string() } else { t_string!(i18n, settings.acp.test_connection).to_string() }}
                </button>

                <button
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>

                <button
                    on:click=handle_toggle
                    class=move || {
                        if enabled.get() {
                            "px-4 py-2.5 bg-surface border border-border text-text-secondary rounded-lg hover:bg-surface-raised transition-colors font-medium"
                        } else {
                            "px-4 py-2.5 bg-success/10 text-success border border-success/20 rounded-lg hover:bg-success/20 transition-colors font-medium"
                        }
                    }
                >
                    {move || if enabled.get() { t_string!(i18n, settings.acp.disable).to_string() } else { t_string!(i18n, settings.acp.enable).to_string() }}
                </button>

                {if !is_preset {
                    view! {
                        <button
                            on:click=handle_delete
                            disabled=move || deleting.get()
                            class="px-4 py-2.5 bg-danger/10 text-danger border border-danger/20 rounded-lg hover:bg-danger/20 transition-colors font-medium"
                        >
                            {move || if deleting.get() { t_string!(i18n, settings.acp.deleting).to_string() } else { t_string!(i18n, settings.acp.delete).to_string() }}
                        </button>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }.into_any()
}

// ============================================================================
// Add Harness Panel
// ============================================================================

#[component]
fn AddHarnessPanel(
    harnesses: RwSignal<Vec<AcpHarnessInfo>>,
    show_add_form: RwSignal<bool>,
    selected_id: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    // Form state
    let id = RwSignal::new(String::new());
    let display_name = RwSignal::new(String::new());
    let executable = RwSignal::new(String::new());
    let mode = RwSignal::new("oneshot".to_string());
    let timeout_seconds = RwSignal::new(300u64);
    let args_text = RwSignal::new(String::new());
    let output_format_type = RwSignal::new("plain_text".to_string());
    let json_field_name = RwSignal::new("result".to_string());
    let env_pairs = RwSignal::new(Vec::<(String, String)>::new());
    let cwd = RwSignal::new(String::new());

    let (saving, set_saving) = signal(false);
    let (action_error, set_action_error) = signal(Option::<String>::None);
    let (id_error, set_id_error) = signal(Option::<String>::None);

    // ID validation
    let validate_id = move |val: &str| -> bool {
        !val.is_empty()
            && val
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };

    let handle_save = move |_| {
        // Validate ID
        let id_val = id.get();
        if id_val.is_empty() {
            set_id_error.set(Some("ID is required".to_string()));
            return;
        }
        if !validate_id(&id_val) {
            set_id_error.set(Some(
                "ID must only contain lowercase letters, digits, and hyphens".to_string(),
            ));
            return;
        }
        set_id_error.set(None);

        if display_name.get().is_empty() {
            set_action_error.set(Some("Display name is required".to_string()));
            return;
        }
        if executable.get().is_empty() {
            set_action_error.set(Some("Executable path is required".to_string()));
            return;
        }

        set_saving.set(true);
        set_action_error.set(None);

        let args: Vec<String> = args_text
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let output_format = if mode.get() == "oneshot" && output_format_type.get() == "json" {
            serde_json::json!({ "json": { "field": json_field_name.get() } })
        } else {
            serde_json::json!("plain_text")
        };

        let env: std::collections::HashMap<String, String> = env_pairs
            .get()
            .into_iter()
            .filter(|(k, _)| !k.is_empty())
            .collect();

        let cwd_val = {
            let c = cwd.get();
            if c.is_empty() {
                None
            } else {
                Some(c)
            }
        };

        let config = AcpHarnessConfig {
            display_name: display_name.get(),
            executable: Some(executable.get()),
            args,
            default_mode: mode.get(),
            output_format,
            env,
            cwd: cwd_val,
            timeout_seconds: timeout_seconds.get(),
            enabled: true,
            preset: None,
        };

        let create_id = id_val.clone();
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            match AcpApi::create(&state, &create_id, &config).await {
                Ok(created) => {
                    let new_id = created.id.clone();
                    harnesses.update(|list| list.push(created));
                    show_add_form.set(false);
                    selected_id.set(Some(new_id));
                }
                Err(e) => {
                    set_action_error.set(Some(format!("Create failed: {}", e)));
                }
            }
            set_saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Fixed header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <div>
                        <h2 class="text-lg font-semibold text-text-primary">
                            {t!(i18n, settings.acp.add_custom_cli_title)}
                        </h2>
                        <p class="text-sm text-text-tertiary mt-0.5">
                            {t!(i18n, settings.acp.add_custom_cli_desc)}
                        </p>
                    </div>
                    <button
                        on:click=move |_| {
                            show_add_form.set(false);
                            // Re-select first harness if any
                            if let Some(first) = harnesses.get().first() {
                                selected_id.set(Some(first.id.clone()));
                            }
                        }
                        class="px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary border border-border rounded-lg hover:bg-surface-raised transition-colors"
                    >
                        {t!(i18n, settings.acp.cancel)}
                    </button>
                </div>
            </div>

            // Scrollable content
            <div class="flex-1 overflow-y-auto p-6 space-y-6">

            // Identity card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.acp.identity_section)}</h3>

                // ID
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.id_label)}
                        <span class="text-danger ml-1">"*"</span>
                    </label>
                    <input
                        type="text"
                        value=move || id.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            id.set(val.clone());
                            if validate_id(&val) || val.is_empty() {
                                set_id_error.set(None);
                            }
                        }
                        placeholder="e.g. my-custom-cli"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                    />
                    {move || id_error.get().map(|e| view! {
                        <p class="text-xs text-danger mt-1">{e}</p>
                    })}
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.id_hint)}</p>
                </div>

                // Display Name
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.display_name_label)}
                        <span class="text-danger ml-1">"*"</span>
                    </label>
                    <input
                        type="text"
                        value=move || display_name.get()
                        on:input=move |ev| display_name.set(event_target_value(&ev))
                        placeholder="e.g. My Custom CLI"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
            </div>

            // Configuration card
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">{t!(i18n, settings.acp.config_section)}</h3>

                // Executable
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.executable_path)}
                        <span class="text-danger ml-1">"*"</span>
                    </label>
                    <input
                        type="text"
                        value=move || executable.get()
                        on:input=move |ev| executable.set(event_target_value(&ev))
                        placeholder="e.g. /usr/local/bin/my-tool"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Mode
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.mode_label)}
                    </label>
                    <select
                        on:change=move |ev| mode.set(event_target_value(&ev))
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    >
                        <option value="oneshot" selected=move || mode.get() == "oneshot">{t!(i18n, settings.acp.mode_oneshot)}</option>
                        <option value="native_acp" selected=move || mode.get() == "native_acp">{t!(i18n, settings.acp.mode_native_acp)}</option>
                    </select>
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.mode_hint_add)}</p>
                </div>

                // Timeout
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.timeout_label)}
                    </label>
                    <input
                        type="number"
                        min="1"
                        value=move || timeout_seconds.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                timeout_seconds.set(v);
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Args
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.arguments_label)}
                    </label>
                    <input
                        type="text"
                        value=move || args_text.get()
                        on:input=move |ev| args_text.set(event_target_value(&ev))
                        placeholder="e.g. --print, --no-input"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.acp.arguments_hint)}</p>
                </div>

                // Output Format (only for oneshot)
                {move || if mode.get() == "oneshot" {
                    view! {
                        <div class="space-y-3">
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    {t!(i18n, settings.acp.output_format)}
                                </label>
                                <select
                                    on:change=move |ev| output_format_type.set(event_target_value(&ev))
                                    class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                                >
                                    <option value="plain_text" selected=move || output_format_type.get() == "plain_text">{t!(i18n, settings.acp.output_plain_text)}</option>
                                    <option value="json" selected=move || output_format_type.get() == "json">{t!(i18n, settings.acp.output_json)}</option>
                                </select>
                            </div>
                            {move || if output_format_type.get() == "json" {
                                view! {
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-1">
                                            {t!(i18n, settings.acp.json_field_name)}
                                        </label>
                                        <input
                                            type="text"
                                            value=move || json_field_name.get()
                                            on:input=move |ev| json_field_name.set(event_target_value(&ev))
                                            placeholder="result"
                                            class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }}

                // Environment Variables
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.env_vars_label)}
                    </label>
                    <div class="space-y-2">
                        {move || env_pairs.get().into_iter().enumerate().map(|(i, (k, v))| {
                            let k_val = k.clone();
                            let v_val = v.clone();
                            view! {
                                <div class="flex gap-2 items-center">
                                    <input
                                        type="text"
                                        value=k_val
                                        on:input=move |ev| {
                                            env_pairs.update(|pairs| {
                                                if let Some(pair) = pairs.get_mut(i) {
                                                    pair.0 = event_target_value(&ev);
                                                }
                                            });
                                        }
                                        placeholder="KEY"
                                        class="flex-1 px-3 py-2 border border-border rounded bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                    />
                                    <span class="text-text-tertiary">"="</span>
                                    <input
                                        type="text"
                                        value=v_val
                                        on:input=move |ev| {
                                            env_pairs.update(|pairs| {
                                                if let Some(pair) = pairs.get_mut(i) {
                                                    pair.1 = event_target_value(&ev);
                                                }
                                            });
                                        }
                                        placeholder="VALUE"
                                        class="flex-1 px-3 py-2 border border-border rounded bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                                    />
                                    <button
                                        on:click=move |_| {
                                            env_pairs.update(|pairs| { pairs.remove(i); });
                                        }
                                        class="px-2 py-2 text-danger hover:bg-danger/10 rounded transition-colors"
                                        title=t_string!(i18n, settings.acp.env_remove).to_string()
                                    >
                                        "x"
                                    </button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                    <button
                        on:click=move |_| {
                            env_pairs.update(|pairs| pairs.push((String::new(), String::new())));
                        }
                        class="mt-2 px-3 py-1.5 text-xs text-primary border border-primary/30 rounded-lg hover:bg-primary/5 transition-colors"
                    >
                        {t!(i18n, settings.acp.add_variable)}
                    </button>
                </div>

                // Working Directory
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.acp.working_directory)}
                    </label>
                    <input
                        type="text"
                        value=move || cwd.get()
                        on:input=move |ev| cwd.set(event_target_value(&ev))
                        placeholder="Leave empty for default"
                        class="w-full px-3 py-2 border border-border rounded bg-surface text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>
            </div>

            // Action error
            {move || action_error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Actions
            <div class="flex flex-row gap-3 pt-2">
                <button
                    on:click=handle_save
                    disabled=move || saving.get()
                    class="flex-1 px-4 py-2.5 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors font-medium"
                >
                    {move || if saving.get() { t_string!(i18n, settings.acp.creating).to_string() } else { t_string!(i18n, settings.acp.create).to_string() }}
                </button>
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }
}

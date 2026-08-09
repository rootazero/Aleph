use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{AcpApi, AcpHarnessConfig, AcpHarnessInfo, AcpPresetMeta};
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub(super) fn HarnessDetailPanel(
    harness_id: String,
    harnesses: RwSignal<Vec<AcpHarnessInfo>>,
    selected_id: RwSignal<Option<String>>,
    preset_metas: RwSignal<Vec<AcpPresetMeta>>,
) -> impl IntoView {
    // `preset_metas` is a builder-required prop reserved for an upcoming
    // preset picker; silence the unused-variable warning until it's wired.
    let _ = preset_metas;
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
    let hid_for_preset = harness_id;
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
                    set_test_result.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("RPC error: {e}")
                        }),
                    )));
                }
            }
            set_testing.set(false);
        });
    };

    // Save handler
    let build_for_save = build_config;
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
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Save failed: {e}")
                        }),
                    ));
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
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Toggle failed: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Delete handler (custom only)
    let hid_delete = hid;
    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
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
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Delete failed: {e}")
                        }),
                    ));
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
                                    let k_val = k;
                                    let v_val = v;
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
                        {move || if confirming.get() {
                            view! {
                                <ConfirmButton confirming=confirming on_confirm=on_confirm_delete.clone() />
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    on:click=move |_| confirming.set(true)
                                    disabled=move || deleting.get()
                                    class="px-4 py-2.5 bg-danger/10 text-danger border border-danger/20 rounded-lg hover:bg-danger/20 transition-colors font-medium"
                                >
                                    {move || if deleting.get() { t_string!(i18n, settings.acp.deleting).to_string() } else { t_string!(i18n, settings.acp.delete).to_string() }}
                                </button>
                            }.into_any()
                        }}
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>

            </div> // scrollable content
        </div> // flex wrapper
    }.into_any()
}

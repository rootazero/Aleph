use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{AcpApi, AcpHarnessConfig, AcpHarnessInfo};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub(super) fn AddHarnessPanel(
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

        let create_id = id_val;
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
                    set_action_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Create failed: {e}")
                        }),
                    ));
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

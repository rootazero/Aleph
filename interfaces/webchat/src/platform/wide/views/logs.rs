use crate::api::{LogsApi, LogsResponse};
use crate::components::ui::{Button, ButtonVariant, Card};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;

/// Return a Tailwind text color class based on the log level found in the line.
fn log_line_color(line: &str) -> &'static str {
    if line.contains(" ERROR ") {
        "text-danger"
    } else if line.contains(" WARN ") {
        "text-warning"
    } else if line.contains(" DEBUG ") || line.contains(" TRACE ") {
        "text-text-tertiary"
    } else {
        "text-text-secondary"
    }
}

#[component]
#[must_use]
pub fn Logs() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Reactive state
    let logs_data = RwSignal::new(None::<LogsResponse>);
    let error_msg = RwSignal::new(None::<String>);
    let is_loading = RwSignal::new(false);
    let selected_level = RwSignal::new("all".to_string());
    let selected_lines = RwSignal::new(100usize);

    // Fetch logs action
    let fetch_logs = move || {
        let state = state;
        leptos::task::spawn_local(async move {
            is_loading.set(true);
            error_msg.set(None);

            let level = {
                let l = selected_level.get_untracked();
                if l == "all" {
                    None
                } else {
                    Some(l)
                }
            };
            let lines = selected_lines.get_untracked();

            match LogsApi::fetch(&state, lines, level.as_deref()).await {
                Ok(response) => {
                    logs_data.set(Some(response));
                }
                Err(e) => {
                    error_msg.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| e.to_string(),
                    )));
                }
            }
            is_loading.set(false);
        });
    };

    // Auto-fetch when connected
    let fetch_on_connect = fetch_logs;
    Effect::new(move || {
        if state.is_connected.get() {
            fetch_on_connect();
        } else {
            logs_data.set(None);
        }
    });

    // Refresh handler
    let fetch_on_click = fetch_logs;
    let handle_refresh = move |_| {
        fetch_on_click();
    };

    // Level change handler
    let fetch_on_level = fetch_logs;
    let handle_level_change = move |ev: web_sys::Event| {
        let target = event_target::<web_sys::HtmlSelectElement>(&ev);
        selected_level.set(target.value());
        fetch_on_level();
    };

    // Lines change handler
    let handle_lines_change = move |ev: web_sys::Event| {
        let target = event_target::<web_sys::HtmlSelectElement>(&ev);
        if let Ok(n) = target.value().parse::<usize>() {
            selected_lines.set(n);
            fetch_logs();
        }
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-6">
            // Header
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                            <polyline points="14 2 14 8 20 8" />
                            <line x1="16" y1="13" x2="8" y2="13" />
                            <line x1="16" y1="17" x2="8" y2="17" />
                            <polyline points="10 9 9 9 8 9" />
                        </svg>
                        {t!(i18n, logs.title)}
                    </h2>
                    <p class="text-text-secondary">{t!(i18n, logs.description)}</p>
                </div>
                <Button
                    on:click=handle_refresh
                    variant=ButtonVariant::Secondary
                    disabled=Signal::derive(move || is_loading.get() || !state.is_connected.get())
                >
                    {move || if is_loading.get() { t_string!(i18n, common.loading).to_string() } else { t_string!(i18n, logs.refresh).to_string() }}
                </Button>
            </header>

            // Error banner
            {move || error_msg.get().map(|e| view! {
                <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger">
                    <strong>"Error: "</strong> {e}
                </div>
            })}

            // Controls bar
            <Card class="p-4">
                <div class="flex items-center gap-6 flex-wrap">
                    // Level filter
                    <div class="flex items-center gap-2">
                        <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">{t!(i18n, logs.level)}</label>
                        <select
                            on:change=handle_level_change
                            class="bg-surface-sunken border border-border rounded-lg px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/50"
                        >
                            <option value="all">{t!(i18n, logs.filter_all)}</option>
                            <option value="error">{t!(i18n, logs.filter_error)}</option>
                            <option value="warn">{t!(i18n, logs.filter_warn)}</option>
                            <option value="info">{t!(i18n, logs.filter_info)}</option>
                            <option value="debug">{t!(i18n, logs.filter_debug)}</option>
                            <option value="trace">{t!(i18n, logs.filter_trace)}</option>
                        </select>
                    </div>

                    // Lines count
                    <div class="flex items-center gap-2">
                        <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">{t!(i18n, logs.lines)}</label>
                        <select
                            on:change=handle_lines_change
                            class="bg-surface-sunken border border-border rounded-lg px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/50"
                        >
                            <option value="50">"50"</option>
                            <option value="100" selected>"100"</option>
                            <option value="200">"200"</option>
                            <option value="500">"500"</option>
                        </select>
                    </div>

                    // File path display
                    {move || logs_data.get().and_then(|d| d.file).map(|f| view! {
                        <div class="flex items-center gap-2 ml-auto">
                            <span class="text-xs text-text-tertiary font-mono">{f}</span>
                        </div>
                    })}
                </div>
            </Card>

            // Log content
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">{t!(i18n, logs.connect_to_view)}</p>
                        </Card>
                    }.into_any()
                } else if let Some(data) = logs_data.get() {
                    if data.logs.is_empty() {
                        view! {
                            <Card class="p-12 text-center">
                                <p class="text-text-tertiary">{t!(i18n, logs.no_entries)}</p>
                            </Card>
                        }.into_any()
                    } else {
                        view! {
                            <Card class="overflow-hidden">
                                <div class="max-h-[600px] overflow-y-auto p-4 bg-surface-sunken">
                                    <pre class="font-mono text-xs leading-relaxed whitespace-pre-wrap break-all">
                                        {data.logs.into_iter().map(|line| {
                                            let color = log_line_color(&line);
                                            view! {
                                                <div class=format!("{} hover:bg-surface-raised/50 px-1 -mx-1 rounded", color)>
                                                    {line}
                                                </div>
                                            }
                                        }).collect_view()}
                                    </pre>
                                </div>
                            </Card>
                        }.into_any()
                    }
                } else {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">{t!(i18n, common.loading)}</p>
                        </Card>
                    }.into_any()
                }
            }}
        </div>
    }
}

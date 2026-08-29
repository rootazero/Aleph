//! Reembed migration card — drives `EmbeddingProvidersApi::reembed` and renders
//! progress streamed from the gateway via `memory.reembed.*` events. Embedded
//! inside [`super::detail_panel::ProviderDetailPanel`].

use crate::api::EmbeddingProvidersApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub(super) fn ReembedMigrationCard() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let (migrating, set_migrating) = signal(false);
    let (progress_phase, set_progress_phase) = signal(String::new());
    let (progress_total, set_progress_total) = signal(0usize);
    let (progress_completed, set_progress_completed) = signal(0usize);
    let (progress_failed, set_progress_failed) = signal(0usize);
    // `(facts migrated, facts total, errors)` — the numbers, not a rendered
    // sentence. The sentence used to be built here, inside the `'static` event
    // closure: hard-coded English, and frozen at event time even if it had not
    // been, because a locale switch cannot reach a `String` already stored.
    let (result_counts, set_result_counts) = signal(Option::<(u64, u64, u64)>::None);
    let (result_errors, set_result_errors) = signal(Vec::<String>::new());
    let (error_message, set_error_message) = signal(Option::<String>::None);

    // Subscribe to reembed events via Gateway event bus
    Effect::new(move || {
        if state.is_connected.get() {
            // Register interest in reembed topics on the server
            spawn_local(async move {
                let _ = state.subscribe_topic("memory.reembed.*").await;
            });

            // Listen for all events, filter by topic
            state.subscribe_events(move |event: crate::context::GatewayEvent| {
                let data = &event.data;
                match event.topic.as_str() {
                    "memory.reembed.progress" => {
                        if let Some(phase) = data.get("phase").and_then(|v| v.as_str()) {
                            set_progress_phase.set(phase.to_string());
                        }
                        if let Some(total) = data.get("total").and_then(serde_json::Value::as_u64) {
                            set_progress_total.set(total as usize);
                        }
                        if let Some(completed) =
                            data.get("completed").and_then(serde_json::Value::as_u64)
                        {
                            set_progress_completed.set(completed as usize);
                        }
                        if let Some(failed) = data.get("failed").and_then(serde_json::Value::as_u64)
                        {
                            set_progress_failed.set(failed as usize);
                        }
                    }
                    "memory.reembed.completed" => {
                        set_migrating.set(false);

                        if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
                            set_error_message.set(Some(format!("Migration failed: {error}")));
                        } else {
                            let facts_updated = data
                                .get("facts_updated")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            let facts_total = data
                                .get("facts_total")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            let error_list: Vec<String> = data
                                .get("errors")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|e| e.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let errors = error_list.len();
                            // Surface the first few concrete reasons so failures
                            // are diagnosable from the panel, not just a count.
                            set_result_errors.set(error_list.into_iter().take(5).collect());

                            set_result_counts.set(Some((
                                facts_updated as u64,
                                facts_total as u64,
                                errors as u64,
                            )));
                        }
                    }
                    _ => {}
                }
            });
        }
    });

    // Start migration
    let handle_start = move |_| {
        set_migrating.set(true);
        set_result_counts.set(None);
        set_result_errors.set(Vec::new());
        set_error_message.set(None);
        set_progress_completed.set(0);
        set_progress_total.set(0);
        set_progress_failed.set(0);

        spawn_local(async move {
            match EmbeddingProvidersApi::reembed(&state, None).await {
                Ok(_) => {} // Progress tracked via events
                Err(e) => {
                    set_migrating.set(false);
                    set_error_message.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to start: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Cancel migration
    let handle_cancel = move |_| {
        spawn_local(async move {
            let _ = EmbeddingProvidersApi::reembed_cancel(&state).await;
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-3">
            <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider">
                {t!(i18n, settings.embedding.reembed)}
            </h3>
            <p class="text-sm text-text-secondary">
                {t!(i18n, settings.embedding.reembed_desc)}
            </p>

            // Progress bar (shown during migration)
            {move || {
                if migrating.get() {
                    let total = progress_total.get();
                    let completed = progress_completed.get();
                    let failed = progress_failed.get();
                    let phase = progress_phase.get();
                    let pct = (completed * 100).checked_div(total).unwrap_or(0);
                    let phase_label = match phase.as_str() {
                        "facts" => t_string!(i18n, settings.embedding.phase_facts),
                        "memories" => t_string!(i18n, settings.embedding.phase_memories),
                        _ => t_string!(i18n, settings.embedding.phase_preparing),
                    };

                    view! {
                        <div class="space-y-2">
                            <div class="flex justify-between text-xs text-text-tertiary">
                                <span>{phase_label}</span>
                                <span>{format!("{completed}/{total}")}{if failed > 0 { format!(" ({failed} failed)") } else { String::new() }}</span>
                            </div>
                            <div class="w-full h-2 bg-surface-sunken rounded-full overflow-hidden">
                                <div
                                    class="h-full bg-primary rounded-full transition-all duration-300"
                                    style=move || format!("width: {pct}%")
                                ></div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Result message
            {move || result_counts.get().map(|(done, total, errors)| {
                let mut msg = t_string!(
                    i18n,
                    settings.embedding.result_facts,
                    done = done,
                    total = total
                )
                .to_string();
                if errors > 0 {
                    msg.push_str(&t_string!(
                        i18n,
                        settings.embedding.result_errors_suffix,
                        count = errors
                    ));
                }
                view! {
                    <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                        {msg}
                    </div>
                }
            })}

            // Per-note error detail (first few concrete reasons)
            {move || {
                let errs = result_errors.get();
                (!errs.is_empty()).then(|| view! {
                    <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-xs space-y-1">
                        <div class="font-semibold">{t!(i18n, settings.embedding.error_details)}</div>
                        <ul class="list-disc list-inside space-y-0.5 font-mono">
                            {errs.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}
                        </ul>
                    </div>
                })
            }}

            // Error message
            {move || error_message.get().map(|msg| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                    {msg}
                </div>
            })}

            // Buttons
            <div class="flex gap-3">
                <button
                    on:click=handle_start
                    disabled=move || migrating.get()
                    class="flex-1 px-4 py-2.5 bg-warning text-white rounded-lg hover:bg-warning/90 disabled:opacity-50 transition-colors font-medium text-sm"
                >
                    {move || if migrating.get() { t_string!(i18n, settings.embedding.migrating).to_string() } else { t_string!(i18n, settings.embedding.migrate).to_string() }}
                </button>
                {move || {
                    if migrating.get() {
                        view! {
                            <button
                                on:click=handle_cancel
                                class="px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger rounded-lg hover:bg-danger-subtle/80 transition-colors font-medium text-sm"
                            >
                                {t!(i18n, common.cancel)}
                            </button>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

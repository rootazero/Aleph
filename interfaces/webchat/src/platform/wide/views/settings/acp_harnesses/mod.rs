//! ACP Harnesses settings page — split into three sibling modules.
//!
//! - `AcpHarnessesView` (this file) — left list + right pane router
//! - [`detail_panel::HarnessDetailPanel`] — right pane when a harness is selected
//! - [`add_panel::AddHarnessPanel`] — right pane when adding a custom harness

mod add_panel;
mod detail_panel;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{AcpApi, AcpHarnessInfo, AcpPresetMeta};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

use add_panel::AddHarnessPanel;
use detail_panel::HarnessDetailPanel;

const PRESET_ICON_COLORS: &[&str] = &[
    "#F97316", "#3B82F6", "#10B981", "#8B5CF6", "#EF4444", "#06B6D4", "#F59E0B", "#EC4899",
    "#6366F1", "#14B8A6", "#84CC16", "#D946EF", "#F43F5E", "#0EA5E9", "#A855F7", "#22C55E",
];

fn preset_icon_color(id: &str) -> &'static str {
    let hash = id
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let idx = (hash as usize) % PRESET_ICON_COLORS.len();
    PRESET_ICON_COLORS[idx]
}

/// ACP Harnesses settings page — manages external CLI tools
#[component]
#[must_use]
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
                        // `try_get_untracked` — not `get_untracked`. These
                        // signals belong to this view's reactive scope, and the
                        // view can be disposed while the RPC above is in flight
                        // (the user navigated away). Reading a disposed signal
                        // PANICS, taking the whole panel down with the "Aleph
                        // Panel crashed" overlay; `set` on one is a harmless
                        // no-op, which is why only this read had to change.
                        // Reproduced by opening this page and tapping back
                        // within ~250 ms — an ordinary thing to do on a phone,
                        // where these settings pages only recently became
                        // reachable at all.
                        let Some(current) = selected_id.try_get_untracked() else {
                            return;
                        };
                        if current.is_none() {
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
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel — harness list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border aleph-md-list">
                <div class="px-6 pb-4 border-b border-border">
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
                                                let pid_check = pid;

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
                                                                format!("{base} bg-primary-subtle border-primary")
                                                            } else {
                                                                format!("{base} bg-surface-sunken border-border hover:border-border-strong")
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
                                        let custom_list = custom_harnesses;
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
                                                        let hid_check = hid;

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
                                                                        format!("{base} bg-primary-subtle border-primary")
                                                                    } else {
                                                                        format!("{base} bg-surface-sunken border-border hover:border-border-strong")
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
            <div class="flex-1 flex flex-col overflow-hidden aleph-md-detail">
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

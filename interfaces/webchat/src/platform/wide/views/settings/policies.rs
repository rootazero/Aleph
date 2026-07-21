use crate::api::agents::{AgentsApi, ToolGroupInfo};
use crate::api::tool_permissions::{ModePreset, TierPreset, ToolPermissionsApi};
use crate::components::exec_tier_labels::{tier_desc, tier_label, FULL_TIER};
use crate::components::mode_labels::{mode_desc, mode_label};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

/// Permission level constants
const ALLOW: &str = "allow";
const ASK: &str = "ask";
const DENY: &str = "deny";

#[component]
#[must_use]
pub fn PoliciesView() -> impl IntoView {
    let i18n = use_i18n();

    // Tool permissions state
    let state = expect_context::<DashboardState>();
    let groups = RwSignal::new(Vec::<ToolGroupInfo>::new());
    let tool_perms = RwSignal::new(HashMap::<String, String>::new());
    let original_perms = RwSignal::new(HashMap::<String, String>::new());
    let default_perm = RwSignal::new(ALLOW.to_string());
    let original_default = RwSignal::new(ALLOW.to_string());
    let tp_loading = RwSignal::new(true);
    let tp_saving = RwSignal::new(false);
    let tp_error = RwSignal::new(Option::<String>::None);
    let tp_success = RwSignal::new(Option::<String>::None);

    // Execution-permission tier state. Presets come from Core (`builtin_tiers()`),
    // never from here.
    let tiers = RwSignal::new(Vec::<TierPreset>::new());
    let exec_tier = RwSignal::new(String::new());
    // Tier awaiting an explicit second confirmation (Full only).
    let tier_confirm = RwSignal::new(Option::<String>::None);
    let tier_saving = RwSignal::new(false);
    let tier_error = RwSignal::new(Option::<String>::None);
    let tier_applied = RwSignal::new(false);

    // Global usage-mode state — the tier's third twin ([policies] mode).
    // Presets come from Core (`builtin_modes()`), never from here.
    let modes = RwSignal::new(Vec::<ModePreset>::new());
    let global_mode = RwSignal::new(String::new());
    let mode_saving = RwSignal::new(false);
    let mode_error = RwSignal::new(Option::<String>::None);
    let mode_applied = RwSignal::new(false);

    // Load tool permissions + schema
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        spawn_local(async move {
            let schema = match AgentsApi::tools_schema(&dash).await {
                Ok(s) => s,
                Err(e) => {
                    tp_error.set(Some(format!("Failed to load tool schema: {e}")));
                    tp_loading.set(false);
                    return;
                }
            };

            let perms = match ToolPermissionsApi::get_global(&dash).await {
                Ok(p) => p,
                Err(e) => {
                    tp_error.set(Some(format!("Failed to load permissions: {e}")));
                    tp_loading.set(false);
                    return;
                }
            };

            let all_tools: Vec<String> = schema
                .groups
                .iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            let mut current = HashMap::new();
            for t in &all_tools {
                let perm = perms
                    .overrides
                    .get(t)
                    .cloned()
                    .unwrap_or_else(|| perms.default.clone());
                current.insert(t.clone(), perm);
            }

            exec_tier.set(perms.exec_tier);
            tiers.set(perms.tiers);
            global_mode.set(perms.mode);
            modes.set(perms.modes);
            default_perm.set(perms.default.clone());
            original_default.set(perms.default);
            groups.set(schema.groups);
            tool_perms.set(current.clone());
            original_perms.set(current);
            tp_loading.set(false);
        });
    });

    // Apply a global default mode. Partial update — nothing else on the
    // permission surface is touched. Sessions with an explicit override keep
    // it; follow-global sessions pick the new default up on their next turn.
    let apply_mode = move |id: String| {
        mode_saving.set(true);
        mode_error.set(None);
        mode_applied.set(false);
        spawn_local(async move {
            match ToolPermissionsApi::set_mode(&dash, &id).await {
                Ok(resp) => {
                    global_mode.set(resp.mode);
                    modes.set(resp.modes);
                    mode_applied.set(true);
                }
                Err(e) => mode_error.set(Some(format!("Failed to apply mode: {e}"))),
            }
            mode_saving.set(false);
        });
    };

    let set_tool_perm = move |tool_name: String, level: String| {
        tool_perms.update(|map| {
            map.insert(tool_name, level);
        });
        tp_success.set(None);
    };

    let toggle_group = move |tool_names: Vec<String>| {
        tool_perms.update(|map| {
            let all_allowed = tool_names
                .iter()
                .all(|t| map.get(t).map(|v| v == ALLOW).unwrap_or(false));
            let target = if all_allowed { DENY } else { ALLOW };
            for t in &tool_names {
                map.insert(t.clone(), target.to_string());
            }
        });
        tp_success.set(None);
    };

    let tp_has_changes = Memo::new(move |_| {
        tool_perms.get() != original_perms.get() || default_perm.get() != original_default.get()
    });

    let tp_save = move |_| {
        tp_saving.set(true);
        tp_error.set(None);
        tp_success.set(None);

        spawn_local(async move {
            let current_default = default_perm.get();
            let current_perms = tool_perms.get();

            let overrides: HashMap<String, String> = current_perms
                .into_iter()
                .filter(|(_, v)| *v != current_default)
                .collect();

            match ToolPermissionsApi::update_global(&dash, &current_default, &overrides).await {
                Ok(()) => {
                    original_perms.set(tool_perms.get());
                    original_default.set(default_perm.get());
                    tp_success.set(Some(
                        t_string!(i18n, settings.policies.saved_note).to_string(),
                    ));
                }
                Err(e) => {
                    tp_error.set(Some(format!("Failed to save: {e}")));
                }
            }
            tp_saving.set(false);
        });
    };

    let tp_reset = move |_| {
        tool_perms.set(original_perms.get());
        default_perm.set(original_default.get());
        tp_success.set(None);
    };

    // Apply a tier. Partial update — the advanced overrides below are untouched;
    // we re-render from whatever Core echoes back.
    let apply_tier = move |id: String| {
        tier_saving.set(true);
        tier_error.set(None);
        tier_applied.set(false);
        spawn_local(async move {
            match ToolPermissionsApi::set_exec_tier(&dash, &id).await {
                Ok(resp) => {
                    exec_tier.set(resp.exec_tier);
                    tiers.set(resp.tiers);
                    tier_confirm.set(None);
                    tier_applied.set(true);
                }
                Err(e) => tier_error.set(Some(format!("Failed to apply tier: {e}"))),
            }
            tier_saving.set(false);
        });
    };

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top">
            <div class="max-w-2xl space-y-6">
                // Page Header
                <div>
                    <h1 class="text-2xl font-semibold text-text-primary mb-1">
                        {t!(i18n, settings.policies.title)}
                    </h1>
                    <p class="text-sm text-text-secondary">
                        {t!(i18n, settings.policies.description)}
                    </p>
                </div>

                // Execution Permission Tier — the headline dial. Cards are built
                // from the presets Core ships, so every surface shows the same three.
                <div class="space-y-3">
                    <div>
                        <h2 class="text-lg font-medium text-text-primary">{t!(i18n, settings.policies.exec_tier_title)}</h2>
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.policies.exec_tier_desc)}
                        </p>
                    </div>

                    {move || tier_error.get().map(|e| view! {
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                    })}
                    <Show when=move || tier_applied.get()>
                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                            {t!(i18n, settings.policies.exec_tier_applied)}
                        </div>
                    </Show>

                    {move || tiers.get().into_iter().map(|preset| {
                        let id = preset.id.clone();
                        let id_click = id.clone();
                        let selected = Signal::derive(move || exec_tier.get() == id);
                        view! {
                            <button
                                disabled=move || tier_saving.get()
                                on:click=move |_| {
                                    let id = id_click.clone();
                                    // Full is the one tier that must be confirmed before it fires.
                                    if id == FULL_TIER && exec_tier.get_untracked() != FULL_TIER {
                                        tier_applied.set(false);
                                        tier_confirm.set(Some(id));
                                    } else {
                                        tier_confirm.set(None);
                                        apply_tier(id);
                                    }
                                }
                                class=move || if selected.get() {
                                    "w-full text-left p-4 rounded-xl border-2 border-primary bg-primary-subtle transition-colors disabled:opacity-50"
                                } else {
                                    "w-full text-left p-4 rounded-xl border border-border bg-surface-raised hover:bg-surface-sunken transition-colors disabled:opacity-50"
                                }
                            >
                                <div class="flex items-center gap-2 mb-1">
                                    <span class=move || if selected.get() {
                                        "w-3 h-3 rounded-full bg-primary"
                                    } else {
                                        "w-3 h-3 rounded-full border border-border"
                                    }></span>
                                    <span class="text-sm font-semibold text-text-primary">
                                        {tier_label(i18n, &preset.id)}
                                    </span>
                                </div>
                                <p class="text-xs text-text-secondary ml-5">
                                    {tier_desc(i18n, &preset.id)}
                                </p>
                            </button>
                        }
                    }).collect_view()}

                    // Inline confirmation for Full — an informed choice, not an accident.
                    {move || tier_confirm.get().map(|id| {
                        let id_yes = id;
                        view! {
                            <div class="p-4 bg-danger-subtle border border-danger/30 rounded-xl space-y-2">
                                <div class="text-sm font-semibold text-danger">
                                    {t!(i18n, settings.policies.exec_tier_full_confirm_title)}
                                </div>
                                <p class="text-xs text-text-secondary">
                                    {t!(i18n, settings.policies.exec_tier_full_confirm_body)}
                                </p>
                                <div class="flex gap-2 pt-1">
                                    <button
                                        class="px-3 py-1.5 text-xs font-medium text-white bg-danger rounded-lg hover:bg-danger/90 transition-colors disabled:opacity-50"
                                        disabled=move || tier_saving.get()
                                        on:click=move |_| apply_tier(id_yes.clone())
                                    >
                                        {t!(i18n, settings.policies.exec_tier_full_confirm_yes)}
                                    </button>
                                    <button
                                        class="px-3 py-1.5 text-xs font-medium text-text-secondary bg-surface-raised border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                                        on:click=move |_| tier_confirm.set(None)
                                    >
                                        {t!(i18n, common.cancel)}
                                    </button>
                                </div>
                            </div>
                        }
                    })}
                </div>

                // Global Usage Mode — the tier's third twin. Shapes the tool
                // PRESENTATION surface only (never permissions); a per-session
                // pill override always wins over this default.
                <Show when=move || !modes.get().is_empty()>
                <div class="space-y-3">
                    <div>
                        <h2 class="text-lg font-medium text-text-primary">{t!(i18n, settings.policies.mode_title)}</h2>
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.policies.mode_global_desc)}
                        </p>
                    </div>

                    {move || mode_error.get().map(|e| view! {
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                    })}
                    <Show when=move || mode_applied.get()>
                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                            {t!(i18n, settings.policies.mode_applied)}
                        </div>
                    </Show>

                    <div class="flex gap-2">
                        {move || modes.get().into_iter().map(|preset| {
                            let id = preset.id.clone();
                            let id_click = id.clone();
                            let selected = Signal::derive(move || global_mode.get() == id);
                            view! {
                                <button
                                    disabled=move || mode_saving.get()
                                    on:click=move |_| apply_mode(id_click.clone())
                                    class=move || if selected.get() {
                                        "flex-1 text-left p-3 rounded-xl border-2 border-primary bg-primary-subtle transition-colors disabled:opacity-50"
                                    } else {
                                        "flex-1 text-left p-3 rounded-xl border border-border bg-surface-raised hover:bg-surface-sunken transition-colors disabled:opacity-50"
                                    }
                                >
                                    <div class="text-sm font-semibold text-text-primary mb-0.5">
                                        {mode_label(i18n, &preset.id)}
                                    </div>
                                    <p class="text-[11px] leading-snug text-text-secondary">
                                        {mode_desc(i18n, &preset.id)}
                                    </p>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>
                </Show>

                // Tool Permissions Section — advanced overrides layered on top of
                // the tier preset above.
                <div class="space-y-4">
                    <div>
                        <h2 class="text-lg font-medium text-text-primary">{t!(i18n, settings.policies.tool_permissions_advanced)}</h2>
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.policies.tool_permissions_advanced_desc)}
                        </p>
                    </div>

                    {move || tp_error.get().map(|e| view! {
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                    })}
                    {move || tp_success.get().map(|msg| view! {
                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{msg}</div>
                    })}

                    {move || {
                        if tp_loading.get() {
                            return view! {
                                <div class="text-text-secondary py-4 text-center text-sm">{t!(i18n, settings.policies.loading_tool_permissions)}</div>
                            }.into_any();
                        }

                        let current_groups = groups.get();

                        view! {
                            <div class="space-y-3">
                                // Default permission selector
                                <div class="flex items-center justify-between p-4 bg-surface-raised border border-border rounded-xl">
                                    <div>
                                        <span class="text-sm font-semibold text-text-primary">{t!(i18n, settings.policies.default_permission)}</span>
                                        <p class="text-xs text-text-tertiary mt-0.5">{t!(i18n, settings.policies.default_permission_desc)}</p>
                                    </div>
                                    <PolicySegmentedControl
                                        value=Signal::derive(move || default_perm.get())
                                        on_change=move |level: String| {
                                            default_perm.set(level);
                                            tp_success.set(None);
                                        }
                                    />
                                </div>

                                {current_groups.into_iter().map(|group| {
                                    let group_tools: Vec<String> = group.tools.iter().map(|t| t.name.clone()).collect();
                                    let gt_toggle = group_tools.clone();
                                    let gt_check = group_tools;

                                    view! {
                                        <div class="bg-surface-raised border border-border rounded-xl overflow-hidden">
                                            <div class="flex items-center justify-between px-5 py-3 bg-surface-sunken/50 border-b border-border">
                                                <span class="text-sm font-semibold text-text-primary">{group.name.clone()}</span>
                                                <button
                                                    class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none"
                                                    class=("bg-primary", {
                                                        let gt = gt_check.clone();
                                                        move || {
                                                            let perms = tool_perms.get();
                                                            gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                        }
                                                    })
                                                    class=("bg-border", {
                                                        let gt = gt_check;
                                                        move || {
                                                            let perms = tool_perms.get();
                                                            !gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                        }
                                                    })
                                                    on:click=move |_| toggle_group(gt_toggle.clone())
                                                >
                                                    <span
                                                        class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                        class=("translate-x-4.5", {
                                                            let gt = gt_check.clone();
                                                            move || {
                                                                let perms = tool_perms.get();
                                                                gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                            }
                                                        })
                                                        class=("translate-x-0.5", {
                                                            let gt = gt_check.clone();
                                                            move || {
                                                                let perms = tool_perms.get();
                                                                !gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                            }
                                                        })
                                                    />
                                                </button>
                                            </div>
                                            <div class="divide-y divide-border/50">
                                                {group.tools.into_iter().map(|tool| {
                                                    let tn = tool.name.clone();
                                                    let tn_perm = tn.clone();
                                                    let tn_set = tn.clone();

                                                    view! {
                                                        <div class="flex items-center justify-between px-5 py-2.5">
                                                            <div class="flex-1 min-w-0">
                                                                <span class="text-sm font-medium text-text-primary">{tn}</span>
                                                                <p class="text-xs text-text-tertiary truncate mt-0.5">{tool.description}</p>
                                                            </div>
                                                            <div class="ml-4 flex-shrink-0">
                                                                <PolicySegmentedControl
                                                                    value=Signal::derive(move || {
                                                                        let perms = tool_perms.get();
                                                                        perms.get(&tn_perm).cloned().unwrap_or_else(|| default_perm.get())
                                                                    })
                                                                    on_change={
                                                                        let tn_c = tn_set;
                                                                        move |level: String| {
                                                                            set_tool_perm(tn_c.clone(), level);
                                                                        }
                                                                    }
                                                                />
                                                            </div>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}

                                <p class="text-xs text-text-tertiary italic">
                                    {t!(i18n, settings.policies.changes_note)}
                                </p>

                                <div class="flex justify-end gap-3 pt-1">
                                    <button
                                        class="px-4 py-2 text-sm font-medium text-text-secondary bg-surface-raised border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                                        on:click=tp_reset
                                    >
                                        {t!(i18n, settings.policies.reset)}
                                    </button>
                                    <button
                                        class="px-4 py-2 text-sm font-medium text-white bg-primary rounded-lg hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                        disabled=move || !tp_has_changes.get() || tp_saving.get()
                                        on:click=tp_save
                                    >
                                        {move || if tp_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

/// Segmented control for global policy (no restrictions since this IS the global ceiling)
#[component]
fn PolicySegmentedControl(
    value: Signal<String>,
    on_change: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    let i18n = use_i18n();
    let on_allow = {
        let cb = on_change.clone();
        move |_| cb(ALLOW.to_string())
    };
    let on_ask = {
        let cb = on_change.clone();
        move |_| cb(ASK.to_string())
    };
    let on_deny = {
        let cb = on_change;
        move |_| cb(DENY.to_string())
    };

    view! {
        <div class="flex rounded-lg border border-border overflow-hidden">
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-success", move || value.get() == ALLOW)
                class=("text-white", move || value.get() == ALLOW)
                class=("text-text-secondary", move || value.get() != ALLOW)
                class=("hover:bg-surface-sunken", move || value.get() != ALLOW)
                on:click=on_allow
            >
                {t!(i18n, settings.policies.allow)}
            </button>
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors border-x border-border"
                class=("bg-warning", move || value.get() == ASK)
                class=("text-white", move || value.get() == ASK)
                class=("text-text-secondary", move || value.get() != ASK)
                class=("hover:bg-surface-sunken", move || value.get() != ASK)
                on:click=on_ask
            >
                {t!(i18n, settings.policies.ask)}
            </button>
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-danger", move || value.get() == DENY)
                class=("text-white", move || value.get() == DENY)
                class=("text-text-secondary", move || value.get() != DENY)
                class=("hover:bg-surface-sunken", move || value.get() != DENY)
                on:click=on_deny
            >
                {t!(i18n, settings.policies.deny)}
            </button>
        </div>
    }
}

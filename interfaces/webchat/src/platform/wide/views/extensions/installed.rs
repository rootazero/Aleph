//! Installed extensions slide-in panel — lists all reconciled installed
//! extensions with enable/disable toggle, two-step Remove, update badge,
//! and a "manual · not in catalog" tag for unverified/unmatched items.
//!
//! Lifecycle RPCs (`toggle` / `uninstall`) receive the installed list's
//! `local:{kind}:{backend}` ids directly — do NOT substitute catalog ids.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::extensions::{ExtensionEntry, ExtensionsApi};
use crate::components::extensions::labels::kind_label;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, Locale};
use crate::views::extensions::model::kind_badge_class;
use crate::views::extensions::StoreState;
use leptos_i18n::I18nContext;

fn load_installed(
    state: DashboardState,
    items: RwSignal<Vec<ExtensionEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    i18n: I18nContext<Locale>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match ExtensionsApi::installed(&state).await {
            Ok(list) => {
                items.set(list);
                loading.set(false);
            }
            Err(e) => {
                let prefix = t_string!(i18n, extensions.error.installed_load).to_string();
                error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("{prefix}: {e}")
                    }),
                ));
                loading.set(false);
            }
        }
    });
}

#[component]
#[must_use]
pub fn InstalledPanel() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let items = RwSignal::new(Vec::<ExtensionEntry>::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move || {
        if store.show_installed.get() && state.is_connected.get() {
            load_installed(state, items, loading, error, i18n);
        }
    });

    let close = move |_| store.show_installed.set(false);

    view! {
        <Show when=move || store.show_installed.get()>
            <div class="fixed inset-0 z-[60] flex justify-end">
                <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
                <aside class="glass relative w-[480px] max-w-[94vw] h-full bg-surface-overlay/90 border-l border-border shadow-xl flex flex-col">
                    <header class="px-4 py-3 border-b border-border flex items-center justify-between">
                        <h2 class="font-serif text-lg text-text-primary">{t!(i18n, extensions.installed_title)}</h2>
                        <button class="text-text-tertiary hover:text-text-primary" on:click=close>"✕"</button>
                    </header>
                    <div class="flex-1 overflow-y-auto p-4 space-y-2">
                        {move || error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">{e}</div>
                        })}
                        {move || if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                                </div>
                            }.into_any()
                        } else if items.get().is_empty() {
                            view! {
                                <div class="text-center py-12 border border-dashed border-border rounded-xl">
                                    <p class="text-text-secondary">{t!(i18n, extensions.none_installed)}</p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <For
                                    each=move || items.get()
                                    key=|e: &ExtensionEntry| e.id.clone()
                                    children=move |e| view! { <InstalledRow entry=e items=items loading=loading error=error /> }
                                />
                            }.into_any()
                        }}
                    </div>
                </aside>
            </div>
        </Show>
    }
}

#[component]
fn InstalledRow(
    entry: ExtensionEntry,
    items: RwSignal<Vec<ExtensionEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let enabled = RwSignal::new(entry.enabled);
    let toggling = RwSignal::new(false);
    let confirming = RwSignal::new(false);

    // Clone the id for each closure that needs it — the installed list ids are
    // already `local:{kind}:{backend}`, the only valid ids for toggle/uninstall.
    let id_for_toggle = entry.id.clone();
    let id_for_remove = entry.id.clone();

    let badge_cls = format!(
        "px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase whitespace-nowrap flex-shrink-0 {}",
        kind_badge_class(&entry.kind)
    );
    // Heuristic: unverified tier → "manual · not in catalog" tag (v1 proxy).
    let manual = entry.trust_tier == "unverified";
    // Capture fields needed in closures.
    let kind_str = entry.kind.clone();
    let name_initial = entry
        .name
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default();
    let name_display = entry.name.clone();
    let version_display = entry.version.clone();
    let update_available = entry.update_available;

    let on_toggle = move |ev: leptos::ev::Event| {
        let new_val = event_target_checked(&ev);
        enabled.set(new_val);
        toggling.set(true);
        let id = id_for_toggle.clone();
        spawn_local(async move {
            match ExtensionsApi::toggle(&state, id, new_val).await {
                Ok(()) => toggling.set(false),
                Err(e) => {
                    let prefix = t_string!(i18n, extensions.error.toggle_failed).to_string();
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("{prefix}: {e}")
                        }),
                    ));
                    enabled.set(!new_val);
                    toggling.set(false);
                }
            }
        });
    };

    let on_remove = move || {
        let id = id_for_remove.clone();
        spawn_local(async move {
            match ExtensionsApi::uninstall(&state, id).await {
                Ok(()) => load_installed(state, items, loading, error, i18n),
                Err(e) => {
                    let prefix = t_string!(i18n, extensions.error.remove_failed).to_string();
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("{prefix}: {e}"),
                    )));
                    confirming.set(false);
                }
            }
        });
    };

    // Compute the kind label string once (not reactive — kind doesn't change).
    let kind_label_str = kind_label(i18n, &kind_str);

    view! {
        <div class="p-3 bg-surface-raised border border-border rounded-xl flex items-center gap-3">
            <div class="w-10 h-10 rounded-lg bg-primary-subtle flex items-center justify-center flex-shrink-0">
                {name_initial}
            </div>
            <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                    <span class="text-text-primary truncate">{name_display}</span>
                    <span class=badge_cls>{kind_label_str}</span>
                </div>
                <p class="text-xs text-text-tertiary truncate">
                    {version_display.map(|v| format!("v{v}")).unwrap_or_default()}
                    {manual.then(|| view! {
                        <span class="ml-2 px-1.5 py-0.5 border border-dashed border-border rounded font-mono text-[10px]">
                            {t!(i18n, extensions.manual_tag)}
                        </span>
                    })}
                    {update_available.then(|| view! {
                        <span class="ml-2 px-1.5 py-0.5 bg-warning-subtle text-warning rounded text-[10px]">
                            {t!(i18n, extensions.update_available)}
                        </span>
                    })}
                </p>
            </div>
            <label class="relative inline-flex items-center cursor-pointer">
                <input
                    type="checkbox"
                    class="sr-only peer"
                    prop:checked=move || enabled.get()
                    on:change=on_toggle
                    disabled=move || toggling.get()
                />
                <div class="w-11 h-6 bg-surface-sunken rounded-full peer peer-checked:bg-primary peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all"></div>
            </label>
            {move || if confirming.get() {
                view! {
                    <ConfirmButton
                        confirming=confirming
                        on_confirm=on_remove.clone()
                        size_class="px-2 py-1 text-xs"
                    />
                }.into_any()
            } else {
                view! {
                    <button
                        class="px-2 py-1 text-xs text-danger hover:bg-danger-subtle rounded"
                        on:click=move |_| confirming.set(true)
                    >
                        {t!(i18n, extensions.remove)}
                    </button>
                }.into_any()
            }}
        </div>
    }
}
